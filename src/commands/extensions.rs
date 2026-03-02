//! Tauri commands for extension, theme, and store management.

use crate::extensions;
use crate::state::AppState;
use log::{error, info};
use tauri::{Emitter, Manager, State};

// ---------------------------------------------------------------------------
// Extension discovery
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_extensions(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, String> {
    let config = state.config.lock().await;
    Ok(extensions::discover_items("extension", None, &config.extension_states))
}

#[tauri::command]
pub async fn list_themes(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, String> {
    let config = state.config.lock().await;
    Ok(extensions::discover_items("theme", None, &config.extension_states))
}

#[tauri::command]
pub async fn list_command_packs(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, String> {
    let config = state.config.lock().await;
    Ok(extensions::discover_items("commands", None, &config.extension_states))
}

// ---------------------------------------------------------------------------
// Extension config
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_extension_config(
    id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let config = state.config.lock().await;
    Ok(config.extensions.get(&id).cloned().unwrap_or(serde_json::json!({})))
}

#[tauri::command]
pub async fn save_extension_config(
    id: String,
    value: serde_json::Value,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut config = state.config.lock().await;
    config.extensions.insert(id.clone(), value);
    config.save().map_err(|e| format!("Failed to save config: {}", e))?;
    info!("Saved extension config for '{}'", id);
    if let Err(e) = app.emit("config_updated", ()) {
        error!("Failed to emit config_updated: {}", e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Enable / disable
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn set_extension_enabled(
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut config = state.config.lock().await;
    config.extension_states.insert(id.clone(), enabled);
    config.save().map_err(|e| format!("Failed to save config: {}", e))?;
    info!("Extension '{}' enabled={}", id, enabled);
    if let Err(e) = app.emit("config_updated", ()) {
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
) -> Result<serde_json::Value, String> {
    match extensions::load_theme_colors(&theme_id, &variant, None) {
        Ok(Some(colors)) => Ok(colors),
        Ok(None) => Ok(serde_json::json!(null)),
        Err(e) => {
            error!("Failed to load theme colors for '{}': {}", theme_id, e);
            Err(format!("Failed to load theme: {}", e))
        }
    }
}

// ---------------------------------------------------------------------------
// Read extension file content (for loading user-installed extension JS/CSS)
// ---------------------------------------------------------------------------

/// Read a file from a user-installed extension's directory.
/// Returns the file content as a string. Used by the frontend to dynamically
/// load search providers and settings modules from user-installed extensions.
#[tauri::command]
pub async fn read_extension_file(
    extension_id: String,
    kind: String,
    file_path: String,
) -> Result<String, String> {
    // Validate file_path to prevent directory traversal
    if file_path.contains("..") || file_path.contains('\\') || file_path.starts_with('/') {
        return Err("Invalid file path".to_string());
    }

    let subdir = extensions::kind_to_subdir(&kind)
        .map_err(|e| format!("Invalid kind: {}", e))?;
    let base = extensions::user_item_dir(subdir)
        .map_err(|e| format!("Failed to get directory: {}", e))?;
    let full_path = base.join(&extension_id).join(&file_path);

    // Verify the resolved path is within the extension directory
    let canonical_base = base.join(&extension_id);
    if full_path.exists() {
        let canonical = full_path.canonicalize()
            .map_err(|e| format!("Path error: {}", e))?;
        let canonical_parent = canonical_base.canonicalize()
            .map_err(|e| format!("Path error: {}", e))?;
        if !canonical.starts_with(&canonical_parent) {
            return Err("Path traversal detected".to_string());
        }
    }

    std::fs::read_to_string(&full_path)
        .map_err(|e| format!("Failed to read file: {}", e))
}

// ---------------------------------------------------------------------------
// Install / Uninstall
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn install_extension_from_path(
    source_path: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<extensions::InstalledItem, String> {
    let source = std::path::PathBuf::from(&source_path);

    let item = if source.extension().map(|e| e == "zip").unwrap_or(false) {
        // Install from zip file
        extensions::install_from_zip(&source)
            .map_err(|e| format!("Installation failed: {}", e))?
    } else {
        // Install from directory
        extensions::install_from_directory(&source)
            .map_err(|e| format!("Installation failed: {}", e))?
    };

    // Auto-enable
    let mut config = state.config.lock().await;
    config.extension_states.insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    if let Err(e) = app.emit("extensions_changed", ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }
    Ok(item)
}

#[tauri::command]
pub async fn uninstall_extension(
    id: String,
    kind: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    extensions::uninstall(&id, &kind)
        .map_err(|e| format!("Uninstall failed: {}", e))?;

    // Remove from enabled states and extension config
    let mut config = state.config.lock().await;
    config.extension_states.remove(&id);
    config.extensions.remove(&id);
    let _ = config.save();
    drop(config);

    if let Err(e) = app.emit("extensions_changed", ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Store window
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn open_store_window(app: tauri::AppHandle, tab: Option<String>) -> Result<(), String> {
    use tauri::WebviewWindowBuilder;

    if let Some(w) = app.get_webview_window("store") {
        let _ = w.show();
        let _ = w.set_focus();
        // Navigate to requested tab — use eval_script for reliability since
        // the window is already loaded and events can race with show()
        if let Some(ref t) = tab {
            // Sanitize: only allow alphanumeric and hyphens to prevent JS injection
            let safe_tab: String = t.chars().filter(|c| c.is_alphanumeric() || *c == '-').collect();
            let js = format!("if(typeof switchTab==='function')switchTab('{}')", safe_tab);
            let _ = w.eval(&js);
        }
        return Ok(());
    }

    // Build URL with tab query param so the page knows which tab to show on load
    let url_str = match &tab {
        Some(t) => format!("store.html?tab={}", t),
        None => "store.html".to_string(),
    };

    let w = WebviewWindowBuilder::new(
        &app,
        "store",
        tauri::WebviewUrl::App(url_str.into()),
    )
    .title("Extension Store")
    .inner_size(900.0, 640.0)
    .min_inner_size(600.0, 400.0)
    .center()
    .visible(true)
    .build()
    .map_err(|e| format!("Failed to open store window: {}", e))?;

    let _ = w.set_background_color(Some(tauri::window::Color(30, 30, 30, 255)));

    Ok(())
}

// ---------------------------------------------------------------------------
// Store API proxy (fetches from configured or default store URL)
// ---------------------------------------------------------------------------

/// Dev server URL used as default store in dev mode.
const DEV_STORE_URL: &str = "http://localhost:1420";

/// Resolve the store base URL: user-configured > dev default (in dev mode) > empty.
fn resolve_store_url(config: &crate::config::Config, dev_mode: bool) -> String {
    if let Some(ref url) = config.store_url {
        if !url.is_empty() {
            return url.clone();
        }
    }
    if dev_mode {
        return DEV_STORE_URL.to_string();
    }
    String::new()
}

#[tauri::command]
pub async fn store_get_catalog(
    kind: Option<String>,
    search: Option<String>,
    page: Option<u32>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let config = state.config.lock().await;
    let base_url = resolve_store_url(&config, state.dev_mode);
    drop(config);

    if base_url.is_empty() {
        // No store configured — return empty catalog
        return Ok(serde_json::json!({
            "items": [],
            "total": 0,
            "page": 1,
            "pageSize": 20
        }));
    }

    let mut url = format!("{}/store/catalog?page={}", base_url, page.unwrap_or(1));
    if let Some(ref k) = kind {
        url.push_str(&format!("&type={}", k));
    }
    if let Some(ref s) = search {
        url.push_str(&format!("&search={}", urlencoding::encode(s)));
    }

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("Store request failed: {}", e))?;
    let body = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Invalid store response: {}", e))?;
    Ok(body)
}

#[tauri::command]
pub async fn store_get_detail(
    id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let config = state.config.lock().await;
    let base_url = resolve_store_url(&config, state.dev_mode);
    drop(config);

    if base_url.is_empty() {
        return Err("No store URL configured".to_string());
    }

    let url = format!("{}/store/catalog/{}", base_url, id);
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("Store request failed: {}", e))?;
    let body = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Invalid store response: {}", e))?;
    Ok(body)
}

#[tauri::command]
pub async fn store_install(
    id: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<extensions::InstalledItem, String> {
    let config = state.config.lock().await;
    let base_url = resolve_store_url(&config, state.dev_mode);
    drop(config);

    if base_url.is_empty() {
        return Err("No store URL configured".to_string());
    }

    let url = format!("{}/store/catalog/{}/download", base_url, id);

    // Download the zip to a temp file
    let zip_path = std::env::temp_dir().join(format!("kiro-download-{}.zip", id));

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    // Verify content type or at least that we got bytes
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download: {}", e))?;

    if bytes.len() < 4 {
        return Err("Downloaded file is too small to be a valid zip".to_string());
    }

    // Verify zip magic bytes (PK\x03\x04)
    if &bytes[0..4] != b"PK\x03\x04" {
        return Err("Downloaded file is not a valid zip archive".to_string());
    }

    std::fs::write(&zip_path, &bytes)
        .map_err(|e| format!("Failed to save download: {}", e))?;

    // Extract and install
    let item = extensions::install_from_zip(&zip_path)
        .map_err(|e| format!("Installation failed: {}", e))?;

    // Cleanup the downloaded zip
    let _ = std::fs::remove_file(&zip_path);

    // Auto-enable
    let mut config = state.config.lock().await;
    config.extension_states.insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    if let Err(e) = app.emit("extensions_changed", ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }

    Ok(item)
}
