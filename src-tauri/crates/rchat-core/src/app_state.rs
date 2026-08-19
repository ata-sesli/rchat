use crate::network::command::NetworkCommand;
use crate::storage::config::ConfigManager;
use crate::storage::db::Message;
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
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
    /// Number of outgoing sends (text or media) whose network dispatch is
    /// still in flight. Archive refuses to reserve the session while this is
    /// non-zero so a snapshot can never capture a message whose delivery
    /// status is unknown.
    #[serde(default)]
    pub pending_send_count: u32,
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

/// Protocol domain for temporary-group membership ops, bound into every
/// signature so an op minted for one group can never be replayed against a
/// different group (or a different protocol).
pub const TEMP_GROUP_PROTOCOL_DOMAIN: &str = "rchat-temp-group";

/// Wire version of the temporary-group membership protocol. Bumped when the
/// signed payload schema changes; ops signed under another version fail
/// verification and are rejected.
pub const TEMP_GROUP_PROTOCOL_VERSION: u8 = 1;

/// Maximum acceptable Lamport-counter jump of a single received membership op
/// above the current winner for its target. Bounds how far ahead a signed but
/// attacker-controlled counter can leap, so a hostile participant cannot claim
/// a near-`u64::MAX` counter and permanently lock out legitimate ops for a
/// target.
pub const MAX_MEMBERSHIP_OP_COUNTER_JUMP: u64 = 1_000_000;

/// A single ordered membership operation for a temporary group.
///
/// Ops are bound to the actor that issued them (a libp2p peer id), carry a
/// Lamport-style counter that is strictly increasing per actor, and are signed
/// by the actor's keypair. The winner for a target is the op with the greatest
/// `(counter, actor)` pair, compared lexicographically — a deterministic total
/// order with no wall-clock skew — so a removal issued later than an add wins
/// regardless of arrival order and rosters converge group-wide instead of
/// union-only merging resurrecting removed members.
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
    /// The temporary-group chat id this op was minted for. Bound into the
    /// signature below so a verified op can never be replayed into another
    /// group.
    #[serde(default)]
    pub chat_id: String,
    /// Protocol domain this op was minted under (see
    /// `TEMP_GROUP_PROTOCOL_DOMAIN`).
    #[serde(default)]
    pub domain: String,
    /// Protocol version this op was minted under (see
    /// `TEMP_GROUP_PROTOCOL_VERSION`).
    #[serde(default)]
    pub version: u8,
    /// Base64 protobuf public key of the actor, used to verify `signature_b64`.
    #[serde(default)]
    pub public_key_b64: String,
    /// Base64 signature by the actor over the canonical serialization of
    /// `(domain, version, chat_id, actor, counter, op, target)`. Signed ops are
    /// self-authenticating, so they can be forwarded by any member and verified
    /// on receipt.
    #[serde(default)]
    pub signature_b64: String,
}

/// The fields a membership op signs, serialized canonically so `sign` and
/// `verify` always agree on the exact bytes.
#[derive(Debug, serde::Serialize)]
struct TemporaryMembershipOpSigningPayload<'a> {
    domain: &'a str,
    version: u8,
    chat_id: &'a str,
    actor: &'a str,
    counter: u64,
    op: &'a TemporaryMembershipOpKind,
    target: &'a str,
}

impl TemporaryMembershipOp {
    /// Sign this operation with `keypair`, embedding the actor's public key so
    /// receivers can verify it without prior knowledge of the actor. Returns
    /// `false` when the actor does not match the keypair, the op is not bound
    /// to a group (`chat_id` empty), or signing fails. `domain`/`version`
    /// default to the current protocol when left empty.
    pub fn sign(&mut self, keypair: &libp2p::identity::Keypair) -> bool {
        if self.chat_id.is_empty() {
            return false;
        }
        if self.domain.is_empty() {
            self.domain = TEMP_GROUP_PROTOCOL_DOMAIN.to_string();
        }
        if self.version == 0 {
            self.version = TEMP_GROUP_PROTOCOL_VERSION;
        }
        if libp2p::PeerId::from_public_key(&keypair.public()).to_string() != self.actor {
            return false;
        }
        let Ok(canonical) = serde_json::to_vec(&TemporaryMembershipOpSigningPayload {
            domain: &self.domain,
            version: self.version,
            chat_id: &self.chat_id,
            actor: &self.actor,
            counter: self.counter,
            op: &self.op,
            target: &self.target,
        }) else {
            return false;
        };
        let Ok(signature) = keypair.sign(&canonical) else {
            return false;
        };
        self.public_key_b64 = BASE64.encode(keypair.public().encode_protobuf());
        self.signature_b64 = BASE64.encode(signature);
        true
    }

