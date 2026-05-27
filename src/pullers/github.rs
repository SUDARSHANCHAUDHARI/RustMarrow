use crate::{
    config::Config,
    memory::{Chunk, Store},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use serde::Deserialize;
use tracing::info;

const GITHUB_API: &str = "https://api.github.com";

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

async fn fetch_repos(client: &reqwest::Client, cfg: &Config) -> Result<Vec<Repo>> {
    let url = format!(
        "{GITHUB_API}/users/{}/repos?per_page=100&sort=updated",
        cfg.github_username
    );
    Ok(client
        .get(&url)
        .bearer_auth(&cfg.github_token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<Repo>>()
        .await?)
}

async fn fetch_issues(
    client: &reqwest::Client,
    cfg: &Config,
    full_name: &str,
    since: &str,
) -> Result<Vec<Issue>> {
    let url = format!("{GITHUB_API}/repos/{full_name}/issues?state=open&per_page=50&since={since}");
    Ok(client
        .get(&url)
        .bearer_auth(&cfg.github_token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<Issue>>()
        .await?)
}

async fn fetch_commits(
    client: &reqwest::Client,
    cfg: &Config,
    full_name: &str,
    since: &str,
) -> Result<Vec<Commit>> {
    let url = format!(
        "{GITHUB_API}/repos/{full_name}/commits?per_page=20&since={since}&author={}",
        cfg.github_username
    );
    let resp = client
        .get(&url)
        .bearer_auth(&cfg.github_token)
        .send()
        .await?;

    // 409 = empty repo, 404 = no access — skip gracefully
    if matches!(resp.status().as_u16(), 404 | 409) {
        return Ok(vec![]);
    }

    Ok(resp.error_for_status()?.json::<Vec<Commit>>().await?)
}
