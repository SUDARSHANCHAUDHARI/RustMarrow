# Marrow

![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust)
![License](https://img.shields.io/badge/License-MIT-blue)

Marrow is a personal local AI memory agent. It pulls selected GitHub, Gmail, Calendar, and Slack data into a local SQLite database, optionally mirrors memory chunks into Obsidian Markdown, and lets you search or ask questions over that local context with Claude.

## Why This Exists

Personal work context is spread across commits, issues, messages, calendar events, and email. Marrow is designed as a local-first memory layer that helps you recover that context without sending everything to a hosted database.

## Features

- Pulls recent GitHub repository activity, issues, pull requests, and commits.
- Pulls Gmail messages when Google OAuth credentials are configured.
- Pulls Google Calendar events when Google OAuth credentials are configured.
- Pulls Slack channel or DM history when a Slack user token is configured.
- Stores memory chunks locally in SQLite.
- Tracks incremental pulls so repeated syncs only fetch new data.
- Searches local memory without spending model tokens.
- Asks Claude questions using recent local memory as context.
- Generates a digest across configured sources.
- Clears all memory for a source or deletes a single chunk by id.
- Optionally writes memory chunks into an Obsidian vault.

## Privacy Model

Marrow is local-first. The SQLite database lives on your machine, and `.env` credentials are intentionally ignored by git. Data only leaves your machine when Marrow calls the configured source APIs or sends selected context to Claude for the `ask` and `digest` flows.

Do not commit `.env`, database files, OAuth credentials, Slack tokens, GitHub tokens, API keys, or Obsidian-generated private memory.

## Installation

```bash
git clone https://github.com/SUDARSHANCHAUDHARI/RustMarrow.git
cd RustMarrow
cargo build --release
```

The binary is created at:

```bash
target/release/marrow
```

Optional local install:

```bash
cargo install --path .
```

## Configuration

Copy the template and fill in real values locally:

```bash
mkdir -p ~/.marrow
cp .env.example ~/.marrow/.env
```

| Variable | Required | Description |
|---|---|---|
| `GITHUB_TOKEN` | Yes | GitHub personal access token for repository data |
| `GITHUB_USERNAME` | Yes | GitHub username to scope activity |
| `ANTHROPIC_API_KEY` | Yes | API key used for Claude-powered answers |
| `MARROW_DB_PATH` | Optional | SQLite path, defaults to `~/.marrow/marrow.db` |
| `OBSIDIAN_VAULT_PATH` | Optional | Obsidian vault root for Markdown mirroring |
| `GOOGLE_CLIENT_ID` | Optional | Google OAuth client id for Gmail and Calendar |
| `GOOGLE_CLIENT_SECRET` | Optional | Google OAuth client secret |
| `GOOGLE_REFRESH_TOKEN` | Optional | Refresh token from `marrow auth google` |
| `SLACK_TOKEN` | Optional | Slack user token for channel/DM history |
| `SLACK_CHANNELS` | Optional | Comma-separated Slack channel IDs to restrict pulls |

## Usage

```bash
# Pull all configured sources
marrow pull

# Pull one source
marrow pull --source github
marrow pull --source gmail
marrow pull --source calendar
marrow pull --source slack

# Search local memory
marrow search "android crash"
marrow search "open issues" --limit 20

# Ask Claude using local memory context
marrow ask "what am I working on this week?"
marrow ask "which issues look urgent?"

# Generate a cross-source digest
marrow digest

# Inspect and manage stored memory
marrow status
marrow clear github
marrow forget 42
marrow open

# One-time Google OAuth setup
marrow auth google
```

## Included Example

The repository includes local setup guidance in [examples/local-setup.md](examples/local-setup.md). It uses placeholder values only and is safe to commit.

Real CLI help output:

```text
Personal local AI memory agent

Usage: marrow <COMMAND>

Commands:
  pull    Pull data from configured sources into memory
  digest  Morning briefing — schedule, commits, emails, slack
  search  Search memory without asking Claude
  ask     Ask Marrow a question using your memory context
  clear   Delete all memory for a source
  forget  Delete one chunk by id (get id from `marrow search`)
  open    Open the Obsidian vault Marrow folder (macOS)
  status  Show memory stats
  auth    Authenticate with a provider and print the refresh token
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

## Storage Layout

```text
~/.marrow/.env          Local credentials, never committed
~/.marrow/marrow.db     SQLite database with memory_chunks and pull_log
~/ObsidianVault/Marrow/ Optional Markdown mirror by source
```

## Automation

Marrow can be run manually or scheduled with macOS `launchd`, cron, or another scheduler. A typical cadence is every 20 minutes:

```bash
marrow pull
```

Keep scheduler logs outside the repository and avoid writing secrets to stdout.

## Development

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build --release
```

Run these checks locally before publishing changes.

## Project Structure

```text
src/
  main.rs             CLI commands and orchestration
  config.rs           Environment-driven configuration
  db.rs               SQLite setup
  memory.rs           Search, ingest, clear, forget, Obsidian mirror
  agent.rs            Claude context and answer flow
  google_auth.rs      Google OAuth token exchange
  pullers/            GitHub, Gmail, Calendar, and Slack ingestion
```

## Project Docs

- [Architecture](docs/ARCHITECTURE.md)
- [Roadmap](docs/ROADMAP.md)
- [Maintainer notes](docs/NOTES.md)
- [Content plan](docs/CONTENT_PLAN.md)

## Release Status

Current production release: `v1.0.0`

The `v1.0.0` release was verified with formatting, clippy, tests, optimized release build, and `cargo package`.

## License

MIT. See [LICENSE](LICENSE).

## Developer

Built by [Sudarshan Chaudhari](https://github.com/SUDARSHANCHAUDHARI) under SudarshanTechLabs.