    /// Verify the signature over this operation. The embedded public key must
    /// derive exactly to `actor`, the op must be bound to the current protocol
    /// domain/version and a non-empty chat id, and the signature must match the
    /// canonical serialization of the op fields. Unsigned (legacy) ops fail
    /// verification.
    pub fn verify(&self) -> bool {
        if self.public_key_b64.is_empty() || self.signature_b64.is_empty() {
            return false;
        }
        if self.chat_id.is_empty()
            || self.domain != TEMP_GROUP_PROTOCOL_DOMAIN
            || self.version != TEMP_GROUP_PROTOCOL_VERSION
        {
            return false;
        }
        let Ok(public_key_bytes) = BASE64.decode(&self.public_key_b64) else {
            return false;
        };
        let Ok(signature) = BASE64.decode(&self.signature_b64) else {
            return false;
        };
        let Ok(public_key) = libp2p::identity::PublicKey::try_decode_protobuf(&public_key_bytes)
        else {
            return false;
        };
        if libp2p::PeerId::from_public_key(&public_key).to_string() != self.actor {
            return false;
        }
        let Ok(canonical) = serde_json::to_vec(&TemporaryMembershipOpSigningPayload {
            domain: &self.domain,
            version: self.version,
            chat_id: &self.chat_id,
            actor: &self.actor,
            counter: self.counter,
            op: &self.op,
            target: &self.target,
        }) else {
            return false;
        };
        public_key.verify(&canonical, &signature)
    }
}

fn is_valid_temp_group_member(peer_id: &str) -> bool {
    peer_id == "Me" || peer_id.parse::<libp2p::PeerId>().is_ok()
}

