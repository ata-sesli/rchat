use tauri::{Emitter, Manager, State};

use crate::app_state::{
    ActiveTemporaryInvite, TemporaryChatKind, TemporaryChatSession, TemporaryInvitePayload,
};
use crate::network::command::NetworkCommand;
use crate::storage;
use crate::{AppState, NetworkState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::io::{Read, Write};

const TEMP_INVITE_SCHEME_PREFIX: &str = "rchat://temp/";
const TEMP_INVITE_TTL_SECS: u64 = 120;
const TEMP_INVITE_VERSION: u8 = 1;

#[derive(serde::Serialize, Clone)]
pub struct TemporaryInviteView {
    pub deep_link: String,
    pub payload: TemporaryInvitePayload,
    pub remaining_seconds: u64,
}

#[derive(serde::Serialize)]
pub struct TemporaryChatResult {
    pub chat_id: String,
    pub name: String,
    pub kind: String,
    pub expires_at: u64,
    pub peer_id: Option<String>,
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_temp_kind(kind: &str) -> Result<TemporaryChatKind, String> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "dm" => Ok(TemporaryChatKind::Dm),
        "group" => Ok(TemporaryChatKind::Group),
        _ => Err("Invalid temporary chat kind. Use 'dm' or 'group'".to_string()),
    }
}

fn temp_kind_label(kind: &TemporaryChatKind) -> String {
    match kind {
        TemporaryChatKind::Dm => "dm".to_string(),
        TemporaryChatKind::Group => "group".to_string(),
    }
}

fn encode_temporary_payload(payload: &TemporaryInvitePayload) -> Result<String, String> {
    let json = serde_json::to_vec(payload).map_err(|e| format!("Failed to encode payload: {}", e))?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&json)
        .map_err(|e| format!("Failed to gzip payload: {}", e))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize gzip payload: {}", e))?;
    Ok(URL_SAFE_NO_PAD.encode(compressed))
}

fn decode_temporary_payload(encoded: &str) -> Result<TemporaryInvitePayload, String> {
    let gzipped = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("Invalid temporary invite payload: {}", e))?;
    let mut decoder = GzDecoder::new(gzipped.as_slice());
    let mut json = Vec::new();
    decoder
        .read_to_end(&mut json)
        .map_err(|e| format!("Failed to gunzip temporary invite payload: {}", e))?;
    let payload: TemporaryInvitePayload = serde_json::from_slice(&json)
        .map_err(|e| format!("Failed to parse temporary invite payload: {}", e))?;
    Ok(payload)
}

fn extract_temporary_payload_token(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Temporary invite link is empty".to_string());
    }
    if let Some(token) = trimmed.strip_prefix(TEMP_INVITE_SCHEME_PREFIX) {
        if token.is_empty() {
            return Err("Temporary invite link payload is empty".to_string());
        }
        return Ok(token.to_string());
    }
    Ok(trimmed.to_string())
}

async fn resolve_current_public_address(net_state: &NetworkState) -> Result<String, String> {
    let v4_stun = net_state.public_address_v4.lock().await.clone();
    let stun_port = *net_state.stun_external_port.lock().await;

    if let (Some(ip), Some(port)) = (v4_stun, stun_port) {
        return Ok(format!("/ip4/{}/udp/{}/quic-v1", ip, port));
    }

    let addrs = net_state.listening_addresses.lock().await;
    addrs
        .iter()
        .find(|a| {
            a.contains("/udp/")
                && a.contains("/quic-v1")
                && !a.contains("127.0.0.1")
                && !a.contains("::1")
        })
        .or_else(|| {
            addrs
                .iter()
                .find(|a| a.contains("/tcp/") && !a.contains("127.0.0.1") && !a.contains("::1"))
        })
        .or_else(|| addrs.first())
        .cloned()
        .ok_or("No listening address available. Is the network started?".to_string())
}

fn canonical_temp_dm_chat_id(a: &str, b: &str) -> String {
    if a <= b {
        a.to_string()
    } else {
        b.to_string()
    }
}

/// Generate a 14-character password for invitations
#[tauri::command]
pub async fn generate_invite_password() -> Result<String, String> {
    Ok(rvault_core::crypto::generate_password(14, false))
}

