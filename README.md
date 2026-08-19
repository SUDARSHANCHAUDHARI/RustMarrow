# RustMarrow

[![crates.io](https://img.shields.io/crates/v/rustmarrow?logo=rust)](https://crates.io/crates/rustmarrow)
[![Downloads](https://img.shields.io/crates/d/rustmarrow?logo=rust)](https://crates.io/crates/rustmarrow)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust)

> A personal, local-first AI memory agent — pull your GitHub, Gmail, Calendar, and Slack context into local SQLite and query it with Claude.

**Marrow** (installed as the `rustmarrow` command) is a personal local AI memory agent. It
pulls selected GitHub, Gmail, Calendar, and Slack data into a local SQLite database,
optionally mirrors memory chunks into Obsidian Markdown, and lets you search or ask
questions over that local context with Claude.

## Table of Contents

- [Overview](#overview)
- [Features](#features)
- [Privacy Model](#privacy-model)
- [Installation](#installation)
- [Configuration](#configuration)
- [Usage](#usage)
- [Included Example](#included-example)
- [Storage Layout](#storage-layout)
- [Automation](#automation)
- [Development](#development)
- [Project Structure](#project-structure)
- [Documentation](#documentation)
- [Release Status](#release-status)
- [License](#license)
- [About](#about)

## Overview

Personal work context is spread across commits, issues, messages, calendar events, and
email. Marrow is designed as a local-first memory layer that helps you recover that context
without sending everything to a hosted database.

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

Marrow is local-first. The SQLite database lives on your machine, and `.env` credentials are
intentionally ignored by git. Data only leaves your machine when Marrow calls the configured
source APIs or sends selected context to Claude for the `ask` and `digest` flows.

Do not commit `.env`, database files, OAuth credentials, Slack tokens, GitHub tokens, API
keys, or Obsidian-generated private memory.

## Installation

### From crates.io (recommended)

```bash
cargo install rustmarrow
```

### From source

```bash
git clone https://github.com/SUDARSHANCHAUDHARI/RustMarrow.git
cd RustMarrow
cargo build --release
```

The binary is created at:

```bash
target/release/rustmarrow
```

Optional local install from a source checkout:

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
| `GOOGLE_REFRESH_TOKEN` | Optional | Refresh token from `rustmarrow auth google` |
| `SLACK_TOKEN` | Optional | Slack user token for channel/DM history |
| `SLACK_CHANNELS` | Optional | Comma-separated Slack channel IDs to restrict pulls |

## Usage

```bash
# Pull all configured sources
rustmarrow pull

# Pull one source
rustmarrow pull --source github
rustmarrow pull --source gmail
rustmarrow pull --source calendar
rustmarrow pull --source slack

# Search local memory
rustmarrow search "android crash"
rustmarrow search "open issues" --limit 20

# Ask Claude using local memory context
rustmarrow ask "what am I working on this week?"
rustmarrow ask "which issues look urgent?"

# Generate a cross-source digest
rustmarrow digest

# Inspect and manage stored memory
rustmarrow status
rustmarrow clear github
rustmarrow forget 42
rustmarrow open

# Back up and restore local memory
rustmarrow export --json marrow-backup.json
rustmarrow import --json marrow-backup.json

# One-time Google OAuth setup
rustmarrow auth google
```

## Included Example

The repository includes local setup guidance in [examples/local-setup.md](examples/local-setup.md).
It uses placeholder values only and is safe to commit.

Real CLI help output:

```text
Personal local AI memory agent

Usage: rustmarrow <COMMAND>

Commands:
  pull    Pull data from configured sources into memory
  digest  Morning briefing — schedule, commits, emails, slack
  search  Search memory without asking Claude
  ask     Ask Marrow a question using your memory context
  clear   Delete all memory for a source
  forget  Delete one chunk by id (get id from `rustmarrow search`)
  open    Open the Obsidian vault Marrow folder (macOS)
  status  Show memory stats
  export  Export memory chunks to a JSON backup file
  import  Import memory chunks from a JSON backup file
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

JSON exports contain memory chunks only: source, source ID, title, content, URL, tags, and
fetched timestamp. They do not include provider credentials or `.env` values.

## Automation

Marrow can be run manually or scheduled with macOS `launchd`, cron, or another scheduler. A
typical cadence is every 20 minutes:

```bash
rustmarrow pull
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

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Roadmap](docs/ROADMAP.md)
- [Maintainer notes](docs/NOTES.md)
- [Content plan](docs/CONTENT_PLAN.md)

## Release Status

Current release: **`v1.1.1`**, published on [crates.io](https://crates.io/crates/rustmarrow).

Each release is verified with formatting, Clippy, tests, an optimized release build, and
`cargo package` before publishing.

## License

MIT — see [LICENSE](LICENSE).

---

## About

I'm Sudarshan Chaudhari, a Senior Quality Engineer, Test Automation specialist, and AI systems builder based in Bangkok, Thailand.

I have 13+ years of experience in software quality engineering, working across SaaS, fintech, gaming, web, mobile, cloud, and digital signage platforms. My background combines hands-on test automation with QA leadership, test strategy, CI/CD, release quality, production investigation, and cross-platform validation.

Alongside my professional QA career, I run [SudarshanTechLabs](https://sudarshantechlabs.com/), my independent engineering and product lab where I design, build, test, and ship software across Android, web, AI, cybersecurity, developer tooling, and cross-platform applications.

### What I work on

- ⚙️ **Quality Engineering & Test Automation** — Playwright, Selenium, Cypress, Appium, API testing, automation frameworks, end-to-end testing, CI/CD, release gates, GitHub Actions, risk-based testing, and production validation
- 🤖 **AI Systems & Automation** — AI agents, multi-agent orchestration, MCP servers, AI-assisted QA, prompt tooling, developer workflows, automation systems, and Claude Code plugins
- 📱 **Mobile & Cross-Platform Applications** — Android applications built with Kotlin and Jetpack Compose, Google Play releases, automated build and publishing pipelines, and cross-platform development spanning iOS, web, Windows, and macOS
- 🌐 **Web Applications & Platforms** — Full-stack applications using Next.js, TypeScript, Firebase, Cloudflare, REST APIs, and modern web infrastructure
- 🛠️ **Developer Tooling & CLI Engineering** — Rust, Python, TypeScript, CLI utilities, multi-repository tooling, build automation, release tooling, and engineering productivity systems
- 🛡️ **Cybersecurity & Observability** — Threat detection, log analysis, security auditing, vulnerability assessment, monitoring, and security-focused developer tools
- 📺 **Digital Signage & Device Platforms** — Content validation, playback testing, device compatibility, production investigation, monitoring, and QA across diverse hardware and operating-system environments

My work sits at the intersection of quality engineering, automation, AI, and software development. I approach products with a QA mindset from the beginning: understanding failure modes, designing for testability, automating repetitive work, and building release confidence into the engineering process.

Through SudarshanTechLabs, I also build products and tools from idea to production, covering architecture, development, testing, CI/CD, release automation, monitoring, and ongoing maintenance.

🌐 [sudarshantechlabs.com](https://sudarshantechlabs.com/) · 💼 [LinkedIn](https://linkedin.com/in/sudarshan-chaudhari) · 🐙 [GitHub](https://github.com/SUDARSHANCHAUDHARI) · ✉️ [sunny.sudarshan@gmail.com](mailto:sunny.sudarshan@gmail.com)
