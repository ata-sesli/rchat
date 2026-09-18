use crate::storage::{config::ConfigManager, db};
use crate::AppState;
use anyhow::{anyhow, Context, Result};
use directories::BaseDirs;
use std::path::PathBuf;
use std::sync::Arc;

pub const APP_IDENTIFIER: &str = "com.atasesli.rchat";

pub fn default_app_data_dir() -> Result<PathBuf> {
    let base_dirs = BaseDirs::new().ok_or_else(|| anyhow!("failed to resolve user data dir"))?;
    Ok(base_dirs.data_dir().join(APP_IDENTIFIER))
}

pub fn create_app_state(app_dir: PathBuf) -> Result<AppState> {
    std::fs::create_dir_all(&app_dir).context("failed to create app data dir")?;
    let config_manager = ConfigManager::new(app_dir.clone());
    let db_conn = db::connect_to_db().context("failed to initialize database")?;

    Ok(AppState {
        config_manager: Arc::new(tokio::sync::Mutex::new(config_manager)),
        db_conn: Arc::new(std::sync::Mutex::new(db_conn)),
        local_peer_id: Arc::new(std::sync::RwLock::new(None)),
        app_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_app_data_dir_uses_rchat_identifier() {
        let path = default_app_data_dir().expect("app data dir resolves");
        assert!(path.ends_with(APP_IDENTIFIER));
    }

    #[test]
    fn create_app_state_uses_requested_app_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_dir = temp.path().join("rchat-data");

        let state = create_app_state(app_dir.clone()).expect("state starts");

        assert_eq!(state.app_dir, app_dir);
        assert!(state.app_dir.exists());
    }
}
