use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LocalPeerEvent {
    pub peer_id: String,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageStatusUpdatedEvent {
    pub msg_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewGithubChatEvent {
    pub chat_id: String,
    pub github_username: String,
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporaryChatConnectedEvent {
    pub chat_id: String,
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporaryChatEndedEvent {
    pub chat_id: String,
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemporaryChatRestoredEvent {
    pub chat_id: String,
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileTransferCompleteEvent {
    pub file_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupInviteReceivedEvent {
    pub invite_id: String,
    pub group_id: String,
    pub group_name: String,
    pub inviter_peer_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupRosterUpdatedEvent {
    pub group_id: String,
    pub peer_id: String,
    pub membership_state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupRecordAppliedEvent {
    pub group_id: String,
    pub record_id: String,
    pub record_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupSyncStateUpdatedEvent {
    pub group_id: String,
    pub state: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupMessageReceiptUpdatedEvent {
    pub group_id: String,
    pub message_id: String,
    pub peer_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoQualityEvent {
    pub call_id: String,
    pub mode: String,
    pub profile: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoCameraStateEvent {
    pub call_id: String,
    pub peer_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoLocalPreviewFrameEvent {
    pub call_id: String,
    pub timestamp_us: i64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoEncodedRemoteFrameEvent {
    pub call_id: String,
    pub peer_id: String,
    pub seq: u32,
    pub timestamp: i64,
    pub mime: String,
    pub codec: String,
    pub chunk_type: crate::live::video::protocol::VideoChunkType,
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoCameraErrorEvent {
    pub call_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenBroadcastLocalPreviewFrameEvent {
    pub session_id: String,
    pub timestamp_us: i64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenBroadcastCaptureErrorEvent {
    pub session_id: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum CoreEvent {
    ConnectedChatIdsUpdated(Vec<String>),
    LocalPeerDiscovered(LocalPeerEvent),
    LocalPeerExpired(String),
    ConnectionWaiting(String),
    ConnectionRequestReceived(String),
    PeerConnected(String),
    MessageReceived(crate::storage::db::Message),
    MessageStatusUpdated(MessageStatusUpdatedEvent),
    NewGithubChat(NewGithubChatEvent),
    TemporaryChatConnected(TemporaryChatConnectedEvent),
    TemporaryChatEnded(TemporaryChatEndedEvent),
    TemporaryChatRestored(TemporaryChatRestoredEvent),
    FileTransferComplete(FileTransferCompleteEvent),
    GroupInviteReceived(GroupInviteReceivedEvent),
    GroupRosterUpdated(GroupRosterUpdatedEvent),
    GroupRecordApplied(GroupRecordAppliedEvent),
    GroupSyncStateUpdated(GroupSyncStateUpdatedEvent),
    GroupMessageReceiptUpdated(GroupMessageReceiptUpdatedEvent),
    VoiceCallStateUpdated(crate::app_state::VoiceCallState),
    BroadcastStateUpdated(crate::app_state::BroadcastState),
    ScreenBroadcastLocalPreviewFrame(ScreenBroadcastLocalPreviewFrameEvent),
    ScreenBroadcastCaptureError(ScreenBroadcastCaptureErrorEvent),
    BroadcastFrame(crate::live::broadcast::protocol::BroadcastFrameEvent),
    VideoCallCameraError(VideoCameraErrorEvent),
    VideoCallLocalPreviewFrame(VideoLocalPreviewFrameEvent),
    VideoCallQualityUpdated(VideoQualityEvent),
    VideoCallCameraState(VideoCameraStateEvent),
    VideoCallEncodedRemoteFrame(VideoEncodedRemoteFrameEvent),
}

pub trait CoreEventSink: Send + Sync {
    fn emit(&self, event: CoreEvent);
}

#[derive(Debug, Default)]
pub struct NoopEventSink;

impl CoreEventSink for NoopEventSink {
    fn emit(&self, _event: CoreEvent) {}
}

pub type SharedCoreEventSink = std::sync::Arc<dyn CoreEventSink>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_camera_error_is_a_semantic_core_event() {
        let event = CoreEvent::VideoCallCameraError(VideoCameraErrorEvent {
            call_id: "call-1".to_string(),
            message: "camera failed".to_string(),
        });

        match event {
            CoreEvent::VideoCallCameraError(payload) => {
                assert_eq!(payload.call_id, "call-1");
                assert_eq!(payload.message, "camera failed");
            }
            _ => panic!("expected semantic video camera error event"),
        }
    }
}
