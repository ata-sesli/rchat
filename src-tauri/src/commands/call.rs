use tauri::State;

use crate::chat_identity;
use crate::chat_kind::{self, ChatKind};
use crate::network::command::NetworkCommand;
use crate::NetworkState;
use std::collections::HashSet;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct VideoRenderStatsInput {
    pub received_frames: u64,
    pub rendered_frames: u64,
    pub dropped_frames: u64,
    pub decode_errors: u64,
    #[serde(default)]
    pub window_seconds: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VideoCaptureDeviceInfo {
    pub id: String,
    pub index: u32,
    pub name: String,
    pub description: String,
    pub backend: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VideoCaptureSupport {
    pub supported: bool,
    pub reason: Option<String>,
    pub devices: Vec<VideoCaptureDeviceInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScreenCaptureSupport {
    pub supported: bool,
    pub reason: Option<String>,
    pub backend: String,
}

fn direct_presence_key(chat_id: &str) -> String {
    let normalized = if chat_id == "self" { "Me" } else { chat_id };
    chat_identity::extract_peer_id_from_chat_id(normalized)
        .unwrap_or_else(|| normalized.to_string())
}

fn connected_ids_contain_direct_peer(peer_id: &str, connected: &HashSet<String>) -> bool {
    let target = direct_presence_key(peer_id);
    connected
        .iter()
        .any(|connected_id| direct_presence_key(connected_id) == target)
}

fn validate_dm_call_target(
    peer_id: &str,
    connected: &HashSet<String>,
    media_label: &str,
) -> Result<(), String> {
    if !matches!(chat_kind::parse_chat_kind(peer_id), ChatKind::Direct) {
        return Err(format!(
            "{} calls are only available for regular DM chats",
            media_label
        ));
    }

    if !connected_ids_contain_direct_peer(peer_id, connected) {
        return Err("Peer is not currently connected".to_string());
    }

    Ok(())
}

async fn ensure_dm_connected(
    peer_id: &str,
    state: &State<'_, NetworkState>,
    media_label: &str,
) -> Result<(), String> {
    let connected = {
        let connected = state.connected_chat_ids.lock().await;
        connected.clone()
    };
    validate_dm_call_target(peer_id, &connected, media_label)
}

#[tauri::command]
pub async fn start_voice_call(
    peer_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    ensure_dm_connected(&peer_id, &state, "Voice").await?;

    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::StartVoiceCall { peer_id })
        .await
        .map_err(|e| format!("Failed to start voice call: {}", e))
}

#[tauri::command]
pub async fn accept_voice_call(
    call_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::AcceptVoiceCall { call_id })
        .await
        .map_err(|e| format!("Failed to accept voice call: {}", e))
}

#[tauri::command]
pub async fn reject_voice_call(
    call_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::RejectVoiceCall { call_id })
        .await
        .map_err(|e| format!("Failed to reject voice call: {}", e))
}

#[tauri::command]
pub async fn end_voice_call(call_id: String, state: State<'_, NetworkState>) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::EndVoiceCall { call_id })
        .await
        .map_err(|e| format!("Failed to end voice call: {}", e))
}

#[tauri::command]
pub async fn set_voice_call_muted(
    call_id: String,
    muted: bool,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::SetVoiceCallMuted { call_id, muted })
        .await
        .map_err(|e| format!("Failed to update mute state: {}", e))
}

#[tauri::command]
pub async fn start_video_call(
    peer_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    ensure_dm_connected(&peer_id, &state, "Video").await?;

    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::StartVideoCall { peer_id })
        .await
        .map_err(|e| format!("Failed to start video call: {}", e))
}

#[tauri::command]
pub async fn accept_video_call(
    call_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::AcceptVideoCall { call_id })
        .await
        .map_err(|e| format!("Failed to accept video call: {}", e))
}

#[tauri::command]
pub async fn reject_video_call(
    call_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::RejectVideoCall { call_id })
        .await
        .map_err(|e| format!("Failed to reject video call: {}", e))
}

