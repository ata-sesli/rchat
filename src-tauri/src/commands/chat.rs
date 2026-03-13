use tauri::{Manager, State};

use crate::chat;
use crate::chat_kind::{self, ChatKind};
use crate::network::command::NetworkCommand;
use crate::network::gossip::{GroupContentType, GroupMessageEnvelope};
use crate::storage;
use crate::{AppState, NetworkState};

async fn mapped_github_chat_id_for_peer(
    app_state: &State<'_, AppState>,
    peer_id: &str,
) -> Option<String> {
    let mgr = app_state.config_manager.lock().await;
    let Ok(config) = mgr.load().await else {
        return None;
    };
    config
        .user
        .github_peer_mapping
        .iter()
        .find_map(|(github, mapped_peer)| {
            if mapped_peer == peer_id {
                Some(format!("gh:{}", github))
            } else {
                None
            }
        })
}

async fn resolve_peer_id_for_chat(
    app_state: &State<'_, AppState>,
    chat_id: &str,
) -> Option<String> {
    let github_username = chat_id.strip_prefix("gh:")?;
    let mgr = app_state.config_manager.lock().await;
    let Ok(config) = mgr.load().await else {
        return None;
    };
    config.user.github_peer_mapping.get(github_username).cloned()
}

#[derive(serde::Serialize)]
pub struct GroupChatResult {
    pub chat_id: String,
    pub name: String,
}

#[derive(serde::Serialize)]
pub struct ArchivedChatResult {
    pub chat_id: String,
    pub name: String,
}

