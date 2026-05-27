use crate::{config::Config, memory::Store};
use anyhow::Result;
use serde_json::json;

const CLAUDE_MODEL: &str = "claude-sonnet-4-6";
const MAX_CONTEXT_CHUNKS: usize = 20;
/// Rough token budget: 1 token ≈ 4 chars. Keep well under 200k context limit.
const MAX_CONTEXT_CHARS: usize = 60_000 * 4;

pub async fn ask(cfg: &Config, store: &Store, question: &str) -> Result<String> {
    let client = reqwest::Client::new();

    let search_results = store.search(question)?;
    let recent = store.recent_context(MAX_CONTEXT_CHUNKS)?;

    // Merge: search results first (most relevant), then fill with recent
    let mut chunks = search_results;
    for r in recent {
        if !chunks.contains(&r) {
            chunks.push(r);
        }
    }

    // Token overflow guard — trim to budget
    let mut total_chars = 0usize;
    chunks.retain(|c| {
        if total_chars + c.len() <= MAX_CONTEXT_CHARS {
            total_chars += c.len();
            true
        } else {
            false
        }
    });

    let context = chunks.join("\n\n---\n\n");
    call_claude(cfg, &client, &context, question, 1024).await
}

pub async fn digest(cfg: &Config, store: &Store) -> Result<String> {
    let client = reqwest::Client::new();

    let calendar = store.chunks_by_source("calendar", 10)?;
    let commits = store.chunks_by_source("github", 15)?;
    let emails = store.chunks_by_source("gmail", 8)?;
    let slack = store.chunks_by_source("slack", 8)?;

    if calendar.is_empty() && commits.is_empty() && emails.is_empty() && slack.is_empty() {
        return Ok("No data in memory yet. Run `marrow pull` first.".into());
    }

    let mut sections: Vec<String> = vec![];
    if !calendar.is_empty() {
        sections.push(format!("## Calendar (upcoming)\n{}", calendar.join("\n\n")));
    }
    if !commits.is_empty() {
        sections.push(format!(
            "## GitHub (recent activity)\n{}",
            commits.join("\n\n")
        ));
    }
    if !emails.is_empty() {
        sections.push(format!("## Gmail (recent)\n{}", emails.join("\n\n")));
    }
    if !slack.is_empty() {
        sections.push(format!("## Slack (recent)\n{}", slack.join("\n\n")));
    }

    let context = sections.join("\n\n---\n\n");
    let prompt = "Generate a concise morning briefing. Structure:\n\
        1. Today's schedule — what's on the calendar\n\
        2. What I was working on — from commits and open issues\n\
        3. Emails needing attention\n\
        4. Slack follow-ups\n\n\
        Be specific and actionable. Skip sections with no data. No filler.";

    call_claude(cfg, &client, &context, prompt, 1500).await
}

async fn call_claude(
    cfg: &Config,
    client: &reqwest::Client,
    context: &str,
    user_message: &str,
    max_tokens: u32,
) -> Result<String> {
    let system = format!(
        "You are Marrow — a personal AI assistant with full context of Sudarshan's \
        work across GitHub, Gmail, Calendar, and Slack. \
        Answer with direct specificity. No filler. No disclaimers.\n\n\
        ## Memory context\n\n{context}"
    );

    let body = json!({
        "model": CLAUDE_MODEL,
        "max_tokens": max_tokens,
        "system": [
            {
                "type": "text",
                "text": system,
                "cache_control": {"type": "ephemeral"}
            }
        ],
        "messages": [
            {"role": "user", "content": user_message}
        ]
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &cfg.anthropic_api_key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "prompt-caching-2024-07-31")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;

    Ok(resp["content"][0]["text"]
        .as_str()
        .unwrap_or("(no response)")
        .to_string())
}
