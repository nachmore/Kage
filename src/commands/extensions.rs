//! Tauri commands for extension, theme, and store management.

use crate::extensions;
use crate::state::AppState;
use log::{error, info, warn};
use tauri::{Emitter, Manager, State};

// ---------------------------------------------------------------------------
// Extension discovery
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_extensions(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, String> {
    let config = state.config.lock().unwrap();
    Ok(extensions::discover_items("extension", None, &config.extension_states))
}

#[tauri::command]
pub async fn list_themes(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, String> {
    let config = state.config.lock().unwrap();
    Ok(extensions::discover_items("theme", None, &config.extension_states))
}

#[tauri::command]
pub async fn list_command_packs(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, String> {
    let config = state.config.lock().unwrap();
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
    let config = state.config.lock().unwrap();
    Ok(config.extensions.get(&id).cloned().unwrap_or(serde_json::json!({})))
}

#[tauri::command]
pub async fn save_extension_config(
    id: String,
    value: serde_json::Value,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
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
    let mut config = state.config.lock().unwrap();
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
    let mut config = state.config.lock().unwrap();
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
    let mut config = state.config.lock().unwrap();
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
// Store URL config
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn save_store_url(
    url: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.store_url = url.filter(|s| !s.is_empty());
    config.save().map_err(|e| format!("Failed to save config: {}", e))?;
    info!("Store URL updated");
    Ok(())
}

// ---------------------------------------------------------------------------
// Store API proxy (fetches from configured or default store URL)
// ---------------------------------------------------------------------------

/// Dev server URL used as default store in dev mode.
const DEV_STORE_URL: &str = "http://localhost:1420";

/// Request timeout for store API calls.
const STORE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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

/// Validate a store URL: must be https, or http only for localhost (dev).
fn validate_store_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        return Ok(());
    }
    if url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1") {
        return Ok(());
    }
    Err(format!("Store URL must use HTTPS (got: {}). HTTP is only allowed for localhost.", url))
}

/// Build a reqwest client with timeout.
fn store_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(STORE_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

#[tauri::command]
pub async fn store_get_catalog(
    kind: Option<String>,
    search: Option<String>,
    page: Option<u32>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let base_url = {
        let config = state.config.lock().unwrap();
        resolve_store_url(&config, state.dev_mode)
    };

    if base_url.is_empty() {
        // No store configured — return empty catalog
        return Ok(serde_json::json!({
            "items": [],
            "total": 0,
            "page": 1,
            "pageSize": 20
        }));
    }

    validate_store_url(&base_url)?;

    let mut url = format!("{}/store/catalog?page={}", base_url, page.unwrap_or(1));
    if let Some(ref k) = kind {
        url.push_str(&format!("&type={}", k));
    }
    if let Some(ref s) = search {
        url.push_str(&format!("&search={}", urlencoding::encode(s)));
    }

    let client = store_client()?;
    let resp = client.get(&url)
        .send()
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
    let base_url = {
        let config = state.config.lock().unwrap();
        resolve_store_url(&config, state.dev_mode)
    };

    if base_url.is_empty() {
        return Err("No store URL configured".to_string());
    }

    validate_store_url(&base_url)?;

    let url = format!("{}/store/catalog/{}", base_url, id);
    let client = store_client()?;
    let resp = client.get(&url)
        .send()
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
    let base_url = {
        let config = state.config.lock().unwrap();
        resolve_store_url(&config, state.dev_mode)
    };

    if base_url.is_empty() {
        return Err("No store URL configured".to_string());
    }

    validate_store_url(&base_url)?;

    store_install_inner(&base_url, &id, &state, &app).await
}

/// Check for updates to installed extensions and optionally auto-install them.
/// Returns { updated: N, checked: N }
#[tauri::command]
pub async fn check_extension_updates(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let base_url = {
        let config = state.config.lock().unwrap();
        resolve_store_url(&config, state.dev_mode)
    };

    if base_url.is_empty() {
        return Ok(serde_json::json!({ "updated": 0, "checked": 0 }));
    }

    validate_store_url(&base_url)?;

    // Gather all installed items
    let mut installed: Vec<(String, String, String)> = Vec::new(); // (id, version, kind)
    let states = {
        let config = state.config.lock().unwrap();
        config.extension_states.clone()
    };

    for kind in &["extension", "theme", "commands"] {
        let items = extensions::discover_items(kind, None, &states);
        for item in items {
            if item.bundled { continue; }
            installed.push((
                item.manifest.id.clone(),
                item.manifest.version.clone(),
                kind.to_string(),
            ));
        }
    }

    if installed.is_empty() {
        return Ok(serde_json::json!({ "updated": 0, "checked": 0 }));
    }

    // Fetch the full catalog (no type filter) to get all available versions
    let url = format!("{}/store/catalog?page=1", base_url);
    let client = store_client()?;
    let resp = client.get(&url)
        .send()
        .await
        .map_err(|e| format!("Store request failed: {}", e))?;
    let catalog: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid store response: {}", e))?;

    let catalog_items = catalog.get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut updated = 0u32;
    let checked = installed.len() as u32;

    for (id, local_version, _kind) in &installed {
        // Find this item in the catalog
        let catalog_item = catalog_items.iter().find(|ci| {
            ci.get("id").and_then(|v| v.as_str()) == Some(id.as_str())
        });

        if let Some(ci) = catalog_item {
            let remote_version = ci.get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.0.0");

            // Compare versions using semver
            let local_sv = semver::Version::parse(local_version).ok();
            let remote_sv = semver::Version::parse(remote_version).ok();

            if let (Some(local), Some(remote)) = (local_sv, remote_sv) {
                if remote > local {
                    info!("Extension '{}' has update: {} -> {}", id, local_version, remote_version);
                    // Install the update (store_install handles download + extract)
                    match store_install_inner(&base_url, id, &state, &app).await {
                        Ok(_) => {
                            updated += 1;
                            info!("Updated extension '{}' to {}", id, remote_version);
                        }
                        Err(e) => {
                            warn!("Failed to update '{}': {}", id, e);
                        }
                    }
                }
            }
        }
    }

    // Update the last check timestamp
    let mut config = state.config.lock().unwrap();
    config.last_extension_update_check = Some(chrono::Utc::now().to_rfc3339());
    let _ = config.save();
    drop(config);

    Ok(serde_json::json!({ "updated": updated, "checked": checked }))
}

/// Inner install logic reused by both store_install and check_extension_updates.
async fn store_install_inner(
    base_url: &str,
    id: &str,
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
) -> Result<extensions::InstalledItem, String> {
    let url = format!("{}/store/catalog/{}/download", base_url, id);
    let zip_path = std::env::temp_dir().join(format!("kiro-download-{}.zip", id));

    let client = store_client()?;
    let resp = client.get(&url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;
    let bytes = resp.bytes().await
        .map_err(|e| format!("Failed to read download: {}", e))?;

    if bytes.len() < 4 || &bytes[0..4] != b"PK\x03\x04" {
        return Err("Invalid zip archive".to_string());
    }

    std::fs::write(&zip_path, &bytes)
        .map_err(|e| format!("Failed to save download: {}", e))?;

    let item = extensions::install_from_zip(&zip_path)
        .map_err(|e| format!("Installation failed: {}", e))?;

    let _ = std::fs::remove_file(&zip_path);

    let mut config = state.config.lock().unwrap();
    config.extension_states.insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    if let Err(e) = app.emit("extensions_changed", ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }

    Ok(item)
}
