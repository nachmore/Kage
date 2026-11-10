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

/// The copy of each extension's settings each window last got from
/// `get_extension_config` or saved, by `(window, extension)`: the base of
/// that window's next save (docs/design/SYNC.md §9.5 item 3).
static WINDOW_COPIES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<(String, String), serde_json::Value>>,
> = std::sync::LazyLock::new(Default::default);

/// Most copies kept; past it the oldest windows' saves fall back to
/// replacing the whole object, as before.
const MAX_WINDOW_COPIES: usize = 512;

fn remember_copy(window: &str, id: &str, value: &serde_json::Value) {
    let mut m = WINDOW_COPIES.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() >= MAX_WINDOW_COPIES && !m.contains_key(&(window.to_string(), id.to_string())) {
        m.clear();
    }
    m.insert((window.to_string(), id.to_string()), value.clone());
}

fn window_copy(window: &str, id: &str) -> Option<serde_json::Value> {
    WINDOW_COPIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(window.to_string(), id.to_string()))
        .cloned()
}

/// A window's save of one extension's settings, merged three ways: a field
/// equal to the window's copy wasn't changed by it, so the current value
/// stays; a field it changed, added or removed is its edit. Without a copy
/// the save replaces the whole object, as before (§9.5 item 3).
pub fn merge_extension_settings(
    current: Option<&serde_json::Value>,
    base: Option<&serde_json::Value>,
    incoming: serde_json::Value,
) -> serde_json::Value {
    let (Some(base), Some(cur), Some(inc)) = (
        base.and_then(|b| b.as_object()),
        current.and_then(|c| c.as_object()),
        incoming.as_object(),
    ) else {
        return incoming;
    };
    let mut out = cur.clone();
    let mut fields: std::collections::BTreeSet<&String> = base.keys().collect();
    fields.extend(inc.keys());
    for f in fields {
        let (b, i) = (base.get(f), inc.get(f));
        if b == i {
            continue;
        }
        match i {
            Some(v) => {
                out.insert(f.clone(), v.clone());
            }
            None => {
                out.remove(f);
            }
        }
    }
    serde_json::Value::Object(out)
}

#[tauri::command]
pub async fn get_extension_config<R: tauri::Runtime>(
    id: String,
    features: State<'_, FeatureServices>,
    window: tauri::WebviewWindow<R>,
) -> Result<serde_json::Value, AppError> {
    let config = features.config.lock_or_recover();
    let value = config
        .extensions
        .get(&id)
        .cloned()
        .unwrap_or(serde_json::json!({}));
    remember_copy(window.label(), &id, &value);
    Ok(value)
}

#[tauri::command]
pub async fn save_extension_config<R: tauri::Runtime>(
    id: String,
    value: serde_json::Value,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    let base = window_copy(window.label(), &id);
    let merged = merge_extension_settings(config.extensions.get(&id), base.as_ref(), value);
    config.extensions.insert(id.clone(), merged.clone());
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);
    remember_copy(window.label(), &id, &merged);
    info!("Saved extension config for '{}'", id);
    if let Err(e) = app.emit(events::CONFIG_UPDATED, ()) {
        error!("Failed to emit config_updated: {}", e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::merge_extension_settings;
    use serde_json::json;

    #[test]
    fn a_stale_save_keeps_a_setting_changed_elsewhere() {
        // The window read units=metric; another computer changed it to
        // imperial; the window saves with only its own city edit.
        let base = json!({ "units": "metric", "city": "Oslo" });
        let current = json!({ "units": "imperial", "city": "Oslo" });
        let incoming = json!({ "units": "metric", "city": "Bergen" });
        assert_eq!(
            merge_extension_settings(Some(&current), Some(&base), incoming),
            json!({ "units": "imperial", "city": "Bergen" })
        );
    }

    #[test]
    fn removed_and_added_fields_are_edits_and_no_copy_replaces() {
        let base = json!({ "a": 1, "b": 2 });
        let current = json!({ "a": 1, "b": 2, "c": 3 });
        let incoming = json!({ "a": 1, "d": 4 });
        assert_eq!(
            merge_extension_settings(Some(&current), Some(&base), incoming.clone()),
            json!({ "a": 1, "c": 3, "d": 4 })
        );
        assert_eq!(
            merge_extension_settings(Some(&current), None, incoming.clone()),
            incoming
        );
    }
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
    // A turned-off extension's view windows go at once (§7.3 of
    // docs/design/EXTENSION_VIEWS.md).
    if !enabled {
        crate::ext_views::close_for_extension(&app, &id);
        // Its background agent calls end too. The host switch and consent
        // stay, for when it is turned on again.
        features.agent_calls.on_extension_changed(&id);
    }
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