#[tauri::command]
pub async fn get_chat_latest_times(
    state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<std::collections::HashMap<String, i64>, String> {
    let mut result = {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::db::get_chat_latest_times(&conn).map_err(|e| e.to_string())?
    };

    let temp_state = net_state.temporary_state.lock().await;
    for (chat_id, messages) in &temp_state.messages {
        if let Some(last) = messages.last() {
            result.insert(chat_id.clone(), last.timestamp);
        }
    }

    let mapped_chat_ids_by_peer: std::collections::HashMap<String, String> = {
        let mgr = state.config_manager.lock().await;
        match mgr.load().await {
            Ok(config) => config
                .user
                .github_peer_mapping
                .into_iter()
                .map(|(github, peer_id)| (peer_id, format!("gh:{}", github)))
                .collect(),
            Err(_) => std::collections::HashMap::new(),
        }
    };
    let mut canonical = std::collections::HashMap::new();
    for (chat_id, ts) in result {
        let key = mapped_chat_ids_by_peer
            .get(&chat_id)
            .cloned()
            .unwrap_or(chat_id);
        canonical
            .entry(key)
            .and_modify(|existing: &mut i64| {
                if ts > *existing {
                    *existing = ts;
                }
            })
            .or_insert(ts);
    }

    Ok(canonical)
}

#[tauri::command]
pub async fn get_chat_list(
    state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<Vec<storage::db::ChatListItem>, String> {
    let mut items = {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::db::get_chat_list(&conn).map_err(|e| e.to_string())?
    };

    let mut seen: std::collections::HashSet<String> =
        items.iter().map(|item| item.id.clone()).collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut temp_state = net_state.temporary_state.lock().await;
    let expired_chat_ids: Vec<String> = temp_state
        .chats
        .iter()
        .filter_map(|(id, session)| {
            if session.expires_at <= now && !session.archived {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();
    for chat_id in expired_chat_ids {
        temp_state.chats.remove(&chat_id);
        temp_state.messages.remove(&chat_id);
    }

    for (chat_id, session) in &temp_state.chats {
        if session.archived {
            continue;
        }
        if seen.contains(chat_id) {
            continue;
        }
        items.push(storage::db::ChatListItem {
            id: chat_id.clone(),
            name: session.name.clone(),
            is_group: matches!(session.kind, crate::app_state::TemporaryChatKind::Group),
        });
        seen.insert(chat_id.clone());
    }

    let mapped_chat_ids_by_peer: std::collections::HashMap<String, String> = {
        let mgr = state.config_manager.lock().await;
        match mgr.load().await {
            Ok(config) => config
                .user
                .github_peer_mapping
                .into_iter()
                .map(|(github, peer_id)| (peer_id, format!("gh:{}", github)))
                .collect(),
            Err(_) => std::collections::HashMap::new(),
        }
    };
    let mut deduped: Vec<storage::db::ChatListItem> = Vec::with_capacity(items.len());
    let mut by_id: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for mut item in items {
        if !item.is_group {
            if let Some(mapped_chat_id) = mapped_chat_ids_by_peer.get(&item.id) {
                item.id = mapped_chat_id.clone();
            }
        }

        if let Some(existing_idx) = by_id.get(&item.id).copied() {
            // Prefer non-empty/non-default names when collapsing duplicate direct rows.
            let existing_name = deduped[existing_idx].name.trim().to_string();
            let candidate_name = item.name.trim().to_string();
            let existing_is_default =
                existing_name.is_empty() || existing_name == deduped[existing_idx].id;
            let candidate_is_default = candidate_name.is_empty() || candidate_name == item.id;
            if existing_is_default && !candidate_is_default {
                deduped[existing_idx] = item;
            }
            continue;
        }

        by_id.insert(item.id.clone(), deduped.len());
        deduped.push(item);
    }

    Ok(deduped)
}

#[tauri::command]
pub async fn create_group_chat(
    name: Option<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<GroupChatResult, String> {
    let chat_id = chat_kind::generate_group_chat_id();
    let resolved_name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| chat_kind::default_group_name(&chat_id));

    {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::db::upsert_chat(&conn, &chat_id, &resolved_name, true)
            .map_err(|e| e.to_string())?;
        storage::db::add_chat_member(&conn, &chat_id, "Me", "admin").map_err(|e| e.to_string())?;
    }

    if let Some(net_state) = app_handle.try_state::<NetworkState>() {
        let tx = net_state.sender.lock().await;
        let _ = tx
            .send(NetworkCommand::SubscribeGroup {
                group_id: chat_id.clone(),
            })
            .await;
    }

    Ok(GroupChatResult {
        chat_id,
        name: resolved_name,
    })
}

#[tauri::command]
pub async fn join_group_chat(
    chat_id: String,
    name: Option<String>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<GroupChatResult, String> {
    if !chat_kind::is_group_chat_id(&chat_id) {
        return Err("Invalid group id. Expected format group:<uuid>".to_string());
    }

    let resolved_name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| chat_kind::default_group_name(&chat_id));

    {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::db::upsert_chat(&conn, &chat_id, &resolved_name, true)
            .map_err(|e| e.to_string())?;
        storage::db::add_chat_member(&conn, &chat_id, "Me", "member").map_err(|e| e.to_string())?;
    }

    if let Some(net_state) = app_handle.try_state::<NetworkState>() {
        let tx = net_state.sender.lock().await;
        let _ = tx
            .send(NetworkCommand::SubscribeGroup {
                group_id: chat_id.clone(),
            })
            .await;
    }

    Ok(GroupChatResult {
        chat_id,
        name: resolved_name,
    })
}

#[tauri::command]
pub async fn leave_group_chat(
    chat_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if !chat_kind::is_group_chat_id(&chat_id) {
        return Err("Invalid group id. Expected format group:<uuid>".to_string());
    }

    {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
        let _ = storage::db::remove_chat_member(&conn, &chat_id, "Me");
        storage::db::delete_group_chat(&conn, &chat_id).map_err(|e| e.to_string())?;
    }

    if let Some(net_state) = app_handle.try_state::<NetworkState>() {
        let tx = net_state.sender.lock().await;
        let _ = tx
            .send(NetworkCommand::UnsubscribeGroup {
                group_id: chat_id.clone(),
            })
            .await;
    }

    Ok(())
}

#[tauri::command]
pub async fn send_message_to_self(
    message: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    println!("[Backend] send_message_to_self: {}", message);
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    let msg = storage::db::Message {
        id: msg_id,
        chat_id: "self".to_string(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "text".to_string(),
        text_content: Some(message),
        file_hash: None,
        status: "read".to_string(),
        content_metadata: None,
        sender_alias: None,
    };

    match storage::db::insert_message(&conn, &msg) {
        Ok(_) => {
            println!("[Backend] Note saved successfully");
            Ok(())
        }
        Err(e) => {
            eprintln!("[Backend] Failed to save note: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn send_message(
    peer_id: String,
    message: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<String, String> {
    println!("[Backend] send_message to {}: {}", peer_id, message);

    let canonical_peer_id = if matches!(chat_kind::parse_chat_kind(&peer_id), ChatKind::Direct) {
        mapped_github_chat_id_for_peer(&app_state, &peer_id)
            .await
            .unwrap_or_else(|| peer_id.clone())
    } else {
        peer_id.clone()
    };
    let chat_kind = chat_kind::parse_chat_kind(&canonical_peer_id);

    let my_alias = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        config.user.profile.alias.clone()
    };

    let is_temporary = matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    );
    let is_archived = matches!(chat_kind, ChatKind::Archived);
    if is_archived {
        return Err("Archived chats are read-only".to_string());
    }

    let (msg_id, timestamp, outgoing_msg) = {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let id_suffix: u32 = rand::random();
        let msg_id = format!("{}-{}", timestamp, id_suffix);

        let status = match chat_kind {
            ChatKind::SelfChat => "read",
            ChatKind::Direct | ChatKind::TemporaryDirect => "pending",
            ChatKind::Group | ChatKind::TemporaryGroup => "delivered",
            ChatKind::Archived => "read",
        };

        let chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
            "self".to_string()
        } else {
            canonical_peer_id.clone()
        };

        let msg = storage::db::Message {
            id: msg_id.clone(),
            chat_id,
            peer_id: "Me".to_string(),
            timestamp,
            content_type: "text".to_string(),
            text_content: Some(message.clone()),
            file_hash: None,
            status: status.to_string(),
            content_metadata: None,
            sender_alias: my_alias.clone(),
        };

        if !is_temporary {
            let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
            match chat_kind {
                ChatKind::Direct => {
                    if !storage::db::is_peer(&conn, &canonical_peer_id) {
                        if let Err(e) =
                            storage::db::add_peer(&conn, &canonical_peer_id, None, None, "local")
                        {
                            eprintln!("[Backend] Failed to auto-add peer: {}", e);
                        }
                    }

                    if !storage::db::chat_exists(&conn, &canonical_peer_id) {
                        if let Err(e) = storage::db::create_chat(
                            &conn,
                            &canonical_peer_id,
                            &canonical_peer_id,
                            false,
                        ) {
                            eprintln!("[Backend] Failed to auto-create chat: {}", e);
                        }
                    }
                }
                ChatKind::Group => {
                    if !storage::db::chat_exists(&conn, &canonical_peer_id) {
                        storage::db::upsert_chat(
                            &conn,
                            &canonical_peer_id,
                            &chat_kind::default_group_name(&canonical_peer_id),
                            true,
                        )
                        .map_err(|e| e.to_string())?;
                        storage::db::add_chat_member(&conn, &canonical_peer_id, "Me", "member")
                            .map_err(|e| e.to_string())?;
                    }
                }
                ChatKind::SelfChat
                | ChatKind::TemporaryDirect
                | ChatKind::TemporaryGroup
                | ChatKind::Archived => {}
            }

            if let Err(e) = storage::db::insert_message(&conn, &msg) {
                eprintln!("[Backend] Failed to save outgoing message: {}", e);
                return Err(e.to_string());
            }
        }

        (msg_id, timestamp, msg)
    };

    if is_temporary {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state
            .messages
            .entry(canonical_peer_id.clone())
            .or_default()
            .push(outgoing_msg);
    }

    let direct_target_peer_id = if matches!(chat_kind, ChatKind::Direct | ChatKind::TemporaryDirect)
    {
        resolve_peer_id_for_chat(&app_state, &canonical_peer_id)
            .await
            .unwrap_or_else(|| canonical_peer_id.clone())
    } else {
        canonical_peer_id.clone()
    };

    let tx = net_state.sender.lock().await;

    match chat_kind {
        ChatKind::SelfChat => {}
        ChatKind::Direct | ChatKind::TemporaryDirect => {
            tx.send(NetworkCommand::SendDirectText {
                target_peer_id: direct_target_peer_id,
                msg_id: msg_id.clone(),
                timestamp,
                sender_alias: my_alias,
                content: message,
            })
            .await
            .map_err(|e| e.to_string())?;
        }
        ChatKind::Group | ChatKind::TemporaryGroup => {
            let envelope = GroupMessageEnvelope {
                id: msg_id.clone(),
                group_id: canonical_peer_id.clone(),
                sender_id: "Me".to_string(),
                sender_alias: my_alias,
                timestamp,
                content_type: GroupContentType::Text,
                text_content: Some(message),
                file_hash: None,
            };
            tx.send(NetworkCommand::PublishGroup { envelope })
                .await
                .map_err(|e| e.to_string())?;
        }
        ChatKind::Archived => {}
    }

    Ok(msg_id)
}

#[tauri::command]
pub async fn get_chat_history(
    chat_id: String,
    state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<Vec<storage::db::Message>, String> {
    println!("[Backend] get_chat_history for: {}", chat_id);

    let resolved_chat_id = if matches!(chat_kind::parse_chat_kind(&chat_id), ChatKind::Direct) {
        mapped_github_chat_id_for_peer(&state, &chat_id)
            .await
            .unwrap_or(chat_id.clone())
    } else {
        chat_id.clone()
    };
    let chat_kind = chat_kind::parse_chat_kind(&resolved_chat_id);
    if matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    ) {
        let temp_state = net_state.temporary_state.lock().await;
        let messages = temp_state
            .messages
            .get(&resolved_chat_id)
            .cloned()
            .unwrap_or_default();
        return Ok(messages);
    }

    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    let mut messages =
        storage::db::get_messages(&conn, &resolved_chat_id).map_err(|e| e.to_string())?;

    for db_msg in &mut messages {
        if (db_msg.content_type == "photo" || db_msg.content_type == "image")
            && db_msg.content_metadata.is_none()
            && db_msg.file_hash.is_some()
        {
            let mut rich_msg = chat::message::Message::from_db_row(db_msg);
            if rich_msg.hydrate(&conn) {
                let updated = rich_msg.to_db_row();
                db_msg.content_metadata = updated.content_metadata;
            }
        }
    }

    println!("[Backend] Found {} messages", messages.len());
    Ok(messages)
}

#[tauri::command]
pub async fn mark_messages_read(
    chat_id: String,
    state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<Vec<String>, String> {
    println!("[Backend] mark_messages_read for chat: {}", chat_id);

    let resolved_chat_id = if matches!(chat_kind::parse_chat_kind(&chat_id), ChatKind::Direct) {
        mapped_github_chat_id_for_peer(&state, &chat_id)
            .await
            .unwrap_or(chat_id.clone())
    } else {
        chat_id.clone()
    };
    let chat_kind = chat_kind::parse_chat_kind(&resolved_chat_id);

    let marked_ids = {
        if matches!(
            chat_kind,
            ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
        ) {
            let mut temp_state = net_state.temporary_state.lock().await;
            let messages = temp_state.messages.entry(resolved_chat_id.clone()).or_default();
            let mut ids = Vec::new();
            for message in messages.iter_mut() {
                if message.peer_id != "Me" && message.status != "read" {
                    message.status = "read".to_string();
                    ids.push(message.id.clone());
                }
            }
            ids
        } else {
            match chat_kind {
                ChatKind::Group => {
                    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
                    storage::db::mark_group_messages_read(&conn, &resolved_chat_id)
                        .map_err(|e| e.to_string())?
                }
                _ => {
                    let sender_id = resolve_peer_id_for_chat(&state, &resolved_chat_id)
                        .await
                        .unwrap_or_else(|| resolved_chat_id.clone());
                    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
                    storage::db::mark_messages_read(&conn, &resolved_chat_id, &sender_id)
                        .map_err(|e| e.to_string())?
                }
            }
        }
    };

    println!("[Backend] Marked {} messages as read", marked_ids.len());

    if !marked_ids.is_empty() && matches!(chat_kind, ChatKind::Direct | ChatKind::TemporaryDirect) {
        let target_peer_id = resolve_peer_id_for_chat(&state, &resolved_chat_id)
            .await
            .unwrap_or_else(|| resolved_chat_id.clone());
        let tx = net_state.sender.lock().await;
        if let Err(e) = tx
            .send(NetworkCommand::SendReadReceipt {
                target_peer_id,
                msg_ids: marked_ids.clone(),
            })
            .await
        {
            eprintln!("[Backend] Failed to send read receipt: {}", e);
        } else {
            println!(
                "[Backend] Read receipt sent for {} messages",
                marked_ids.len()
            );
        }
    }

    Ok(marked_ids)
}

#[tauri::command]
pub async fn get_unread_counts(
    my_peer_id: String,
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, i64>, String> {
    let counts = {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::db::get_unread_counts(&conn, &my_peer_id).map_err(|e| e.to_string())?
    };

    let mapped_chat_ids_by_peer: std::collections::HashMap<String, String> = {
        let mgr = state.config_manager.lock().await;
        match mgr.load().await {
            Ok(config) => config
                .user
                .github_peer_mapping
                .into_iter()
                .map(|(github, peer_id)| (peer_id, format!("gh:{}", github)))
                .collect(),
            Err(_) => std::collections::HashMap::new(),
        }
    };

    let mut canonical = std::collections::HashMap::new();
    for (chat_id, count) in counts {
        let key = mapped_chat_ids_by_peer
            .get(&chat_id)
            .cloned()
            .unwrap_or(chat_id);
        *canonical.entry(key).or_insert(0) += count;
    }
    Ok(canonical)
}

#[tauri::command]
pub async fn save_temporary_chat_to_archive(
    chat_id: String,
    state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<ArchivedChatResult, String> {
    if !chat_kind::is_temporary_chat_id(&chat_id) {
        return Err("Only temporary chats can be archived".to_string());
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let archive_chat_id = format!("archived:{}:{}", chat_id, now);

    let (session, messages) = {
        let mut temp_state = net_state.temporary_state.lock().await;
        let Some(session) = temp_state.chats.get(&chat_id).cloned() else {
            return Err("Temporary chat not found".to_string());
        };
        let messages = temp_state
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        if messages.is_empty() {
            return Err("No temporary messages to archive".to_string());
        }
        temp_state.chats.remove(&chat_id);
        temp_state.messages.remove(&chat_id);
        (session, messages)
    };

    {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

        if conn
            .query_row("SELECT 1 FROM envelopes WHERE id = 'archived'", [], |_| {
                Ok(())
            })
            .is_err()
        {
            storage::db::create_envelope(&conn, "archived", "Archived", None)
                .map_err(|e| e.to_string())?;
        }

        let archived_is_group = matches!(session.kind, crate::app_state::TemporaryChatKind::Group);
        storage::db::create_chat(&conn, &archive_chat_id, &session.name, archived_is_group)
            .map_err(|e| e.to_string())?;
        let _ = storage::db::add_chat_member(&conn, &archive_chat_id, "Me", "member");

        for (idx, mut msg) in messages.into_iter().enumerate() {
            msg.id = format!("{}-{}", msg.id, idx);
            msg.chat_id = archive_chat_id.clone();
            msg.status = "read".to_string();

            if msg.peer_id != "Me" && !storage::db::is_peer(&conn, &msg.peer_id) {
                let _ = storage::db::add_peer(&conn, &msg.peer_id, None, None, "archived");
            }

            if let Some(file_hash) = &msg.file_hash {
                let file_exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM files WHERE file_hash = ?1",
                        [file_hash],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);
                if !file_exists {
                    msg.text_content = Some(
                        msg.text_content
                            .clone()
                            .unwrap_or_else(|| "Media unavailable".to_string()),
                    );
                    msg.file_hash = None;
                }
            }

            storage::db::insert_message(&conn, &msg).map_err(|e| e.to_string())?;
        }

        storage::db::assign_chat_to_envelope(&conn, &archive_chat_id, Some("archived"))
            .map_err(|e| e.to_string())?;
    }

    {
        let tx = net_state.sender.lock().await;
        let _ = tx
            .send(NetworkCommand::EndTemporarySession {
                chat_id: chat_id.clone(),
            })
            .await;
    }

    Ok(ArchivedChatResult {
        chat_id: archive_chat_id,
        name: session.name,
    })
}
