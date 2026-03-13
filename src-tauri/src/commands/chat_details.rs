use tauri::State;

use crate::chat_kind::{self, ChatKind};
use crate::network::command::NetworkCommand;
use crate::{AppState, NetworkState};

#[derive(serde::Serialize, Clone, Default)]
pub struct ChatConnectionView {
    pub connected: bool,
    pub remote_addr: Option<String>,
    pub connected_since: Option<i64>,
    pub last_connected_at: Option<i64>,
    pub first_connected_at: Option<i64>,
    pub reconnect_count: i64,
}

#[derive(serde::Serialize, Clone)]
pub struct ChatDetailsOverview {
    pub chat_id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub peer_alias: Option<String>,
    pub avatar_url: Option<String>,
    pub connection: ChatConnectionView,
}

#[derive(serde::Serialize, Clone)]
pub struct ChatStats {
    pub sent_total: i64,
    pub received_total: i64,
    pub sent: crate::storage::db::ChatContentBreakdown,
    pub received: crate::storage::db::ChatContentBreakdown,
    pub reconnect_count: i64,
}

fn ensure_dm_chat(chat_id: &str) -> Result<(), String> {
    if matches!(chat_kind::parse_chat_kind(chat_id), ChatKind::Direct) {
        Ok(())
    } else {
        Err("Chat details are available for direct chats only in this phase".to_string())
    }
}

async fn resolve_dm_peer_id(chat_id: &str, app_state: &State<'_, AppState>) -> Result<String, String> {
    ensure_dm_chat(chat_id)?;

    let mgr = app_state.config_manager.lock().await;
    let config = mgr.load().await.map_err(|e| e.to_string())?;
    crate::chat_identity::resolve_peer_id_for_direct_chat_id(chat_id, &config.user.github_peer_mapping)
        .ok_or_else(|| format!("No active peer mapping found for {}", chat_id))
}

fn avatar_url_for_chat(chat_id: &str) -> Option<String> {
    if let Some(parsed) = crate::chat_identity::parse_scoped_direct_chat_id(chat_id) {
        if matches!(parsed.scope, crate::chat_identity::DirectChatScope::Github) {
            return Some(format!("https://github.com/{}.png?size=96", parsed.name));
        }
    }
    if let Some(username) = chat_id.strip_prefix("gh:") {
        if !username.contains('-') {
            return Some(format!("https://github.com/{}.png?size=96", username));
        }
    }
    None
}

#[tauri::command]
pub async fn get_chat_details_overview(
    chat_id: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<ChatDetailsOverview, String> {
    ensure_dm_chat(&chat_id)?;

    let peer_id = resolve_dm_peer_id(&chat_id, &app_state)
        .await
        .unwrap_or_else(|_| chat_id.clone());

    let (peer_name, peer_alias, connection_stats) = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;

        let peer_name = crate::storage::db::get_chat_name(&conn, &chat_id)
            .map_err(|e| e.to_string())?
            .or_else(|| {
                crate::chat_identity::extract_name_from_chat_id(&chat_id)
            })
            .unwrap_or_else(|| chat_id.clone());

        let peer_alias = crate::storage::db::get_peer_alias(&conn, &chat_id)
            .map_err(|e| e.to_string())?
            .or_else(|| {
                if peer_id != chat_id {
                    crate::storage::db::get_peer_alias(&conn, &peer_id).ok().flatten()
                } else {
                    None
                }
            });

        let connection_stats = crate::storage::db::get_chat_connection_stats(&conn, &chat_id)
            .map_err(|e| e.to_string())?;

        (peer_name, peer_alias, connection_stats)
    };

    let runtime_connection = {
        let runtime = net_state.chat_connections.lock().await;
        runtime
            .get(&chat_id)
            .cloned()
            .or_else(|| {
                if peer_id != chat_id {
                    runtime.get(&peer_id).cloned()
                } else {
                    None
                }
            })
            .unwrap_or_default()
    };
    let connected_via_set = {
        let connected = net_state.connected_chat_ids.lock().await;
        connected.contains(&chat_id) || connected.contains(&peer_id)
    };

    Ok(ChatDetailsOverview {
        chat_id: chat_id.clone(),
        peer_id,
        peer_name,
        peer_alias,
        avatar_url: avatar_url_for_chat(&chat_id),
        connection: ChatConnectionView {
            connected: runtime_connection.connected || connected_via_set,
            remote_addr: runtime_connection.remote_addr,
            connected_since: runtime_connection.connected_since,
            last_connected_at: connection_stats
                .last_connected_at
                .or(runtime_connection.last_connected_at),
            first_connected_at: connection_stats.first_connected_at,
            reconnect_count: connection_stats.reconnect_count,
        },
    })
}

#[tauri::command]
pub async fn get_chat_stats(
    chat_id: String,
    app_state: State<'_, AppState>,
) -> Result<ChatStats, String> {
    ensure_dm_chat(&chat_id)?;

    let (message_stats, connection_stats) = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        let message_stats =
            crate::storage::db::get_chat_message_stats(&conn, &chat_id).map_err(|e| e.to_string())?;
        let connection_stats =
            crate::storage::db::get_chat_connection_stats(&conn, &chat_id).map_err(|e| e.to_string())?;
        (message_stats, connection_stats)
    };

    Ok(ChatStats {
        sent_total: message_stats.sent_total,
        received_total: message_stats.received_total,
        sent: message_stats.sent,
        received: message_stats.received,
        reconnect_count: connection_stats.reconnect_count,
    })
}

#[tauri::command]
pub async fn list_chat_files(
    chat_id: String,
    filter: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    app_state: State<'_, AppState>,
) -> Result<Vec<crate::storage::db::ChatFileRow>, String> {
    ensure_dm_chat(&chat_id)?;

    let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
    crate::storage::db::list_chat_files(
        &conn,
        &chat_id,
        filter.as_deref().unwrap_or("all"),
        limit.unwrap_or(50),
        offset.unwrap_or(0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn drop_chat_connection(
    chat_id: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<(), String> {
    let peer_id = resolve_dm_peer_id(&chat_id, &app_state).await?;

    let sender = net_state.sender.lock().await;
    sender
        .send(NetworkCommand::DropConnection { peer_id })
        .await
        .map_err(|e| format!("Failed to drop connection: {}", e))
}

#[tauri::command]
pub async fn force_chat_reconnect(
    chat_id: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<(), String> {
    let peer_id = resolve_dm_peer_id(&chat_id, &app_state).await?;

    let sender = net_state.sender.lock().await;

    let _ = sender
        .send(NetworkCommand::DropConnection {
            peer_id: peer_id.clone(),
        })
        .await;

    sender
        .send(NetworkCommand::RequestConnection { peer_id })
        .await
        .map_err(|e| format!("Failed to request reconnect: {}", e))
}
