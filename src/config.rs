use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct Config {
    pub github_token: Option<String>,
    pub github_username: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub db_path: PathBuf,
    pub obsidian_vault_path: Option<PathBuf>,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub google_refresh_token: Option<String>,
    pub slack_token: Option<String>,
    pub slack_channels: Option<String>, // comma-separated channel IDs; if None, pulls all joined
}

impl Config {
    pub fn load() -> Result<Self> {
        // ~/.marrow/.env takes priority (used when binary runs outside project dir, e.g. launchd)
        let home_env = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".marrow")
            .join(".env");
        if home_env.exists() {
            dotenvy::from_path(&home_env).ok();
        }
        // Current directory only. dotenvy::dotenv() also walks up parent directories, which
        // picked up other projects' .env files when run inside them.
        dotenvy::from_path(".env").ok();

        let db_path = std::env::var("MARROW_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".marrow")
                    .join("marrow.db")
            });

        if let Some(parent) = db_path.parent() {
            // Lock down only a folder we create: MARROW_DB_PATH may point into a shared one.
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
                }
            }
        }

        Ok(Self {
            // Required only by the commands that use them (see github() / anthropic_key()),
            // so local commands like search, status and export work without them.
            github_token: std::env::var("GITHUB_TOKEN").ok(),
            github_username: std::env::var("GITHUB_USERNAME").ok(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            db_path,
            obsidian_vault_path: std::env::var("OBSIDIAN_VAULT_PATH").ok().map(PathBuf::from),
            google_client_id: std::env::var("GOOGLE_CLIENT_ID").ok(),
            google_client_secret: std::env::var("GOOGLE_CLIENT_SECRET").ok(),
            google_refresh_token: std::env::var("GOOGLE_REFRESH_TOKEN").ok(),
            slack_token: std::env::var("SLACK_TOKEN").ok(),
            slack_channels: std::env::var("SLACK_CHANNELS").ok(),
        })
    }

    /// GitHub token and username, or an error naming the missing variable.
    pub fn github(&self) -> Result<(&str, &str)> {
        let token = self
            .github_token
            .as_deref()
            .context("GITHUB_TOKEN not set — copy .env.example to ~/.marrow/.env and fill it in")?;
        let user = self
            .github_username
            .as_deref()
            .context("GITHUB_USERNAME not set")?;
        Ok((token, user))
    }

    pub fn anthropic_key(&self) -> Result<&str> {
        self.anthropic_api_key
            .as_deref()
            .context("ANTHROPIC_API_KEY not set")
    }

    pub fn google_creds(&self) -> Option<(&str, &str, &str)> {
        match (
            &self.google_client_id,
            &self.google_client_secret,
            &self.google_refresh_token,
        ) {
            (Some(id), Some(secret), Some(token)) => Some((id, secret, token)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_creds_returns_none_when_any_field_missing() {
        let cfg = Config {
            github_token: Some("t".into()),
            github_username: Some("u".into()),
            anthropic_api_key: Some("k".into()),
            db_path: PathBuf::from("/tmp/marrow.db"),
            obsidian_vault_path: None,
            google_client_id: Some("id".into()),
            google_client_secret: None,
            google_refresh_token: Some("tok".into()),
            slack_token: None,
            slack_channels: None,
        };
        assert!(cfg.google_creds().is_none());
    }

    #[test]
    fn google_creds_returns_some_when_all_present() {
        let cfg = Config {
            github_token: Some("t".into()),
            github_username: Some("u".into()),
            anthropic_api_key: Some("k".into()),
            db_path: PathBuf::from("/tmp/marrow.db"),
            obsidian_vault_path: None,
            google_client_id: Some("id".into()),
            google_client_secret: Some("secret".into()),
            google_refresh_token: Some("tok".into()),
            slack_token: None,
            slack_channels: None,
        };
        let creds = cfg.google_creds().unwrap();
        assert_eq!(creds.0, "id");
        assert_eq!(creds.1, "secret");
        assert_eq!(creds.2, "tok");
    }
}
