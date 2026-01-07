mod chat;
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
    pub app_dir: std::path::PathBuf,
}

#[tauri::command]
async fn save_api_token(token: String, state: State<'_, AppState>) -> Result<(), String> {
    // Fetch username from GitHub API using octocrab
    let octocrab = octocrab::Octocrab::builder()
        .personal_token(token.clone())
        .build()
        .map_err(|e| format!("Failed to build octocrab client: {}", e))?;
    
    let user: octocrab::models::Author = octocrab
        .get("/user", None::<&()>)
        .await
        .map_err(|e| format!("Failed to fetch GitHub user: {}", e))?;
    
    let username = user.login;
    println!("[Backend] GitHub username fetched: {}", username);
    
    // Save both token and username
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    config.system.github_token = Some(token);
    config.system.github_username = Some(username);
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn check_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let mgr = state.config_manager.lock().await;
    // Note: checking has_token requires reading the file, which is fine.
    // It returns false if locked.

    let is_online = if mgr.is_unlocked() {
        if let Ok(config) = mgr.load().await {
            config.user.is_online
        } else {
            false
        }
    } else {
        false
    };

    // Migration: if token exists but username is missing, fetch and save it
    if mgr.is_unlocked() {
        if let Ok(config) = mgr.load().await {
            if config.system.github_token.is_some() && config.system.github_username.is_none() {
                if let Some(ref token) = config.system.github_token {
                    // Fetch username from GitHub
                    if let Ok(octocrab) = octocrab::Octocrab::builder()
                        .personal_token(token.clone())
                        .build()
                    {
                        if let Ok(user) = octocrab.get::<octocrab::models::Author, _, _>("/user", None::<&()>).await {
                            println!("[Backend] Migrating: fetched GitHub username {}", user.login);
                            let mut updated_config = config.clone();
                            updated_config.system.github_username = Some(user.login);
                            let _ = mgr.save(&updated_config).await;
                        }
                    }
                }
            }
        }
    }

    Ok(AuthStatus {
        is_setup: mgr.exists(),
        is_unlocked: mgr.is_unlocked(),
        is_github_connected: mgr.has_token().await,
        is_online,
    })
}

