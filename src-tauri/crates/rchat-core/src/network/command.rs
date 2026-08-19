use crate::network::gossip::{GroupInvitePayload, GroupMessageEnvelope, SignedGroupRecord};

#[derive(Debug, Clone)]
pub enum DirectMediaKind {
    Image,
    Sticker,
    Document,
    Video,
    Audio,
}

#[derive(Debug)]
pub enum NetworkCommand {
    StartPunch {
        multiaddr: String,
        target_username: String,
        my_username: String,
    },
    RequestConnection {
        peer_id: String,
    },
    DropConnection {
        peer_id: String,
    },
    RegisterShadow {
        invitee: String,
        password: String,
        my_username: String,
    },
    RegisterTemporarySession {
        chat_id: String,
        peer_id: String,
        multiaddr: String,
        is_group: bool,
    },
    /// Begin finalizing a temporary session (direct or group) for archiving —
    /// phase one of a two-phase finalization.
    ///
    /// The manager retains the session, its message buffer, the routing maps
    /// (both directions), the gossip subscription and the punch target in a
    /// `pending_finalization` entry; the session stays in the temporary state
    /// with its reservation (`archived`) set, so sends and incoming messages
    /// keep being rejected while the freeze is unresolved. The farewell
    /// winners are broadcast as the leave boundary (group sessions only),
    /// then the final message set is drained and acknowledged so the caller
    /// can persist the archive in a single transaction. `kind` is carried
    /// explicitly so a direct-message archive is never mistaken for a group
    /// one.
    ///
    /// `alive` is held by the caller until the freeze is resolved by a
    /// Commit/Abort command; if it is dropped first (the caller's task was
    /// cancelled) a manager watchdog aborts the archive so the conversation
    /// is recovered instead of left reserved forever.
    FreezeTemporaryArchive {
        chat_id: String,
        kind: crate::app_state::TemporaryChatKind,
        farewell_winners: Vec<crate::app_state::TemporaryMembershipOp>,
        /// The membership counter carried by the farewell remove; an abort's
        /// rejoin add must exceed it to supersede that remove on every peer.
        min_add_counter: u64,
        alive: tokio::sync::watch::Sender<bool>,
        ack: Option<
            tokio::sync::oneshot::Sender<Result<Vec<crate::storage::db::Message>, String>>,
        >,
    },
    /// Finalize an archived temporary session — phase two of a two-phase
    /// finalization, sent after the caller has durably persisted the archive.
    /// The manager removes the session, message buffer, routing maps, gossip
    /// subscription and punch target, then emits `TemporaryChatEnded`. A no-op
    /// when no freeze is pending for the chat (already committed or aborted).
    CommitTemporaryArchive {
        chat_id: String,
    },
    /// Recover a frozen temporary session whose archive failed, whose caller
    /// was cancelled, or whose acknowledgement was lost — the safe default
    /// whenever a freeze is never resolved. The manager clears the
    /// reservation, restores the drained messages, re-caches the routing maps,
    /// re-subscribes, re-adds the punch target, re-broadcasts a signed rejoin
    /// add (group sessions only) that outranks the farewell remove, and emits
    /// `TemporaryChatRestored`. A no-op when no freeze is pending.
    ///
    /// `epoch` pins the recovery to a specific freeze (used by the watchdog);
    /// `None` resolves whichever freeze is currently pending. The handler
    /// acknowledges only once the session, data and (for groups) the signed
    /// rejoin are all restored, so the caller never believes the peer rejoined
    /// while remote members still treat it as removed.
    AbortTemporaryArchive {
        chat_id: String,
        epoch: Option<u64>,
        ack: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    },
    SubscribeGroup {
        group_id: String,
    },
    UnsubscribeGroup {
        group_id: String,
    },
    PublishGroup {
        envelope: GroupMessageEnvelope,
    },
    PublishGroupRecord {
        record: SignedGroupRecord,
    },
    SendGroupInvite {
        target_peer_id: String,
        invite: GroupInvitePayload,
    },
    SyncGroup {
        group_id: String,
    },
    SendDirectText {
        target_peer_id: String,
        msg_id: String,
        timestamp: i64,
        sender_alias: Option<String>,
        content: String,
    },
    SendReadReceipt {
        target_peer_id: String,
        msg_ids: Vec<String>,
    },
    SendDirectMedia {
        kind: DirectMediaKind,
        target_peer_id: String,
        file_hash: String,
        file_name: Option<String>,
        msg_id: String,
        timestamp: i64,
    },
    RequestDirectFileMetadata {
        target_peer_id: String,
        file_hash: String,
    },
    StartVoiceCall {
        peer_id: String,
    },
    AcceptVoiceCall {
        call_id: String,
    },
    RejectVoiceCall {
        call_id: String,
    },
    EndVoiceCall {
        call_id: String,
    },
    SetVoiceCallMuted {
        call_id: String,
        muted: bool,
    },
    StartVideoCall {
        peer_id: String,
    },
    AcceptVideoCall {
        call_id: String,
    },
    RejectVideoCall {
        call_id: String,
    },
    EndVideoCall {
        call_id: String,
    },
    SetVideoCallMuted {
        call_id: String,
        muted: bool,
    },
    SetVideoCallCameraEnabled {
        call_id: String,
        enabled: bool,
    },
    SendVideoCallChunk {
        call_id: String,
        seq: u32,
        timestamp: i64,
        mime: String,
        codec: String,
        chunk_type: String,
        payload: Vec<u8>,
    },
    SubmitVideoCallI420Frame {
        call_id: String,
        timestamp_us: i64,
        width: u32,
        height: u32,
        profile: String,
        data: Vec<u8>,
    },
    SetVideoCallQuality {
        call_id: String,
        mode: String,
    },
    ReportVideoCallRenderStats {
        call_id: String,
        received_frames: u64,
        rendered_frames: u64,
        dropped_frames: u64,
        decode_errors: u64,
        window_seconds: Option<f64>,
    },
    StartScreenBroadcast {
        peer_id: String,
        profile: rchat_screen_capture::ScreenCaptureProfile,
    },
    AcceptScreenBroadcast {
        session_id: String,
    },
    RejectScreenBroadcast {
        session_id: String,
    },
    EndScreenBroadcast {
        session_id: String,
    },
}
