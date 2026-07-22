//! Extension/theme discovery, per-extension config, enable/disable, and
//! theme-colour loading — the cheap, config-backed surface of the
//! extensions commands.

use super::*;

// ---------------------------------------------------------------------------
// Extension discovery
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_extensions(
    features: State<'_, FeatureServices>,
) -> Result<Vec<extensions::InstalledItem>, AppError> {
    let config = features.config.lock_or_recover();
    Ok(extensions::discover_items(
        "extension",
        &config.extension_states,
    ))
}

#[tauri::command]
pub async fn list_themes(
    features: State<'_, FeatureServices>,
) -> Result<Vec<extensions::InstalledItem>, AppError> {
    let config = features.config.lock_or_recover();
    Ok(extensions::discover_items(
        "theme",
        &config.extension_states,
    ))
}

#[tauri::command]
pub async fn list_command_packs(
    features: State<'_, FeatureServices>,
) -> Result<Vec<extensions::InstalledItem>, AppError> {
    let config = features.config.lock_or_recover();
    Ok(extensions::discover_items(
        "commands",
        &config.extension_states,
    ))
}

// ---------------------------------------------------------------------------
// Extension config
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_extension_config(
    id: String,
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    let config = features.config.lock_or_recover();
    Ok(config
        .extensions
        .get(&id)
        .cloned()
        .unwrap_or(serde_json::json!({})))
}

#[tauri::command]
pub async fn save_extension_config<R: tauri::Runtime>(
    id: String,
    value: serde_json::Value,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.extensions.insert(id.clone(), value);
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    info!("Saved extension config for '{}'", id);
    if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
        error!("Failed to emit config_updated: {}", e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Enable / disable
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn set_extension_enabled<R: tauri::Runtime>(
    id: String,
    enabled: bool,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.extension_states.insert(id.clone(), enabled);
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    info!("Extension '{}' enabled={}", id, enabled);
    drop(config);
    crate::telemetry::track(
        &app,
        "extension_enabled_toggled",
        Some(serde_json::json!({
            "extension_id": id,
            "enabled": enabled,
        })),
    );
    if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
        error!("Failed to emit config_updated: {}", e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Theme colors
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn load_theme_colors(
    theme_id: String,
    variant: String,
) -> Result<serde_json::Value, AppError> {
    info!(
        "load_theme_colors: id='{}', variant='{}'",
        theme_id, variant
    );
    match extensions::load_theme_colors(&theme_id, &variant) {
        Ok(Some(colors)) => {
            info!("load_theme_colors: found colors for '{}'", theme_id);
            Ok(colors)
        }
        Ok(None) => {
            warn!(
                "load_theme_colors: no colors found for '{}' ({})",
                theme_id, variant
            );
            Ok(serde_json::json!(null))
        }
        Err(e) => {
            error!("Failed to load theme colors for '{}': {}", theme_id, e);
            Err(format!("Failed to load theme: {}", e).into())
        }
    }
}
