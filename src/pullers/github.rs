use crate::{
    config::Config,
    memory::{Chunk, Store},
};
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
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
    updated_at: String,
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

    // Watermark is the pull start, so items arriving mid-pull are fetched next time.
    let started = Utc::now();
    pull_from(&client, cfg, GITHUB_API, store, started).await
}

/// The pull itself. `api_base` is a parameter so a test can point it at a local server.
async fn pull_from(
    client: &reqwest::Client,
    cfg: &Config,
    api_base: &str,
    store: &Store,
    started: DateTime<Utc>,
) -> Result<()> {
    let (_, username) = cfg.github()?;

    // Incremental: fetch only since last pull (fallback to 7 days)
    let since = store
        .last_pull_at("github")
        .unwrap_or_else(|| Utc::now() - Duration::days(7));

    info!(
        "Pulling GitHub for {} (since {})",
        username,
        since.format("%Y-%m-%d %H:%M")
    );

    let repos = fetch_repos(client, cfg, api_base).await?;
    let active: Vec<_> = repos.iter().filter(|r| !r.archived).collect();
    info!("{} active repos", active.len());

    let mut total = 0usize;
    // The marker moves only up to the point below which everything was fetched.
    let mut safe_point = started;

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
        let (issues, capped) =
            fetch_issues(client, cfg, api_base, &repo.full_name, &since.to_rfc3339()).await?;
        if capped {
            // Oldest-updated first, so everything up to the last item is in. `since` is
            // inclusive: that item is refetched next run and the upsert dedupes it.
            let last = issues
                .last()
                .and_then(|i| DateTime::parse_from_rfc3339(&i.updated_at).ok())
                .map_or(since, |dt| dt.with_timezone(&Utc));
            safe_point = safe_point.min(last);
        }
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
        let (commits, capped) =
            fetch_commits(client, cfg, api_base, &repo.full_name, &since.to_rfc3339()).await?;
        if capped {
            // shortcut: commits hold the marker on cap; switch to the until= gap if a repo ever hits it
            safe_point = safe_point.min(since);
        }
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
                // Repo in the id: a fork and its upstream share SHAs.
                source_id: format!("commit:{}:{}", repo.full_name, &commit.sha[..8]),
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

    store.log_pull("github", total, safe_point)?;
    info!("GitHub pull complete: {} items", total);
    Ok(())
}

/// Walks `page=1,2,...` until a short page arrives, so nothing past the first page is
/// dropped. `base_url` must already carry its own query string. `tolerated` lists status
/// codes that mean "nothing here" rather than a failure. The flag is true when the page
/// cap, not the API, ended the walk.
async fn fetch_paged<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    cfg: &Config,
    base_url: &str,
    tolerated: &[u16],
) -> Result<(Vec<T>, bool)> {
    let mut all: Vec<T> = Vec::new();

    for page in 1..=MAX_PAGES {
        let url = format!("{base_url}&per_page={PER_PAGE}&page={page}");
        let resp = client.get(&url).bearer_auth(cfg.github()?.0).send().await?;

        if tolerated.contains(&resp.status().as_u16()) {
            return Ok((all, false));
        }

        let batch: Vec<T> = resp.error_for_status()?.json().await?;
        let short_page = batch.len() < PER_PAGE as usize;
        all.extend(batch);

        if short_page {
            return Ok((all, false));
        }
    }

    // Hit the cap with a full last page. The caller holds the marker back so the rest is
    // fetched on a later run. Loud on purpose.
    warn!(
        "{base_url}: stopped at the {MAX_PAGES}-page cap ({} items); later items were not fetched",
        all.len()
    );
    Ok((all, true))
}

async fn fetch_repos(client: &reqwest::Client, cfg: &Config, api_base: &str) -> Result<Vec<Repo>> {
    let url = format!("{api_base}/users/{}/repos?sort=updated", cfg.github()?.1);
    // A capped repo list does not touch the marker: the dropped repos are the least
    // recently updated ones.
    fetch_paged(client, cfg, &url, &[])
        .await
        .map(|(repos, _)| repos)
}

/// Oldest-updated first, so a capped walk has a clean cut-off at its last item.
async fn fetch_issues(
    client: &reqwest::Client,
    cfg: &Config,
    api_base: &str,
    full_name: &str,
    since: &str,
) -> Result<(Vec<Issue>, bool)> {
    let url = format!(
        "{api_base}/repos/{full_name}/issues?state=all&since={}&sort=updated&direction=asc",
        urlencoding::encode(since)
    );
    fetch_paged(client, cfg, &url, &[]).await
}

