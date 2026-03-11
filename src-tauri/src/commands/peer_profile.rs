use rand::RngCore;
use tauri::State;

use crate::storage;
use crate::storage::config::{CustomThemeEntry, FriendConfig, ThemeConfig, UserProfile};
use crate::AppState;

#[derive(serde::Serialize, Clone)]
pub struct PresetInfo {
    pub key: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub theme: Option<ThemeConfig>,
}

fn now_unix_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn trim_optional_description(description: Option<String>) -> Option<String> {
    description.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn validate_theme_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Theme title is required".to_string());
    }
    Ok(trimmed.to_string())
}

fn generate_custom_theme_key() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);

    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    );

    format!("custom:{}", uuid)
}

fn custom_entry_to_preset(entry: &CustomThemeEntry) -> PresetInfo {
    PresetInfo {
        key: entry.key.clone(),
        name: entry.name.clone(),
        description: entry.description.clone().unwrap_or_default(),
        source: "custom".to_string(),
        created_at: Some(entry.created_at),
        updated_at: Some(entry.updated_at),
        theme: Some(entry.theme.clone()),
    }
}

#[tauri::command]
pub async fn get_trusted_peers(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    let peers = crate::storage::db::get_all_peers(&conn).map_err(|e| e.to_string())?;

    let peer_ids: Vec<String> = peers.into_iter().map(|p| p.id).collect();
    Ok(peer_ids)
}

#[tauri::command]
pub async fn delete_peer(peer_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    crate::storage::db::delete_peer(&conn, &peer_id).map_err(|e| e.to_string())?;
    println!("[Backend] Deleted peer: {}", peer_id);
    Ok(())
}

