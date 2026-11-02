use crate::{
    config::Config,
    memory::{Chunk, Store},
};
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
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
    let token = cfg
        .slack_token
        .as_deref()
        .context("SLACK_TOKEN not set in .env")?;

    let client = reqwest::Client::builder()
        .user_agent("marrow/0.1")
        .build()?;

    // Incremental: only fetch since last pull (fallback to 24h)
    let since = store
        .last_pull_at("slack")
        .unwrap_or_else(|| Utc::now() - Duration::hours(24));
    let oldest = since.timestamp().to_string();

    info!("Pulling Slack since {}", since.format("%Y-%m-%d %H:%M"));

    let channels = fetch_channels(&client, token, SLACK_API, cfg.slack_channels.as_deref()).await?;
    info!("{} channels to scan", channels.len());

    let mut total = 0usize;
    for channel in &channels {
        let name =
            channel
                .name
                .as_deref()
                .unwrap_or(if channel.is_dm { "DM" } else { &channel.id });
        let messages = fetch_history(&client, token, SLACK_API, &channel.id, &oldest).await?;

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

    store.log_pull("slack", total)?;
    info!("Slack pull complete: {} messages", total);
    Ok(())
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

async fn fetch_history(
    client: &reqwest::Client,
    token: &str,
    api_base: &str,
    channel_id: &str,
    oldest: &str,
) -> Result<Vec<SlackMessage>> {
    let mut all: Vec<SlackMessage> = Vec::new();
    let mut cursor = String::new();

    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{api_base}/conversations.history?channel={channel_id}&oldest={oldest}&limit={PAGE_LIMIT}"
        );
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
            // not_in_channel or missing scope — skip gracefully, keeping any earlier page
            tracing::debug!(
                "Skipping channel {channel_id}: {}",
                resp.error.as_deref().unwrap_or("unknown error")
            );
            return Ok(all);
        }

        all.extend(resp.messages);

        cursor = resp.response_metadata.next_cursor;
        if cursor.is_empty() {
            return Ok(all);
        }
    }

    // Those messages are lost for good, because the pull watermark moves forward
    // regardless. Loud on purpose.
    warn!(
        "Slack history for {channel_id} stopped at the {MAX_PAGES}-page cap ({} messages); later messages were not fetched",
        all.len()
    );
    Ok(all)
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut cursors = Vec::new();
            for body in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let sent = req
                    .split("&cursor=")
                    .nth(1)
                    .map(|s| {
                        s.split(|c: char| c == '&' || c.is_whitespace())
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .unwrap_or_default();
                cursors.push(sent);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
            cursors
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

        let msgs = fetch_history(&client, "t", &base, "C1", "0").await.unwrap();

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

        let msgs = fetch_history(&client, "t", &base, "C1", "0").await.unwrap();

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

        let msgs = fetch_history(&client, "t", &base, "C1", "0").await.unwrap();

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
        let msgs = fetch_history(&client, "t", &base, "C1", "0").await.unwrap();

        assert_eq!(msgs.len(), (MAX_PAGES * 200) as usize);
        assert_eq!(server.await.unwrap().len(), MAX_PAGES as usize);
    }
}
