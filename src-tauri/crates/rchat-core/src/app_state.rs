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
    /// Ordered, bounded membership-op log used to converge rosters across
    /// peers (adds and remove tombstones).
    #[serde(default)]
    pub member_ops: Vec<TemporaryMembershipOp>,
    /// Per-target winner of the membership-op merge: target -> (seq, issuer).
    #[serde(default)]
    pub member_op_winners: HashMap<String, (u64, String)>,
    /// Next local membership-op sequence number (strictly increasing).
    #[serde(default)]
    pub next_member_op_seq: u64,
    #[serde(default)]
    pub archived: bool,
}

/// Hard cap on the number of members a temporary group tracks. Handshake-
/// provided entries are validated and bounded so a participant cannot inflate
/// rosters, handshake payloads, routing fan-out, or archived peer rows.
pub const TEMP_GROUP_MAX_MEMBERS: usize = 64;

/// Upper bound on the propagated membership-op log of a temporary-group
/// session. Old ops may be trimmed from the log, but the per-target winner
/// map keeps the applied state authoritative.
pub const TEMP_GROUP_MAX_MEMBERSHIP_OPS: usize = 256;

/// A single ordered membership operation for a temporary group.
///
/// Ops carry a wall-clock sequence number that is strictly increasing per
/// issuer, so a removal issued later than an add wins regardless of arrival
/// order. Merging is last-writer-wins per target (ties broken by issuer id),
/// which makes rosters converge group-wide instead of union-only merging
/// resurrecting removed members.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryMembershipOpKind {
    Add,
    Remove,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TemporaryMembershipOp {
    pub issuer: String,
    pub seq: u64,
    pub op: TemporaryMembershipOpKind,
    pub target: String,
}

fn is_valid_temp_group_member(peer_id: &str) -> bool {
    peer_id == "Me" || peer_id.parse::<libp2p::PeerId>().is_ok()
}

impl TemporaryChatSession {
    /// Whether `peer_id` is part of the tracked member roster.
    pub fn is_member(&self, peer_id: &str) -> bool {
        self.members.iter().any(|member| member == peer_id)
    }

    /// Add a member to the roster. Rejects invalid peer ids and enforces the
    /// roster cap. Returns `true` when the roster changed.
    pub fn add_member(&mut self, peer_id: &str) -> bool {
        if !is_valid_temp_group_member(peer_id) {
            return false;
        }
        if self.is_member(peer_id) {
            return false;
        }
        if self.members.len() >= TEMP_GROUP_MAX_MEMBERS {
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

    /// Issue a membership operation on behalf of `issuer` and apply it
    /// locally. Returns `true` when the roster changed.
    pub fn issue_membership_op(
        &mut self,
        issuer: &str,
        op: TemporaryMembershipOpKind,
        target: &str,
    ) -> bool {
        if !is_valid_temp_group_member(issuer) || !is_valid_temp_group_member(target) {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let seq = now.max(self.next_member_op_seq + 1);
        self.next_member_op_seq = seq;
        self.apply_membership_op(TemporaryMembershipOp {
            issuer: issuer.to_string(),
            seq,
            op,
            target: target.to_string(),
        })
    }

    /// Apply incoming membership ops (from a handshake or broadcast). Ops are
    /// last-writer-wins per target by `(seq, issuer)`, so stale adds cannot
    /// resurrect a member that a newer remove already evicted. Returns `true`
    /// when the roster changed.
    pub fn apply_membership_ops(&mut self, ops: &[TemporaryMembershipOp]) -> bool {
        let mut changed = false;
        for op in ops {
            changed |= self.apply_membership_op(op.clone());
        }
        changed
    }

    fn apply_membership_op(&mut self, op: TemporaryMembershipOp) -> bool {
        if !is_valid_temp_group_member(&op.issuer) || !is_valid_temp_group_member(&op.target) {
            return false;
        }
        let winner = self
            .member_op_winners
            .entry(op.target.clone())
            .or_insert((0, String::new()));
        if op.seq < winner.0 || (op.seq == winner.0 && op.issuer <= winner.1) {
            return false;
        }
        winner.0 = op.seq;
        winner.1 = op.issuer.clone();
        let changed = match op.op {
            TemporaryMembershipOpKind::Add => self.add_member(&op.target),
            TemporaryMembershipOpKind::Remove => self.remove_member(&op.target),
        };
        self.member_ops.push(op);
        if self.member_ops.len() > TEMP_GROUP_MAX_MEMBERSHIP_OPS {
            self.member_ops
                .drain(0..self.member_ops.len() - TEMP_GROUP_MAX_MEMBERSHIP_OPS);
        }
        changed
    }

    /// Whether this session knows a membership op the `other` ops do not
    /// (compared by issuer + sequence). Drives the handshake response so a
    /// reconnecting member with a stale log is brought up to date.
    pub fn has_ops_missing_from(&self, other: &[TemporaryMembershipOp]) -> bool {
        self.member_ops.iter().any(|op| {
            !other
                .iter()
                .any(|candidate| candidate.issuer == op.issuer && candidate.seq == op.seq)
        })
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
