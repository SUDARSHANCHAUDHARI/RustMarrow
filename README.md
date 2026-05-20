# Marrow

Personal local AI memory agent. Pulls your GitHub, Gmail, Calendar, and Slack into a local SQLite database and lets you query it with Claude.

## What it does

- **Pulls** data from your connected sources every 20 minutes (via launchd)
- **Stores** everything locally in SQLite — nothing leaves your machine except API calls
- **Syncs** to your Obsidian vault as Markdown files
- **Answers** questions about your work using Claude with your data as context
- **Digests** a morning briefing — schedule, commits, emails, Slack in one shot

## Install

Requires Rust 1.75+.

```bash
git clone https://github.com/SUDARSHANCHAUDHARI/RustMarrow
cd marrow
cp .env.example ~/.marrow/.env
# Fill in your tokens (see Configuration below)
cargo build --release
ln -s $(pwd)/target/release/marrow ~/.local/bin/marrow
```

## Configuration

Copy `.env.example` to `~/.marrow/.env` and fill in:

| Variable | Required | Description |
|---|---|---|
| `GITHUB_TOKEN` | ✅ | [Personal access token](https://github.com/settings/tokens) — `repo` scope |
| `GITHUB_USERNAME` | ✅ | Your GitHub username |
| `ANTHROPIC_API_KEY` | ✅ | [Anthropic API key](https://console.anthropic.com) |
| `MARROW_DB_PATH` | optional | Default: `~/.marrow/marrow.db` |
| `OBSIDIAN_VAULT_PATH` | optional | Path to your Obsidian vault root |
| `GOOGLE_CLIENT_ID` | optional | For Gmail + Calendar |
| `GOOGLE_CLIENT_SECRET` | optional | For Gmail + Calendar |
| `GOOGLE_REFRESH_TOKEN` | optional | Run `marrow auth google` to get this |
| `SLACK_TOKEN` | optional | User token (`xoxp-`) with `channels:history`, `im:history` scopes |
| `SLACK_CHANNELS` | optional | Comma-separated channel IDs to restrict to |

## Usage

```bash
# Pull all configured sources
marrow pull

# Pull a specific source
marrow pull --source github
marrow pull --source gmail
marrow pull --source calendar
marrow pull --source slack

# Morning briefing
marrow digest

# Search memory (no Claude tokens spent)
marrow search "android crash"
marrow search "open issues"

# Ask a question
marrow ask "what am I working on this week?"
marrow ask "any urgent emails?"

# Memory management
marrow status                  # chunk counts + last pull times
marrow clear github            # wipe all GitHub data
marrow forget 42               # delete one chunk by id (get id from search)
marrow open                    # open Obsidian vault folder (macOS)

# Google OAuth setup (one-time)
marrow auth google
```

## Auto-pull (macOS launchd)

Runs `marrow pull` every 20 minutes in the background:

```bash
# Copy the plist (adjust path to match your install)
cp scripts/ai.marrow.pull.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/ai.marrow.pull.plist

# Check logs
tail -f ~/.marrow/logs/pull.log
```

## Architecture

```
~/.marrow/.env          — credentials (never committed)
~/.marrow/marrow.db     — SQLite: memory_chunks + pull_log
~/ObsidianVault/Marrow/ — Markdown files per source (optional)
```

Sources are incremental — each pull only fetches data since the last run.

## License

MIT — see [LICENSE](LICENSE)

Built by [SudarshanTechLabs](https://github.com/SUDARSHANCHAUDHARI)
