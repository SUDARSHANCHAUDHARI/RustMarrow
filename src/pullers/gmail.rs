use crate::{
    config::Config,
    google_auth,
    memory::{Chunk, Store},
};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use tracing::{info, warn};

const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const PER_PAGE: u32 = 100;
/// Safety cap so a misbehaving API cannot spin the page loop forever.
const MAX_PAGES: u32 = 10;

#[derive(Deserialize)]
struct MessageList {
    #[serde(default)]
    messages: Vec<MessageRef>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct MessageRef {
    id: String,
}

#[derive(Deserialize)]
struct Message {
    id: String,
    payload: Payload,
    snippet: String,
    /// Epoch milliseconds, as a string.
    #[serde(rename = "internalDate")]
    internal_date: String,
}

#[derive(Deserialize)]
struct Payload {
    headers: Vec<Header>,
    #[serde(default)]
    parts: Vec<Part>,
    body: Option<Body>,
    #[serde(rename = "mimeType", default)]
    mime_type: String,
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct Part {
    #[serde(rename = "mimeType")]
    mime_type: String,
    body: Option<Body>,
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    data: String,
}

pub async fn pull(cfg: &Config, store: &Store) -> Result<()> {
    // Watermark is the pull start, so items arriving mid-pull are fetched next time.
    let started = Utc::now();
    let (client_id, client_secret, refresh_token) = cfg
        .google_creds()
        .context("Google credentials not set — run `rustmarrow auth google` first")?;

    let client = reqwest::Client::builder()
        .user_agent("marrow/0.1")
        .build()?;

    let token = google_auth::access_token(&client, client_id, client_secret, refresh_token).await?;
    pull_from(&client, &token, GMAIL_API, store, started).await
}

/// The pull itself. `api_base` is a parameter so a test can point it at a local server.
async fn pull_from(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
    store: &Store,
    started: DateTime<Utc>,
) -> Result<()> {
    // Incremental: fetch only since last pull (fallback to 1 day)
    let since = store
        .last_pull_at("gmail")
        .unwrap_or_else(|| Utc::now() - Duration::days(1));
    // A capped run left a gap below `resume_before`. Drain only that; newer mail waits a run.
    let gap = store.pull_gap("gmail");

    // Gmail after:/before: filters use epoch seconds. before: gets +1 so the boundary
    // second is refetched rather than skipped; the upsert dedupes it.
    let window = match gap {
        Some((from, before)) => format!(
            "after:{} before:{}",
            from.timestamp(),
            before.timestamp() + 1
        ),
        None => format!("after:{}", since.timestamp()),
    };
    let query =
        format!("{window} (is:unread OR is:important) -category:promotions -category:social");

    match gap {
        Some((from, before)) => info!(
            "Draining Gmail gap {} to {}",
            from.format("%Y-%m-%d %H:%M"),
            before.format("%Y-%m-%d %H:%M")
        ),
        None => info!("Pulling Gmail since {}", since.format("%Y-%m-%d %H:%M")),
    }

    let (messages, capped) = fetch_message_ids(client, token, api_base, &query).await?;

    info!("{} new messages", messages.len());

    let mut total = 0usize;
    let mut oldest: Option<DateTime<Utc>> = None;
    for msg_ref in &messages {
        let msg_url = format!("{api_base}/messages/{}?format=full", msg_ref.id);
        let msg = client
            .get(&msg_url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json::<Message>()
            .await?;

        let received = msg
            .internal_date
            .parse::<i64>()
            .ok()
            .and_then(DateTime::from_timestamp_millis)
            .with_context(|| format!("Gmail message {} has a bad internalDate", msg.id))?;
        oldest = Some(oldest.map_or(received, |o| o.min(received)));

        let subject = header_val(&msg.payload.headers, "Subject").unwrap_or("(no subject)");
        let from = header_val(&msg.payload.headers, "From").unwrap_or("unknown");
        let date = header_val(&msg.payload.headers, "Date").unwrap_or("");

        let body_text = extract_body(&msg.payload);
        let content = format!(
            "From: {from}\nDate: {date}\n\n{}",
            if body_text.is_empty() {
                msg.snippet.clone()
            } else {
                body_text.chars().take(800).collect()
            }
        );

        store.ingest(&Chunk {
            source: "gmail".into(),
            source_id: msg.id.clone(),
            title: format!("Email: {subject}"),
            content,
            url: Some(format!(
                "https://mail.google.com/mail/u/0/#inbox/{}",
                msg.id
            )),
            tags: vec!["gmail".into(), "email".into()],
        })?;
        total += 1;
    }

    // A drain keeps the marker where it was; a fresh run moves it to `started`. Capped:
    // everything from the oldest message fetched up to `upper` is in, so the gap shrinks
    // to below it. Not capped: the gap is closed.
    let (pulled_at, gap_from, upper) = match gap {
        Some((from, before)) => (since, from, before),
        None => (started, since, started),
    };
    let next_gap = capped.then(|| (gap_from, oldest.unwrap_or(upper).min(upper)));
    store.log_pull_gap("gmail", total, pulled_at, next_gap)?;
    info!("Gmail pull complete: {} messages", total);
    Ok(())
}

/// Follows `nextPageToken` until Gmail stops sending one, so a backlog larger than one
/// page is not dropped. `api_base` is a parameter so a test can point it at a local server.
/// The flag is true when the page cap, not Gmail, ended the walk.
async fn fetch_message_ids(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
    query: &str,
) -> Result<(Vec<MessageRef>, bool)> {
    let mut all: Vec<MessageRef> = Vec::new();
    let mut page_token: Option<String> = None;

    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{api_base}/messages?maxResults={PER_PAGE}&q={}",
            urlencoding::encode(query)
        );
        if let Some(t) = &page_token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }

