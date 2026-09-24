use crate::{
    storage::{
        config::{CustomThemeEntry, ThemeConfig},
        theme as theme_storage,
    },
    AppState,
};
use anyhow::{anyhow, Result};
use rand::RngCore;

#[derive(serde::Serialize, Clone, Debug)]
pub struct PresetInfo {
    pub key: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub theme: Option<ThemeConfig>,
}

pub async fn get_theme(app_state: &AppState) -> Result<ThemeConfig> {
    let mgr = app_state.config_manager.lock().await;
    Ok(mgr.load().await?.user.theme)
}

pub fn generate_simple_theme(primary: &str, secondary: &str, text: &str) -> Result<ThemeConfig> {
    theme_storage::generate_simple_theme(primary, secondary, text)
}

pub async fn list_theme_presets(app_state: &AppState) -> Result<Vec<PresetInfo>> {
    let mgr = app_state.config_manager.lock().await;
    let config = mgr.load().await?;
    let theme_manager = theme_storage::ThemeManager::new(&app_state.app_dir);

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

pub async fn apply_preset(app_state: &AppState, name: &str) -> Result<ThemeConfig> {
    let theme_manager = theme_storage::ThemeManager::new(&app_state.app_dir);
    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;

    let theme = if name.starts_with("custom:") {
        config
            .user
            .custom_themes
            .iter()
            .find(|entry| entry.key == name)
            .map(|entry| entry.theme.clone())
            .ok_or_else(|| anyhow!("Custom theme '{}' not found", name))?
    } else {
        theme_manager.load_preset(name)?
    };

    config.user.theme = theme.clone();
    config.user.selected_preset = Some(name.to_string());
    mgr.save(&config).await?;
    Ok(theme)
}

pub async fn create_custom_theme(
    app_state: &AppState,
    name: String,
    description: Option<String>,
    theme: ThemeConfig,
) -> Result<PresetInfo> {
    let normalized_name = validate_theme_name(&name)?;
    let normalized_description = trim_optional_description(description);
    let normalized_theme = theme_storage::validate_and_normalize_theme(&theme)?;
    let now = now_unix_ts();
    let entry = CustomThemeEntry {
        key: generate_custom_theme_key(),
        name: normalized_name,
        description: normalized_description,
        theme: normalized_theme.clone(),
        created_at: now,
        updated_at: now,
    };

    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    config.user.custom_themes.push(entry.clone());
    config.user.theme = normalized_theme;
    config.user.selected_preset = Some(entry.key.clone());
    mgr.save(&config).await?;

    Ok(custom_entry_to_preset(&entry))
}

pub async fn update_custom_theme(
    app_state: &AppState,
    key: String,
    name: String,
    description: Option<String>,
    theme: ThemeConfig,
) -> Result<PresetInfo> {
    if !key.starts_with("custom:") {
        return Err(anyhow!("Only custom themes can be updated"));
    }

    let normalized_name = validate_theme_name(&name)?;
    let normalized_description = trim_optional_description(description);
    let normalized_theme = theme_storage::validate_and_normalize_theme(&theme)?;

    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    let Some(index) = config
        .user
        .custom_themes
        .iter()
        .position(|entry| entry.key == key)
    else {
        return Err(anyhow!("Custom theme not found"));
    };

    let mut entry = config.user.custom_themes[index].clone();
    entry.name = normalized_name;
    entry.description = normalized_description;
    entry.theme = normalized_theme.clone();
    entry.updated_at = now_unix_ts();

    config.user.custom_themes[index] = entry.clone();
    config.user.theme = normalized_theme;
    config.user.selected_preset = Some(entry.key.clone());
    mgr.save(&config).await?;

    Ok(custom_entry_to_preset(&entry))
}

pub async fn delete_custom_theme(app_state: &AppState, key: &str) -> Result<()> {
    if !key.starts_with("custom:") {
        return Err(anyhow!("Only custom themes can be deleted"));
    }

    let mgr = app_state.config_manager.lock().await;
    let mut config = mgr.load().await?;
    let before = config.user.custom_themes.len();
    config.user.custom_themes.retain(|entry| entry.key != key);
    if config.user.custom_themes.len() == before {
        return Err(anyhow!("Custom theme not found"));
    }
    if config.user.selected_preset.as_deref() == Some(key) {
        config.user.selected_preset = None;
        config.user.theme = ThemeConfig::default();
    }
    mgr.save(&config).await?;
    Ok(())
}

pub async fn get_selected_preset(app_state: &AppState) -> Result<Option<String>> {
    let mgr = app_state.config_manager.lock().await;
    Ok(mgr.load().await?.user.selected_preset)
}

fn now_unix_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
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

fn validate_theme_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Theme title is required"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::profile::test_app_state;

    #[tokio::test]
    async fn preset_apply_and_custom_theme_crud_round_trips() {
        let (_temp, app_state) = test_app_state().await;

        let presets = list_theme_presets(&app_state).await.expect("presets");
        assert!(presets.iter().any(|preset| preset.source == "builtin"));

        let generated =
            generate_simple_theme("#14b8a6", "#a855f7", "#e2e8f0").expect("simple theme");
        let created = create_custom_theme(
            &app_state,
            "  Terminal  ".to_string(),
            Some("  nice  ".to_string()),
            generated.clone(),
        )
        .await
        .expect("create custom");
        assert!(created.key.starts_with("custom:"));
        assert_eq!(created.name, "Terminal");
        assert_eq!(created.description, "nice");

        let applied = apply_preset(&app_state, &created.key)
            .await
            .expect("apply custom");
        assert_eq!(applied.base.c950, generated.base.c950);
        assert_eq!(
            get_selected_preset(&app_state)
                .await
                .expect("selected")
                .as_deref(),
            Some(created.key.as_str())
        );

        let updated = update_custom_theme(
            &app_state,
            created.key.clone(),
            "Terminal v2".to_string(),
            None,
            generated,
        )
        .await
        .expect("update custom");
        assert_eq!(updated.name, "Terminal v2");

        delete_custom_theme(&app_state, &created.key)
            .await
            .expect("delete custom");
        assert!(apply_preset(&app_state, &created.key).await.is_err());
        let active = get_theme(&app_state).await.expect("active theme");
        assert_eq!(active.primary.c500, ThemeConfig::default().primary.c500);
    }
}
