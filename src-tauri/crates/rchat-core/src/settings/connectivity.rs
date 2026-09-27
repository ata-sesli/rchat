use crate::{
    storage::config::{ConnectivityMode, ConnectivitySettings},
    AppState, NetworkState,
};
use anyhow::Result;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConnectivitySettingsPatch {
    pub mdns_enabled: Option<bool>,
    pub github_sync_enabled: Option<bool>,
    pub nat_keepalive_enabled: Option<bool>,
    pub punch_assist_enabled: Option<bool>,
}

pub fn normalize_connectivity(settings: ConnectivitySettings) -> ConnectivitySettings {
    settings.with_derived_mode()
}

pub async fn sync_runtime_connectivity(
    network_state: Option<&NetworkState>,
    settings: &ConnectivitySettings,
) {
    if let Some(network_state) = network_state {
        let mut runtime = network_state.connectivity.lock().await;
        *runtime = settings.clone();
    }
}

pub async fn get_connectivity_settings(app_state: &AppState) -> Result<ConnectivitySettings> {
    let mgr = app_state.config_manager.lock().await;
    let config = mgr.load().await?;
    Ok(normalize_connectivity(config.user.connectivity))
}

pub async fn set_connectivity_mode(
    app_state: &AppState,
    network_state: Option<&NetworkState>,
    mode: ConnectivityMode,
) -> Result<ConnectivitySettings> {
    let mgr = app_state.config_manager.lock().await;
    let config = mgr.load().await?;
    let next = match mode {
        ConnectivityMode::Invisible => ConnectivitySettings::from_mode(ConnectivityMode::Invisible),
        ConnectivityMode::Lan => ConnectivitySettings::from_mode(ConnectivityMode::Lan),
        ConnectivityMode::Reachable => ConnectivitySettings::from_mode(ConnectivityMode::Reachable),
        ConnectivityMode::Custom => normalize_connectivity(config.user.connectivity),
    };
    drop(mgr);

    save_connectivity(app_state, &next).await?;
    sync_runtime_connectivity(network_state, &next).await;
    Ok(next)
}

pub async fn update_connectivity_settings(
    app_state: &AppState,
    network_state: Option<&NetworkState>,
    patch: ConnectivitySettingsPatch,
) -> Result<ConnectivitySettings> {
    let mut next = get_connectivity_settings(app_state).await?;
    if let Some(value) = patch.mdns_enabled {
        next.mdns_enabled = value;
    }
    if let Some(value) = patch.github_sync_enabled {
        next.github_sync_enabled = value;
    }
    if let Some(value) = patch.nat_keepalive_enabled {
        next.nat_keepalive_enabled = value;
    }
    if let Some(value) = patch.punch_assist_enabled {
        next.punch_assist_enabled = value;
    }
    next = normalize_connectivity(next);

    save_connectivity(app_state, &next).await?;
    sync_runtime_connectivity(network_state, &next).await;
    Ok(next)
}

async fn save_connectivity(app_state: &AppState, settings: &ConnectivitySettings) -> Result<()> {
    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    config.user.connectivity = settings.clone();
    config.user.is_online = settings.github_sync_enabled;
    mgr.save(&config).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app_state::{
            BroadcastState, ChatConnectionRuntime, TemporaryRuntimeState, VoiceCallState,
        },
        network::command::NetworkCommand,
        settings::profile::test_app_state,
    };
    use std::{
        collections::{HashMap, HashSet},
        sync::Arc,
    };
    use tokio::sync::{mpsc, Mutex};

    fn test_network_state() -> NetworkState {
        let (tx, _rx) = mpsc::channel::<NetworkCommand>(4);
        NetworkState {
            sender: Arc::new(Mutex::new(tx)),
            local_peer_id: Arc::new(Mutex::new(None)),
            listening_addresses: Arc::new(Mutex::new(Vec::new())),
            public_endpoint: Default::default(),
            temporary_state: Arc::new(Mutex::new(TemporaryRuntimeState::default())),
            connected_chat_ids: Arc::new(Mutex::new(HashSet::new())),
            chat_connections: Arc::new(Mutex::new(HashMap::<String, ChatConnectionRuntime>::new())),
            voice_call_state: Arc::new(Mutex::new(VoiceCallState::default())),
            broadcast_state: Arc::new(Mutex::new(BroadcastState::default())),
            connectivity: Arc::new(Mutex::new(ConnectivitySettings::default())),
        }
    }

    #[tokio::test]
    async fn mode_set_updates_config_and_runtime_state() {
        let (_temp, app_state) = test_app_state().await;
        let network_state = test_network_state();

        let updated = set_connectivity_mode(
            &app_state,
            Some(&network_state),
            ConnectivityMode::Invisible,
        )
        .await
        .expect("set mode");

        assert_eq!(updated.mode, ConnectivityMode::Invisible);
        assert_eq!(
            *network_state.connectivity.lock().await,
            ConnectivitySettings::from_mode(ConnectivityMode::Invisible)
        );
        assert_eq!(
            get_connectivity_settings(&app_state).await.expect("settings"),
            ConnectivitySettings::from_mode(ConnectivityMode::Invisible)
        );
    }

    #[tokio::test]
    async fn patch_updates_config_and_runtime_state() {
        let (_temp, app_state) = test_app_state().await;
        let network_state = test_network_state();

        let updated = update_connectivity_settings(
            &app_state,
            Some(&network_state),
            ConnectivitySettingsPatch {
                mdns_enabled: Some(true),
                github_sync_enabled: Some(false),
                nat_keepalive_enabled: Some(false),
                punch_assist_enabled: Some(false),
            },
        )
        .await
        .expect("patch");

        assert_eq!(updated.mode, ConnectivityMode::Lan);
        assert_eq!(*network_state.connectivity.lock().await, updated);
    }
}
