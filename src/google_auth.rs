use anyhow::{Context, Result};
use serde::Deserialize;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Exchange a stored refresh token for a short-lived access token.
pub async fn access_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<String> {
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?
        .error_for_status()
        .context("Failed to refresh Google access token — check your credentials in .env")?
        .json::<TokenResponse>()
        .await?;

    Ok(resp.access_token)
}

/// Print the URL the user must visit to get their one-time auth code,
/// then read it from stdin and exchange it for a refresh token.
pub async fn authorize_and_print_refresh_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
) -> Result<()> {
    let scopes = [
        "https://www.googleapis.com/auth/gmail.readonly",
        "https://www.googleapis.com/auth/calendar.readonly",
    ]
    .join(" ");

    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
        ?client_id={client_id}\
        &redirect_uri=urn:ietf:wg:oauth:2.0:oob\
        &response_type=code\
        &scope={}\
        &access_type=offline\
        &prompt=consent",
        urlencoding::encode(&scopes)
    );

    println!("Open this URL in your browser:\n\n{auth_url}\n");
    println!("Paste the auth code here:");

    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    let code = code.trim();

    #[derive(Deserialize)]
    struct ExchangeResponse {
        refresh_token: String,
    }

    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", "urn:ietf:wg:oauth:2.0:oob"),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<ExchangeResponse>()
        .await?;

    println!(
        "\nAdd this to your .env:\nGOOGLE_REFRESH_TOKEN={}",
        resp.refresh_token
    );

    Ok(())
}