#[tauri::command]
async fn toggle_online_status(online: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    config.user.is_online = online;
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    Ok(())
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

/// Start the P2P network - call this AFTER vault is unlocked
/// This ensures the persisted keypair can be loaded from the encrypted config
#[tauri::command]
async fn start_network(app_handle: tauri::AppHandle) -> Result<(), String> {
    use tauri::Emitter;
    println!("[Backend] start_network called (post-unlock)");

    // Check if network is already running
    if app_handle.try_state::<NetworkState>().is_some() {
        println!("[Backend] Network already initialized, skipping...");
        // Don't emit auth-status here - would cause infinite loop with frontend's refreshData
        return Ok(());
    }

    match network::init(app_handle.clone()).await {
        Ok(_) => {
            println!("[Backend] Network started successfully!");
            // Emit auth-status event to trigger frontend refresh
            let _ = app_handle.emit("auth-status", serde_json::json!({"unlocked": true}));
            Ok(())
        }
        Err(e) => {
            eprintln!("[Backend] Failed to start network: {}", e);
            Err(e.to_string())
        }
    }
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
    is_online: bool,
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
    // Read from peers table (source of truth for friends)
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    let peers = crate::storage::db::get_all_peers(&conn).map_err(|e| e.to_string())?;

    // Return peer IDs (aliases could be used for display names)
    let peer_ids: Vec<String> = peers.into_iter().map(|p| p.id).collect();
    Ok(peer_ids)
}

#[tauri::command]
async fn delete_peer(peer_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    crate::storage::db::delete_peer(&conn, &peer_id).map_err(|e| e.to_string())?;
    println!("[Backend] Deleted peer: {}", peer_id);
    Ok(())
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
async fn get_peer_aliases(state: State<'_, AppState>) -> Result<std::collections::HashMap<String, String>, String> {
    println!("[Backend] get_peer_aliases called");
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::get_peer_aliases(&conn).map_err(|e| e.to_string())
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
                    alias: None,
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
async fn get_theme(state: State<'_, AppState>) -> Result<storage::config::ThemeConfig, String> {
    println!("[Backend] get_theme called");
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.theme.clone()),
        Err(e) => {
            eprintln!("[Backend] Error loading theme: {}", e);
            Ok(storage::config::ThemeConfig::default())
        }
    }
}

#[tauri::command]
async fn update_theme(
    theme: storage::config::ThemeConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    println!("[Backend] update_theme called");
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    config.user.theme = theme;
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    println!("[Backend] Theme updated successfully");
    Ok(())
}

#[derive(serde::Serialize)]
struct PresetInfo {
    key: String,
    name: String,
    description: String,
}

#[tauri::command]
async fn list_theme_presets(state: State<'_, AppState>) -> Result<Vec<PresetInfo>, String> {
    println!("[Backend] list_theme_presets called");
    let theme_manager = storage::theme::ThemeManager::new(&state.app_dir);
    Ok(theme_manager
        .list_presets_info()
        .into_iter()
        .map(|(key, name, description)| PresetInfo { key, name, description })
        .collect())
}

#[tauri::command]
async fn apply_preset(
    name: String,
    state: State<'_, AppState>,
) -> Result<storage::theme::ThemeConfig, String> {
    println!("[Backend] apply_preset called with: {}", name);
    let theme_manager = storage::theme::ThemeManager::new(&state.app_dir);
    let theme = theme_manager.load_preset(&name).map_err(|e| e.to_string())?;
    
    // Save to user config with selected preset key
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    config.user.theme = theme.clone();
    config.user.selected_preset = Some(name.clone());
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    
    println!("[Backend] Preset {} applied successfully", name);
    Ok(theme)
}

#[tauri::command]
async fn get_selected_preset(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.selected_preset),
        Err(_) => Ok(None),
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
async fn get_chat_latest_times(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, i64>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::get_chat_latest_times(&conn).map_err(|e| e.to_string())
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
        status: "read".to_string(), // Self messages are always read
        content_metadata: None,
        sender_alias: None, // Self messages don't need alias
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
) -> Result<String, String> {
    println!("[Backend] send_message to {}: {}", peer_id, message);

    // Get my alias from config
    let my_alias = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        config.user.profile.alias.clone()
    };

    // 1. Persist to DB
    let msg_id_for_dm = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;

        // Ensure peer exists in database (auto-add if not)
        if !storage::db::is_peer(&conn, &peer_id) {
            if let Err(e) = storage::db::add_peer(&conn, &peer_id, None, None, "local") {
                eprintln!("[Backend] Failed to auto-add peer: {}", e);
            }
        }

        // Ensure chat exists for this peer (create if not)
        if !storage::db::chat_exists(&conn, &peer_id) {
            if let Err(e) = storage::db::create_chat(&conn, &peer_id, &peer_id, false) {
                eprintln!("[Backend] Failed to auto-create chat: {}", e);
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let id_suffix: u32 = rand::random();
        let msg_id = format!("{}-{}", timestamp, id_suffix);

        let msg = storage::db::Message {
            id: msg_id.clone(),
            chat_id: peer_id.clone(),  // User checks chat with this peer
            peer_id: "Me".to_string(), // Sender is Me
            timestamp,
            content_type: "text".to_string(),
            text_content: Some(message.clone()),
            file_hash: None,
            status: "pending".to_string(), // Outgoing = pending until delivered
            content_metadata: None,
            sender_alias: my_alias.clone(),
        };

        if let Err(e) = storage::db::insert_message(&conn, &msg) {
            eprintln!("[Backend] Failed to save outgoing message: {}", e);
            return Err(e.to_string());
        }

        msg_id // Return msg_id for use in network message
    };

    // 2. Send to Network Manager
    let tx = net_state.sender.lock().await;

    if peer_id == "General" {
        // Group chat: use gossipsub
        tx.send(message).await.map_err(|e| e.to_string())?;
    } else {
        // 1v1 chat: use direct message format (DM:peer_id:msg_id:timestamp:alias:content)
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let alias_str = my_alias.unwrap_or_default();
        let dm_command = format!("DM:{}:{}:{}:{}:{}", peer_id, msg_id_for_dm, timestamp, alias_str, message);
        tx.send(dm_command).await.map_err(|e| e.to_string())?;
    }


    Ok(msg_id_for_dm)
}

#[tauri::command]
async fn send_image_message(
    peer_id: String,
    file_path: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<String, String> {
    println!(
        "[Backend] send_image_message: to {} from {}",
        peer_id, file_path
    );

    // 1. Read the image file
    let file_data = std::fs::read(&file_path).map_err(|e| format!("Failed to read file: {}", e))?;

    // Determine mime type from extension
    let mime_type = match std::path::Path::new(&file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
    {
        Some(ext) if ext == "jpg" || ext == "jpeg" => "image/jpeg",
        Some(ext) if ext == "png" => "image/png",
        Some(ext) if ext == "gif" => "image/gif",
        Some(ext) if ext == "webp" => "image/webp",
        _ => "image/png", // default
    };

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

    // 2. Store with FastCDC chunking
    let file_hash = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::object::create(
            &conn,
            &file_data,
            file_name.as_deref(),
            Some(mime_type),
            None,
        )
        .map_err(|e| format!("Failed to store image: {}", e))?
    };

    // 3. Persist message to DB
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        // For self chat, use 'self' as chat_id
        let chat_id = if peer_id == "Me" { "self".to_string() } else { peer_id.clone() };
        let msg = storage::db::Message {
            id: msg_id.clone(),
            chat_id,
            peer_id: "Me".to_string(),
            timestamp,
            content_type: "image".to_string(),
            text_content: None,
            file_hash: Some(file_hash.clone()),
            status: if peer_id == "Me" { "read".to_string() } else { "pending".to_string() },
            content_metadata: None,
            sender_alias: None,
        };

        if let Err(e) = storage::db::insert_message(&conn, &msg) {
            eprintln!("[Backend] Failed to save image message: {}", e);
            return Err(e.to_string());
        }
    }

    // 4. Broadcast via network (skip for self messages)
    if peer_id != "Me" {
        let broadcast_msg = format!("__IMAGE_MSG__:{}:{}", file_hash, peer_id);
        let tx = net_state.sender.lock().await;
        tx.send(broadcast_msg).await.map_err(|e| e.to_string())?;
    }

    println!("[Backend] Image message sent: hash={}", file_hash);
    Ok(file_hash)
}

#[tauri::command]
async fn get_image_data(file_hash: String, state: State<'_, AppState>) -> Result<String, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    // Load image data from chunks
    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load image: {}", e))?;

    // Get mime type from files table
    let mime_type: String = conn
        .query_row(
            "SELECT COALESCE(mime_type, 'image/png') FROM files WHERE file_hash = ?1",
            [&file_hash],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "image/png".to_string());

    // Return as base64 data URL
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);
    let data_url = format!("data:{};base64,{}", mime_type, b64);

    Ok(data_url)
}

