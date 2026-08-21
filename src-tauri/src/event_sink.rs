use rchat_core::events::{CoreEvent, CoreEventSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Clone)]
pub struct TauriEventSink {
    app_handle: AppHandle,
}

impl TauriEventSink {
    pub fn new(app_handle: AppHandle) -> Self {
        Self { app_handle }
    }

    fn emit_frontend<T: Serialize + Clone>(&self, name: &str, payload: T) {
        let _ = self.app_handle.emit(name, payload);
    }
}

fn frontend_event_name(event: &CoreEvent) -> &'static str {
    match event {
        CoreEvent::ConnectedChatIdsUpdated(_) => "connected-chat-ids-updated",
        CoreEvent::LocalPeerDiscovered(_) => "local-peer-discovered",
        CoreEvent::LocalPeerExpired(_) => "local-peer-expired",
        CoreEvent::ConnectionWaiting(_) => "connection-waiting",
        CoreEvent::ConnectionRequestReceived(_) => "connection-request-received",
        CoreEvent::PeerConnected(_) => "peer-connected",
        CoreEvent::MessageReceived(_) => "message-received",
        CoreEvent::MessageStatusUpdated(_) => "message-status-updated",
        CoreEvent::NewGithubChat(_) => "new-github-chat",
        CoreEvent::TemporaryChatConnected(_) => "temporary-chat-connected",
        CoreEvent::TemporaryChatEnded(_) => "temporary-chat-ended",
        CoreEvent::TemporaryChatRestored(_) => "temporary-chat-restored",
        CoreEvent::FileTransferComplete(_) => "file-transfer-complete",
        CoreEvent::GroupInviteReceived(_) => "group-invite-received",
        CoreEvent::GroupRosterUpdated(_) => "group-roster-updated",
        CoreEvent::GroupRecordApplied(_) => "group-record-applied",
        CoreEvent::GroupSyncStateUpdated(_) => "group-sync-state-updated",
        CoreEvent::GroupMessageReceiptUpdated(_) => "group-message-receipt-updated",
        CoreEvent::VoiceCallStateUpdated(_) => "voice-call-state-updated",
        CoreEvent::BroadcastStateUpdated(_) => "broadcast-state-updated",
        CoreEvent::ScreenBroadcastLocalPreviewFrame(_) => "screen-broadcast-local-preview-frame",
        CoreEvent::ScreenBroadcastCaptureError(_) => "screen-broadcast-capture-error",
        CoreEvent::BroadcastFrame(_) => "broadcast-frame",
        CoreEvent::VideoCallCameraError(_) => "video-call-camera-error",
        CoreEvent::VideoCallLocalPreviewFrame(_) => "video-call-local-preview-frame",
        CoreEvent::VideoCallQualityUpdated(_) => "video-call-quality-updated",
        CoreEvent::VideoCallCameraState(_) => "video-call-camera-state",
        CoreEvent::VideoCallEncodedRemoteFrame(_) => "video-call-encoded-remote-frame",
    }
}

impl CoreEventSink for TauriEventSink {
    fn emit(&self, event: CoreEvent) {
        let name = frontend_event_name(&event);
        match event {
            CoreEvent::ConnectedChatIdsUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::LocalPeerDiscovered(payload) => self.emit_frontend(name, payload),
            CoreEvent::LocalPeerExpired(payload) => self.emit_frontend(name, payload),
            CoreEvent::ConnectionWaiting(payload) => self.emit_frontend(name, payload),
            CoreEvent::ConnectionRequestReceived(payload) => self.emit_frontend(name, payload),
            CoreEvent::PeerConnected(payload) => self.emit_frontend(name, payload),
            CoreEvent::MessageReceived(payload) => self.emit_frontend(name, payload),
            CoreEvent::MessageStatusUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::NewGithubChat(payload) => self.emit_frontend(name, payload),
            CoreEvent::TemporaryChatConnected(payload) => self.emit_frontend(name, payload),
            CoreEvent::TemporaryChatEnded(payload) => self.emit_frontend(name, payload),
            CoreEvent::TemporaryChatRestored(payload) => self.emit_frontend(name, payload),
            CoreEvent::FileTransferComplete(payload) => self.emit_frontend(name, payload),
            CoreEvent::GroupInviteReceived(payload) => self.emit_frontend(name, payload),
            CoreEvent::GroupRosterUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::GroupRecordApplied(payload) => self.emit_frontend(name, payload),
            CoreEvent::GroupSyncStateUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::GroupMessageReceiptUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::VoiceCallStateUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::BroadcastStateUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::ScreenBroadcastLocalPreviewFrame(payload) => {
                self.emit_frontend(name, payload)
            }
            CoreEvent::ScreenBroadcastCaptureError(payload) => self.emit_frontend(name, payload),
            CoreEvent::BroadcastFrame(payload) => self.emit_frontend(name, payload),
            CoreEvent::VideoCallCameraError(payload) => self.emit_frontend(name, payload),
            CoreEvent::VideoCallLocalPreviewFrame(payload) => self.emit_frontend(name, payload),
            CoreEvent::VideoCallQualityUpdated(payload) => self.emit_frontend(name, payload),
            CoreEvent::VideoCallCameraState(payload) => self.emit_frontend(name, payload),
            CoreEvent::VideoCallEncodedRemoteFrame(payload) => self.emit_frontend(name, payload),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchat_core::events::VideoCameraErrorEvent;

    #[test]
    fn video_camera_error_maps_to_existing_frontend_event_name() {
        let event = CoreEvent::VideoCallCameraError(VideoCameraErrorEvent {
            call_id: "call-1".to_string(),
            message: "camera failed".to_string(),
        });

        assert_eq!(frontend_event_name(&event), "video-call-camera-error");
    }
}
