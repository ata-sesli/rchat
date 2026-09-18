use crate::{
    storage::config::UserProfile,
    AppState,
};
use anyhow::Result;

pub async fn get_user_profile(app_state: &AppState) -> Result<UserProfile> {
    let mgr = app_state.config_manager.lock().await;
    Ok(mgr
        .load()
        .await
        .map(|config| config.user.profile)
        .unwrap_or_default())
}

pub async fn update_user_profile(
    app_state: &AppState,
    alias: Option<String>,
    avatar_path: Option<String>,
) -> Result<()> {
    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    if let Some(alias) = alias {
        config.user.profile.alias = normalize_optional(alias);
    }
    if let Some(avatar_path) = avatar_path {
        config.user.profile.avatar_path = normalize_optional(avatar_path);
    }
    mgr.save(&config).await?;
    Ok(())
}

fn normalize_optional(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
pub(crate) async fn test_app_state() -> (tempfile::TempDir, AppState) {
    use crate::storage::config::ConfigManager;
    use std::{path::PathBuf, sync::Arc};
    use tokio::sync::Mutex;

    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("rchat-data");
    std::fs::create_dir_all(&app_dir).expect("app dir");
    let mut manager = ConfigManager::new(app_dir.clone());
    manager.init("password").await.expect("init config");
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::storage::db::create_tables(&conn).expect("schema");

    (
        temp,
        AppState {
            config_manager: Arc::new(Mutex::new(manager)),
            db_conn: Arc::new(std::sync::Mutex::new(conn)),
            local_peer_id: Arc::new(std::sync::RwLock::new(None)),
            app_dir: PathBuf::from(app_dir),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn profile_get_update_round_trips() {
        let (_temp, app_state) = test_app_state().await;

        let initial = get_user_profile(&app_state).await.expect("initial profile");
        assert!(initial.alias.is_none());
        assert!(initial.avatar_path.is_none());

        update_user_profile(
            &app_state,
            Some("  Ata  ".to_string()),
            Some(" /tmp/avatar.png ".to_string()),
        )
        .await
        .expect("update profile");

        let updated = get_user_profile(&app_state).await.expect("updated profile");
        assert_eq!(updated.alias.as_deref(), Some("Ata"));
        assert_eq!(updated.avatar_path.as_deref(), Some("/tmp/avatar.png"));
    }
}
