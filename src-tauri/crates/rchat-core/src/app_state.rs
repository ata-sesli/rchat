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
    /// Authoritative membership state: the winning membership op per target
    /// (adds and remove tombstones). One winner per target keeps this bounded
    /// by the member cap, and because it is the complete per-target state it
    /// is safe to transmit in handshakes — unlike a truncated op log, it can
    /// always reconstruct the current roster.
    #[serde(default)]
    pub member_op_winners: HashMap<String, TemporaryMembershipOp>,
    /// Next local membership-op counter (strictly increasing Lamport counter
    /// for the local actor).
    #[serde(default)]
    pub next_member_op_counter: u64,
    #[serde(default)]
    pub archived: bool,
}

/// Hard cap on the number of members a temporary group tracks. Handshake-
/// provided entries are validated and bounded so a participant cannot inflate
/// rosters, handshake payloads, routing fan-out, or archived peer rows.
pub const TEMP_GROUP_MAX_MEMBERS: usize = 64;

/// Upper bound on the per-target winner map of a temporary-group session.
/// Each target contributes exactly one winning operation, so with the member
/// cap this is naturally bounded; the map is never trimmed, only new targets
/// are rejected once the cap is reached, so the transmitted membership state
/// stays complete.
pub const TEMP_GROUP_MAX_MEMBERSHIP_OPS: usize = 256;

/// A single ordered membership operation for a temporary group.
///
/// Ops are bound to the actor that issued them (a libp2p peer id) and carry a
/// Lamport-style counter that is strictly increasing per actor. The winner for
/// a target is the op with the greatest `(counter, actor)` pair, compared
/// lexicographically — a deterministic total order with no wall-clock skew —
/// so a removal issued later than an add wins regardless of arrival order and
/// rosters converge group-wide instead of union-only merging resurrecting
/// removed members.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryMembershipOpKind {
    Add,
    Remove,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TemporaryMembershipOp {
    pub actor: String,
    pub counter: u64,
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

    /// Issue a membership operation on behalf of `actor` (the local peer) and
    /// apply it locally. The counter is a strictly increasing Lamport counter
    /// for that actor, so locally issued ops always win over any earlier op
    /// from the same actor. Returns `true` when the roster changed.
    pub fn issue_membership_op(
        &mut self,
        actor: &str,
        op: TemporaryMembershipOpKind,
        target: &str,
    ) -> bool {
        if !is_valid_temp_group_member(actor) || !is_valid_temp_group_member(target) {
            return false;
        }
        self.next_member_op_counter += 1;
        self.apply_membership_op(TemporaryMembershipOp {
            actor: actor.to_string(),
            counter: self.next_member_op_counter,
            op,
            target: target.to_string(),
        })
    }

    /// Apply membership ops received from `authenticated_sender`.
    ///
    /// Every op must be bound to the peer that actually sent it: ops whose
    /// `actor` does not match `authenticated_sender` are rejected outright, so
    /// a participant cannot forge operations as another member or inflate
    /// counters under someone else's identity. The winner for a target is the
    /// op with the greatest `(counter, actor)` pair, so stale adds cannot
    /// resurrect a member that a newer remove already evicted. Returns `true`
    /// when the roster changed.
    pub fn apply_membership_ops(
        &mut self,
        ops: &[TemporaryMembershipOp],
        authenticated_sender: &str,
    ) -> bool {
        let mut changed = false;
        for op in ops {
            // Bind every received operation to its authenticated sender.
            if op.actor != authenticated_sender {
                continue;
            }
            changed |= self.apply_membership_op(op.clone());
        }
        changed
    }

    fn apply_membership_op(&mut self, op: TemporaryMembershipOp) -> bool {
        if !is_valid_temp_group_member(&op.actor) || !is_valid_temp_group_member(&op.target) {
            return false;
        }
        if let Some(current) = self.member_op_winners.get(&op.target) {
            // Lamport-style total order: `(counter, actor)`, strictly greater
            // wins. Equal ops (same actor + counter) are replays and ignored.
            if op.counter < current.counter
                || (op.counter == current.counter && op.actor <= current.actor)
            {
                return false;
            }
        } else if self.member_op_winners.len() >= TEMP_GROUP_MAX_MEMBERSHIP_OPS {
            // New targets are only admitted while the winner map is under its
            // cap; the map is never trimmed, so tracked targets always keep
            // their authoritative winner.
            return false;
        }
        self.member_op_winners.insert(op.target.clone(), op.clone());
        match op.op {
            TemporaryMembershipOpKind::Add => self.add_member(&op.target),
            TemporaryMembershipOpKind::Remove => self.remove_member(&op.target),
        }
    }

    /// The current winning ops of this session, one per target. This is the
    /// complete, transferable membership state (bounded by the member cap).
    pub fn membership_winners(&self) -> Vec<TemporaryMembershipOp> {
        self.member_op_winners.values().cloned().collect()
    }

    /// Whether this session holds a winner that is newer than, or missing
    /// from, the `other` ops for the same target. Drives the handshake
    /// response so a reconnecting member with a stale roster is brought up to
    /// date.
    pub fn winners_missing_from(&self, other: &[TemporaryMembershipOp]) -> bool {
        self.member_op_winners.values().any(|op| {
            let newer = other
                .iter()
                .filter(|candidate| candidate.target == op.target)
                .all(|candidate| {
                    op.counter > candidate.counter
                        || (op.counter == candidate.counter && op.actor > candidate.actor)
                });
            newer
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
