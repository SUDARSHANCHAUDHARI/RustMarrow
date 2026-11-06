use crate::{
    config::Config,
    memory::{Chunk, Store},
};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use tracing::{info, warn};

const SLACK_API: &str = "https://slack.com/api";
const PAGE_LIMIT: u32 = 200;
/// Safety cap so a misbehaving API cannot spin the cursor loop forever.
const MAX_PAGES: u32 = 10;

/// Slack returns `next_cursor` as an empty string, not a missing field, when done.
#[derive(Deserialize, Default)]
struct ResponseMetadata {
    #[serde(default)]
    next_cursor: String,
}

#[derive(Deserialize)]
struct ConversationsListResp {
    ok: bool,
    #[serde(default)]
    channels: Vec<Channel>,
    error: Option<String>,
    #[serde(default)]
    response_metadata: ResponseMetadata,
}

#[derive(Deserialize)]
struct Channel {
    id: String,
    name: Option<String>,
    #[serde(rename = "is_im", default)]
    is_dm: bool,
    #[serde(rename = "is_member", default)]
    is_member: bool,
}

#[derive(Deserialize)]
struct HistoryResp {
    ok: bool,
    #[serde(default)]
    messages: Vec<SlackMessage>,
    error: Option<String>,
    #[serde(default)]
    response_metadata: ResponseMetadata,
}

#[derive(Deserialize)]
struct SlackMessage {
    ts: String,
    text: Option<String>,
    #[serde(rename = "type", default)]
    msg_type: String,
    user: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
}

pub async fn pull(cfg: &Config, store: &Store) -> Result<()> {
    let started = Utc::now();
    let token = cfg
        .slack_token
        .as_deref()
        .context("SLACK_TOKEN not set in .env")?;

    let client = reqwest::Client::builder()
        .user_agent("marrow/0.1")
        .build()?;

    pull_from(
        &client,
        token,
        SLACK_API,
        cfg.slack_channels.as_deref(),
        store,
        started,
    )
    .await
}

/// The pull itself. `api_base` is a parameter so a test can point it at a local server.
async fn pull_from(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
    channel_filter: Option<&str>,
    store: &Store,
    started: DateTime<Utc>,
) -> Result<()> {
    // Incremental: only fetch since last pull (fallback to 24h)
    let since = store
        .last_pull_at("slack")
        .unwrap_or_else(|| Utc::now() - Duration::hours(24));
    // A capped run left a gap below `resume_before`. Drain only that; newer messages wait
    // a run.
    let gap = store.pull_gap("slack");
    let (oldest, latest) = match gap {
        Some((from, before)) => {
            info!(
                "Draining Slack gap {} to {}",
                from.format("%Y-%m-%d %H:%M"),
                before.format("%Y-%m-%d %H:%M")
            );
            (to_ts(from), Some(to_ts(before)))
        }
        None => {
            info!("Pulling Slack since {}", since.format("%Y-%m-%d %H:%M"));
            (since.timestamp().to_string(), None)
        }
    };
    // A drain keeps the marker where it was; a fresh run moves it to `started`. Capped:
    // the gap shrinks to below `resume_before`. Not capped: the gap is closed.
    let (pulled_at, gap_from, upper) = match gap {
        Some((from, before)) => (since, from, before),
        None => (started, since, started),
    };

    let channels = fetch_channels(client, token, api_base, channel_filter).await?;
    info!("{} channels to scan", channels.len());

    let mut total = 0usize;
    // Highest per-channel oldest ts among capped channels: below it, some channel is
    // still missing messages.
    let mut resume_before: Option<DateTime<Utc>> = None;
    for channel in &channels {
        let name =
            channel
                .name
                .as_deref()
                .unwrap_or(if channel.is_dm { "DM" } else { &channel.id });
        let (messages, capped) = fetch_history(
            client,
            token,
            api_base,
            &channel.id,
            &oldest,
            latest.as_deref(),
        )
        .await?;
        if capped {
            let o = messages
                .iter()
                .filter_map(|m| from_ts(&m.ts))
                .min()
                .unwrap_or(upper);
            resume_before = Some(resume_before.map_or(o, |r| r.max(o)));
        }

        for msg in &messages {
            // Skip bot messages, join/leave events
            if msg.msg_type != "message" {
                continue;
            }
            if matches!(
                msg.subtype.as_deref(),
                Some("bot_message") | Some("channel_join") | Some("channel_leave")
            ) {
                continue;
            }

            let text = match &msg.text {
                Some(t) if !t.is_empty() => t.clone(),
                _ => continue,
            };

            // ts is a Unix timestamp string like "1234567890.123456"
            let ts_secs = msg
                .ts
                .split('.')
                .next()
                .unwrap_or("0")
                .parse::<i64>()
                .unwrap_or(0);
            let dt = chrono::DateTime::from_timestamp(ts_secs, 0)
                .unwrap_or_default()
                .format("%Y-%m-%d %H:%M")
                .to_string();

            let user_ref = msg.user.as_deref().unwrap_or("unknown");

            store.ingest(&Chunk {
                source: "slack".into(),
                source_id: format!("{}:{}", channel.id, msg.ts),
                title: format!(
                    "Slack #{name}: {}",
                    text.chars().take(60).collect::<String>()
                ),
                content: format!("Channel: {name}\nUser: {user_ref}\nTime: {dt}\n\n{text}"),
                url: None,
                tags: vec!["slack".into(), name.to_string()],
            })?;
            total += 1;
        }
    }

    let next_gap = resume_before.map(|r| (gap_from, r.min(upper)));
    store.log_pull_gap("slack", total, pulled_at, next_gap)?;
    info!("Slack pull complete: {} messages", total);
    Ok(())
}

