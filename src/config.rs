use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct Config {
    pub github_token: String,
    pub github_username: String,
    pub anthropic_api_key: String,
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
        dotenvy::dotenv().ok();

        let db_path = std::env::var("MARROW_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".marrow")
                    .join("marrow.db")
            });

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        Ok(Self {
            github_token: std::env::var("GITHUB_TOKEN")
                .context("GITHUB_TOKEN not set — copy .env.example to .env and fill it in")?,
            github_username: std::env::var("GITHUB_USERNAME").context("GITHUB_USERNAME not set")?,
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY not set")?,
            db_path,
            obsidian_vault_path: std::env::var("OBSIDIAN_VAULT_PATH").ok().map(PathBuf::from),
            google_client_id: std::env::var("GOOGLE_CLIENT_ID").ok(),
            google_client_secret: std::env::var("GOOGLE_CLIENT_SECRET").ok(),
            google_refresh_token: std::env::var("GOOGLE_REFRESH_TOKEN").ok(),
            slack_token: std::env::var("SLACK_TOKEN").ok(),
            slack_channels: std::env::var("SLACK_CHANNELS").ok(),
        })
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
            github_token: "t".into(),
            github_username: "u".into(),
            anthropic_api_key: "k".into(),
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
            github_token: "t".into(),
            github_username: "u".into(),
            anthropic_api_key: "k".into(),
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
