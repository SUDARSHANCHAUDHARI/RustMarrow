use crate::{
    config::Config,
    memory::{Chunk, Store},
};
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde::Deserialize;
use tracing::info;

const SLACK_API: &str = "https://slack.com/api";

#[derive(Deserialize)]
struct ConversationsListResp {
    ok: bool,
    #[serde(default)]
    channels: Vec<Channel>,
    error: Option<String>,
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

    let channels = fetch_channels(&client, token, cfg.slack_channels.as_deref()).await?;
    info!("{} channels to scan", channels.len());

    let mut total = 0usize;
    for channel in &channels {
        let name = channel.name.as_deref().unwrap_or(if channel.is_dm { "DM" } else { &channel.id });
        let messages = fetch_history(&client, token, &channel.id, &oldest).await?;

        for msg in &messages {
            // Skip bot messages, join/leave events
            if msg.msg_type != "message" { continue; }
            if matches!(msg.subtype.as_deref(), Some("bot_message") | Some("channel_join") | Some("channel_leave")) {
                continue;
            }

            let text = match &msg.text {
                Some(t) if !t.is_empty() => t.clone(),
                _ => continue,
            };

            // ts is a Unix timestamp string like "1234567890.123456"
            let ts_secs = msg.ts.split('.').next().unwrap_or("0").parse::<i64>().unwrap_or(0);
            let dt = chrono::DateTime::from_timestamp(ts_secs, 0)
                .unwrap_or_default()
                .format("%Y-%m-%d %H:%M")
                .to_string();

            let user_ref = msg.user.as_deref().unwrap_or("unknown");

            store.ingest(&Chunk {
                source: "slack".into(),
                source_id: format!("{}:{}", channel.id, msg.ts),
                title: format!("Slack #{name}: {}", text.chars().take(60).collect::<String>()),
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

async fn fetch_channels(
    client: &reqwest::Client,
    token: &str,
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

    // Otherwise: fetch all conversations the user/bot is a member of
    let url = format!(
        "{SLACK_API}/conversations.list?types=public_channel,private_channel,im&limit=200&exclude_archived=true"
    );
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

    Ok(resp.channels.into_iter().filter(|c| c.is_member || c.is_dm).collect())
}

async fn fetch_history(
    client: &reqwest::Client,
    token: &str,
    channel_id: &str,
    oldest: &str,
) -> Result<Vec<SlackMessage>> {
    let url = format!(
        "{SLACK_API}/conversations.history?channel={channel_id}&oldest={oldest}&limit=100"
    );
    let resp: HistoryResp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if !resp.ok {
        // not_in_channel or missing scope — skip gracefully
        tracing::debug!("Skipping channel {channel_id}: {}", resp.error.as_deref().unwrap_or("unknown error"));
        return Ok(vec![]);
    }

    Ok(resp.messages)
}
