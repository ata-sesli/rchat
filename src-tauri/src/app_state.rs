use crate::network::command::NetworkCommand;
use crate::storage::config::ConfigManager;
use crate::storage::db::Message;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryChatKind {
    Dm,
    Group,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemporaryInvitePayload {
    pub version: u8,
    pub kind: TemporaryChatKind,
    pub chat_id: String,
    pub inviter_peer_id: String,
    pub inviter_username: String,
    pub inviter_addr: String,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActiveTemporaryInvite {
    pub deep_link: String,
    pub payload: TemporaryInvitePayload,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemporaryChatSession {
    pub chat_id: String,
    pub name: String,
    pub kind: TemporaryChatKind,
    pub expires_at: u64,
    #[serde(default)]
    pub peer_id: Option<String>,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Debug, Default)]
pub struct TemporaryRuntimeState {
    pub active_invite: Option<ActiveTemporaryInvite>,
    pub chats: HashMap<String, TemporaryChatSession>,
    pub messages: HashMap<String, Vec<Message>>,
}

// This struct holds the Sender channel.
// We wrap it in Mutex so multiple UI threads can use it safely.
pub struct NetworkState {
    pub sender: Mutex<mpsc::Sender<NetworkCommand>>,
    pub local_peer_id: Mutex<Option<String>>, // Local libp2p peer id
    pub listening_addresses: Mutex<Vec<String>>, // Current libp2p listening addresses
    pub public_address_v6: Mutex<Option<String>>, // STUN-discovered IPv6
    pub public_address_v4: Mutex<Option<String>>, // STUN-discovered IPv4
    pub stun_external_port: Mutex<Option<u16>>,   // NAT-mapped UDP port for QUIC invites
    pub temporary_state: Mutex<TemporaryRuntimeState>, // In-memory temporary chat sessions/invites
}

pub struct AppState {
    pub config_manager: tokio::sync::Mutex<ConfigManager>,
    pub db_conn: std::sync::Mutex<rusqlite::Connection>,
    pub app_dir: std::path::PathBuf,
}
