use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

// You should replace this with your actual Client ID for the production app.
// For now, these are often public for CLIs/Desktop apps using Device Flow.
pub const CLIENT_ID: &str = "Ov23liXhUOLJ0WxMkpDL";
const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: i64,
    pub interval: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthState {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: i64,
}

pub async fn start_device_flow() -> Result<AuthState> {
    let client = Client::new();
    let params = [
        ("client_id", CLIENT_ID),
        ("scope", "gist"), // Standard gist scope
    ];

    let res = client
        .post(GITHUB_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "rchat-app")
        .form(&params)
        .send()
        .await?;

    if !res.status().is_success() {
        return Err(anyhow!("Failed to request device code: {}", res.status()));
    }

    let body: DeviceCodeResponse = res.json().await?;

    Ok(AuthState {
        device_code: body.device_code,
        user_code: body.user_code,
        verification_uri: body.verification_uri,
        interval: body.interval,
    })
}

pub async fn poll_for_token(device_code: &str) -> Result<String> {
    let client = Client::new();
    let params = [
        ("client_id", CLIENT_ID),
        ("device_code", device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];

    let res = client
        .post(GITHUB_TOKEN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "rchat-app")
        .form(&params)
        .send()
        .await?;

    let body: TokenResponse = res.json().await?;

    if let Some(error) = body.error {
        return Err(anyhow!("{}", error));
    }

    match body.access_token {
        Some(token) => Ok(token),
        None => Err(anyhow!("No access token in response")),
    }
}
