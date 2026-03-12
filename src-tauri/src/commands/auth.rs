use tauri::{Emitter, Manager, State};

use crate::storage::config::{ConnectivityMode, ConnectivitySettings};
use crate::{network, oauth, AppState, NetworkState};

#[derive(serde::Serialize)]
pub struct AuthStatus {
    is_setup: bool,
    is_unlocked: bool,
    is_github_connected: bool,
    is_online: bool,
    connectivity: ConnectivitySettings,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectivitySettingsPatch {
    pub mdns_enabled: Option<bool>,
    pub github_sync_enabled: Option<bool>,
    pub nat_keepalive_enabled: Option<bool>,
    pub punch_assist_enabled: Option<bool>,
}

fn normalize_connectivity(settings: ConnectivitySettings) -> ConnectivitySettings {
    settings.with_derived_mode()
}

async fn sync_runtime_connectivity(
    app_handle: &tauri::AppHandle,
    settings: &ConnectivitySettings,
) {
    if let Some(network_state) = app_handle.try_state::<NetworkState>() {
        let mut runtime = network_state.connectivity.lock().await;
        *runtime = settings.clone();
    }
}

#[tauri::command]
pub async fn save_api_token(token: String, state: State<'_, AppState>) -> Result<(), String> {
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
pub async fn check_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let mgr = state.config_manager.lock().await;

    let connectivity = if mgr.is_unlocked() {
        if let Ok(config) = mgr.load().await {
            normalize_connectivity(config.user.connectivity)
        } else {
            ConnectivitySettings::default()
        }
    } else {
        ConnectivitySettings::default()
    };
    let is_online = connectivity.github_sync_enabled;

    // Migration: if token exists but username is missing, fetch and save it
    if mgr.is_unlocked() {
        if let Ok(config) = mgr.load().await {
            if config.system.github_token.is_some() && config.system.github_username.is_none() {
                if let Some(ref token) = config.system.github_token {
                    if let Ok(octocrab) = octocrab::Octocrab::builder()
                        .personal_token(token.clone())
                        .build()
                    {
                        if let Ok(user) = octocrab
                            .get::<octocrab::models::Author, _, _>("/user", None::<&()>)
                            .await
                        {
                            println!(
                                "[Backend] Migrating: fetched GitHub username {}",
                                user.login
                            );
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
        connectivity,
    })
}

#[tauri::command]
pub async fn toggle_online_status(
    online: bool,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Compatibility wrapper for legacy clients.
    let mapped = if online {
        ConnectivitySettings::from_mode(ConnectivityMode::Reachable)
    } else {
        ConnectivitySettings::from_mode(ConnectivityMode::Invisible)
    };

    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    config.user.connectivity = mapped.clone();
    config.user.is_online = mapped.github_sync_enabled;
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    drop(mgr);

    sync_runtime_connectivity(&app_handle, &mapped).await;
    Ok(())
}

#[tauri::command]
pub async fn get_connectivity_settings(
    state: State<'_, AppState>,
) -> Result<ConnectivitySettings, String> {
    let mgr = state.config_manager.lock().await;
    let config = mgr.load().await.map_err(|e| e.to_string())?;
    Ok(normalize_connectivity(config.user.connectivity))
}

#[tauri::command]
pub async fn set_connectivity_mode(
    mode: ConnectivityMode,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<ConnectivitySettings, String> {
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;

    let next = match mode {
        ConnectivityMode::Invisible => ConnectivitySettings::from_mode(ConnectivityMode::Invisible),
        ConnectivityMode::Lan => ConnectivitySettings::from_mode(ConnectivityMode::Lan),
        ConnectivityMode::Reachable => ConnectivitySettings::from_mode(ConnectivityMode::Reachable),
        ConnectivityMode::Custom => normalize_connectivity(config.user.connectivity.clone()),
    };

    config.user.connectivity = next.clone();
    config.user.is_online = next.github_sync_enabled;
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    drop(mgr);

    sync_runtime_connectivity(&app_handle, &next).await;
    Ok(next)
}

#[tauri::command]
pub async fn update_connectivity_settings(
    patch: ConnectivitySettingsPatch,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<ConnectivitySettings, String> {
    let mgr = state.config_manager.lock().await;
    let mut config = mgr.load().await.map_err(|e| e.to_string())?;
    let mut next = config.user.connectivity.clone();

    if let Some(v) = patch.mdns_enabled {
        next.mdns_enabled = v;
    }
    if let Some(v) = patch.github_sync_enabled {
        next.github_sync_enabled = v;
    }
    if let Some(v) = patch.nat_keepalive_enabled {
        next.nat_keepalive_enabled = v;
    }
    if let Some(v) = patch.punch_assist_enabled {
        next.punch_assist_enabled = v;
    }
    next = normalize_connectivity(next);

    config.user.connectivity = next.clone();
    config.user.is_online = next.github_sync_enabled;
    mgr.save(&config).await.map_err(|e| e.to_string())?;
    drop(mgr);

    sync_runtime_connectivity(&app_handle, &next).await;
    Ok(next)
}

#[tauri::command]
pub async fn init_vault(password: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.config_manager.lock().await;
    mgr.init(password.trim()).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn unlock_vault(password: String, state: State<'_, AppState>) -> Result<(), String> {
    println!(
        "[Backend] unlock_vault called. Password len: {}",
        password.len()
    );
    let mut mgr = state.config_manager.lock().await;
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
pub async fn start_network(app_handle: tauri::AppHandle) -> Result<(), String> {
    println!("[Backend] start_network called (post-unlock)");

    // Check if network is already running
    if app_handle.try_state::<NetworkState>().is_some() {
        println!("[Backend] Network already initialized, skipping...");
        return Ok(());
    }

    match network::init(app_handle.clone()).await {
        Ok(_) => {
            println!("[Backend] Network started successfully!");
            let _ = app_handle.emit("auth-status", serde_json::json!({"unlocked": true}));
            Ok(())
        }
        Err(e) => {
            eprintln!("[Backend] Failed to start network: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn start_github_auth() -> Result<oauth::AuthState, String> {
    oauth::start_device_flow().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn poll_github_auth(device_code: String) -> Result<String, String> {
    oauth::poll_for_token(&device_code)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reset_vault(state: State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.config_manager.lock().await;
    mgr.reset().await.map_err(|e| e.to_string())?;
    Ok(())
}