#[tauri::command]
pub async fn end_video_call(call_id: String, state: State<'_, NetworkState>) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::EndVideoCall { call_id })
        .await
        .map_err(|e| format!("Failed to end video call: {}", e))
}

#[tauri::command]
pub async fn set_video_call_muted(
    call_id: String,
    muted: bool,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::SetVideoCallMuted { call_id, muted })
        .await
        .map_err(|e| format!("Failed to update video mute state: {}", e))
}

#[tauri::command]
pub async fn set_video_call_camera_enabled(
    call_id: String,
    enabled: bool,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::SetVideoCallCameraEnabled { call_id, enabled })
        .await
        .map_err(|e| format!("Failed to update camera state: {}", e))
}

#[tauri::command]
pub async fn send_video_call_chunk(
    call_id: String,
    seq: u32,
    timestamp: i64,
    mime: String,
    codec: String,
    chunk_type: String,
    payload: Vec<u8>,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::SendVideoCallChunk {
            call_id,
            seq,
            timestamp,
            mime,
            codec,
            chunk_type,
            payload,
        })
        .await
        .map_err(|e| format!("Failed to send video chunk: {}", e))
}

#[tauri::command]
pub async fn submit_video_call_i420_frame(
    call_id: String,
    timestamp_us: i64,
    width: u32,
    height: u32,
    profile: String,
    data: Vec<u8>,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    match sender.try_send(NetworkCommand::SubmitVideoCallI420Frame {
        call_id,
        timestamp_us,
        width,
        height,
        profile,
        data,
    }) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Ok(()),
        Err(e) => Err(format!("Failed to submit video frame: {}", e)),
    }
}

#[tauri::command]
pub async fn set_video_call_quality(
    call_id: String,
    mode: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::SetVideoCallQuality { call_id, mode })
        .await
        .map_err(|e| format!("Failed to update video quality: {}", e))
}

#[tauri::command]
pub async fn report_video_call_render_stats(
    call_id: String,
    stats: VideoRenderStatsInput,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::ReportVideoCallRenderStats {
            call_id,
            received_frames: stats.received_frames,
            rendered_frames: stats.rendered_frames,
            dropped_frames: stats.dropped_frames,
            decode_errors: stats.decode_errors,
            window_seconds: stats.window_seconds,
        })
        .await
        .map_err(|e| format!("Failed to report video render stats: {}", e))
}

#[tauri::command]
pub async fn get_video_capture_support() -> Result<VideoCaptureSupport, String> {
    match rchat_video_capture::list_devices() {
        Ok(devices) => {
            let devices = devices.into_iter().map(video_capture_device_info).collect();
            Ok(VideoCaptureSupport {
                supported: true,
                reason: None,
                devices,
            })
        }
        Err(error) => Ok(VideoCaptureSupport {
            supported: false,
            reason: Some(error.to_string()),
            devices: Vec::new(),
        }),
    }
}

#[tauri::command]
pub async fn get_screen_capture_support() -> Result<ScreenCaptureSupport, String> {
    let support = rchat_screen_capture::screen_capture_support().await;
    Ok(ScreenCaptureSupport {
        supported: support.supported,
        reason: support.reason,
        backend: support.backend.label().to_string(),
    })
}

#[tauri::command]
pub async fn get_video_capture_devices() -> Result<Vec<VideoCaptureDeviceInfo>, String> {
    rchat_video_capture::list_devices()
        .map(|devices| devices.into_iter().map(video_capture_device_info).collect())
        .map_err(|error| error.to_string())
}

fn video_capture_device_info(
    device: rchat_video_capture::CaptureDeviceInfo,
) -> VideoCaptureDeviceInfo {
    VideoCaptureDeviceInfo {
        id: device.id,
        index: device.index,
        name: device.name,
        description: device.description,
        backend: device.backend,
    }
}

