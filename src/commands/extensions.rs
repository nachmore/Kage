//! Tauri commands for extension, theme, and store management.

use crate::error::AppError;
use crate::extensions;
use crate::lock_ext::LockExt;
use crate::state::AppState;
use log::{error, info, warn};
use tauri::{Emitter, Manager, State};

// ---------------------------------------------------------------------------
// Extension discovery
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_extensions(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, AppError> {
    let config = state.config.lock_or_recover();
    Ok(extensions::discover_items("extension", None, &config.extension_states))
}

#[tauri::command]
pub async fn list_themes(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, AppError> {
    let config = state.config.lock_or_recover();
    Ok(extensions::discover_items("theme", None, &config.extension_states))
}

#[tauri::command]
pub async fn list_command_packs(state: State<'_, AppState>) -> Result<Vec<extensions::InstalledItem>, AppError> {
    let config = state.config.lock_or_recover();
    Ok(extensions::discover_items("commands", None, &config.extension_states))
}

// ---------------------------------------------------------------------------
// Extension config
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_extension_config(
    id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, AppError> {
    let config = state.config.lock_or_recover();
    Ok(config.extensions.get(&id).cloned().unwrap_or(serde_json::json!({})))
}

#[tauri::command]
pub async fn save_extension_config(
    id: String,
    value: serde_json::Value,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    let mut config = state.config.lock_or_recover();
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
) -> Result<(), AppError> {
    let mut config = state.config.lock_or_recover();
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
) -> Result<serde_json::Value, AppError> {
    info!("load_theme_colors: id='{}', variant='{}'", theme_id, variant);
    match extensions::load_theme_colors(&theme_id, &variant, None) {
        Ok(Some(colors)) => {
            info!("load_theme_colors: found colors for '{}'", theme_id);
            Ok(colors)
        }
        Ok(None) => {
            warn!("load_theme_colors: no colors found for '{}' ({})", theme_id, variant);
            Ok(serde_json::json!(null))
        }
        Err(e) => {
            error!("Failed to load theme colors for '{}': {}", theme_id, e);
            Err(format!("Failed to load theme: {}", e).into())
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
) -> Result<String, AppError> {
    // Validate the extension id before it's spliced into any path. The
    // file_path containment check below is gated by `.exists()` and would
    // be skipped for a non-existent path; validating the id upfront makes
    // this fail closed on hostile ids regardless of which branch wins.
    extensions::validate_extension_id(&extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    // Validate file_path to prevent directory traversal
    if file_path.contains("..") || file_path.contains('\\') || file_path.starts_with('/') {
        return Err("Invalid file path".into());
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
            return Err("Path traversal detected".into());
        }
    }

    Ok(std::fs::read_to_string(&full_path)
        .map_err(|e| format!("Failed to read file: {}", e))?)
}

// ---------------------------------------------------------------------------
// Install / Uninstall
// ---------------------------------------------------------------------------

/// Stage a manual install from a local zip or directory path.
/// Like `store_install`, this does NOT emit `extensions_changed` — the
/// caller must show the permission prompt and call
/// `commit_extension_install` before the extension will load.
#[tauri::command]
pub async fn install_extension_from_path(
    source_path: String,
    state: State<'_, AppState>,
    _app: tauri::AppHandle,
) -> Result<extensions::InstalledItem, AppError> {
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

    // Mark enabled so the commit step can flip the grant and load it.
    let mut config = state.config.lock_or_recover();
    config.extension_states.insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    Ok(item)
}

#[tauri::command]
pub async fn uninstall_extension(
    id: String,
    kind: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    // extensions::uninstall validates internally too — checking here as well
    // keeps this command's error message specific (frontend gets a clean
    // "invalid extension id" rather than a generic "uninstall failed").
    extensions::validate_extension_id(&id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    extensions::uninstall(&id, &kind)
        .map_err(|e| format!("Uninstall failed: {}", e))?;

    // Remove from enabled states and extension config
    let mut config = state.config.lock_or_recover();
    config.extension_states.remove(&id);
    config.extensions.remove(&id);
    config.extension_grants.remove(&id);
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
pub async fn open_store_window(app: tauri::AppHandle, tab: Option<String>) -> Result<(), AppError> {
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
) -> Result<(), AppError> {
    let mut config = state.config.lock_or_recover();
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
    Err(format!("Store URL must use HTTPS (got: {}). HTTP is only allowed for localhost.", url).into())
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
    source: Option<String>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, AppError> {
    let (primary_url, sources) = {
        let config = state.config.lock_or_recover();
        let primary = resolve_store_url(&config, state.dev_mode);
        let sources = config.store_sources.clone();
        (primary, sources)
    };

    // Build list of (name, url) pairs to fetch from
    let mut store_urls: Vec<(String, String)> = Vec::new();
    if !primary_url.is_empty() {
        store_urls.push(("Default".to_string(), primary_url));
    }
    for s in &sources {
        if s.enabled && !s.url.is_empty() && store_urls.len() < 3 {
            if validate_store_url(&s.url).is_ok() {
                store_urls.push((s.name.clone(), s.url.clone()));
            }
        }
    }

    // Filter by source name if requested
    if let Some(ref src_filter) = source {
        store_urls.retain(|(name, _)| name == src_filter);
    }

    if store_urls.is_empty() {
        return Ok(serde_json::json!({
            "items": [],
            "total": 0,
            "page": 1,
            "pageSize": 20,
            "sources": []
        }));
    }

    // Fetch from all sources in parallel
    let client = store_client()?;
    let source_names: Vec<String> = store_urls.iter().map(|(name, _)| name.clone()).collect();

    let mut handles = Vec::new();
    for (name, base_url) in store_urls {
        let client = client.clone();
        let mut url = format!("{}/store/catalog?page={}", base_url, page.unwrap_or(1));
        if let Some(ref k) = kind {
            url.push_str(&format!("&type={}", k));
        }
        if let Some(ref s) = search {
            url.push_str(&format!("&search={}", urlencoding::encode(s)));
        }
        handles.push(tokio::spawn(async move {
            match client.get(&url).send().await {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(body) => {
                            let mut items = Vec::new();
                            if let Some(arr) = body.get("items").and_then(|v| v.as_array()) {
                                for item in arr {
                                    let mut tagged = item.clone();
                                    if let Some(obj) = tagged.as_object_mut() {
                                        obj.insert("_source".to_string(), serde_json::json!(name));
                                    }
                                    items.push(tagged);
                                }
                            }
                            items
                        }
                        Err(_) => Vec::new(),
                    }
                }
                Err(e) => {
                    log::warn!("Failed to fetch from store '{}': {}", name, e);
                    Vec::new()
                }
            }
        }));
    }

    let mut all_items: Vec<serde_json::Value> = Vec::new();
    for handle in handles {
        if let Ok(items) = handle.await {
            all_items.extend(items);
        }
    }

    // Deduplicate by ID (first source wins)
    let mut seen = std::collections::HashSet::new();
    all_items.retain(|item| {
        if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
            seen.insert(id.to_string())
        } else {
            true
        }
    });

    let total = all_items.len();
    Ok(serde_json::json!({
        "items": all_items,
        "total": total,
        "page": page.unwrap_or(1),
        "pageSize": 20,
        "sources": source_names
    }))
}


#[tauri::command]
pub async fn store_get_detail(
    id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, AppError> {
    let base_url = {
        let config = state.config.lock_or_recover();
        resolve_store_url(&config, state.dev_mode)
    };

    if base_url.is_empty() {
        return Err("No store URL configured".into());
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

/// Stage an install from the store. The extension files are written to
/// disk and `extension_states` is set to enabled, but `extensions_changed`
/// is NOT emitted yet, so nothing loads the extension's code. The caller
/// (frontend) shows a permission prompt based on the returned manifest;
/// on approval it calls `commit_extension_install` which records the
/// grant and emits the event. On rejection it calls `uninstall_extension`
/// to roll back.
#[tauri::command]
pub async fn store_install(
    id: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<extensions::InstalledItem, AppError> {
    let base_url = {
        let config = state.config.lock_or_recover();
        resolve_store_url(&config, state.dev_mode)
    };

    if base_url.is_empty() {
        return Err("No store URL configured".into());
    }

    validate_store_url(&base_url)?;

    store_install_inner(&base_url, &id, &state, &app, false)
        .await
        .map_err(AppError::from)
}

/// Finalize a staged install: save the user-approved capability grant and
/// emit `extensions_changed` so the loader picks it up.
///
/// Call this only after the user has approved the extension's capability
/// set via the install-time permission prompt. Granting without user
/// consent is a direct security violation.
#[tauri::command]
pub async fn commit_extension_install(
    extension_id: String,
    granted: Vec<String>,
    approved_version: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    // Reject hostile ids before they're recorded as a grant. Even though
    // this function only writes to the in-memory config map (no direct
    // filesystem op), a stored bad id would later be consulted by code
    // paths that *do* hit the filesystem. Defense in depth.
    extensions::validate_extension_id(&extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    let mut config = state.config.lock_or_recover();
    let record = crate::config::ExtensionGrant {
        granted,
        approved_version,
        approved_at: chrono::Utc::now().to_rfc3339(),
    };
    info!(
        "Committing install for '{}': capabilities {:?}",
        extension_id, record.granted
    );
    config.extension_grants.insert(extension_id.clone(), record);
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);

    if let Err(e) = app.emit("extensions_changed", ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }
    Ok(())
}

/// Check for updates to installed extensions and optionally auto-install them.
/// Returns { updated: N, checked: N }
#[tauri::command]
pub async fn check_extension_updates(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, AppError> {
    let base_url = {
        let config = state.config.lock_or_recover();
        resolve_store_url(&config, state.dev_mode)
    };

    if base_url.is_empty() {
        return Ok(serde_json::json!({ "updated": 0, "checked": 0 }));
    }

    validate_store_url(&base_url)?;

    // Gather all installed items
    let mut installed: Vec<(String, String, String)> = Vec::new(); // (id, version, kind)
    let states = {
        let config = state.config.lock_or_recover();
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
                    // Install the update in place. The existing capability
                    // grant is preserved; if the updated manifest requests
                    // more capabilities, the runtime drops them until the
                    // user re-approves. We do emit here because auto-update
                    // is an in-place refresh of already-approved software.
                    match store_install_inner(&base_url, id, &state, &app, true).await {
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
    let mut config = state.config.lock_or_recover();
    config.last_extension_update_check = Some(chrono::Utc::now().to_rfc3339());
    let _ = config.save();
    drop(config);

    Ok(serde_json::json!({ "updated": updated, "checked": checked }))
}

/// Inner install logic reused by both store_install and check_extension_updates.
/// The `emit_changed` flag controls whether we fire the `extensions_changed`
/// event. Installs initiated by the user are staged without emitting so the
/// frontend can show an install-time permission prompt before loading the
/// extension. If the user approves, the frontend calls
/// `commit_extension_install`, which saves the grant and emits.
async fn store_install_inner(
    base_url: &str,
    id: &str,
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
    emit_changed: bool,
) -> Result<extensions::InstalledItem, String> {
    let url = format!("{}/store/catalog/{}/download", base_url, id);
    let zip_path = std::env::temp_dir().join(format!("kage-download-{}.zip", id));

    let client = store_client()?;
    let resp = client.get(&url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;
    let bytes = resp.bytes().await
        .map_err(|e| format!("Failed to read download: {}", e))?;

    if bytes.len() < 4 || &bytes[0..4] != b"PK\x03\x04" {
        return Err("Invalid zip archive".into());
    }

    std::fs::write(&zip_path, &bytes)
        .map_err(|e| format!("Failed to save download: {}", e))?;

    let item = extensions::install_from_zip(&zip_path)
        .map_err(|e| format!("Installation failed: {}", e))?;

    let _ = std::fs::remove_file(&zip_path);

    let mut config = state.config.lock_or_recover();
    config.extension_states.insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    if emit_changed {
        if let Err(e) = app.emit("extensions_changed", ()) {
            error!("Failed to emit extensions_changed: {}", e);
        }
    }

    Ok(item)
}

// ---------------------------------------------------------------------------
// Generic extension data persistence
// ---------------------------------------------------------------------------
// Stores extension data as JSON files at:
//   <config_dir>/kage/extension-data/<extension_id>/<key>.json
//
// The path-resolution and migration logic lives in src/extensions.rs so it
// can be unit-tested directly (this module is gated under #[cfg(not(test))]
// because of the tauri::command macros). The host JS bridge force-injects
// extension_id from its own record before forwarding storage commands here,
// so a sandboxed caller can't spoof a different extension's identity.

/// Returns the root extension-data directory, creating it if needed.
fn extension_data_root() -> Result<std::path::PathBuf, String> {
    let dir = dirs::config_dir()
        .ok_or("No config directory")?
        .join("kage")
        .join("extension-data");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create extension-data dir: {}", e))?;
    }
    Ok(dir)
}

/// Resolve the on-disk path for a given (extension_id, key).
fn resolve_data_path(extension_id: &str, key: &str) -> Result<std::path::PathBuf, String> {
    let root = extension_data_root()?;
    extensions::resolve_extension_data_path(&root, extension_id, key)
        .map_err(|e| format!("{}", e))
}

/// Save arbitrary JSON data for an extension.
/// Stored at: <config_dir>/kage/extension-data/<extension_id>/<key>.json
#[tauri::command]
pub async fn save_extension_data(
    extension_id: String,
    key: String,
    data: String,
) -> Result<(), AppError> {
    let path = resolve_data_path(&extension_id, &key)?;
    std::fs::write(&path, &data)
        .map_err(|e| format!("Failed to save extension data '{}/{}': {}", extension_id, key, e))?;
    Ok(())
}

/// Load JSON data for an extension. Returns null if the file doesn't exist.
#[tauri::command]
pub async fn load_extension_data(
    extension_id: String,
    key: String,
) -> Result<Option<String>, AppError> {
    let path = resolve_data_path(&extension_id, &key)?;
    match std::fs::read_to_string(&path) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Failed to load extension data '{}/{}': {}", extension_id, key, e))?,
    }
}

/// Delete extension data file.
#[tauri::command]
pub async fn delete_extension_data(
    extension_id: String,
    key: String,
) -> Result<(), AppError> {
    let path = resolve_data_path(&extension_id, &key)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to delete extension data '{}/{}': {}", extension_id, key, e))?,
    }
}

// ---------------------------------------------------------------------------
// Bundled package installation (for first-run wizard)
// ---------------------------------------------------------------------------

/// Resolve the path to the bundled store/packages directory.
/// Checks dev path first (project root), then next to the executable (production).
fn bundled_packages_dir() -> Option<std::path::PathBuf> {
    // Dev mode: relative to project root
    let dev_path = std::path::PathBuf::from("store/packages");
    if dev_path.exists() {
        return Some(dev_path);
    }

    // Production: next to the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let bundled = exe_dir.join("store").join("packages");
            if bundled.exists() {
                return Some(bundled);
            }
            // One level up (some installer layouts)
            if let Some(parent) = exe_dir.parent() {
                let up_one = parent.join("store").join("packages");
                if up_one.exists() {
                    return Some(up_one);
                }
            }
        }
    }

    None
}

/// Install an extension from the bundled packages directory.
/// Used by the first-run wizard to install recommended extensions without
/// network access. Like `store_install`, this is a staged install: the
/// caller must show the permission prompt and call
/// `commit_extension_install` before the extension will load.
#[tauri::command]
pub async fn install_bundled_package(
    id: String,
    state: State<'_, AppState>,
    _app: tauri::AppHandle,
) -> Result<extensions::InstalledItem, AppError> {
    let packages_dir = bundled_packages_dir()
        .ok_or_else(|| AppError::from("Bundled packages directory not found"))?;

    // Try common naming patterns: id.zip, id-theme.zip
    let candidates = vec![
        packages_dir.join(format!("{}.zip", id)),
        packages_dir.join(format!("{}-theme.zip", id)),
    ];

    let zip_path = candidates.into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| AppError::from(format!("Bundled package not found for '{}'", id)))?;

    info!("Installing bundled package '{}' from {:?}", id, zip_path);

    let item = extensions::install_from_zip(&zip_path)
        .map_err(|e| AppError::from(format!("Failed to install bundled package '{}': {}", id, e)))?;

    let mut config = state.config.lock_or_recover();
    config.extension_states.insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    Ok(item)
}

// ---------------------------------------------------------------------------
// Extension capability grants
// ---------------------------------------------------------------------------

/// Remove a recorded grant (e.g. when uninstalling an extension).
#[tauri::command]
pub async fn remove_extension_grant(
    extension_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    extensions::validate_extension_id(&extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    let mut config = state.config.lock_or_recover();
    if config.extension_grants.remove(&extension_id).is_some() {
        info!("Removed capability grant for '{}'", extension_id);
        config
            .save()
            .map_err(|e| format!("Failed to save config: {}", e))?;
    }
    Ok(())
}