#[tauri::command]
async fn get_image_from_path(file_path: String) -> Result<String, String> {
    // Read image file from disk
    let data = std::fs::read(&file_path)
        .map_err(|e| format!("Failed to read image file: {}", e))?;

    // Determine mime type from extension
    let mime_type = if file_path.ends_with(".png") {
        "image/png"
    } else if file_path.ends_with(".jpg") || file_path.ends_with(".jpeg") {
        "image/jpeg"
    } else if file_path.ends_with(".gif") {
        "image/gif"
    } else if file_path.ends_with(".webp") {
        "image/webp"
    } else {
        "image/png"
    };

    // Return as base64 data URL
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);
    let data_url = format!("data:{};base64,{}", mime_type, b64);

    Ok(data_url)
}

#[tauri::command]
async fn save_image_to_file(
    file_hash: String,
    target_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    // Load image data from chunks
    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load image: {}", e))?;

    // Write to target path
    std::fs::write(&target_path, &data)
        .map_err(|e| format!("Failed to save image: {}", e))?;

    println!("[Backend] Image saved to: {}", target_path);
    Ok(())
}

#[tauri::command]
async fn send_document_message(
    peer_id: String,
    file_path: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<String, String> {
    println!("[Backend] Sending document to {}: {}", peer_id, file_path);

    // 1. Read file and get original filename
    let file_data = std::fs::read(&file_path)
        .map_err(|e| format!("Failed to read document: {}", e))?;

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "document".to_string());

    // Determine mime type from extension
    let mime_type = match file_path.rsplit('.').next() {
        Some("pdf") => "application/pdf",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("txt") => "text/plain",
        Some("xls") => "application/vnd.ms-excel",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("ppt") => "application/vnd.ms-powerpoint",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("csv") => "text/csv",
        _ => "application/octet-stream",
    };

    // 2. Store with FastCDC chunking
    let file_hash = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::object::create(
            &conn,
            &file_data,
            Some(&file_name),
            Some(mime_type),
            None,
        )
        .map_err(|e| format!("Failed to store document: {}", e))?
    };

    // 3. Persist message to DB
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        // For self chat, use 'self' as chat_id
        let chat_id = if peer_id == "Me" { "self".to_string() } else { peer_id.clone() };
        let msg = storage::db::Message {
            id: msg_id.clone(),
            chat_id,
            peer_id: "Me".to_string(),
            timestamp,
            content_type: "document".to_string(),
            text_content: Some(file_name.clone()), // Store filename in text_content
            file_hash: Some(file_hash.clone()),
            status: if peer_id == "Me" { "read".to_string() } else { "pending".to_string() },
            content_metadata: Some(format!("{{\"size_bytes\":{}}}", file_data.len())),
            sender_alias: None,
        };

        if let Err(e) = storage::db::insert_message(&conn, &msg) {
            eprintln!("[Backend] Failed to save document message: {}", e);
            return Err(e.to_string());
        }
    }

    // 4. Broadcast via network (skip for self messages)
    if peer_id != "Me" {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let filename_b64 = STANDARD.encode(&file_name);
        let broadcast_msg = format!("__DOCUMENT_MSG__:{}:{}:{}", file_hash, peer_id, filename_b64);
        let tx = net_state.sender.lock().await;
        tx.send(broadcast_msg).await.map_err(|e| e.to_string())?;
    }

    println!("[Backend] Document message sent: hash={}, name={}", file_hash, file_name);
    Ok(file_hash)
}

