use crate::{
    config::Config,
    memory::{Chunk, Store},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use serde::Deserialize;
use tracing::{info, warn};

const GITHUB_API: &str = "https://api.github.com";
const PER_PAGE: u32 = 100;
/// Safety cap so a misbehaving API cannot spin the page loop forever.
const MAX_PAGES: u32 = 10;

#[derive(Deserialize)]
struct Repo {
    name: String,
    full_name: String,
    description: Option<String>,
    html_url: String,
    updated_at: String,
    open_issues_count: u32,
    #[serde(default)]
    archived: bool,
}

#[derive(Deserialize)]
struct Issue {
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    state: String,
    #[serde(default)]
    labels: Vec<Label>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Label {
    name: String,
}

#[derive(Deserialize)]
struct Commit {
    sha: String,
    commit: CommitDetail,
    html_url: String,
}

#[derive(Deserialize)]
struct CommitDetail {
    message: String,
    author: CommitAuthor,
}

#[derive(Deserialize)]
struct CommitAuthor {
    date: String,
}

pub async fn pull(cfg: &Config, store: &Store) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("marrow/0.1")
        .build()?;

    // Incremental: fetch only since last pull (fallback to 7 days)
    let since = store
        .last_pull_at("github")
        .unwrap_or_else(|| Utc::now() - Duration::days(7));

    info!(
        "Pulling GitHub for {} (since {})",
        cfg.github_username,
        since.format("%Y-%m-%d %H:%M")
    );

    let repos = fetch_repos(&client, cfg).await?;
    let active: Vec<_> = repos.iter().filter(|r| !r.archived).collect();
    info!("{} active repos", active.len());

    let mut total = 0usize;

    for repo in &active {
        // Always refresh repo summaries (cheap, gives current issue count)
        let content = format!(
            "{}\n\nOpen issues: {}\nLast updated: {}",
            repo.description.as_deref().unwrap_or("No description"),
            repo.open_issues_count,
            repo.updated_at
        );
        store.ingest(&Chunk {
            source: "github".into(),
            source_id: format!("repo:{}", repo.full_name),
            title: format!("Repo: {}", repo.name),
            content,
            url: Some(repo.html_url.clone()),
            tags: vec!["github".into(), "repo".into()],
        })?;
        total += 1;

        // Incremental issues — only those updated since last pull
        let issues = fetch_issues(&client, cfg, &repo.full_name, &since.to_rfc3339()).await?;
        for issue in &issues {
            if issue.pull_request.is_some() {
                continue;
            }
            let labels: Vec<_> = issue.labels.iter().map(|l| l.name.clone()).collect();
            let body_preview: String = issue
                .body
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(500)
                .collect();
            store.ingest(&Chunk {
                source: "github".into(),
                source_id: format!("issue:{}:{}", repo.full_name, issue.number),
                title: format!("Issue #{}: {}", issue.number, issue.title),
                content: format!(
                    "State: {}\nLabels: {}\n\n{}",
                    issue.state,
                    labels.join(", "),
                    body_preview
                ),
                url: Some(issue.html_url.clone()),
                tags: {
                    let mut t = vec!["github".into(), "issue".into()];
                    t.extend(labels);
                    t
                },
            })?;
            total += 1;
        }

        // Incremental commits — only since last pull
        let commits = fetch_commits(&client, cfg, &repo.full_name, &since.to_rfc3339()).await?;
        for commit in &commits {
            let first_line = commit
                .commit
                .message
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            store.ingest(&Chunk {
                source: "github".into(),
                source_id: format!("commit:{}", &commit.sha[..8]),
                title: format!("[{}] {}", repo.name, first_line),
                content: format!(
                    "Date: {}\n\n{}",
                    commit.commit.author.date, commit.commit.message
                ),
                url: Some(commit.html_url.clone()),
                tags: vec!["github".into(), "commit".into(), repo.name.clone()],
            })?;
            total += 1;
        }
    }

    store.log_pull("github", total)?;
    info!("GitHub pull complete: {} items", total);
    Ok(())
}

/// Walks `page=1,2,...` until a short page arrives, so nothing past the first page is
/// dropped. `base_url` must already carry its own query string. `tolerated` lists status
/// codes that mean "nothing here" rather than a failure.
async fn fetch_paged<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    cfg: &Config,
    base_url: &str,
    tolerated: &[u16],
) -> Result<Vec<T>> {
    let mut all: Vec<T> = Vec::new();

    for page in 1..=MAX_PAGES {
        let url = format!("{base_url}&per_page={PER_PAGE}&page={page}");
        let resp = client
            .get(&url)
            .bearer_auth(&cfg.github_token)
            .send()
            .await?;

        if tolerated.contains(&resp.status().as_u16()) {
            return Ok(all);
        }

        let batch: Vec<T> = resp.error_for_status()?.json().await?;
        let short_page = batch.len() < PER_PAGE as usize;
        all.extend(batch);

        if short_page {
            return Ok(all);
        }
    }

    // Hit the cap with a full last page: items beyond it are lost for good, because the
    // pull watermark still moves forward. Loud on purpose.
    warn!(
        "{base_url}: stopped at the {MAX_PAGES}-page cap ({} items); later items were not fetched",
        all.len()
    );
    Ok(all)
}

