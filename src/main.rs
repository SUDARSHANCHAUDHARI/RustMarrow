mod agent;
mod config;
mod db;
mod google_auth;
mod memory;
mod pullers;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "marrow", about = "Personal local AI memory agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pull data from configured sources into memory
    Pull {
        /// Source: github, gmail, calendar, slack (omit for all)
        #[arg(short, long)]
        source: Option<String>,
    },
    /// Morning briefing — schedule, commits, emails, slack
    Digest,
    /// Search memory without asking Claude
    Search {
        query: String,
        /// Max results (default 10)
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Ask Marrow a question using your memory context
    Ask {
        question: String,
    },
    /// Delete all memory for a source
    Clear {
        /// Source to clear: github, gmail, calendar, slack
        source: String,
    },
    /// Delete one chunk by id (get id from `marrow search`)
    Forget {
        id: i64,
    },
    /// Open the Obsidian vault Marrow folder (macOS)
    Open,
    /// Show memory stats
    Status,
    /// Authenticate with a provider and print the refresh token
    Auth {
        /// Provider: google
        provider: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("marrow=info".parse()?))
        .init();

    let cli = Cli::parse(); // parse first so --help works without .env
    let cfg = config::Config::load()?;

    // Auth doesn't need the DB
    if let Command::Auth { provider } = &cli.command {
        return match provider.as_str() {
            "google" => {
                let client = reqwest::Client::new();
                let id = cfg
                    .google_client_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("GOOGLE_CLIENT_ID not set in .env"))?;
                let secret = cfg
                    .google_client_secret
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("GOOGLE_CLIENT_SECRET not set in .env"))?;
                google_auth::authorize_and_print_refresh_token(&client, id, secret).await
            }
            p => anyhow::bail!("Unknown provider '{p}'. Supported: google"),
        };
    }

    let db = db::Database::open(&cfg.db_path)?;
    let store = memory::Store::new(db, cfg.db_path.clone(), cfg.obsidian_vault_path.clone());

    match cli.command {
        Command::Pull { source } => match source.as_deref() {
            Some("github") => pullers::github::pull(&cfg, &store).await?,
            Some("gmail") => pullers::gmail::pull(&cfg, &store).await?,
            Some("calendar") => pullers::calendar::pull(&cfg, &store).await?,
            Some("slack") => pullers::slack::pull(&cfg, &store).await?,
            None => {
                // GitHub always runs; Google + Slack run concurrently if configured
                pullers::github::pull(&cfg, &store).await?;

                let gmail_fut = async {
                    if cfg.google_creds().is_some() {
                        pullers::gmail::pull(&cfg, &store).await
                    } else {
                        Ok(())
                    }
                };
                let cal_fut = async {
                    if cfg.google_creds().is_some() {
                        pullers::calendar::pull(&cfg, &store).await
                    } else {
                        Ok(())
                    }
                };
                let slack_fut = async {
                    if cfg.slack_token.is_some() {
                        pullers::slack::pull(&cfg, &store).await
                    } else {
                        Ok(())
                    }
                };

                let (r1, r2, r3) = tokio::join!(gmail_fut, cal_fut, slack_fut);
                r1?; r2?; r3?;
            }
            Some(s) => anyhow::bail!(
                "Unknown source '{s}'. Supported: github, gmail, calendar, slack"
            ),
        },

        Command::Digest => {
            let briefing = agent::digest(&cfg, &store).await?;
            println!("{briefing}");
        }

        Command::Search { query, limit } => {
            let results = store.search_display(&query, limit)?;
            if results.is_empty() {
                println!("No results for \"{query}\"");
            } else {
                println!("Found {} result(s) for \"{query}\"\n", results.len());
                for r in &results {
                    println!("[{}] {} (id: {})", r.source, r.title, r.id);
                    if let Some(url) = &r.url {
                        println!("  {url}");
                    }
                    println!("  {}", r.preview.replace('\n', " "));
                    println!("  fetched: {}\n", r.fetched_at);
                }
            }
        }

        Command::Ask { question } => {
            let answer = agent::ask(&cfg, &store, &question).await?;
            println!("{answer}");
        }

        Command::Clear { source } => {
            let deleted = store.clear(&source)?;
            println!("Cleared {deleted} chunks from source '{source}'");
        }

        Command::Forget { id } => {
            store.forget(id)?;
            println!("Deleted chunk {id}");
        }

        Command::Open => {
            let path = cfg
                .obsidian_vault_path
                .as_ref()
                .map(|p| p.join("Marrow"))
                .or_else(|| cfg.db_path.parent().map(|p| p.to_path_buf()))
                .ok_or_else(|| anyhow::anyhow!("No vault path configured"))?;
            std::process::Command::new("open").arg(&path).spawn()?;
            println!("Opening {}", path.display());
        }

        Command::Status => store.print_stats()?,

        Command::Auth { .. } => unreachable!(),
    }

    Ok(())
}