/// Create an invitation for a friend
#[tauri::command]
pub async fn create_invite(
    invitee: String,
    password: String,
    app_state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use crate::network::discovery;
    use crate::network::gist;
    use crate::network::invite;

    let (my_username, token) = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        let username = config
            .system
            .github_username
            .clone()
            .ok_or("GitHub username not set")?;
        let tok = config
            .system
            .github_token
            .clone()
            .ok_or("GitHub token not set")?;
        (username, tok)
    };

    let net_state = app.state::<NetworkState>();
    let my_address = {
        let v4_stun = net_state.public_address_v4.lock().await.clone();
        let stun_port = net_state.stun_external_port.lock().await.clone();

        if let (Some(ref ip), Some(port)) = (&v4_stun, stun_port) {
            let addr = format!("/ip4/{}/udp/{}/quic-v1", ip, port);
            println!("[Invite] Using QUIC STUN: {}", addr);
            addr
        } else {
            let addrs = net_state.listening_addresses.lock().await;
            addrs
                .iter()
                .find(|a| {
                    a.contains("/udp/")
                        && a.contains("/quic-v1")
                        && !a.contains("127.0.0.1")
                        && !a.contains("::1")
                })
                .or_else(|| {
                    addrs
                        .iter()
                        .find(|a| a.contains("/tcp/") && !a.contains("127.0.0.1") && !a.contains("::1"))
                })
                .or_else(|| addrs.first())
                .cloned()
                .ok_or("No listening address available. Is the network started?")?
        }
    };

    let encrypted_invite = invite::generate_invite(&password, &my_username, &invitee, &my_address, 120)
        .map_err(|e| format!("Failed to generate invite: {}", e))?;

    let tracked = gist::track_invite(encrypted_invite);

    {
        let mgr = app_state.config_manager.lock().await;
        let mut config = mgr.load().await.map_err(|e| e.to_string())?;

        if config.user.pending_invitations.is_none() {
            config.user.pending_invitations = Some(Vec::new());
        }

        if let Some(ref mut invites) = config.user.pending_invitations {
            let invite_json = serde_json::to_string(&tracked)
                .map_err(|e| format!("Failed to serialize invite: {}", e))?;
            invites.push(invite_json);
        }

        mgr.save(&config).await.map_err(|e| e.to_string())?;
    }

    println!("[Backend] Publishing invite to Gist immediately...");
    discovery::publish_peer_info(&token, vec![], app.clone())
        .await
        .map_err(|e| format!("Failed to publish invite: {}", e))?;

    println!("[Backend] Published invite to Gist");

    {
        let net_state = app.state::<NetworkState>();
        let tx = net_state.sender.lock().await;
        if let Err(e) = tx
            .send(NetworkCommand::RegisterShadow {
                invitee: invitee.clone(),
                password: password.clone(),
                my_username: my_username.clone(),
            })
            .await
        {
            println!("[Backend] Failed to register shadow poll: {}", e);
        } else {
            println!("[Backend] Registered shadow poll for {}", invitee);
        }
    }

    Ok(())
}