async fn fetch_commits(
    client: &reqwest::Client,
    cfg: &Config,
    api_base: &str,
    full_name: &str,
    since: &str,
) -> Result<(Vec<Commit>, bool)> {
    let url = format!(
        "{api_base}/repos/{full_name}/commits?since={}&author={}",
        urlencoding::encode(since),
        cfg.github()?.1
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
            github_token: Some("t".into()),
            github_username: Some("u".into()),
            anthropic_api_key: Some("k".into()),
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
        let (base, raw) = serve_raw(replies).await;
        let handle = tokio::spawn(async move {
            raw.await
                .unwrap()
                .iter()
                .map(|req| {
                    // "&page=" and not "page=", so per_page is not mistaken for it
                    req.split("&page=")
                        .nth(1)
                        .map(|s| s.trim_start_matches(|c: char| !c.is_ascii_digit()))
                        .and_then(|s| {
                            s.split(|c: char| !c.is_ascii_digit())
                                .next()
                                .and_then(|d| d.parse::<u32>().ok())
                        })
                        .unwrap_or(0)
                })
                .collect()
        });
        (format!("{base}/items?state=open"), handle)
    }

    /// As `serve`, but yields each raw request and returns the bare `http://addr` base.
    async fn serve_raw(
        replies: Vec<(&'static str, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
            seen
        });
        (format!("http://{addr}"), handle)
    }

    const REPOS: &str = r#"[{"name":"r","full_name":"u/r","description":null,"html_url":"h","updated_at":"x","open_issues_count":0}]"#;

    /// A full page of issues updated one minute apart from `start`. Marked as pull requests
    /// so the pull skips ingesting them; only the marker logic is under test.
    fn issues_page(start: usize) -> String {
        let items: Vec<String> = (start..start + PER_PAGE as usize)
            .map(|i| {
                format!(
                    r#"{{"number":{i},"title":"t","body":null,"html_url":"h","state":"open","pull_request":{{}},"updated_at":"{}"}}"#,
                    issue_time(i).format("%Y-%m-%dT%H:%M:%SZ")
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn issue_time(i: usize) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + i as i64 * 60, 0).unwrap()
    }

    fn commits_page(start: usize) -> String {
        let items: Vec<String> = (start..start + PER_PAGE as usize)
            .map(|i| {
                format!(
                    r#"{{"sha":"{i:040x}","html_url":"h","commit":{{"message":"m","author":{{"date":"d"}}}}}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    #[tokio::test]
    async fn capped_issues_move_the_marker_to_the_last_one_fetched() {
        let path = crate::memory::tests::temp_db_path("gh-issues");
        let store = crate::memory::tests::store_at(&path);
        let client = reqwest::Client::new();

        // Run 1: issues hit the cap, commits are empty.
        let mut replies = vec![("200 OK", REPOS.to_string())];
        replies.extend((0..MAX_PAGES as usize).map(|p| ("200 OK", issues_page(p * 100))));
        replies.push(("200 OK", "[]".to_string()));
        let (base, server) = serve_raw(replies).await;
        let started = Utc::now();
        pull_from(&client, &test_cfg(), &base, &store, started)
            .await
            .unwrap();
        let reqs = server.await.unwrap();

        assert!(
            reqs[1].contains("sort=updated&direction=asc"),
            "{}",
            reqs[1]
        );
        let last = issue_time(MAX_PAGES as usize * 100 - 1);
        assert_eq!(store.last_pull_at("github"), Some(last));

        // Run 2: nothing capped, so the marker moves to this run's start, and the issue
        // request starts from the run-1 safe point.
        let (base, server) = serve_raw(vec![
            ("200 OK", REPOS.to_string()),
            ("200 OK", "[]".to_string()),
            ("200 OK", "[]".to_string()),
        ])
        .await;
        let started = Utc::now();
        pull_from(&client, &test_cfg(), &base, &store, started)
            .await
            .unwrap();
        let reqs = server.await.unwrap();

        assert!(
            reqs[1].contains(&format!(
                "since={}",
                urlencoding::encode(&last.to_rfc3339())
            )),
            "{}",
            reqs[1]
        );
        assert_eq!(store.last_pull_at("github"), Some(started));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn capped_commits_hold_the_marker() {
        let path = crate::memory::tests::temp_db_path("gh-commits");
        let store = crate::memory::tests::store_at(&path);
        let marker = Utc::now() - Duration::days(1);
        store.log_pull("github", 0, marker).unwrap();

        let mut replies = vec![("200 OK", REPOS.to_string()), ("200 OK", "[]".to_string())];
        replies.extend((0..MAX_PAGES as usize).map(|p| ("200 OK", commits_page(p * 100))));
        let (base, _server) = serve_raw(replies).await;
        pull_from(
            &reqwest::Client::new(),
            &test_cfg(),
            &base,
            &store,
            Utc::now(),
        )
        .await
        .unwrap();

        assert_eq!(store.last_pull_at("github"), Some(marker));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn since_is_percent_encoded_so_plus_is_not_read_as_a_space() {
        let (base, server) = serve_raw(vec![("200 OK", "[]".to_string())]).await;

        fetch_issues(
            &reqwest::Client::new(),
            &test_cfg(),
            &base,
            "u/r",
            "2026-11-01T00:00:00+00:00",
        )
        .await
        .unwrap();

        let reqs = server.await.unwrap();
        assert!(
            reqs[0].contains("since=2026-11-01T00%3A00%3A00%2B00%3A00"),
            "{}",
            reqs[0]
        );
    }

    #[tokio::test]
    async fn walks_every_page_until_a_short_one() {
        let (url, server) = serve(vec![
            ("200 OK", page_of(PER_PAGE as usize)),
            ("200 OK", page_of(5)),
        ])
        .await;
        let client = reqwest::Client::new();

        let (items, capped): (Vec<Item>, _) =
            fetch_paged(&client, &test_cfg(), &url, &[]).await.unwrap();

        assert!(!capped);
        // Before the fix only the first page was read, so this was 100, not 105.
        assert_eq!(items.len(), PER_PAGE as usize + 5);
        assert_eq!(server.await.unwrap(), vec![1, 2]);
    }

    #[tokio::test]
    async fn stops_on_a_tolerated_status_without_erroring() {
        let (url, server) = serve(vec![("409 Conflict", String::new())]).await;
        let client = reqwest::Client::new();

        let (items, capped): (Vec<Item>, _) = fetch_paged(&client, &test_cfg(), &url, &[404, 409])
            .await
            .unwrap();

        assert!(items.is_empty() && !capped);
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
        let (items, capped): (Vec<Item>, _) =
            fetch_paged(&client, &test_cfg(), &url, &[]).await.unwrap();

        assert!(capped);
        assert_eq!(items.len(), (MAX_PAGES * PER_PAGE) as usize);
        assert_eq!(server.await.unwrap(), (1..=MAX_PAGES).collect::<Vec<_>>());
    }
}
