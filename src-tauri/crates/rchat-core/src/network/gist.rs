use super::hks::{PublishedBlob, TrackedInvite};
use super::invite::EncryptedInvite;
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use octocrab::{models::gists::Gist, Octocrab};
use std::io::prelude::*;
use std::time::{SystemTime, UNIX_EPOCH};

// use std::collections::HashMap;

const RCHAT_GIST_DESC: &str = "rchat-peer-info";
const RCHAT_FILE_NAME: &str = "peers.txt";

// All local read/modify/write operations share this lock, including UI shadow
// writes and background discovery publication. Never replace a fetched blob
// from a snapshot taken before acquiring the lock.
static GIST_MUTATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) async fn serialized_mutation<T>(
    mutation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let _guard = GIST_MUTATION.lock().await;
        mutation.await
    })
    .await
    .map_err(|_| anyhow::anyhow!("Gist update timed out"))?
}

async fn read_own_blob(gist: &Gist) -> Result<PublishedBlob> {
    let file = gist
        .files
        .get(RCHAT_FILE_NAME)
        .ok_or_else(|| anyhow::anyhow!("RChat Gist is missing peers.txt"))?;
    let content = reqwest::get(file.raw_url.clone())
        .await?
        .error_for_status()?
        .text()
        .await?;
    // A failed read must not turn into an empty replacement, losing shadows.
    parse_blob(&content)
}

fn merge_live_shadows(replacement: &mut PublishedBlob, current: PublishedBlob, now: u64) {
    replacement.shadow_invites = current
        .shadow_invites
        .into_iter()
        .filter(|shadow| now.saturating_sub(shadow.created_at) < INVITE_TTL_SECS)
        .collect();
}

/// Caller holds GIST_MUTATION across content generation and this update.
pub(crate) async fn publish_preserving_shadows(token: &str, content: String) -> Result<()> {
    let mut replacement = parse_blob(&content)?;
    if let Some(existing) = find_rchat_gist(token).await? {
        let current = read_own_blob(&existing).await?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        merge_live_shadows(&mut replacement, current, now);
        update_peer_info(token, &existing.id, serialize_blob(&replacement)?).await?;
    } else {
        create_peer_info(token, content).await?;
    }
    Ok(())
}

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

// ============================================================================
// Invitation Management Helpers
// ============================================================================

/// TTL for invitations: 2 minutes (120 seconds)
const INVITE_TTL_SECS: u64 = 120;

/// Parse compressed Base64 blob into PublishedBlob
pub fn parse_blob(blob_b64: &str) -> Result<PublishedBlob> {
    // 1. Decode Base64
    let compressed = BASE64
        .decode(blob_b64)
        .map_err(|e| anyhow::anyhow!("Failed to decode blob: {}", e))?;

    // 2. Decompress
    let mut decoder = ZlibDecoder::new(&compressed[..]);
    let mut json_str = String::new();
    decoder.read_to_string(&mut json_str)?;

    // 3. Deserialize
    let blob: PublishedBlob = serde_json::from_str(&json_str)?;
    Ok(blob)
}

/// Serialize PublishedBlob to compressed Base64
pub fn serialize_blob(blob: &PublishedBlob) -> Result<String> {
    // 1. Serialize to JSON
    let json_str = serde_json::to_string(blob)?;

    // 2. Compress
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(json_str.as_bytes())?;
    let compressed = encoder.finish()?;

    // 3. Encode Base64
    Ok(BASE64.encode(compressed))
}

/// Remove expired invitations from blob (2-minute TTL)
/// Returns the number of invitations removed
pub fn clean_expired_invitations(blob: &mut PublishedBlob) -> usize {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let before = blob.invitations.len();

    blob.invitations.retain(|inv| {
        let age = now.saturating_sub(inv.created_at);
        age < INVITE_TTL_SECS
    });

    before - blob.invitations.len()
}

/// Convert EncryptedInvite to TrackedInvite with current timestamp
pub fn track_invite(invite: EncryptedInvite) -> TrackedInvite {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    TrackedInvite {
        salt: invite.salt,
        nonce: invite.nonce,
        ciphertext: invite.ciphertext,
        created_at: now,
    }
}

/// Convert TrackedInvite to EncryptedInvite (strips tracking metadata)
pub fn untrack_invite(tracked: &TrackedInvite) -> EncryptedInvite {
    EncryptedInvite {
        salt: tracked.salt.clone(),
        nonce: tracked.nonce.clone(),
        ciphertext: tracked.ciphertext.clone(),
    }
}

/// Fetch friend's invitations from their Gist
pub async fn get_friend_invitations(username: &str) -> Result<Vec<EncryptedInvite>> {
    // 1. Fetch friend's Gist content
    if let Some(blob_b64) = get_friend_content(username).await? {
        // 2. Parse blob
        if let Ok(blob) = parse_blob(&blob_b64) {
            // 3. Filter expired invites and convert to EncryptedInvite
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let valid_invites: Vec<EncryptedInvite> = blob
                .invitations
                .iter()
                .filter(|inv| now.saturating_sub(inv.created_at) < INVITE_TTL_SECS)
                .map(untrack_invite)
                .collect();

            return Ok(valid_invites);
        }
    }

    Ok(vec![])
}