/// Complete invitation redemption with friend persistence and auto-message
#[tauri::command]
pub async fn redeem_and_connect(
    handle: tauri::AppHandle,
    inviter: String,
    password: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<String, String> {
    use crate::network::gist;
    use crate::network::invite;
    use crate::storage::config::FriendConfig;

    let my_username = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        config
            .system
            .github_username
            .clone()
            .ok_or("GitHub username not set")?
    };

    let encrypted_invites = gist::get_friend_invitations(&inviter)
        .await
        .map_err(|e| format!("Failed to fetch invitations: {}", e))?;

    if encrypted_invites.is_empty() {
        return Err("No invitations found from this user".to_string());
    }

    let result = invite::process_invites(&encrypted_invites, &password, &inviter, &my_username)
        .map_err(|e| format!("Failed to process invites: {}", e))?;

    match result {
        Some((payload, _index)) => {
            let github_username = inviter.clone();
            let chat_id = format!("gh:{}", github_username);

            {
                let mgr = app_state.config_manager.lock().await;
                let mut config = mgr.load().await.map_err(|e| e.to_string())?;

                if !config.user.friends.iter().any(|f| f.username == github_username) {
                    config.user.friends.push(FriendConfig {
                        username: github_username.clone(),
                        alias: None,
                        x25519_pubkey: None,
                        ed25519_pubkey: None,
                        leaf_index: 0,
                        encrypted_leaf_key: None,
                        nonce: None,
                    });
                    mgr.save(&config).await.map_err(|e| e.to_string())?;
                }
            }

            {
                let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;

                if !storage::db::is_peer(&conn, &chat_id) {
                    storage::db::add_peer(&conn, &chat_id, Some(&github_username), None, "github")
                        .map_err(|e| e.to_string())?;
                }

                if !storage::db::chat_exists(&conn, &chat_id) {
                    storage::db::create_chat(&conn, &chat_id, &github_username, false)
                        .map_err(|e| e.to_string())?;
                }
            }

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            {
                let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
                let id_suffix: u32 = rand::random();
                let msg_id = format!("{}-{}", timestamp, id_suffix);

                let msg = storage::db::Message {
                    id: msg_id.clone(),
                    chat_id: chat_id.clone(),
                    peer_id: "Me".to_string(),
                    timestamp,
                    content_type: "text".to_string(),
                    text_content: Some("Hi!".to_string()),
                    file_hash: None,
                    status: "delivered".to_string(),
                    content_metadata: None,
                    sender_alias: None,
                };

                storage::db::insert_message(&conn, &msg).map_err(|e| e.to_string())?;
            }

            {
                let my_address = {
                    let v4_stun = net_state.public_address_v4.lock().await.clone();
                    let stun_port = net_state.stun_external_port.lock().await.clone();

                    if let (Some(ip), Some(port)) = (v4_stun, stun_port) {
                        format!("/ip4/{}/udp/{}/quic-v1", ip, port)
                    } else {
                        let addrs = net_state.listening_addresses.lock().await;
                        addrs
                            .iter()
                            .find(|a| {
                                a.contains("/udp/")
                                    && a.contains("/quic-v1")
                                    && !a.contains("127.0.0.1")
                            })
                            .cloned()
                            .unwrap_or_else(|| "unknown".to_string())
                    }
                };

                let github_token = {
                    let mgr = app_state.config_manager.lock().await;
                    let config = mgr.load().await.map_err(|e| e.to_string())?;
                    config.system.github_token.clone()
                };

                if let Some(token) = github_token {
                    match invite::generate_shadow_invite(
                        &password,
                        &inviter,
                        &my_username,
                        &my_address,
                        "pending",
                    ) {
                        Ok(shadow) => {
                            if let Err(e) = gist::publish_shadow_invite(&token, shadow).await {
                                eprintln!("[Shadow] Failed to publish: {}", e);
                            } else {
                                println!("[Shadow] ✅ Published to Gist for {}", inviter);

                                println!("[Shadow] ⏳ Waiting 2.5s for shadow invite propagation...");
                                tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

                                println!(
                                    "[Backend] Sending punch command: {} -> {}",
                                    github_username, payload.ip_address
                                );

                                let tx = net_state.sender.lock().await;
                                if let Err(e) = tx
                                    .send(NetworkCommand::StartPunch {
                                        multiaddr: payload.ip_address.clone(),
                                        target_username: github_username.clone(),
                                        my_username: my_username.clone(),
                                    })
                                    .await
                                {
                                    eprintln!("[Backend] Failed to send punch command: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[Shadow] Failed to create: {}", e);
                        }
                    }
                }
            }

            println!(
                "[Backend] GitHub invite accepted from {}. Chat created: {}",
                github_username, chat_id
            );

            handle
                .emit(
                    "new-github-chat",
                    serde_json::json!({
                        "chat_id": chat_id,
                        "github_username": github_username,
                    }),
                )
                .ok();

            Ok(chat_id)
        }
        None => Err("No valid invitation found for you. Check password and usernames.".to_string()),
    }
}

#[tauri::command]
pub async fn create_temporary_invite(
    kind: String,
    name: Option<String>,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<TemporaryInviteView, String> {
    let temp_kind = parse_temp_kind(&kind)?;
    let chat_id = match temp_kind {
        TemporaryChatKind::Dm => crate::chat_kind::generate_temp_direct_chat_id(),
        TemporaryChatKind::Group => crate::chat_kind::generate_temp_group_chat_id(),
    };
    let session_name = name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| match temp_kind {
            TemporaryChatKind::Dm => crate::chat_kind::default_temp_direct_name(&chat_id),
            TemporaryChatKind::Group => crate::chat_kind::default_temp_group_name(&chat_id),
        });

    let inviter_peer_id = net_state
        .local_peer_id
        .lock()
        .await
        .clone()
        .ok_or("Network is not started yet")?;
    let inviter_addr = resolve_current_public_address(&net_state).await?;
    let inviter_username = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        config
            .system
            .github_username
            .clone()
            .or(config.user.profile.alias.clone())
            .unwrap_or_else(|| "unknown".to_string())
    };

    let created_at = now_unix_secs();
    let expires_at = created_at + TEMP_INVITE_TTL_SECS;
    let payload = TemporaryInvitePayload {
        version: TEMP_INVITE_VERSION,
        kind: temp_kind.clone(),
        chat_id: chat_id.clone(),
        inviter_peer_id,
        inviter_username,
        inviter_addr,
        created_at,
        expires_at,
    };
    let encoded = encode_temporary_payload(&payload)?;
    let deep_link = format!("{}{}", TEMP_INVITE_SCHEME_PREFIX, encoded);

    {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state.active_invite = Some(ActiveTemporaryInvite {
            deep_link: deep_link.clone(),
            payload: payload.clone(),
        });
        temp_state.chats.insert(
            chat_id.clone(),
            TemporaryChatSession {
                chat_id: chat_id.clone(),
                name: session_name,
                kind: temp_kind,
                expires_at,
                peer_id: None,
                archived: false,
            },
        );
        temp_state.messages.entry(chat_id).or_default();
    }

    Ok(TemporaryInviteView {
        deep_link,
        payload,
        remaining_seconds: TEMP_INVITE_TTL_SECS,
    })
}

#[tauri::command]
pub async fn get_active_temporary_invite(
    net_state: State<'_, NetworkState>,
) -> Result<Option<TemporaryInviteView>, String> {
    let now = now_unix_secs();
    let mut temp_state = net_state.temporary_state.lock().await;

    if let Some(active) = temp_state.active_invite.as_ref() {
        if active.payload.expires_at <= now {
            temp_state.active_invite = None;
            return Ok(None);
        }
    }

    Ok(temp_state.active_invite.as_ref().map(|active| TemporaryInviteView {
        deep_link: active.deep_link.clone(),
        payload: active.payload.clone(),
        remaining_seconds: active.payload.expires_at.saturating_sub(now),
    }))
}

#[tauri::command]
pub async fn cancel_temporary_invite(net_state: State<'_, NetworkState>) -> Result<(), String> {
    let mut temp_state = net_state.temporary_state.lock().await;
    if let Some(active) = temp_state.active_invite.take() {
        if let Some(session) = temp_state.chats.get(&active.payload.chat_id).cloned() {
            let has_messages = temp_state
                .messages
                .get(&active.payload.chat_id)
                .map(|m| !m.is_empty())
                .unwrap_or(false);
            if session.peer_id.is_none() && !has_messages {
                temp_state.chats.remove(&active.payload.chat_id);
                temp_state.messages.remove(&active.payload.chat_id);
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn redeem_temporary_invite(
    deep_link: String,
    net_state: State<'_, NetworkState>,
) -> Result<TemporaryChatResult, String> {
    let token = extract_temporary_payload_token(&deep_link)?;
    let payload = decode_temporary_payload(&token)?;
    if payload.version != TEMP_INVITE_VERSION {
        return Err(format!(
            "Unsupported temporary invite version: {}",
            payload.version
        ));
    }

    let now = now_unix_secs();
    if payload.expires_at <= now {
        return Err("Temporary invite has expired".to_string());
    }

    let mut temp_state = net_state.temporary_state.lock().await;
    let Some(local_active) = temp_state.active_invite.clone() else {
        return Err("Create a temporary invite first before redeeming one".to_string());
    };
    if local_active.payload.expires_at <= now {
        temp_state.active_invite = None;
        return Err("Your temporary invite has expired. Create a new one first".to_string());
    }
    if local_active.payload.kind != payload.kind {
        return Err("Temporary invite kind mismatch (dm/group)".to_string());
    }

    let is_group = matches!(payload.kind, TemporaryChatKind::Group);
    let resolved_chat_id = if is_group {
        payload.chat_id.clone()
    } else {
        canonical_temp_dm_chat_id(&local_active.payload.chat_id, &payload.chat_id)
    };
    let expires_at = local_active.payload.expires_at.min(payload.expires_at);
    let resolved_name = if is_group {
        crate::chat_kind::default_temp_group_name(&resolved_chat_id)
    } else {
        crate::chat_kind::default_temp_direct_name(&resolved_chat_id)
    };

    if local_active.payload.chat_id != resolved_chat_id {
        temp_state.chats.remove(&local_active.payload.chat_id);
        temp_state.messages.remove(&local_active.payload.chat_id);
    }

    let entry = temp_state
        .chats
        .entry(resolved_chat_id.clone())
        .or_insert_with(|| TemporaryChatSession {
            chat_id: resolved_chat_id.clone(),
            name: resolved_name.clone(),
            kind: payload.kind.clone(),
            expires_at,
            peer_id: Some(payload.inviter_peer_id.clone()),
            archived: false,
        });
    entry.name = resolved_name.clone();
    entry.kind = payload.kind.clone();
    entry.expires_at = expires_at;
    entry.peer_id = Some(payload.inviter_peer_id.clone());
    entry.archived = false;
    temp_state.messages.entry(resolved_chat_id.clone()).or_default();
    drop(temp_state);

    {
        let tx = net_state.sender.lock().await;
        tx.send(NetworkCommand::RegisterTemporarySession {
            chat_id: resolved_chat_id.clone(),
            peer_id: payload.inviter_peer_id.clone(),
            multiaddr: payload.inviter_addr.clone(),
            is_group,
        })
        .await
        .map_err(|e| format!("Failed to start temporary session: {}", e))?;
    }

    Ok(TemporaryChatResult {
        chat_id: resolved_chat_id,
        name: resolved_name,
        kind: temp_kind_label(&payload.kind),
        expires_at,
        peer_id: Some(payload.inviter_peer_id),
    })
}
