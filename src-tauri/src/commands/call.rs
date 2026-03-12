use tauri::State;

use crate::chat_kind::{self, ChatKind};
use crate::network::command::NetworkCommand;
use crate::NetworkState;

async fn ensure_dm_connected(
    peer_id: &str,
    state: &State<'_, NetworkState>,
    media_label: &str,
) -> Result<(), String> {
    if !matches!(chat_kind::parse_chat_kind(peer_id), ChatKind::Direct) {
        return Err(format!(
            "{} calls are only available for regular DM chats",
            media_label
        ));
    }

    let connected = {
        let connected = state.connected_chat_ids.lock().await;
        connected.contains(peer_id)
    };
    if !connected {
        return Err("Peer is not currently connected".to_string());
    }
    Ok(())
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
pub async fn end_voice_call(
    call_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
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
pub async fn end_video_call(
    call_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
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
pub async fn get_voice_call_state(
    state: State<'_, NetworkState>,
) -> Result<crate::app_state::VoiceCallState, String> {
    Ok(state.voice_call_state.lock().await.clone())
}

#[tauri::command]
pub async fn get_connected_chat_ids(state: State<'_, NetworkState>) -> Result<Vec<String>, String> {
    let connected = state.connected_chat_ids.lock().await;
    Ok(connected.iter().cloned().collect())
}