#[tauri::command]
async fn save_document_to_file(
    file_hash: String,
    target_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    // Load document data from chunks
    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load document: {}", e))?;

    // Write to target path
    std::fs::write(&target_path, &data)
        .map_err(|e| format!("Failed to save document: {}", e))?;

    println!("[Backend] Document saved to: {}", target_path);
    Ok(())
}

#[tauri::command]
async fn send_video_message(
    peer_id: String,
    file_path: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<String, String> {
    println!("[Backend] Sending video to {}: {}", peer_id, file_path);

    // 1. Read file and get original filename
    let file_data = std::fs::read(&file_path)
        .map_err(|e| format!("Failed to read video: {}", e))?;

    let file_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "video.mp4".to_string());

    // Determine mime type from extension
    let mime_type = match file_path.rsplit('.').next() {
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("avi") => "video/x-msvideo",
        Some("mkv") => "video/x-matroska",
        _ => "video/mp4",
    };

    // 2. Store with FastCDC chunking
    let file_hash = {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::object::create(
            &conn,
            &file_data,
            Some(&file_name),
            Some(mime_type),
            None,
        )
        .map_err(|e| format!("Failed to store video: {}", e))?
    };

    // 3. Persist message to DB
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let id_suffix: u32 = rand::random();
    let msg_id = format!("{}-{}", timestamp, id_suffix);

    {
        let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
        // For self chat, use 'self' as chat_id
        let chat_id = if peer_id == "Me" { "self".to_string() } else { peer_id.clone() };
        let msg = storage::db::Message {
            id: msg_id.clone(),
            chat_id,
            peer_id: "Me".to_string(),
            timestamp,
            content_type: "video".to_string(),
            text_content: Some(file_name.clone()), // Store filename in text_content
            file_hash: Some(file_hash.clone()),
            status: if peer_id == "Me" { "read".to_string() } else { "pending".to_string() },
            content_metadata: Some(format!("{{\"size_bytes\":{}}}", file_data.len())),
            sender_alias: None,
        };

        if let Err(e) = storage::db::insert_message(&conn, &msg) {
            eprintln!("[Backend] Failed to save video message: {}", e);
            return Err(e.to_string());
        }
    }

    // 4. Broadcast via network (skip for self messages)
    if peer_id != "Me" {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let filename_b64 = STANDARD.encode(&file_name);
        let broadcast_msg = format!("__VIDEO_MSG__:{}:{}:{}", file_hash, peer_id, filename_b64);
        let tx = net_state.sender.lock().await;
        tx.send(broadcast_msg).await.map_err(|e| e.to_string())?;
    }

    println!("[Backend] Video message sent: hash={}, name={}", file_hash, file_name);
    Ok(file_hash)
}

