# Architecture

RustMarrow is a local AI memory agent that stores personal context in SQLite and can pull data from configured sources.

## Goals

- Keep personal memory local by default.
- Separate source pullers from storage and agent behavior.
- Make configuration explicit through environment variables.
- Avoid committing secrets or personal data.

## Module Layout

| Module | Responsibility |
| --- | --- |
| `src/main.rs` | CLI entry point and command routing |
| `src/config.rs` | Environment-based configuration |
| `src/db.rs` | SQLite storage setup and access |
| `src/memory.rs` | Memory data model and operations |
| `src/agent.rs` | Agent-facing behavior |
| `src/pullers/` | Gmail, Calendar, GitHub, and Slack source integrations |
| `src/google_auth.rs` | Google token refresh helper |

## Data Flow

1. The CLI loads configuration.
2. A command chooses a local operation or a source puller.
3. Pullers fetch external data when credentials are configured.
4. Memory records are normalized.
5. SQLite persists the local memory store.

## Design Notes

- Pullers should fail clearly when credentials are missing.
- Secrets must stay in local environment files or secret managers, not source control.
- Storage changes should be migration-friendly.
- Personal data examples should use placeholders only.

## Release Assumptions

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo package` pass before release.
- GitHub Actions are intentionally not used in this repo.
