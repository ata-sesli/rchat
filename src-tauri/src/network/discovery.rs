use std::time::Duration;
// use reqwest::Client; // Removed
use libp2p::Multiaddr;
use tokio::sync::mpsc::Sender;
// use serde::{Deserialize, Serialize}; // Unused
use crate::network::gist; // Import new module
use crate::network::hks::{HksTree, TrackedInvite};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{SigningKey, VerifyingKey};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

// Gist structs removed as they are handled in gist.rs now (or internal there)
// But wait, fetch_friend_peers returns Multiaddr, so we don't need Gist structs here.
const RCHAT_GIST_DESC: &str = "rchat-peer-info"; // Can be removed if not used
                                                 // const RCHAT_FILE_NAME: &str = "peers.txt"; // Can be removed

use crate::AppState;
use tauri::Manager;

pub async fn discover_peers(sender: Sender<Multiaddr>, app: tauri::AppHandle) {
    let mut interval = tokio::time::interval(Duration::from_secs(120));
    loop {
        interval.tick().await;

        // 1. Fetch Config (Friends + My Keys)
        let (friends, my_secret, my_pubkey_b64, is_online) = {
            let state = app.state::<AppState>();
            let mgr = state.config_manager.lock().await;
            if let Ok(config) = mgr.load().await {
                let secret = if let Some(s) = &config.user.encryption_private_key {
                    // Parse secret
                    if let Ok(bytes) = BASE64.decode(s) {
                        let arr: [u8; 32] = bytes.try_into().unwrap_or([0; 32]); // Simplified handling
                        Some(StaticSecret::from(arr))
                    } else {
                        None
                    }
                } else {
                    None
                };

                // If we have a secret, we can derive the public key.
                // Wait, we need my_pubkey_b64 to find my entry in the friend's roster.
                // We can derive it from secret.
                let pubkey_b64 = if let Some(ref s) = secret {
                    let pk = X25519PublicKey::from(s);
                    Some(BASE64.encode(pk.as_bytes()))
                } else {
                    None
                };

                (
                    config.user.friends.clone(),
                    secret,
                    pubkey_b64,
                    config.user.is_online,
                )
            } else {
                (vec![], None, None, false)
            }
        };

        if !is_online {
            continue;
        }

        if friends.is_empty() || my_secret.is_none() || my_pubkey_b64.is_none() {
            continue;
        }

        let my_secret = my_secret.unwrap();
        let my_pubkey_b64 = my_pubkey_b64.unwrap();

        // 2. Poll each friend
        for friend in friends {
            // We need friend's Ed25519 Public Key to verify signature.
            // If we don't have it, we can't secure discover them.
            if let Some(friend_ed_key_b64) = &friend.ed25519_pubkey {
                if let Ok(friend_ed_key_bytes) = BASE64.decode(friend_ed_key_b64) {
                    if let Ok(friend_verifying_key) =
                        VerifyingKey::from_bytes(&friend_ed_key_bytes.try_into().unwrap())
                    {
                        if let Ok(addrs) = fetch_friend_peers(
                            &friend.username,
                            &friend_verifying_key,
                            &my_secret,
                            &my_pubkey_b64,
                        )
                        .await
                        {
                            for addr in addrs {
                                let _ = sender.send(addr).await;
                            }
                        }
                    }
                }
            }
        }
    }
}

pub async fn publish_peer_info(
    token: &str,
    addrs: Vec<String>,
    app: tauri::AppHandle,
) -> anyhow::Result<()> {
    // 1. Prepare Content (HKS Blob) and extract pending invitations
    let (blob_content, pending_invites) = {
        let state = app.state::<AppState>();
        let mgr = state.config_manager.lock().await;
        // Load config to access keys and friends
        let config = mgr
            .load()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read config: {}", e))?;

        // Get Identity Keys
        let identity_priv_b64 = config
            .user
            .identity_private_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Missing Identity Private Key"))?;
        let encryption_priv_b64 = config
            .user
            .encryption_private_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Missing Encryption Private Key"))?;

        // Decode Keys
        let signing_key_bytes = BASE64.decode(identity_priv_b64)?;
        let signing_key = SigningKey::from_bytes(&signing_key_bytes.try_into().unwrap());

        let encryption_secret_bytes = BASE64.decode(encryption_priv_b64)?;
        let encryption_secret =
            StaticSecret::from(encryption_secret_bytes.try_into().unwrap_or([0; 32]));
        let encryption_pubkey = X25519PublicKey::from(&encryption_secret);

        // Build Tree
        let mut tree = HksTree::new();

        // Add Friends
        for friend in &config.user.friends {
            if let Some(friend_x25519_b64) = &friend.x25519_pubkey {
                // Add friend
                if let Err(e) =
                    tree.add_friend(&friend.username, friend_x25519_b64, &encryption_secret)
                {
                    eprintln!(
                        "Failed to add friend {} to HKS tree: {}",
                        friend.username, e
                    );
                }
            }
        }

        // Export
        let payload = addrs.join("\n");
        let blob = tree.export(&payload, &signing_key, &encryption_pubkey)?;
        
        // Parse pending invitations from config
        let invites: Vec<TrackedInvite> = if let Some(ref inv_list) = config.user.pending_invitations {
            inv_list.iter()
                .filter_map(|s| serde_json::from_str(s).ok())
                .collect()
        } else {
            vec![]
        };
        
        (blob, invites)
    };

    // 2. Inject pending invitations into blob
    let final_blob_content = if !pending_invites.is_empty() {
        match gist::parse_blob(&blob_content) {
            Ok(mut blob) => {
                blob.invitations = pending_invites;
                gist::clean_expired_invitations(&mut blob);
                println!("[Discovery] Publishing {} invitations", blob.invitations.len());
                gist::serialize_blob(&blob).unwrap_or_else(|_| blob_content.clone())
            }
            Err(_) => blob_content.clone()
        }
    } else {
        blob_content
    };

    // 3. Check for existing Gist
    let existing_gist = gist::find_rchat_gist(token).await?;

    if let Some(existing) = existing_gist {
        // Update
        let _ = gist::update_peer_info(token, &existing.id, final_blob_content).await?;
    } else {
        // Create
        let _ = gist::create_peer_info(token, final_blob_content).await?;
    }

    Ok(())
}

pub async fn fetch_friend_peers(
    username: &str,
    friend_verifying_key: &VerifyingKey,
    my_secret: &StaticSecret,
    my_pubkey_b64: &str,
) -> anyhow::Result<Vec<Multiaddr>> {
    // Use gist module to fetch content
    if let Some(blob_b64) = gist::get_friend_content(username).await? {
        // Decrypt using HKS Import
        if let Ok(payload_json) =
            HksTree::import(&blob_b64, my_pubkey_b64, my_secret, friend_verifying_key)
        {
            let mut peers = Vec::new();
            for line in payload_json.lines() {
                if let Ok(addr) = line.trim().parse::<Multiaddr>() {
                    peers.push(addr);
                }
            }
            return Ok(peers);
        } else {
            println!("Failed to decrypt blob from friend {}", username);
        }
    }

    Ok(vec![])
}