/// Admission context under which a membership op may change a session.
///
/// Authorization is distinct from signature authentication: a verified op is
/// only admitted when the policy for its context allows it, so a valid actor
/// can still never remove another member, and a removed member cannot re-add
/// itself without a fresh invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipOpAdmission {
    /// The op was issued locally by the session owner (its own join/leave, or
    /// its endorsement of another peer). The owner is trusted for its own
    /// session.
    Local,
    /// A remote op with no invitation capability: only self-ops of existing
    /// members, and endorsement-adds signed by an existing member, are
    /// admitted. Removing another member and non-member ops are rejected.
    Standard,
    /// A remote op accompanied by a valid invitation capability for this
    /// group: additionally admits a non-member's self-add, which is how an
    /// invited peer first joins.
    Invited,
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
    /// from the same actor. The op is signed with the provided `signer`
    /// keypair so it can be forwarded by any member and verified on receipt.
    /// Signing failures are propagated — unsigned ops are never accepted
    /// because they cannot be verified by remote peers. Returns `Ok(true)`
    /// when the roster changed, `Ok(false)` for valid no-ops, and `Err` when
    /// signing fails.
    pub fn issue_membership_op(
        &mut self,
        actor: &str,
        op: TemporaryMembershipOpKind,
        target: &str,
        signer: &libp2p::identity::Keypair,
    ) -> Result<bool> {
        if !is_valid_temp_group_member(actor) || !is_valid_temp_group_member(target) {
            return Ok(false);
        }
        // Strictly increasing per-actor Lamport counter. Refuses to wrap or
        // reach the reserved `u64::MAX` sentinel instead of panicking.
        let Some(next_counter) = self.next_member_op_counter.checked_add(1) else {
            return Err(anyhow::anyhow!("temporary-group membership op counter exhausted"));
        };
        if next_counter == u64::MAX {
            return Err(anyhow::anyhow!("temporary-group membership op counter exhausted"));
        }
        self.next_member_op_counter = next_counter;
        let mut signed_op = TemporaryMembershipOp {
            actor: actor.to_string(),
            counter: self.next_member_op_counter,
            op,
            target: target.to_string(),
            chat_id: self.chat_id.clone(),
            domain: TEMP_GROUP_PROTOCOL_DOMAIN.to_string(),
            version: TEMP_GROUP_PROTOCOL_VERSION,
            public_key_b64: String::new(),
            signature_b64: String::new(),
        };
        if !signed_op.sign(signer) {
            return Err(anyhow::anyhow!(
                "failed to sign membership op: keypair does not match actor {actor}"
            ));
        }
        Ok(self.apply_membership_op(
            signed_op,
            MembershipOpAdmission::Local,
        ))
    }

    /// Apply membership ops received from a handshake or broadcast under the
    /// standard admission policy (no invitation capability).
    ///
    /// Every op must be self-authenticating: only ops whose signature verifies
    /// against the embedded public key (which must derive exactly to the op's
    /// actor) are accepted. Because verification binds the op to its real
    /// author rather than the peer that delivered it, verified ops can be
    /// forwarded by any member and still converge transitively (A → B → C),
    /// while a participant still cannot forge operations as another member.
    /// The winner for a target is the op with the greatest `(counter, actor)`
    /// pair, so stale adds cannot resurrect a member that a newer remove
    /// already evicted. Returns `true` when the winner state changed (which
    /// may or may not change the rendered roster).
    pub fn apply_membership_ops(&mut self, ops: &[TemporaryMembershipOp]) -> bool {
        self.apply_membership_ops_admitted(ops, MembershipOpAdmission::Standard)
    }

    /// Apply membership ops with an explicit admission context (see
    /// [`MembershipOpAdmission`]). Returns `true` when the winner state
    /// changed.
    pub fn apply_membership_ops_admitted(
        &mut self,
        ops: &[TemporaryMembershipOp],
        admission: MembershipOpAdmission,
    ) -> bool {
        let mut changed = false;
        for op in ops {
            if !op.verify() {
                continue;
            }
            if !self.acceptable_op_counter(&op) {
                continue;
            }
            changed |= self.apply_membership_op(op.clone(), admission);
        }
        changed
    }

    /// Whether `op`'s Lamport counter is within the accepted bound for its
    /// target. Ops are rejected when the counter reaches the reserved
    /// `u64::MAX` sentinel or leaps more than `MAX_MEMBERSHIP_OP_COUNTER_JUMP`
    /// above the current winner, so a signed but attacker-controlled counter
    /// cannot dominate the winner map forever.
    fn acceptable_op_counter(&self, op: &TemporaryMembershipOp) -> bool {
        if op.counter == u64::MAX {
            return false;
        }
        match self.member_op_winners.get(&op.target) {
            Some(current) => {
                op.counter
                    <= current
                        .counter
                        .saturating_add(MAX_MEMBERSHIP_OP_COUNTER_JUMP)
            }
            None => op.counter <= MAX_MEMBERSHIP_OP_COUNTER_JUMP,
        }
    }

    fn apply_membership_op(
        &mut self,
        op: TemporaryMembershipOp,
        admission: MembershipOpAdmission,
    ) -> bool {
        if !is_valid_temp_group_member(&op.actor) || !is_valid_temp_group_member(&op.target) {
            return false;
        }
        // Ops are bound to the group they were minted for; a verified op from
        // another group must never leak into this session's winner state.
        if op.chat_id != self.chat_id {
            return false;
        }
        if admission != MembershipOpAdmission::Local && !self.allowed_remote_op(&op, admission) {
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
        // Lamport receive rule: advance the local clock past any accepted
        // remote operation, so a causally-later local operation issued from
        // this point supersedes it.
        self.next_member_op_counter = self.next_member_op_counter.max(op.counter);
        self.member_op_winners.insert(op.target.clone(), op.clone());
        self.derive_roster();
        true
    }

    /// Whether a remote op is authorized to change membership. Authentication
    /// (a valid signature) is a separate concern enforced before this; here we
    /// apply the group's admission policy so a member can never remove another
    /// member, a non-member cannot mutate the roster, and a removed member
    /// cannot re-admit itself without a fresh invitation.
    fn allowed_remote_op(
        &self,
        op: &TemporaryMembershipOp,
        admission: MembershipOpAdmission,
    ) -> bool {
        match op.op {
            TemporaryMembershipOpKind::Remove => {
                // Only self-removal is permitted; evicting another member is
                // never authorized by any admission context.
                if op.actor != op.target {
                    return false;
                }
                // The target must currently be an admitted member (in the
                // rendered roster, whether seeded directly or via an add
                // winner); re-removing a removed peer is a meaningless no-op.
                self.is_member(&op.target)
            }
            TemporaryMembershipOpKind::Add => {
                if op.actor == op.target {
                    // Self-add: an existing member re-announcing is a replay
                    // no-op (already admitted); a non-member joining needs the
                    // invitation capability.
                    self.is_member(&op.target)
                        || admission == MembershipOpAdmission::Invited
                } else {
                    // Endorsement: only a current member may add a new peer.
                    self.is_member(&op.actor)
                }
            }
        }
    }

    /// Re-derive the active roster from the authoritative winner state,
    /// applying the member cap deterministically.
    ///
    /// Add-winner targets are admitted in `(counter, actor)` order (the same
    /// deterministic total order used for winner comparison) up to
    /// `TEMP_GROUP_MAX_MEMBERS`; entries that were seeded directly without a
    /// signed winner (the local creator/redeemer before any ops flow) are
    /// preserved unless superseded by a remove winner, so membership seeded at
    /// session creation survives. Because every winner change re-runs this
    /// derivation, an add that was queued out at full capacity is promoted the
    /// moment a remove frees a slot.
    fn derive_roster(&mut self) {
        let mut roster: Vec<String> = self
            .members
            .iter()
            .filter(|member| {
                !matches!(
                    self.member_op_winners.get(*member),
                    Some(winner) if matches!(winner.op, TemporaryMembershipOpKind::Remove)
                )
            })
            .cloned()
            .collect();
        let mut adds: Vec<(u64, String, String)> = self
            .member_op_winners
            .iter()
            .filter(|(_, winner)| matches!(winner.op, TemporaryMembershipOpKind::Add))
            .map(|(target, winner)| (winner.counter, winner.actor.clone(), target.clone()))
            .collect();
        adds.sort_unstable();
        for (_, _, target) in adds {
            if roster.len() >= TEMP_GROUP_MAX_MEMBERS {
                break;
            }
            if !roster.iter().any(|member| member == &target) {
                roster.push(target);
            }
        }
        self.members = roster;
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

/// Manager-owned record of a temporary session frozen for archiving (two-phase
/// finalization).
///
/// The freeze retains everything needed to either commit the archive (final
/// teardown of session, messages, routing, subscription and punch target) or
/// abort it (full recovery of all of those), so a cancelled caller, an ack
/// loss or a persistence failure can never orphan a reserved session or lose
/// its conversation, routing or transport state.
#[derive(Debug, Clone)]
pub struct PendingTemporaryFinalization {
    /// Monotonic per-freeze identifier. A stale recovery (e.g. a watchdog
    /// firing after the caller moved on) only resolves the freeze it was
    /// created for.
    pub epoch: u64,
    pub chat_id: String,
    pub kind: TemporaryChatKind,
    /// The session as frozen (reservation set); reactivated on abort, dropped
    /// on commit.
    pub session: TemporaryChatSession,
    /// The final message set drained at the freeze boundary, restored on abort.
    pub messages: Vec<Message>,
    /// Connected member peer ids at freeze time, re-cached in both routing
    /// directions on abort.
    pub routing_peers: Vec<String>,
    /// Whether the gossip topic was subscribed at freeze time.
    pub was_subscribed: bool,
    /// The punch target at freeze time (multiaddr string), restored on abort.
    pub punch_target: Option<String>,
    /// Membership counter carried by the farewell remove; a rejoin add must
    /// exceed it to supersede that remove on every peer.
    pub min_add_counter: u64,
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
