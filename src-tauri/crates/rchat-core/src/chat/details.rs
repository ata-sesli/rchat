use anyhow::{anyhow, Result};

use crate::app_state::{AppState, NetworkState};
use crate::chat_identity::{self, DirectChatScope};
use crate::chat_kind::{self, ChatKind};
use crate::network::command::NetworkCommand;
use crate::storage::db::{self, ChatContentBreakdown, ChatFileRow};

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, Default)]
pub struct ChatConnectionView {
    pub connected: bool,
    pub remote_addr: Option<String>,
    pub connected_since: Option<i64>,
    pub last_connected_at: Option<i64>,
    pub first_connected_at: Option<i64>,
    pub reconnect_count: i64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct ChatDetailsOverview {
    pub chat_id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub peer_alias: Option<String>,
    pub avatar_url: Option<String>,
    pub connection: ChatConnectionView,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct ChatStats {
    pub sent_total: i64,
    pub received_total: i64,
    pub sent: ChatContentBreakdown,
    pub received: ChatContentBreakdown,
    pub reconnect_count: i64,
}

pub fn ensure_dm_chat(chat_id: &str) -> Result<()> {
    if matches!(chat_kind::parse_chat_kind(chat_id), ChatKind::Direct) {
        Ok(())
    } else {
        Err(anyhow!(
            "Chat details are available for direct chats only in this phase"
        ))
    }
}

pub fn resolve_dm_peer_id(chat_id: &str) -> Result<String> {
    ensure_dm_chat(chat_id)?;
    chat_identity::resolve_peer_id_for_direct_chat_id(chat_id)
        .ok_or_else(|| anyhow!("No active peer mapping found for {chat_id}"))
}

pub fn avatar_url_for_chat(chat_id: &str) -> Option<String> {
    let parsed = chat_identity::parse_scoped_direct_chat_id(chat_id)?;
    if matches!(parsed.scope, DirectChatScope::Github) {
        Some(format!("https://github.com/{}.png?size=96", parsed.name))
    } else {
        None
    }
}

pub async fn overview(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<ChatDetailsOverview> {
    ensure_dm_chat(chat_id)?;

    let peer_id = resolve_dm_peer_id(chat_id).unwrap_or_else(|_| chat_id.to_string());

    let (peer_name, peer_alias, connection_stats) = {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;

        let peer_name = db::get_chat_name(&conn, chat_id)?
            .or_else(|| chat_identity::extract_name_from_chat_id(chat_id))
            .unwrap_or_else(|| chat_id.to_string());

        let peer_alias = db::get_peer_alias(&conn, chat_id)?.or_else(|| {
            if peer_id != chat_id {
                db::get_peer_alias(&conn, &peer_id).ok().flatten()
            } else {
                None
            }
        });

        let connection_stats = db::get_chat_connection_stats(&conn, chat_id)?;

        (peer_name, peer_alias, connection_stats)
    };

    let runtime_connection = {
        let runtime = net_state.chat_connections.lock().await;
        runtime
            .get(chat_id)
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
        connected.contains(chat_id) || connected.contains(&peer_id)
    };

    Ok(ChatDetailsOverview {
        chat_id: chat_id.to_string(),
        peer_id,
        peer_name,
        peer_alias,
        avatar_url: avatar_url_for_chat(chat_id),
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

pub fn stats(app_state: &AppState, chat_id: &str) -> Result<ChatStats> {
    ensure_dm_chat(chat_id)?;

    let (message_stats, connection_stats) = {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        let message_stats = db::get_chat_message_stats(&conn, chat_id)?;
        let connection_stats = db::get_chat_connection_stats(&conn, chat_id)?;
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

pub fn delete_direct_chat(app_state: &AppState, chat_id: &str) -> Result<()> {
    ensure_dm_chat(chat_id)?;
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|error| anyhow!("database lock failed: {error}"))?;
    db::delete_direct_chat(&conn, chat_id)
}

pub fn files(
    app_state: &AppState,
    chat_id: &str,
    filter: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<ChatFileRow>> {
    ensure_dm_chat(chat_id)?;

    let conn = app_state
        .db_conn
        .lock()
        .map_err(|error| anyhow!("database lock failed: {error}"))?;
    db::list_chat_files(
        &conn,
        chat_id,
        filter.unwrap_or("all"),
        limit.unwrap_or(50),
        offset.unwrap_or(0),
    )
}

pub async fn drop_connection(net_state: &NetworkState, chat_id: &str) -> Result<()> {
    let peer_id = resolve_dm_peer_id(chat_id)?;
    let sender = net_state.sender.lock().await;
    sender
        .send(NetworkCommand::DropConnection { peer_id })
        .await
        .map_err(|error| anyhow!("Failed to drop connection: {error}"))
}

pub async fn force_reconnect(net_state: &NetworkState, chat_id: &str) -> Result<()> {
    let peer_id = resolve_dm_peer_id(chat_id)?;
    let sender = net_state.sender.lock().await;

    let _ = sender
        .send(NetworkCommand::DropConnection {
            peer_id: peer_id.clone(),
        })
        .await;

    sender
        .send(NetworkCommand::RequestConnection { peer_id })
        .await
        .map_err(|error| anyhow!("Failed to request reconnect: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::{
        BroadcastState, TemporaryRuntimeState, VoiceCallState, ChatConnectionRuntime,
    };
    use crate::storage::config::ConnectivitySettings;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex};

    const PEER_ID: &str = "12D3KooWLk1GoEB3MbHbRLHTxXrvNGSxC2UALaCuKAgKuYXkXazU";

    fn test_network_state(
        sender: mpsc::Sender<NetworkCommand>,
    ) -> NetworkState {
        NetworkState {
            sender: Arc::new(Mutex::new(sender)),
            local_peer_id: Arc::new(Mutex::new(None)),
            listening_addresses: Arc::new(Mutex::new(Vec::new())),
            public_address_v6: Arc::new(Mutex::new(None)),
            public_address_v4: Arc::new(Mutex::new(None)),
            stun_external_port: Arc::new(Mutex::new(None)),
            temporary_state: Arc::new(Mutex::new(TemporaryRuntimeState::default())),
            connected_chat_ids: Arc::new(Mutex::new(HashSet::new())),
            chat_connections: Arc::new(Mutex::new(HashMap::<String, ChatConnectionRuntime>::new())),
            voice_call_state: Arc::new(Mutex::new(VoiceCallState::default())),
            broadcast_state: Arc::new(Mutex::new(BroadcastState::default())),
            connectivity: Arc::new(Mutex::new(ConnectivitySettings::default())),
        }
    }

    #[test]
    fn rejects_non_direct_chat_ids() {
        let error = ensure_dm_chat("group:550e8400-e29b-41d4-a716-446655440000")
            .expect_err("group details should be rejected");

        assert!(error.to_string().contains("direct chats only"));
    }

    #[test]
    fn resolves_scoped_direct_peer_id() {
        let chat_id = chat_identity::build_local_chat_id("fedora", PEER_ID);

        assert_eq!(resolve_dm_peer_id(&chat_id).unwrap(), PEER_ID);
    }

    #[test]
    fn builds_github_avatar_url_only_for_github_chat() {
        let gh_id = chat_identity::build_github_chat_id("Ata Sesli", PEER_ID);
        let lh_id = chat_identity::build_local_chat_id("Ata Sesli", PEER_ID);

        assert_eq!(
            avatar_url_for_chat(&gh_id),
            Some("https://github.com/ata-sesli.png?size=96".to_string())
        );
        assert_eq!(avatar_url_for_chat(&lh_id), None);
    }

    #[tokio::test]
    async fn drop_connection_dispatches_network_command() {
        let chat_id = chat_identity::build_local_chat_id("fedora", PEER_ID);
        let (tx, mut rx) = mpsc::channel(4);
        let net_state = test_network_state(tx);

        drop_connection(&net_state, &chat_id).await.unwrap();

        match rx.recv().await.unwrap() {
            NetworkCommand::DropConnection { peer_id } => assert_eq!(peer_id, PEER_ID),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn force_reconnect_dispatches_drop_then_request() {
        let chat_id = chat_identity::build_local_chat_id("fedora", PEER_ID);
        let (tx, mut rx) = mpsc::channel(4);
        let net_state = test_network_state(tx);

        force_reconnect(&net_state, &chat_id).await.unwrap();

        match rx.recv().await.unwrap() {
            NetworkCommand::DropConnection { peer_id } => assert_eq!(peer_id, PEER_ID),
            other => panic!("unexpected first command: {other:?}"),
        }
        match rx.recv().await.unwrap() {
            NetworkCommand::RequestConnection { peer_id } => assert_eq!(peer_id, PEER_ID),
            other => panic!("unexpected second command: {other:?}"),
        }
    }
}