#[tauri::command]
pub async fn start_screen_broadcast(
    peer_id: String,
    profile: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    ensure_dm_connected(&peer_id, &state, "Screen broadcast").await?;
    let profile = rchat_screen_capture::ScreenCaptureProfile::from_label(&profile)
        .ok_or_else(|| format!("Unsupported screen broadcast profile: {}", profile))?;

    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::StartScreenBroadcast { peer_id, profile })
        .await
        .map_err(|e| format!("Failed to start screen broadcast: {}", e))
}

#[tauri::command]
pub async fn accept_screen_broadcast(
    session_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::AcceptScreenBroadcast { session_id })
        .await
        .map_err(|e| format!("Failed to accept screen broadcast: {}", e))
}

#[tauri::command]
pub async fn reject_screen_broadcast(
    session_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::RejectScreenBroadcast { session_id })
        .await
        .map_err(|e| format!("Failed to reject screen broadcast: {}", e))
}

#[tauri::command]
pub async fn end_screen_broadcast(
    session_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::EndScreenBroadcast { session_id })
        .await
        .map_err(|e| format!("Failed to end screen broadcast: {}", e))
}

#[tauri::command]
pub async fn get_voice_call_state(
    state: State<'_, NetworkState>,
) -> Result<crate::app_state::VoiceCallState, String> {
    Ok(state.voice_call_state.lock().await.clone())
}

#[tauri::command]
pub async fn get_broadcast_state(
    state: State<'_, NetworkState>,
) -> Result<crate::app_state::BroadcastState, String> {
    Ok(state.broadcast_state.lock().await.clone())
}

#[tauri::command]
pub async fn get_connected_chat_ids(state: State<'_, NetworkState>) -> Result<Vec<String>, String> {
    let connected = state.connected_chat_ids.lock().await;
    Ok(connected.iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const PEER_ID: &str = "12D3KooWLk1GoEB3MbHbRLHTxXrvNGSxC2UALaCuKAgKuYXkXazU";
    const OTHER_PEER_ID: &str = "12D3KooWQap2BiV8iNRjj23tn2uq3ekjaEFmzjici2RHvht63RGQ";

    fn connected(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn call_validation_accepts_exact_connected_chat_id() {
        let chat_id = format!("lh:ata-{}", PEER_ID);

        assert!(validate_dm_call_target(&chat_id, &connected(&[&chat_id]), "Voice").is_ok());
    }

    #[test]
    fn call_validation_accepts_raw_peer_connection_for_scoped_chat_id() {
        let chat_id = format!("lh:ata-{}", PEER_ID);

        assert!(validate_dm_call_target(&chat_id, &connected(&[PEER_ID]), "Voice").is_ok());
    }

    #[test]
    fn call_validation_accepts_scoped_connection_for_raw_peer_id() {
        let connected_chat_id = format!("gh:ata-{}", PEER_ID);

        assert!(
            validate_dm_call_target(PEER_ID, &connected(&[&connected_chat_id]), "Voice").is_ok()
        );
    }

    #[test]
    fn call_validation_rejects_unrelated_connected_peer() {
        let chat_id = format!("lh:ata-{}", PEER_ID);

        assert_eq!(
            validate_dm_call_target(&chat_id, &connected(&[OTHER_PEER_ID]), "Voice"),
            Err("Peer is not currently connected".to_string())
        );
    }

    #[test]
    fn call_validation_rejects_non_regular_dm_chat_ids() {
        let connected_ids = connected(&[]);
        let non_dm_ids = [
            "group:00000000-0000-4000-8000-000000000000",
            "tempdm:00000000-0000-4000-8000-000000000000",
            "archived:lh:ata-old",
        ];

        for chat_id in non_dm_ids {
            assert_eq!(
                validate_dm_call_target(chat_id, &connected_ids, "Voice"),
                Err("Voice calls are only available for regular DM chats".to_string())
            );
        }
    }
}