/// Publish a shadow invite to the user's own Gist
/// This is called by the invitee after accepting an invite
pub async fn publish_shadow_invite(token: &str, shadow: super::hks::ShadowInvite) -> Result<()> {
    serialized_mutation(publish_shadow_invite_inner(token, shadow)).await
}

async fn publish_shadow_invite_inner(token: &str, shadow: super::hks::ShadowInvite) -> Result<()> {
    // 1. Find or create existing Gist
    let existing = find_rchat_gist(token).await?;
    let mut blob = if let Some(gist) = &existing {
        read_own_blob(gist).await?
    } else {
        default_blob()
    };

    // 2. Add shadow invite to blob
    // Remove expired shadows first (2 minute TTL)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    blob.shadow_invites
        .retain(|s| now.saturating_sub(s.created_at) < INVITE_TTL_SECS);

    // Remove any existing shadow for this target
    blob.shadow_invites
        .retain(|s| s.target_username != shadow.target_username);

    // Add new shadow
    blob.shadow_invites.push(shadow);

    // 3. Serialize and update Gist
    let blob_b64 = serialize_blob(&blob)?;

    if let Some(gist) = existing {
        update_peer_info(token, &gist.id, blob_b64).await?;
    } else {
        create_peer_info(token, blob_b64).await?;
    }

    Ok(())
}

/// Create a default empty blob
fn default_blob() -> PublishedBlob {
    PublishedBlob {
        payload: String::new(),
        payload_nonce: String::new(),
        tree_links: std::collections::HashMap::new(),
        roster: std::collections::HashMap::new(),
        signature: String::new(),
        sender_x25519_pubkey: String::new(),
        invitations: vec![],
        shadow_invites: vec![],
    }
}

/// Fetch shadow invites from a user's Gist
pub async fn get_friend_shadows(username: &str) -> Result<Vec<super::hks::ShadowInvite>> {
    if let Some(blob_b64) = get_friend_content(username).await? {
        if let Ok(blob) = parse_blob(&blob_b64) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let valid_shadows: Vec<_> = blob
                .shadow_invites
                .into_iter()
                .filter(|s| now.saturating_sub(s.created_at) < INVITE_TTL_SECS)
                .collect();
            return Ok(valid_shadows);
        }
    }
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_publication_preserves_live_shadow_for_inviter() {
        let mut current = default_blob();
        current
            .shadow_invites
            .push(super::super::hks::ShadowInvite {
                target_username: "inviter".into(),
                salt: "salt".into(),
                nonce: "nonce".into(),
                ciphertext: "encrypted-shadow".into(),
                created_at: 100,
            });
        let mut replacement = default_blob();
        replacement.payload = "new endpoint".into();
        merge_live_shadows(&mut replacement, current, 150);
        let retrieved = parse_blob(&serialize_blob(&replacement).unwrap()).unwrap();
        assert_eq!(retrieved.shadow_invites.len(), 1);
        assert_eq!(retrieved.shadow_invites[0].target_username, "inviter");
        assert_eq!(retrieved.shadow_invites[0].ciphertext, "encrypted-shadow");
        let mut expired = default_blob();
        merge_live_shadows(&mut expired, retrieved, 220);
        assert!(expired.shadow_invites.is_empty());
    }

    #[tokio::test]
    async fn endpoint_publication_waits_for_concurrent_shadow_mutation() {
        let stored = std::sync::Arc::new(tokio::sync::Mutex::new(default_blob()));
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        let shadow_store = stored.clone();
        let shadow = tokio::spawn(async move {
            serialized_mutation(async {
                let mut snapshot = shadow_store.lock().await.clone();
                entered.send(()).unwrap();
                wait.await.unwrap();
                snapshot
                    .shadow_invites
                    .push(super::super::hks::ShadowInvite {
                        target_username: "inviter".into(),
                        salt: "salt".into(),
                        nonce: "nonce".into(),
                        ciphertext: "shadow".into(),
                        created_at: 100,
                    });
                *shadow_store.lock().await = snapshot;
                Ok(())
            })
            .await
            .unwrap();
        });
        started.await.unwrap();
        let endpoint_store = stored.clone();
        let (read, mut read_rx) = tokio::sync::oneshot::channel();
        let endpoint = tokio::spawn(async move {
            serialized_mutation(async {
                let current = endpoint_store.lock().await.clone();
                read.send(()).unwrap();
                let mut replacement = default_blob();
                replacement.payload = "refreshed".into();
                merge_live_shadows(&mut replacement, current, 150);
                *endpoint_store.lock().await = replacement;
                Ok(())
            })
            .await
            .unwrap();
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut read_rx)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        shadow.await.unwrap();
        endpoint.await.unwrap();
        let result = stored.lock().await;
        assert_eq!(result.payload, "refreshed");
        assert_eq!(result.shadow_invites[0].target_username, "inviter");
    }
}