#[tauri::command]
async fn get_video_data(file_hash: String, state: State<'_, AppState>) -> Result<String, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;

    // Load video data from chunks
    let data = storage::object::load(&conn, &file_hash, None)
        .map_err(|e| format!("Failed to load video: {}", e))?;

    // Get mime type from files table
    let mime_type: String = conn
        .query_row(
            "SELECT COALESCE(mime_type, 'video/mp4') FROM files WHERE file_hash = ?1",
            [&file_hash],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "video/mp4".to_string());

    // Return as base64 data URL
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);
    let data_url = format!("data:{};base64,{}", mime_type, b64);

    Ok(data_url)
}

// ============================================================================
// Invitation Commands
// ============================================================================

/// Generate a 14-character password for invitations
#[tauri::command]
async fn generate_invite_password() -> Result<String, String> {
    Ok(rvault_core::crypto::generate_password(14, false))
}

/// Create an invitation for a friend
#[tauri::command]
async fn create_invite(
    invitee: String,
    password: String,
    app_state: State<'_, AppState>,
) -> Result<(), String> {
    use crate::network::invite;
    use crate::network::gist;
    
    // 1. Get my username from config
    let my_username = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        config.system.github_username.clone().ok_or("GitHub username not set")?
    };
    
    // 2. Get my multiaddress or IP (placeholder - use display name for now)
    let my_address = my_username.clone();
    
    // 3. Generate the encrypted invite
    let encrypted_invite = invite::generate_invite(
        &password,
        &my_username,
        &invitee,
        &my_address,
        120, // 2-minute TTL
    ).map_err(|e| format!("Failed to generate invite: {}", e))?;
    
    // 4. Track it for Gist upload
    let tracked = gist::track_invite(encrypted_invite);
    
    // 5. Store in config for next publish_peer_info call
    {
        let mgr = app_state.config_manager.lock().await;
        let mut config = mgr.load().await.map_err(|e| e.to_string())?;
        
        // Add to pending invitations list
        if config.user.pending_invitations.is_none() {
            config.user.pending_invitations = Some(Vec::new());
        }
        
        if let Some(ref mut invites) = config.user.pending_invitations {
            let invite_json = serde_json::to_string(&tracked)
                .map_err(|e| format!("Failed to serialize invite: {}", e))?;
            invites.push(invite_json);
        }
        
        mgr.save(&config).await.map_err(|e| e.to_string())?;
    }
    
    println!("[Backend] Created invite for {} with password len {}", invitee, password.len());
    Ok(())
}

/// Redeem an invitation from a friend
#[tauri::command]
async fn redeem_invite(
    inviter: String,
    password: String,
    app_state: State<'_, AppState>,
) -> Result<String, String> {
    use crate::network::invite;
    use crate::network::gist;
    
    // 1. Get my username from config
    let my_username = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        config.system.github_username.clone().ok_or("GitHub username not set")?
    };
    
    // 2. Fetch inviter's invitations from their Gist
    let encrypted_invites = gist::get_friend_invitations(&inviter)
        .await
        .map_err(|e| format!("Failed to fetch invitations: {}", e))?;
    
    if encrypted_invites.is_empty() {
        return Err("No invitations found from this user".to_string());
    }
    
    // 3. Try to find and decrypt our invitation
    let result = invite::process_invites(
        &encrypted_invites,
        &password,
        &inviter,
        &my_username,
    ).map_err(|e| format!("Failed to process invites: {}", e))?;
    
    match result {
        Some((payload, _index)) => {
            println!("[Backend] Successfully redeemed invite from {}", inviter);
            Ok(payload.ip_address.clone())
        }
        None => {
            Err("No valid invitation found for you. Check password and usernames.".to_string())
        }
    }
}

