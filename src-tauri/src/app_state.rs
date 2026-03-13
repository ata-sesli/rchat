use crate::network::command::NetworkCommand;
use crate::storage::config::ConfigManager;
use crate::storage::db::Message;
use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCallPhase {
    Idle,
    OutgoingRinging,
    IncomingRinging,
    Active,
    Ending,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BroadcastPhase {
    Idle,
    OutgoingRinging,
    IncomingRinging,
    Active,
    Ending,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    Voice,
    Video,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VoiceCallState {
    pub phase: VoiceCallPhase,
    pub call_kind: Option<CallKind>,
    pub call_id: Option<String>,
    pub peer_id: Option<String>,
    pub started_at: Option<i64>,
    pub ring_expires_at: Option<i64>,
    pub muted: bool,
    pub camera_enabled: bool,
    pub reason: Option<String>,
}

impl Default for VoiceCallState {
    fn default() -> Self {
        Self {
            phase: VoiceCallPhase::Idle,
            call_kind: None,
            call_id: None,
            peer_id: None,
            started_at: None,
            ring_expires_at: None,
            muted: false,
            camera_enabled: true,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BroadcastState {
    pub phase: BroadcastPhase,
    pub session_id: Option<String>,
    pub peer_id: Option<String>,
    pub started_at: Option<i64>,
    pub ring_expires_at: Option<i64>,
    pub is_host: bool,
    pub reason: Option<String>,
}

impl Default for BroadcastState {
    fn default() -> Self {
        Self {
            phase: BroadcastPhase::Idle,
            session_id: None,
            peer_id: None,
            started_at: None,
            ring_expires_at: None,
            is_host: false,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ChatConnectionRuntime {
    pub connected: bool,
    pub remote_addr: Option<String>,
    pub connected_since: Option<i64>,
    pub last_connected_at: Option<i64>,
}

// This struct holds the Sender channel.
// We wrap it in Mutex so multiple UI threads can use it safely.
pub struct NetworkState {
    pub sender: Mutex<mpsc::Sender<NetworkCommand>>,
    pub local_peer_id: Mutex<Option<String>>, // Local libp2p peer id
    pub listening_addresses: Mutex<Vec<String>>, // Current libp2p listening addresses
    pub public_address_v6: Mutex<Option<String>>, // STUN-discovered IPv6
    pub public_address_v4: Mutex<Option<String>>, // STUN-discovered IPv4
    pub stun_external_port: Mutex<Option<u16>>, // NAT-mapped UDP port for QUIC invites
    pub temporary_state: Mutex<TemporaryRuntimeState>, // In-memory temporary chat sessions/invites
    pub connected_chat_ids: Mutex<HashSet<String>>, // Currently connected chats/peers
    pub chat_connections: Mutex<HashMap<String, ChatConnectionRuntime>>, // Runtime connection metadata by chat id
    pub voice_call_state: Mutex<VoiceCallState>, // Runtime voice-call state for UI polling
    pub broadcast_state: Mutex<BroadcastState>, // Runtime DM broadcast state for UI polling
    pub connectivity: Mutex<crate::storage::config::ConnectivitySettings>, // Runtime connectivity controls
}

pub struct AppState {
    pub config_manager: tokio::sync::Mutex<ConfigManager>,
    pub db_conn: std::sync::Mutex<rusqlite::Connection>,
    pub app_dir: std::path::PathBuf,
}
