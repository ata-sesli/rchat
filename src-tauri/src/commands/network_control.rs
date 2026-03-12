use tauri::State;

use crate::network;
use crate::network::command::NetworkCommand;
use crate::NetworkState;

/// Request connection to a local peer (triggers mutual handshake)
#[tauri::command]
pub async fn request_connection(
    peer_id: String,
    state: State<'_, NetworkState>,
) -> Result<(), String> {
    println!("[Backend] request_connection called for: {}", peer_id);

    let sender = state.sender.lock().await;
    sender
        .send(NetworkCommand::RequestConnection { peer_id })
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    Ok(())
}

/// Enable/disable fast mDNS discovery mode
#[tauri::command]
pub fn set_fast_discovery(enabled: bool) {
    if enabled {
        network::mdns::enable_fast_discovery();
    } else {
        network::mdns::disable_fast_discovery();
    }
}
