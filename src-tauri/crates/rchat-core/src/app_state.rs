use crate::network::command::NetworkCommand;
use crate::storage::config::ConfigManager;
use crate::storage::db::Message;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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
    /// Primary remote peer id. Temporary direct chats use this as their single
    /// remote peer; temporary groups keep it in sync with the member roster so
    /// existing consumers keep working while `members` drives multi-party
    /// routing.
    #[serde(default)]
    pub peer_id: Option<String>,
    /// Full member roster for temporary-group sessions, including the local
    /// peer id. Empty for temporary direct chats.
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub archived: bool,
}

impl TemporaryChatSession {
    /// Whether `peer_id` is part of the tracked member roster.
    pub fn is_member(&self, peer_id: &str) -> bool {
        self.members.iter().any(|member| member == peer_id)
    }

    /// Add a member to the roster. Returns `true` when the roster changed.
    pub fn add_member(&mut self, peer_id: &str) -> bool {
        if self.is_member(peer_id) {
            return false;
        }
        self.members.push(peer_id.to_string());
        true
    }

    /// Remove a member from the roster. Returns `true` when the roster changed.
    pub fn remove_member(&mut self, peer_id: &str) -> bool {
        let before = self.members.len();
        self.members.retain(|member| member != peer_id);
        self.members.len() != before
    }

    /// Remote members of a temporary-group session, excluding the local peer
    /// id (and the literal `"Me"` marker). Used to fan out per-peer requests
    /// to every eligible member without duplicates.
    pub fn remote_members(&self, local_peer_id: Option<&str>) -> Vec<String> {
        self.members
            .iter()
            .filter(|member| {
                member.as_str() != "Me" && Some(member.as_str()) != local_peer_id
            })
            .cloned()
            .collect()
    }
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
#[derive(Clone)]
pub struct NetworkState {
    pub sender: Arc<Mutex<mpsc::Sender<NetworkCommand>>>,
    pub local_peer_id: Arc<Mutex<Option<String>>>, // Local libp2p peer id
    pub listening_addresses: Arc<Mutex<Vec<String>>>, // Current libp2p listening addresses
    pub public_address_v6: Arc<Mutex<Option<String>>>, // STUN-discovered IPv6
    pub public_address_v4: Arc<Mutex<Option<String>>>, // STUN-discovered IPv4
    pub stun_external_port: Arc<Mutex<Option<u16>>>, // NAT-mapped UDP port for QUIC invites
    pub temporary_state: Arc<Mutex<TemporaryRuntimeState>>, // In-memory temporary chat sessions/invites
    pub connected_chat_ids: Arc<Mutex<HashSet<String>>>, // Currently connected chats/peers
    pub chat_connections: Arc<Mutex<HashMap<String, ChatConnectionRuntime>>>, // Runtime connection metadata by chat id
    pub voice_call_state: Arc<Mutex<VoiceCallState>>, // Runtime voice-call state for UI polling
    pub broadcast_state: Arc<Mutex<BroadcastState>>, // Runtime DM broadcast state for UI polling
    pub connectivity: Arc<Mutex<crate::storage::config::ConnectivitySettings>>, // Runtime connectivity controls
}

#[derive(Clone)]
pub struct AppState {
    pub config_manager: Arc<tokio::sync::Mutex<ConfigManager>>,
    pub db_conn: Arc<std::sync::Mutex<rusqlite::Connection>>,
    pub app_dir: std::path::PathBuf,
}