/// Complete invitation redemption with friend persistence and auto-message
#[tauri::command]
async fn redeem_and_connect(
    inviter: String,
    password: String,
    app_state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<String, String> {
    use crate::network::invite;
    use crate::network::gist;
    use crate::storage::config::FriendConfig;
    
    // 1. Get my username
    let my_username = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await.map_err(|e| e.to_string())?;
        config.system.github_username.clone().ok_or("GitHub username not set")?
    };
    
    // 2. Fetch and decrypt invite
    let encrypted_invites = gist::get_friend_invitations(&inviter)
        .await
        .map_err(|e| format!("Failed to fetch invitations: {}", e))?;
    
    if encrypted_invites.is_empty() {
        return Err("No invitations found from this user".to_string());
    }
    
    let result = invite::process_invites(
        &encrypted_invites,
        &password,
        &inviter,
        &my_username,
    ).map_err(|e| format!("Failed to process invites: {}", e))?;
    
    match result {
        Some((payload, _index)) => {
            let peer_id = inviter.clone();
            
            // 3. Add as friend to config.json
            {
                let mgr = app_state.config_manager.lock().await;
                let mut config = mgr.load().await.map_err(|e| e.to_string())?;
                
                if !config.user.friends.iter().any(|f| f.username == peer_id) {
                    config.user.friends.push(FriendConfig {
                        username: peer_id.clone(),
                        alias: None,
                        x25519_pubkey: None,
                        ed25519_pubkey: None,
                        leaf_index: 0,
                        encrypted_leaf_key: None,
                        nonce: None,
                    });
                    mgr.save(&config).await.map_err(|e| e.to_string())?;
                }
            }
            
            // 4. Add to SQLite database
            {
                let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
                
                // Add peer if not exists
                if !storage::db::is_peer(&conn, &peer_id) {
                    storage::db::add_peer(&conn, &peer_id, None, None, "gist")
                        .map_err(|e| e.to_string())?;
                }
                
                // Create chat if not exists
                if !storage::db::chat_exists(&conn, &peer_id) {
                    storage::db::create_chat(&conn, &peer_id, &peer_id, false)
                        .map_err(|e| e.to_string())?;
                }
            }
            
            // 5. Send automatic "Hi!" message
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            
            // Save message to database first
            let msg_id = {
                let conn = app_state.db_conn.lock().map_err(|e| e.to_string())?;
                let id_suffix: u32 = rand::random();
                let msg_id = format!("{}-{}", timestamp, id_suffix);
                
                let msg = storage::db::Message {
                    id: msg_id.clone(),
                    chat_id: peer_id.clone(),
                    peer_id: "Me".to_string(),
                    timestamp,
                    content_type: "text".to_string(),
                    text_content: Some("Hi!".to_string()),
                    file_hash: None,
                    status: "pending".to_string(),
                    content_metadata: None,
                    sender_alias: None, // Will send alias in DM
                };
                
                storage::db::insert_message(&conn, &msg).map_err(|e| e.to_string())?;
                msg_id
            };
            
            // Send via network
            let tx = net_state.sender.lock().await;
            let dm_command = format!("DM:{}:{}:{}:Hi!", peer_id, msg_id, timestamp);
            tx.send(dm_command).await.map_err(|e| e.to_string())?;
            
            println!("[Backend] Successfully connected with {} and sent Hi! (ip: {})", inviter, payload.ip_address.clone());
            Ok(peer_id) // Return peer_id for frontend navigation
        }
        None => {
            Err("No valid invitation found for you. Check password and usernames.".to_string())
        }
    }
}