async fn fetch_repos(client: &reqwest::Client, cfg: &Config) -> Result<Vec<Repo>> {
    let url = format!(
        "{GITHUB_API}/users/{}/repos?sort=updated",
        cfg.github_username
    );
    fetch_paged(client, cfg, &url, &[]).await
}

async fn fetch_issues(
    client: &reqwest::Client,
    cfg: &Config,
    full_name: &str,
    since: &str,
) -> Result<Vec<Issue>> {
    let url = format!("{GITHUB_API}/repos/{full_name}/issues?state=open&since={since}");
    fetch_paged(client, cfg, &url, &[]).await
}

async fn fetch_commits(
    client: &reqwest::Client,
    cfg: &Config,
    full_name: &str,
    since: &str,
) -> Result<Vec<Commit>> {
    let url = format!(
        "{GITHUB_API}/repos/{full_name}/commits?since={since}&author={}",
        cfg.github_username
    );
    // 409 = empty repo, 404 = no access — skip gracefully
    fetch_paged(client, cfg, &url, &[404, 409]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Deserialize)]
    struct Item {
        #[allow(dead_code)]
        n: u32,
    }

    fn test_cfg() -> Config {
        Config {
            github_token: "t".into(),
            github_username: "u".into(),
            anthropic_api_key: "k".into(),
            db_path: PathBuf::from("/tmp/marrow-test.db"),
            obsidian_vault_path: None,
            google_client_id: None,
            google_client_secret: None,
            google_refresh_token: None,
            slack_token: None,
            slack_channels: None,
        }
    }

    fn page_of(count: usize) -> String {
        let items: Vec<String> = (0..count).map(|i| format!("{{\"n\":{i}}}")).collect();
        format!("[{}]", items.join(","))
    }

    /// Serves one canned reply per connection and records the `page=` it was asked for.
    /// Returns the base URL and a handle yielding the pages actually requested.
    async fn serve(
        replies: Vec<(&'static str, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<u32>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                // "&page=" and not "page=", so per_page is not mistaken for it
                let page = req
                    .split("&page=")
                    .nth(1)
                    .map(|s| s.trim_start_matches(|c: char| !c.is_ascii_digit()))
                    .and_then(|s| {
                        s.split(|c: char| !c.is_ascii_digit())
                            .next()
                            .and_then(|d| d.parse::<u32>().ok())
                    })
                    .unwrap_or(0);
                seen.push(page);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
            seen
        });
        (format!("http://{addr}/items?state=open"), handle)
    }

    #[tokio::test]
    async fn walks_every_page_until_a_short_one() {
        let (url, server) = serve(vec![
            ("200 OK", page_of(PER_PAGE as usize)),
            ("200 OK", page_of(5)),
        ])
        .await;
        let client = reqwest::Client::new();

        let items: Vec<Item> = fetch_paged(&client, &test_cfg(), &url, &[]).await.unwrap();

        // Before the fix only the first page was read, so this was 100, not 105.
        assert_eq!(items.len(), PER_PAGE as usize + 5);
        assert_eq!(server.await.unwrap(), vec![1, 2]);
    }

    #[tokio::test]
    async fn stops_on_a_tolerated_status_without_erroring() {
        let (url, server) = serve(vec![("409 Conflict", String::new())]).await;
        let client = reqwest::Client::new();

        let items: Vec<Item> = fetch_paged(&client, &test_cfg(), &url, &[404, 409])
            .await
            .unwrap();

        assert!(items.is_empty());
        assert_eq!(server.await.unwrap(), vec![1]);
    }

    #[tokio::test]
    async fn stops_at_the_page_cap() {
        let replies = (0..MAX_PAGES)
            .map(|_| ("200 OK", page_of(PER_PAGE as usize)))
            .collect();
        let (url, server) = serve(replies).await;
        let client = reqwest::Client::new();

        // The server accepts exactly MAX_PAGES connections, so an 11th request would
        // be refused and this would be an Err.
        let items: Vec<Item> = fetch_paged(&client, &test_cfg(), &url, &[]).await.unwrap();

        assert_eq!(items.len(), (MAX_PAGES * PER_PAGE) as usize);
        assert_eq!(server.await.unwrap(), (1..=MAX_PAGES).collect::<Vec<_>>());
    }
}
