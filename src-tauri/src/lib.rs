mod network;
mod oauth;
mod storage; // New module

use tauri::{Manager, State};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
// use tauri::Runtime; // Unused

use crate::storage::config::ConfigManager;
// This struct holds the Sender channel.
// We wrap it in Mutex so multiple UI threads can use it safely.
pub struct NetworkState {
    pub sender: Mutex<mpsc::Sender<String>>,
}
// Add State Management
pub struct AppState {
    pub config_manager: tokio::sync::Mutex<ConfigManager>,
    pub db_conn: std::sync::Mutex<rusqlite::Connection>,
}

#[tauri::command]
async fn save_api_token(token: String, state: State<'_, AppState>) -> Result<(), String> {
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    config.system.github_token = Some(token);
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn check_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let mgr = state.config_manager.lock().await;
    // Note: checking has_token requires reading the file, which is fine.
    // It returns false if locked.
    Ok(AuthStatus {
        is_setup: mgr.exists(),
        is_unlocked: mgr.is_unlocked(),
        is_github_connected: mgr.has_token().await,
    })
}

#[tauri::command]
async fn init_vault(password: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.config_manager.lock().await;
    mgr.init(password.trim()).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn unlock_vault(password: String, state: State<'_, AppState>) -> Result<(), String> {
    println!(
        "[Backend] unlock_vault called. Password len: {}",
        password.len()
    );
    let mut mgr = state.config_manager.lock().await;
    // Note: Logging actual password is bad practice, but length/trim check is okay for debug
    println!("[Backend] Password trimmed len: {}", password.trim().len());
    mgr.unlock_with_password(password.trim())
        .await
        .map_err(|e| {
            eprintln!("[Backend] Unlock failed: {}", e);
            e.to_string()
        })?;
    println!("[Backend] Vault unlocked successfully.");
    Ok(())
}

// OAuth Commands
#[tauri::command]
async fn start_github_auth() -> Result<oauth::AuthState, String> {
    oauth::start_device_flow().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn poll_github_auth(device_code: String) -> Result<String, String> {
    oauth::poll_for_token(&device_code)
        .await
        .map_err(|e| e.to_string())
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[derive(serde::Serialize)]
struct AuthStatus {
    is_setup: bool,
    is_unlocked: bool,
    is_github_connected: bool,
}

#[tauri::command]
async fn reset_vault(state: State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.config_manager.lock().await;
    mgr.reset().await.map_err(|e| e.to_string())?;
    Ok(())
}

use crate::storage::config::{FriendConfig, UserProfile};

#[tauri::command]
async fn get_trusted_peers(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => {
            let mut peers = vec!["Me".to_string()];
            peers.extend(config.user.friends.into_iter().map(|f| f.username));
            Ok(peers)
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn get_friends(state: State<'_, AppState>) -> Result<Vec<FriendConfig>, String> {
    println!("[Backend] get_friends called");
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.friends.clone()),
        Err(e) => {
            eprintln!("[Backend] Error loading friends: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
async fn add_friend(
    username: String,
    x25519_key: Option<String>,
    ed25519_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(mut config) => {
            if !config.user.friends.iter().any(|f| f.username == username) {
                config.user.friends.push(FriendConfig {
                    username,
                    x25519_pubkey: x25519_key,
                    ed25519_pubkey: ed25519_key,
                    leaf_index: 0, // Placeholder
                    encrypted_leaf_key: None,
                    nonce: None,
                });
                mgr.save(&config).await.map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

// Note: add_friend command is just adding to config.
// Ideally it should use HksTree::add_friend logic?
// But HksTree state isn't in Config yet.
// We need to persist HksTree in Config or File.
// For now, let's just fix the method names.

#[tauri::command]
async fn remove_friend(username: String, state: State<'_, AppState>) -> Result<(), String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(mut config) => {
            config.user.friends.retain(|f| f.username != username);
            mgr.save(&config).await.map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn get_user_profile(state: State<'_, AppState>) -> Result<UserProfile, String> {
    println!("[Backend] get_user_profile called");
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => {
            println!("[Backend] Returning profile: {:?}", config.user.profile);
            Ok(config.user.profile.clone())
        }
        Err(e) => {
            eprintln!("[Backend] Error loading config: {}", e);
            // Return default profile to prevent frontend crash
            Ok(UserProfile::default())
        }
    }
}

#[tauri::command]
async fn update_user_profile(
    alias: Option<String>,
    avatar_path: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(mut config) => {
            if let Some(a) = alias {
                config.user.profile.alias = Some(a);
            }
            if let Some(p) = avatar_path {
                config.user.profile.avatar_path = Some(p);
            }
            mgr.save(&config).await.map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn get_pinned_peers(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.pinned_peers.clone()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn toggle_pin_peer(username: String, state: State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(mut config) => {
            let mut is_pinned = false;
            // Check if exists
            if let Some(pos) = config.user.pinned_peers.iter().position(|p| p == &username) {
                config.user.pinned_peers.remove(pos);
            } else {
                config.user.pinned_peers.push(username);
                is_pinned = true;
            }
            mgr.save(&config).await.map_err(|e| e.to_string())?;
            Ok(is_pinned)
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn save_peer_order(order: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(mut config) => {
            config.user.peer_order = order;
            mgr.save(&config).await.map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn get_peer_order(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.peer_order.clone()),
        Err(e) => Err(e.to_string()),
    }
}

// --- Persistence Commands ---

#[tauri::command]
async fn send_message_to_self(message: String, state: State<'_, AppState>) -> Result<(), String> {
    println!("[Backend] send_message_to_self: {}", message);
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Generate simple ID
    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    let msg = storage::db::Message {
        id: msg_id,
        chat_id: "self".to_string(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "text".to_string(),
        text_content: Some(message),
        file_hash: None,
    };

    match storage::db::insert_message(&conn, &msg) {
        Ok(_) => {
            println!("[Backend] Note saved successfully");
            Ok(())
        }
        Err(e) => {
            eprintln!("[Backend] Failed to save note: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
async fn send_message(
    peer_id: String,
    message: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<(), String> {
    println!("[Backend] send_message to {}: {}", peer_id, message);

    // 1. Persist to DB
    {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let id_suffix: u32 = rand::random();
        let msg_id = format!("{}-{}", timestamp, id_suffix);

        let msg = storage::db::Message {
            id: msg_id,
            chat_id: peer_id.clone(),  // User checks chat with this peer
            peer_id: "Me".to_string(), // Sender is Me
            timestamp,
            content_type: "text".to_string(),
            text_content: Some(message.clone()),
            file_hash: None,
        };

        if let Err(e) = storage::db::insert_message(&conn, &msg) {
            eprintln!("[Backend] Failed to save outgoing message: {}", e);
            return Err(e.to_string());
        }
    }

    // 2. Send to Network Manager
    // We send just the content for now as it's a broadcast chat
    let tx = net_state.sender.lock().await;
    tx.send(message).await.map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
async fn get_chat_history(
    chat_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<storage::db::Message>, String> {
    println!("[Backend] get_chat_history for: {}", chat_id);
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    let messages = storage::db::get_messages(&conn, &chat_id).map_err(|e| e.to_string())?;
    println!("[Backend] Found {} messages", messages.len());
    Ok(messages)
}

// --- Envelope Commands ---

// --- Envelope Commands ---

#[tauri::command]
async fn create_envelope(
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
async fn update_envelope(
    id: String,
    name: String,
    icon: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::update_envelope(&conn, &id, &name, icon.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_envelope(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::delete_envelope(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_envelopes(state: State<'_, AppState>) -> Result<Vec<storage::db::Envelope>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::get_envelopes(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
async fn move_chat_to_envelope(
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
async fn get_envelope_assignments(
    state: State<'_, AppState>,
) -> Result<Vec<storage::db::ChatAssignment>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::get_chat_assignments(&conn).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        // --- 1. The Setup Hook ---
        .setup(|app| {
            // This runs BEFORE the window appears
            println!("RChat is initializing...");

            // Get a handle to the app if you need it for events/windows later
            let app_handle = app.handle().clone();

            // Initialize ConfigManager
            let app_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            std::fs::create_dir_all(&app_dir).expect("failed to create app data dir");
            let mut config_manager = ConfigManager::new(app_dir);

            // Initialize Database (Schema in connect)
            // storage::db::connect_to_db ensures schema exists

            // Try to restore session (auto-unlock)
            if config_manager.try_restore_session() {
                println!("Session restored successfully. Vault unlocked.");
            } else {
                println!("Session not restored. Vault locked.");
            }

            // Initialize DB Connection ONCE (Solving Race Conditions)
            let db_connection =
                storage::db::connect_to_db().expect("Failed to initialize database");

            app.manage(AppState {
                config_manager: tokio::sync::Mutex::new(config_manager),
                db_conn: std::sync::Mutex::new(db_connection),
            });

            // --- 2. Run Heavy Background Tasks ---
            // We spawn a separate async task so we don't freeze the UI startup
            tauri::async_runtime::spawn(async move {
                println!("[Backend] Starting Background Services...");

                match network::init(app_handle).await {
                    Ok(_) => println!("[Backend] network::init completed successfully"),
                    Err(e) => eprintln!("[Backend] Failed to start network: {}", e),
                }

                println!("[Backend] Background Services Ready!");
            });

            println!("[Backend] Setup hook returning Ok");
            Ok(())
        })
        // --- End Setup Hook ---
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            save_api_token,
            check_auth_status,
            init_vault,
            unlock_vault,
            start_github_auth,
            poll_github_auth,
            reset_vault,
            get_friends,
            get_trusted_peers,
            add_friend,
            remove_friend,
            get_user_profile,
            update_user_profile,
            get_pinned_peers,
            toggle_pin_peer,
            save_peer_order,
            get_peer_order,
            send_message_to_self,
            send_message,
            get_chat_history,
            create_envelope,
            update_envelope,
            delete_envelope,
            get_envelopes,
            move_chat_to_envelope,
            get_envelope_assignments,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