#[tauri::command]
async fn get_chat_history(
    chat_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<storage::db::Message>, String> {
    println!("[Backend] get_chat_history for: {}", chat_id);
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    let mut messages = storage::db::get_messages(&conn, &chat_id).map_err(|e| e.to_string())?;
    
    // Hydrate photo messages that don't have cached metadata
    for db_msg in &mut messages {
        // Only hydrate photos/images that are missing metadata
        if (db_msg.content_type == "photo" || db_msg.content_type == "image") 
           && db_msg.content_metadata.is_none() 
           && db_msg.file_hash.is_some() 
        {
            // Convert to rich type, hydrate, then update db_msg
            let mut rich_msg = chat::message::Message::from_db_row(db_msg);
            if rich_msg.hydrate(&conn) {
                // Update the db_msg with cached metadata for return
                let updated = rich_msg.to_db_row();
                db_msg.content_metadata = updated.content_metadata;
            }
        }
    }
    
    println!("[Backend] Found {} messages", messages.len());
    Ok(messages)
}

#[tauri::command]
async fn mark_messages_read(
    chat_id: String,
    state: State<'_, AppState>,
    net_state: State<'_, NetworkState>,
) -> Result<Vec<String>, String> {
    println!("[Backend] mark_messages_read for chat: {}", chat_id);

    // For 1v1 chats, chat_id = peer_id, so messages FROM that peer are the ones to mark as read
    let sender_id = chat_id.clone();

    let marked_ids = {
        let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
        storage::db::mark_messages_read(&conn, &chat_id, &sender_id).map_err(|e| e.to_string())?
    };

    println!("[Backend] Marked {} messages as read", marked_ids.len());

    // Send read receipts to the peer via network so their UI updates
    if !marked_ids.is_empty() && chat_id != "General" && chat_id != "self" {
        // Send a read receipt for each message (or batch them)
        // Format: READ_RECEIPT:peer_id:msg_id1,msg_id2,...
        let msg_ids_str = marked_ids.join(",");
        let read_receipt_cmd = format!("READ_RECEIPT:{}:{}", chat_id, msg_ids_str);

        let tx = net_state.sender.lock().await;
        if let Err(e) = tx.send(read_receipt_cmd).await {
            eprintln!("[Backend] Failed to send read receipt: {}", e);
        } else {
            println!(
                "[Backend] Read receipt sent for {} messages",
                marked_ids.len()
            );
        }
    }

    Ok(marked_ids)
}

#[tauri::command]
async fn get_unread_counts(
    my_peer_id: String,
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, i64>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    let counts = storage::db::get_unread_counts(&conn, &my_peer_id).map_err(|e| e.to_string())?;
    Ok(counts)
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

// --- Connection Request Command ---

/// Request connection to a local peer (triggers mutual handshake)
#[tauri::command]
async fn request_connection(peer_id: String, state: State<'_, NetworkState>) -> Result<(), String> {
    println!("[Backend] request_connection called for: {}", peer_id);

    // Send command to NetworkManager
    let sender = state.sender.lock().await;
    sender
        .send(format!("REQUEST_CONNECTION:{}", peer_id))
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    Ok(())
}

/// Enable/disable fast mDNS discovery mode
#[tauri::command]
fn set_fast_discovery(enabled: bool) {
    if enabled {
        network::mdns::enable_fast_discovery();
    } else {
        network::mdns::disable_fast_discovery();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            let mut config_manager = ConfigManager::new(app_dir.clone());

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

            // Note: "Me" entry is automatically seeded in peers table by run_migrations()

            app.manage(AppState {
                config_manager: tokio::sync::Mutex::new(config_manager),
                db_conn: std::sync::Mutex::new(db_connection),
                app_dir: app_dir.clone(),
            });

            // --- 2. Network is now initialized after vault unlock ---
            // The frontend calls start_network after successful unlock_vault
            // This ensures we can load the persisted keypair from the encrypted config
            println!("[Backend] Setup hook returning Ok");
            Ok(())
        })
        // --- End Setup Hook ---
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            save_api_token,
            check_auth_status,
            toggle_online_status,
            init_vault,
            unlock_vault,
            start_network,
            start_github_auth,
            poll_github_auth,
            reset_vault,
            get_friends,
            get_peer_aliases,
            get_trusted_peers,
            add_friend,
            delete_peer,
            remove_friend,
            get_user_profile,
            get_theme,
            update_theme,
            list_theme_presets,
            apply_preset,
            get_selected_preset,
            update_user_profile,
            get_pinned_peers,
            toggle_pin_peer,
            send_message_to_self,
            send_message,
            get_chat_history,
            create_envelope,
            update_envelope,
            delete_envelope,
            get_envelopes,
            move_chat_to_envelope,
            get_envelope_assignments,
            request_connection,
            set_fast_discovery,
            get_chat_latest_times,
            send_image_message,
            get_image_data,
            get_image_from_path,
            save_image_to_file,
            mark_messages_read,
            get_unread_counts,
            send_document_message,
            save_document_to_file,
            send_video_message,
            get_video_data,
            // Invitation commands
            generate_invite_password,
            create_invite,
            redeem_invite,
            redeem_and_connect,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
