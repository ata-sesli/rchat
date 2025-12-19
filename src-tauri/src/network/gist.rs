use anyhow::Result;
use octocrab::{models::gists::Gist, Octocrab};
// use std::collections::HashMap;

const RCHAT_GIST_DESC: &str = "rchat-peer-info";
const RCHAT_FILE_NAME: &str = "peers.txt";

/// Find the user's existing rchat gist
pub async fn find_rchat_gist(token: &str) -> Result<Option<Gist>> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_string())
        .build()?;
    // .gists().list_all_gists() lists gists for the authenticated user
    let gists = octocrab.gists().list_all_gists().send().await?;

    for gist in gists {
        if gist.description.as_deref() == Some(RCHAT_GIST_DESC) {
            return Ok(Some(gist));
        }
    }
    Ok(None)
}

/// Create a new rchat gist
pub async fn create_peer_info(token: &str, content: String) -> Result<Gist> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_string())
        .build()?;

    let gist = octocrab
        .gists()
        .create()
        .description(RCHAT_GIST_DESC)
        .public(true)
        .file(RCHAT_FILE_NAME, content)
        .send()
        .await?;

    Ok(gist)
}

/// Update existing rchat gist
pub async fn update_peer_info(token: &str, gist_id: &str, content: String) -> Result<Gist> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_string())
        .build()?;

    // update(id) returns UpdateGistBuilder
    // .file(name) returns UpdateGistFileBuilder
    // .with_content(content) updates content
    let gist = octocrab
        .gists()
        .update(gist_id)
        .description(RCHAT_GIST_DESC)
        .file(RCHAT_FILE_NAME)
        .with_content(content)
        .send()
        .await?;

    Ok(gist)
}

/// Fetch friend's gist content
pub async fn get_friend_content(username: &str) -> Result<Option<String>> {
    let octocrab = Octocrab::builder().build()?;

    // .gists().list_user_gists(username)
    let gists = octocrab.gists().list_user_gists(username).send().await?;

    for gist in gists {
        if gist.description.as_deref() == Some(RCHAT_GIST_DESC) {
            if let Some(file) = gist.files.get(RCHAT_FILE_NAME) {
                // file.raw_url is Url (not Option)
                let raw_url = &file.raw_url;
                // Using reqwest for raw download is fine here as it's just HTTP GET
                let resp = reqwest::get(raw_url.clone()).await?;
                if resp.status().is_success() {
                    let text = resp.text().await?;
                    return Ok(Some(text));
                }
            }
        }
    }

    Ok(None)
}
