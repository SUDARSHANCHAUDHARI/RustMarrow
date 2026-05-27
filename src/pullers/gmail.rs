use crate::{
    config::Config,
    google_auth,
    memory::{Chunk, Store},
};
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde::Deserialize;
use tracing::info;

const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

#[derive(Deserialize)]
struct MessageList {
    #[serde(default)]
    messages: Vec<MessageRef>,
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
    let (client_id, client_secret, refresh_token) = cfg
        .google_creds()
        .context("Google credentials not set — run `marrow auth google` first")?;

    let client = reqwest::Client::builder()
        .user_agent("marrow/0.1")
        .build()?;

    let token = google_auth::access_token(&client, client_id, client_secret, refresh_token).await?;

    // Incremental: fetch only since last pull (fallback to 1 day)
    let since = store
        .last_pull_at("gmail")
        .unwrap_or_else(|| Utc::now() - Duration::days(1));

    // Gmail after: filter uses epoch seconds
    let after_ts = since.timestamp();
    let query = format!(
        "after:{after_ts} (is:unread OR is:important) -category:promotions -category:social"
    );

    info!("Pulling Gmail since {}", since.format("%Y-%m-%d %H:%M"));

    let url = format!(
        "{GMAIL_API}/messages?maxResults=50&q={}",
        urlencoding::encode(&query)
    );

    let list = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()?
        .json::<MessageList>()
        .await?;

    info!("{} new messages", list.messages.len());

    let mut total = 0usize;
    for msg_ref in &list.messages {
        let msg_url = format!("{GMAIL_API}/messages/{}?format=full", msg_ref.id);
        let msg = client
            .get(&msg_url)
            .bearer_auth(&token)
            .send()
            .await?
            .error_for_status()?
            .json::<Message>()
            .await?;

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

    store.log_pull("gmail", total)?;
    info!("Gmail pull complete: {} messages", total);
    Ok(())
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