/// Slack `ts` ("1234567890.123456") to a time, keeping the microseconds.
fn from_ts(ts: &str) -> Option<DateTime<Utc>> {
    let (secs, micros) = ts.split_once('.').unwrap_or((ts, "0"));
    DateTime::from_timestamp(secs.parse().ok()?, micros.parse::<u32>().ok()? * 1000)
}

fn to_ts(dt: DateTime<Utc>) -> String {
    format!("{}.{:06}", dt.timestamp(), dt.timestamp_subsec_micros())
}

/// `api_base` is a parameter so a test can point it at a local server.
async fn fetch_channels(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
    channel_filter: Option<&str>,
) -> Result<Vec<Channel>> {
    // If specific channels are configured, return stubs for those IDs
    if let Some(ids) = channel_filter {
        return Ok(ids
            .split(',')
            .map(|id| Channel {
                id: id.trim().to_string(),
                name: Some(id.trim().to_string()),
                is_dm: false,
                is_member: true,
            })
            .collect());
    }

    // Otherwise: fetch all conversations the user/bot is a member of, following the
    // cursor so a workspace with more than one page is not truncated.
    let mut all: Vec<Channel> = Vec::new();
    let mut cursor = String::new();

    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{api_base}/conversations.list?types=public_channel,private_channel,im&limit={PAGE_LIMIT}&exclude_archived=true"
        );
        if !cursor.is_empty() {
            url.push_str(&format!("&cursor={}", urlencoding::encode(&cursor)));
        }

        let resp: ConversationsListResp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if !resp.ok {
            anyhow::bail!(
                "Slack conversations.list failed: {}",
                resp.error.unwrap_or_default()
            );
        }

        all.extend(resp.channels.into_iter().filter(|c| c.is_member || c.is_dm));

        cursor = resp.response_metadata.next_cursor;
        if cursor.is_empty() {
            return Ok(all);
        }
    }

    warn!(
        "Slack conversations.list stopped at the {MAX_PAGES}-page cap ({} channels); later channels were not fetched",
        all.len()
    );
    Ok(all)
}