#[tauri::command]
pub async fn get_friends(state: State<'_, AppState>) -> Result<Vec<FriendConfig>, String> {
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
pub async fn get_peer_aliases(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    println!("[Backend] get_peer_aliases called");
    let conn = state.db_conn.lock().map_err(|e| e.to_string())?;
    storage::db::get_peer_aliases(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_friend(
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
                    leaf_index: 0,
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

#[tauri::command]
pub async fn remove_friend(username: String, state: State<'_, AppState>) -> Result<(), String> {
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
pub async fn get_user_profile(state: State<'_, AppState>) -> Result<UserProfile, String> {
    println!("[Backend] get_user_profile called");
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => {
            println!("[Backend] Returning profile: {:?}", config.user.profile);
            Ok(config.user.profile.clone())
        }
        Err(e) => {
            eprintln!("[Backend] Error loading config: {}", e);
            Ok(UserProfile::default())
        }
    }
}

#[tauri::command]
pub async fn update_user_profile(
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
pub async fn get_pinned_peers(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.pinned_peers.clone()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn toggle_pin_peer(username: String, state: State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(mut config) => {
            let mut is_pinned = false;
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
pub async fn get_theme(state: State<'_, AppState>) -> Result<ThemeConfig, String> {
    println!("[Backend] get_theme called");
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.theme.clone()),
        Err(e) => {
            eprintln!("[Backend] Error loading theme: {}", e);
            Ok(ThemeConfig::default())
        }
    }
}

#[tauri::command]
pub async fn update_theme(
    theme: ThemeConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    println!("[Backend] update_theme called");
    let normalized_theme = storage::theme::validate_and_normalize_theme(&theme)
        .map_err(|e| e.to_string())?;

    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    config.user.theme = normalized_theme;
    config.user.selected_preset = None;
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    println!("[Backend] Theme updated successfully");
    Ok(())
}

#[tauri::command]
pub async fn generate_simple_theme(
    primary: String,
    secondary: String,
    text: String,
) -> Result<ThemeConfig, String> {
    storage::theme::generate_simple_theme(&primary, &secondary, &text).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_theme_presets(state: State<'_, AppState>) -> Result<Vec<PresetInfo>, String> {
    println!("[Backend] list_theme_presets called");

    let mgr = state.config_manager.lock().await;
    let config = mgr.load().await.map_err(|e| e.to_string())?;

    let theme_manager = storage::theme::ThemeManager::new(&state.app_dir);

    let mut presets: Vec<PresetInfo> = theme_manager
        .list_presets_info()
        .into_iter()
        .map(|(key, name, description)| PresetInfo {
            key,
            name,
            description,
            source: "builtin".to_string(),
            created_at: None,
            updated_at: None,
            theme: None,
        })
        .collect();

    let mut custom_presets: Vec<PresetInfo> = config
        .user
        .custom_themes
        .iter()
        .map(custom_entry_to_preset)
        .collect();

    custom_presets.sort_by(|a, b| b.updated_at.unwrap_or(0).cmp(&a.updated_at.unwrap_or(0)));
    presets.extend(custom_presets);

    Ok(presets)
}

#[tauri::command]
pub async fn apply_preset(name: String, state: State<'_, AppState>) -> Result<ThemeConfig, String> {
    println!("[Backend] apply_preset called with: {}", name);

    let theme_manager = storage::theme::ThemeManager::new(&state.app_dir);
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;

    let theme = if name.starts_with("custom:") {
        config
            .user
            .custom_themes
            .iter()
            .find(|entry| entry.key == name)
            .map(|entry| entry.theme.clone())
            .ok_or_else(|| format!("Custom theme '{}' not found", name))?
    } else {
        theme_manager.load_preset(&name).map_err(|e| e.to_string())?
    };

    config.user.theme = theme.clone();
    config.user.selected_preset = Some(name.clone());
    mgr.save(&config).await.map_err(|e| e.to_string())?;

    println!("[Backend] Preset {} applied successfully", name);
    Ok(theme)
}

#[tauri::command]
pub async fn create_custom_theme(
    name: String,
    description: Option<String>,
    theme: ThemeConfig,
    state: State<'_, AppState>,
) -> Result<PresetInfo, String> {
    let normalized_name = validate_theme_name(&name)?;
    let normalized_description = trim_optional_description(description);
    let normalized_theme = storage::theme::validate_and_normalize_theme(&theme)
        .map_err(|e| e.to_string())?;

    let now = now_unix_ts();
    let entry = CustomThemeEntry {
        key: generate_custom_theme_key(),
        name: normalized_name,
        description: normalized_description,
        theme: normalized_theme.clone(),
        created_at: now,
        updated_at: now,
    };

    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;

    config.user.custom_themes.push(entry.clone());
    config.user.theme = normalized_theme;
    config.user.selected_preset = Some(entry.key.clone());

    mgr.save(&config).await.map_err(|e| e.to_string())?;

    Ok(custom_entry_to_preset(&entry))
}

#[tauri::command]
pub async fn update_custom_theme(
    key: String,
    name: String,
    description: Option<String>,
    theme: ThemeConfig,
    state: State<'_, AppState>,
) -> Result<PresetInfo, String> {
    if !key.starts_with("custom:") {
        return Err("Only custom themes can be updated".to_string());
    }

    let normalized_name = validate_theme_name(&name)?;
    let normalized_description = trim_optional_description(description);
    let normalized_theme = storage::theme::validate_and_normalize_theme(&theme)
        .map_err(|e| e.to_string())?;

    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;

    let Some(index) = config.user.custom_themes.iter().position(|entry| entry.key == key) else {
        return Err("Custom theme not found".to_string());
    };

    let updated_at = now_unix_ts();
    let mut entry = config.user.custom_themes[index].clone();
    entry.name = normalized_name;
    entry.description = normalized_description;
    entry.theme = normalized_theme.clone();
    entry.updated_at = updated_at;

    config.user.custom_themes[index] = entry.clone();
    config.user.theme = normalized_theme;
    config.user.selected_preset = Some(entry.key.clone());

    mgr.save(&config).await.map_err(|e| e.to_string())?;

    Ok(custom_entry_to_preset(&entry))
}

#[tauri::command]
pub async fn delete_custom_theme(key: String, state: State<'_, AppState>) -> Result<(), String> {
    if !key.starts_with("custom:") {
        return Err("Only custom themes can be deleted".to_string());
    }

    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;

    let before = config.user.custom_themes.len();
    config.user.custom_themes.retain(|entry| entry.key != key);

    if config.user.custom_themes.len() == before {
        return Err("Custom theme not found".to_string());
    }

    if config.user.selected_preset.as_deref() == Some(&key) {
        config.user.selected_preset = None;
    }

    mgr.save(&config).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn get_selected_preset(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let mgr = state.config_manager.lock().await;
    match mgr.load().await {
        Ok(config) => Ok(config.user.selected_preset),
        Err(_) => Ok(None),
    }
}