        let list = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json::<MessageList>()
            .await?;

        all.extend(list.messages);

        match list.next_page_token {
            Some(t) => page_token = Some(t),
            None => return Ok((all, false)),
        }
    }

    // Gmail still had more to give. Newest come first, so the older ones were dropped; the
    // caller records them as a gap to drain next run. Loud on purpose.
    warn!(
        "Gmail stopped at the {MAX_PAGES}-page cap ({} messages); older messages were not fetched",
        all.len()
    );
    Ok((all, true))
}

fn header_val<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn extract_body(payload: &Payload) -> String {
    if payload.mime_type == "text/plain" {
        if let Some(body) = &payload.body {
            if !body.data.is_empty() {
                return decode_base64(&body.data);
            }
        }
    }
    for part in &payload.parts {
        if part.mime_type == "text/plain" {
            if let Some(body) = &part.body {
                if !body.data.is_empty() {
                    return decode_base64(&body.data);
                }
            }
        }
    }
    payload
        .body
        .as_ref()
        .filter(|b| !b.data.is_empty())
        .map(|b| decode_base64(&b.data))
        .unwrap_or_default()
}

fn decode_base64(data: &str) -> String {
    use base64::Engine;
    let fixed = data.replace('-', "+").replace('_', "/");
    base64::engine::general_purpose::STANDARD
        .decode(fixed)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A message list page: `count` ids, and `next` as the nextPageToken when present.
    fn page(start: usize, count: usize, next: Option<&str>) -> String {
        let ids: Vec<String> = (start..start + count)
            .map(|i| format!("{{\"id\":\"m{i}\"}}"))
            .collect();
        match next {
            Some(t) => format!(
                "{{\"messages\":[{}],\"nextPageToken\":\"{t}\"}}",
                ids.join(",")
            ),
            None => format!("{{\"messages\":[{}]}}", ids.join(",")),
        }
    }

    /// Serves one canned reply per connection and records the `pageToken` each request
    /// carried, so a test can prove the token is actually sent back.
    async fn serve(replies: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let (base, raw) = serve_raw(replies).await;
        let handle = tokio::spawn(async move {
            raw.await
                .unwrap()
                .iter()
                .map(|req| {
                    req.split("pageToken=")
                        .nth(1)
                        .map(|s| {
                            s.split(|c: char| c == '&' || c.is_whitespace())
                                .next()
                                .unwrap_or("")
                                .to_string()
                        })
                        .unwrap_or_default()
                })
                .collect()
        });
        (base, handle)
    }

    /// As `serve`, but yields each raw request.
    async fn serve_raw(replies: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for body in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
            seen
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn follows_the_next_page_token() {
        let (base, server) = serve(vec![
            page(0, PER_PAGE as usize, Some("tok-2")),
            page(100, 7, None),
        ])
        .await;
        let client = reqwest::Client::new();

        let (ids, capped) = fetch_message_ids(&client, "t", &base, "after:0")
            .await
            .unwrap();

        assert!(!capped);
        // Before the fix only the first page was read, so this was 100, not 107.
        assert_eq!(ids.len(), PER_PAGE as usize + 7);
        // First request carries no token; the second echoes the one Gmail returned.
        assert_eq!(server.await.unwrap(), vec!["".to_string(), "tok-2".into()]);
    }

    #[tokio::test]
    async fn stops_when_no_token_comes_back() {
        let (base, server) = serve(vec![page(0, 3, None)]).await;
        let client = reqwest::Client::new();

        let (ids, capped) = fetch_message_ids(&client, "t", &base, "after:0")
            .await
            .unwrap();

        assert!(!capped);
        assert_eq!(ids.len(), 3);
        assert_eq!(server.await.unwrap(), vec!["".to_string()]);
    }

    #[tokio::test]
    async fn stops_at_the_page_cap() {
        // Every page keeps handing back a token, so only the cap ends the loop.
        let replies = (0..MAX_PAGES)
            .map(|p| page(p as usize * 100, 100, Some("always-more")))
            .collect();
        let (base, server) = serve(replies).await;
        let client = reqwest::Client::new();

        // The server accepts exactly MAX_PAGES connections, so an 11th request would
        // be refused and this would be an Err.
        let (ids, capped) = fetch_message_ids(&client, "t", &base, "after:0")
            .await
            .unwrap();

        assert!(capped);
        assert_eq!(ids.len(), (MAX_PAGES * 100) as usize);
        assert_eq!(server.await.unwrap().len(), MAX_PAGES as usize);
    }

    /// Message `m{i}`, received `i` minutes before a fixed point, so a higher id is older.
    fn message(i: usize) -> String {
        format!(
            r#"{{"id":"m{i}","snippet":"s","internalDate":"{}","payload":{{"headers":[{{"name":"Subject","value":"m{i}"}}]}}}}"#,
            received(i).timestamp_millis()
        )
    }

    fn received(i: usize) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(1_700_000_000_500 - i as i64 * 60_000).unwrap()
    }

    #[tokio::test]
    async fn a_capped_run_leaves_a_gap_the_next_run_drains() {
        let path = crate::memory::tests::temp_db_path("gmail-gap");
        let store = crate::memory::tests::store_at(&path);
        let client = reqwest::Client::new();
        let pages = MAX_PAGES as usize;

        // Run 1: one id per page and a token every time, so the cap ends the walk.
        let mut replies: Vec<String> = (0..pages).map(|i| page(i, 1, Some("more"))).collect();
        replies.extend((0..pages).map(message));
        let (base, server) = serve_raw(replies).await;
        let started = Utc::now();
        pull_from(&client, "t", &base, &store, started)
            .await
            .unwrap();
        let reqs = server.await.unwrap();

        assert!(!reqs[0].contains("before%3A"), "{}", reqs[0]);
        assert_eq!(store.last_pull_at("gmail"), Some(started));
        let (_, resume_before) = store.pull_gap("gmail").unwrap();
        assert_eq!(resume_before, received(pages - 1));

        // Run 2: the gap holds two older messages.
        let mut replies = vec![page(pages, 2, None)];
        replies.extend((pages..pages + 2).map(message));
        let (base, server) = serve_raw(replies).await;
        pull_from(&client, "t", &base, &store, Utc::now())
            .await
            .unwrap();
        let reqs = server.await.unwrap();

        let before = format!("before%3A{}", resume_before.timestamp() + 1);
        assert!(reqs[0].contains(&before), "{}", reqs[0]);
        assert_eq!(store.pull_gap("gmail"), None);
        assert_eq!(store.last_pull_at("gmail"), Some(started));
        assert_eq!(
            store.chunks_by_source("gmail", 100).unwrap().len(),
            pages + 2
        );

        let _ = std::fs::remove_file(path);
    }
}
