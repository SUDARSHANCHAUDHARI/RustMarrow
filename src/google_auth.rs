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

/// Run the OAuth consent flow and print the refresh token.
///
/// Uses the loopback redirect: a one-shot listener on 127.0.0.1 receives Google's
/// redirect. Google retired the copy-paste (`urn:ietf:wg:oauth:2.0:oob`) flow.
pub async fn authorize_and_print_refresh_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let scopes = [
        "https://www.googleapis.com/auth/gmail.readonly",
        "https://www.googleapis.com/auth/calendar.readonly",
    ]
    .join(" ");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    // shortcut: state is pid + time, not cryptographically random; it only has to beat
    // a stray local request during one interactive run. Add `rand` if that changes.
    let state = format!(
        "{:x}{:x}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );

    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
        ?client_id={client_id}\
        &redirect_uri={}\
        &response_type=code\
        &scope={}\
        &state={state}\
        &access_type=offline\
        &prompt=consent",
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&scopes)
    );

    println!("Open this URL in your browser:\n\n{auth_url}\n");
    println!("Waiting for Google to redirect to {redirect_uri} ...");

    let (mut sock, _) = listener.accept().await?;
    let mut buf = vec![0u8; 8192];
    let n = sock.read(&mut buf).await?;
    let params = redirect_params(&String::from_utf8_lossy(&buf[..n]));
    sock.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n\
        RustMarrow: done. You can close this tab and return to the terminal.\n",
    )
    .await?;

    if let Some(err) = params.get("error") {
        anyhow::bail!("Google returned an error: {err}");
    }
    if params.get("state") != Some(&state) {
        anyhow::bail!("OAuth state did not match; ignoring this redirect. Run the command again.");
    }
    let code = params
        .get("code")
        .context("Google's redirect had no auth code")?;

    #[derive(Deserialize)]
    struct ExchangeResponse {
        refresh_token: String,
    }

    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri.as_str()),
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

/// Query parameters from the request line of the redirect, e.g. `GET /?code=..&state=.. HTTP/1.1`.
fn redirect_params(request: &str) -> std::collections::HashMap<String, String> {
    let target = request.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| {
            let v = urlencoding::decode(v)
                .map(|c| c.into_owned())
                .unwrap_or_default();
            (k.to_string(), v)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_params_decodes_code_and_state() {
        let p = redirect_params(
            "GET /?state=ab12&code=4%2F0Abc&scope=x HTTP/1.1\r\nHost: 127.0.0.1\r\n",
        );
        assert_eq!(p.get("code").map(String::as_str), Some("4/0Abc"));
        assert_eq!(p.get("state").map(String::as_str), Some("ab12"));
    }

    #[test]
    fn redirect_params_reports_a_denied_consent() {
        let p = redirect_params("GET /?error=access_denied&state=s HTTP/1.1");
        assert_eq!(p.get("error").map(String::as_str), Some("access_denied"));
        assert!(!p.contains_key("code"));
    }
}
