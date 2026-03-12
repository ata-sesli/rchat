use tauri::State;

use crate::storage;
use crate::AppState;

#[tauri::command]
pub async fn create_envelope(
    id: String,
    name: String,
    icon: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    println!(
        "[Backend] create_envelope call: {}, {}, icon: {:?}",
        id, name, icon
    );
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    storage::db::create_envelope(&conn, &id, &name, icon.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_envelope(
    id: String,
    name: String,
    icon: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::update_envelope(&conn, &id, &name, icon.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_envelope(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::delete_envelope(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_envelopes(
    state: State<'_, AppState>,
) -> Result<Vec<storage::db::Envelope>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::get_envelopes(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn move_chat_to_envelope(
    chat_id: String,
    envelope_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    println!(
        "[Backend] move_chat_to_envelope: chat_id={}, envelope_id={:?}",
        chat_id, envelope_id
    );
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::assign_chat_to_envelope(&conn, &chat_id, envelope_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_envelope_assignments(
    state: State<'_, AppState>,
) -> Result<Vec<storage::db::ChatAssignment>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::get_chat_assignments(&conn).map_err(|e| e.to_string())
}