/// `latest`, when set, bounds a gap drain from above (inclusive). The flag is true when
/// the page cap, not Slack, ended the walk.
async fn fetch_history(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
    channel_id: &str,
    oldest: &str,
    latest: Option<&str>,
) -> Result<(Vec<SlackMessage>, bool)> {
    let mut all: Vec<SlackMessage> = Vec::new();
    let mut cursor = String::new();

    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{api_base}/conversations.history?channel={channel_id}&oldest={oldest}&limit={PAGE_LIMIT}"
        );
        if let Some(l) = latest {
            url.push_str(&format!("&latest={l}&inclusive=true"));
        }
        if !cursor.is_empty() {
            url.push_str(&format!("&cursor={}", urlencoding::encode(&cursor)));
        }

        let resp: HistoryResp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if !resp.ok {
            // Rate limited: fail the run so the marker does not move past what was skipped.
            if resp.error.as_deref() == Some("ratelimited") {
                anyhow::bail!("Slack conversations.history for {channel_id}: ratelimited");
            }
            // not_in_channel or missing scope — skip gracefully, keeping any earlier page
            tracing::debug!(
                "Skipping channel {channel_id}: {}",
                resp.error.as_deref().unwrap_or("unknown error")
            );
            return Ok((all, false));
        }

        all.extend(resp.messages);

        cursor = resp.response_metadata.next_cursor;
        if cursor.is_empty() {
            return Ok((all, false));
        }
    }

    // Newest come first, so the older messages were dropped; the caller records them as a
    // gap to drain next run. Loud on purpose.
    warn!(
        "Slack history for {channel_id} stopped at the {MAX_PAGES}-page cap ({} messages); older messages were not fetched",
        all.len()
    );
    Ok((all, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A history page. Slack signals "done" with an empty next_cursor, not a missing one,
    /// so `next` of None is rendered as `""` to match the real API.
    fn history_page(start: usize, count: usize, next: Option<&str>) -> String {
        let msgs: Vec<String> = (start..start + count)
            .map(|i| format!("{{\"ts\":\"{i}.0\",\"type\":\"message\",\"text\":\"m{i}\"}}"))
            .collect();
        format!(
            "{{\"ok\":true,\"messages\":[{}],\"response_metadata\":{{\"next_cursor\":\"{}\"}}}}",
            msgs.join(","),
            next.unwrap_or("")
        )
    }

    fn channels_page(start: usize, count: usize, next: Option<&str>) -> String {
        let chans: Vec<String> = (start..start + count)
            .map(|i| format!("{{\"id\":\"C{i}\",\"name\":\"c{i}\",\"is_member\":true}}"))
            .collect();
        format!(
            "{{\"ok\":true,\"channels\":[{}],\"response_metadata\":{{\"next_cursor\":\"{}\"}}}}",
            chans.join(","),
            next.unwrap_or("")
        )
    }

    /// Serves one canned reply per connection and records the `cursor` each request
    /// carried, so a test can prove the cursor is actually sent back.
    async fn serve(replies: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let (base, raw) = serve_raw(replies).await;
        let handle = tokio::spawn(async move {
            raw.await
                .unwrap()
                .iter()
                .map(|req| {
                    req.split("&cursor=")
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
    async fn history_follows_the_cursor() {
        let (base, server) = serve(vec![
            history_page(0, PAGE_LIMIT as usize, Some("cur-2")),
            history_page(200, 11, None),
        ])
        .await;
        let client = reqwest::Client::new();

        let (msgs, capped) = fetch_history(&client, "t", &base, "C1", "0", None)
            .await
            .unwrap();

        assert!(!capped);
        // Before the fix only the first page was read, so this was 200, not 211.
        assert_eq!(msgs.len(), PAGE_LIMIT as usize + 11);
        assert_eq!(server.await.unwrap(), vec!["".to_string(), "cur-2".into()]);
    }

    #[tokio::test]
    async fn channels_follow_the_cursor() {
        let (base, server) = serve(vec![
            channels_page(0, PAGE_LIMIT as usize, Some("cur-2")),
            channels_page(200, 3, None),
        ])
        .await;
        let client = reqwest::Client::new();

        let chans = fetch_channels(&client, "t", &base, None).await.unwrap();

        assert_eq!(chans.len(), PAGE_LIMIT as usize + 3);
        assert_eq!(server.await.unwrap(), vec!["".to_string(), "cur-2".into()]);
    }

    #[tokio::test]
    async fn an_empty_cursor_ends_the_walk() {
        let (base, server) = serve(vec![history_page(0, 5, None)]).await;
        let client = reqwest::Client::new();

        let (msgs, capped) = fetch_history(&client, "t", &base, "C1", "0", None)
            .await
            .unwrap();

        assert!(!capped);
        assert_eq!(msgs.len(), 5);
        assert_eq!(server.await.unwrap(), vec!["".to_string()]);
    }

    #[tokio::test]
    async fn a_not_ok_reply_keeps_the_pages_already_read() {
        // Page 1 succeeds and hands back a cursor; page 2 fails with not_in_channel.
        // The first page's messages must survive rather than being thrown away.
        let (base, _server) = serve(vec![
            history_page(0, 7, Some("cur-2")),
            "{\"ok\":false,\"error\":\"not_in_channel\"}".to_string(),
        ])
        .await;
        let client = reqwest::Client::new();

        let (msgs, capped) = fetch_history(&client, "t", &base, "C1", "0", None)
            .await
            .unwrap();

        assert!(!capped);
        assert_eq!(msgs.len(), 7);
    }

    #[tokio::test]
    async fn a_not_ok_channel_list_is_still_an_error() {
        let (base, _server) =
            serve(vec!["{\"ok\":false,\"error\":\"invalid_auth\"}".to_string()]).await;
        let client = reqwest::Client::new();

        // Matched rather than unwrap_err so Channel does not need a Debug derive it
        // has no other use for.
        let err = match fetch_channels(&client, "t", &base, None).await {
            Ok(_) => panic!("a not-ok conversations.list should be an error"),
            Err(e) => e,
        };

        assert!(err.to_string().contains("invalid_auth"), "{err}");
    }

    #[tokio::test]
    async fn history_stops_at_the_page_cap() {
        // Every page keeps handing back a cursor, so only the cap ends the loop.
        let replies = (0..MAX_PAGES)
            .map(|p| history_page(p as usize * 200, 200, Some("always-more")))
            .collect();
        let (base, server) = serve(replies).await;
        let client = reqwest::Client::new();

        // The server accepts exactly MAX_PAGES connections, so an 11th request would
        // be refused and this would be an Err.
        let (msgs, capped) = fetch_history(&client, "t", &base, "C1", "0", None)
            .await
            .unwrap();

        assert!(capped);
        assert_eq!(msgs.len(), (MAX_PAGES * 200) as usize);
        assert_eq!(server.await.unwrap().len(), MAX_PAGES as usize);
    }

    /// Requests made by `pull_from` for one channel, picked out by `channel=`.
    fn for_channel<'a>(reqs: &'a [String], id: &str) -> Vec<&'a String> {
        let tag = format!("channel={id}&");
        reqs.iter().filter(|r| r.contains(&tag)).collect()
    }

    #[tokio::test]
    async fn a_capped_channel_sets_the_gap_and_the_drain_sends_latest() {
        let path = crate::memory::tests::temp_db_path("slack-gap");
        let store = crate::memory::tests::store_at(&path);
        let client = reqwest::Client::new();
        let pages = MAX_PAGES as usize;

        // Run 1: C1 is capped, one message per page, newest first from ts 1000. C2 has one
        // message, newer than C1's oldest, so taking it into account would raise the gap.
        let mut replies: Vec<String> = (0..pages)
            .map(|i| history_page(1000 - i, 1, Some("more")))
            .collect();
        replies.push(history_page(2000, 1, None));
        let (base, server) = serve_raw(replies).await;
        let started = Utc::now();
        pull_from(&client, "t", &base, Some("C1,C2"), &store, started)
            .await
            .unwrap();
        server.await.unwrap();

        let c1_oldest = from_ts(&format!("{}.0", 1000 - (pages - 1))).unwrap();
        assert_eq!(store.last_pull_at("slack"), Some(started));
        assert_eq!(store.pull_gap("slack").map(|g| g.1), Some(c1_oldest));

        // Run 2: both channels drain the gap, bounded by latest=, and it closes.
        let (base, server) =
            serve_raw(vec![history_page(500, 1, None), history_page(400, 1, None)]).await;
        pull_from(&client, "t", &base, Some("C1,C2"), &store, Utc::now())
            .await
            .unwrap();
        let reqs = server.await.unwrap();

        let latest = format!("&latest={}&inclusive=true", to_ts(c1_oldest));
        for id in ["C1", "C2"] {
            let sent = for_channel(&reqs, id);
            assert!(sent.len() == 1 && sent[0].contains(&latest), "{sent:?}");
        }
        assert_eq!(store.pull_gap("slack"), None);
        assert_eq!(store.last_pull_at("slack"), Some(started));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn rate_limited_mid_run_fails_and_logs_no_pull() {
        let path = crate::memory::tests::temp_db_path("slack-ratelimit");
        let store = crate::memory::tests::store_at(&path);

        // Page 1 succeeds; page 2 is rate limited. Before, that was a partial Ok and the
        // marker moved past the skipped messages.
        let (base, _server) = serve_raw(vec![
            history_page(0, 7, Some("cur-2")),
            "{\"ok\":false,\"error\":\"ratelimited\"}".to_string(),
        ])
        .await;
        let result = pull_from(
            &reqwest::Client::new(),
            "t",
            &base,
            Some("C1"),
            &store,
            Utc::now(),
        )
        .await;

        let err = result.expect_err("a rate-limited page must fail the run");
        assert!(err.to_string().contains("ratelimited"), "{err}");
        assert_eq!(store.last_pull_at("slack"), None);
        assert_eq!(store.pull_gap("slack"), None);

        let _ = std::fs::remove_file(path);
    }
}
