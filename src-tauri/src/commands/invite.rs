use tauri::{Emitter, Manager, State};

use crate::chat::temporary::{TemporaryChatResult, TemporaryInviteView};
use crate::chat::{direct, temporary};
use crate::network::command::NetworkCommand;
use crate::storage;
use crate::{AppState, NetworkState};

/// Generate a 14-character password for invitations
#[tauri::command]
pub async fn generate_invite_password() -> Result<String, String> {
    Ok(direct::generate_invite_password())
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
    let local_peer_id = net_state
        .local_peer_id
        .lock()
        .await
        .clone()
        .ok_or("Network peer id not available. Is the network started?")?;
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
                    addrs.iter().find(|a| {
                        a.contains("/tcp/") && !a.contains("127.0.0.1") && !a.contains("::1")
                    })
                })
                .or_else(|| addrs.first())
                .cloned()
                .ok_or("No listening address available. Is the network started?")?
        }
    };

    let encrypted_invite = invite::generate_invite(
        &password,
        &my_username,
        &invitee,
        &my_address,
        &local_peer_id,
        120,
    )
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
    discovery::publish_peer_info(&token, vec![], &app_state)
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
            let existing_peer_id = {
                let mgr = app_state.config_manager.lock().await;
                mgr.load().await.ok().and_then(|config| {
                    config
                        .user
                        .github_peer_mapping
                        .get(&github_username)
                        .cloned()
                })
            };
            let invite_peer_id = payload.inviter_peer_id.clone().and_then(|candidate| {
                if candidate.parse::<libp2p::PeerId>().is_ok() {
                    Some(candidate)
                } else {
                    None
                }
            });
            let resolved_peer_id = existing_peer_id.or(invite_peer_id).ok_or_else(|| {
                "Invitation is missing inviter peer id. Ask the inviter to generate a new invite."
                    .to_string()
            })?;
            let chat_id =
                crate::chat_identity::build_github_chat_id(&github_username, &resolved_peer_id);

            {
                let mgr = app_state.config_manager.lock().await;
                let mut config = mgr.load().await.map_err(|e| e.to_string())?;

                if !config
                    .user
                    .friends
                    .iter()
                    .any(|f| f.username == github_username)
                {
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

                                println!(
                                    "[Shadow] ⏳ Waiting 2.5s for shadow invite propagation..."
                                );
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
    let temp_kind = temporary::parse_temporary_chat_kind(&kind).map_err(|e| e.to_string())?;
    temporary::create_temporary_invite(&app_state, &net_state, temp_kind, name.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_temporary_group_invite(
    chat_id: String,
    intended_invitee: Option<String>,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<TemporaryInviteView, String> {
    temporary::create_temporary_group_invite(
        &app_state,
        &net_state,
        &chat_id,
        intended_invitee.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_active_temporary_invite(
    net_state: State<'_, NetworkState>,
) -> Result<Option<TemporaryInviteView>, String> {
    temporary::get_active_temporary_invite(&net_state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cancel_temporary_invite(net_state: State<'_, NetworkState>) -> Result<(), String> {
    temporary::cancel_temporary_invite(&net_state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn redeem_temporary_invite(
    deep_link: String,
    net_state: State<'_, NetworkState>,
) -> Result<TemporaryChatResult, String> {
    temporary::redeem_temporary_invite(&net_state, &deep_link)
        .await
        .map_err(|e| e.to_string())
}
