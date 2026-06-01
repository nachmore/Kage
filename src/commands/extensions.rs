//! Tauri commands for extension, theme, and store management.

use crate::error::{AppError, ErrorKind};
use crate::events;
use crate::extensions;
use crate::lock_ext::LockExt;
use crate::state::{FeatureServices, UiState};
use crate::window_labels;
use log::{error, info, warn};
use tauri::{Emitter, Manager, State};

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
pub async fn save_extension_config(
    id: String,
    value: serde_json::Value,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
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
pub async fn set_extension_enabled(
    id: String,
    enabled: bool,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
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

// ---------------------------------------------------------------------------
// Read extension file content (for loading user-installed extension JS/CSS)
// ---------------------------------------------------------------------------

/// Load an extension's `_locales/<lang>/messages.json`. Falls back through
/// region-stripped variants ("zh-CN" → "zh") and finally to "en". Returns the
/// catalog as a JSON object so the host can hand it directly to the sandbox
/// runtime; an extension with no `_locales/` ships back an empty object,
/// which the runtime treats as "no translations, render keys verbatim".
///
/// Path-containment is validated identically to `read_extension_file` to
/// keep extensions from escaping their own directory via `..` segments in
/// the language code. The language argument is restricted to a small
/// alphabet (letters, digits, hyphens) for the same reason.
#[tauri::command]
pub async fn read_extension_locale(
    extension_id: String,
    kind: String,
    language: String,
) -> Result<serde_json::Value, AppError> {
    extensions::validate_extension_id(&extension_id).map_err(|e| {
        AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.invalid_id",
            &[("reason", &e.to_string())],
        )
    })?;

    if !language
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
        || language.is_empty()
        || language.len() > 16
    {
        return Err(AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.invalid_locale",
            &[("language", &language)],
        ));
    }

    let subdir = extensions::kind_to_subdir(&kind).map_err(|e| {
        AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.invalid_kind",
            &[("reason", &e.to_string())],
        )
    })?;
    let base = extensions::user_item_dir(subdir).map_err(|e| {
        AppError::keyed(
            crate::error::ErrorKind::Internal,
            "errors.extension.dir_unavailable",
            &[("reason", &e.to_string())],
        )
    })?;
    let ext_root = base.join(&extension_id);
    let locales_dir = ext_root.join("_locales");

    // Try the requested language, then region-stripped form, then en. The
    // first hit wins; an entirely-absent _locales directory returns `{}`.
    let candidates: Vec<String> = {
        let mut out = vec![language.clone()];
        if let Some((stem, _)) = language.split_once('-') {
            if !out.contains(&stem.to_string()) {
                out.push(stem.to_string());
            }
        }
        if !out.iter().any(|c| c == "en") {
            out.push("en".to_string());
        }
        out
    };

    for cand in &candidates {
        let path = locales_dir.join(cand).join("messages.json");
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            AppError::keyed(
                crate::error::ErrorKind::Internal,
                "errors.extension.locale_read_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            AppError::keyed(
                crate::error::ErrorKind::Internal,
                "errors.extension.locale_parse_failed",
                &[("language", cand), ("reason", &e.to_string())],
            )
        })?;
        return Ok(value);
    }

    // No catalog at all — return an empty object so the runtime can still boot.
    Ok(serde_json::json!({}))
}

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

    let subdir = extensions::kind_to_subdir(&kind).map_err(|e| format!("Invalid kind: {}", e))?;
    let base =
        extensions::user_item_dir(subdir).map_err(|e| format!("Failed to get directory: {}", e))?;
    let full_path = base.join(&extension_id).join(&file_path);

    // Verify the resolved path is within the extension directory
    let canonical_base = base.join(&extension_id);
    if full_path.exists() {
        let canonical = full_path
            .canonicalize()
            .map_err(|e| format!("Path error: {}", e))?;
        let canonical_parent = canonical_base
            .canonicalize()
            .map_err(|e| format!("Path error: {}", e))?;
        if !canonical.starts_with(&canonical_parent) {
            return Err("Path traversal detected".into());
        }
    }

    Ok(std::fs::read_to_string(&full_path).map_err(|e| format!("Failed to read file: {}", e))?)
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
    features: State<'_, FeatureServices>,
    _app: tauri::AppHandle,
) -> Result<extensions::InstalledItem, AppError> {
    let source = std::path::PathBuf::from(&source_path);

    let item = if source.extension().map(|e| e == "zip").unwrap_or(false) {
        // Install from zip file
        extensions::install_from_zip(&source).map_err(|e| format!("Installation failed: {}", e))?
    } else {
        // Install from directory
        extensions::install_from_directory(&source)
            .map_err(|e| format!("Installation failed: {}", e))?
    };

    // Mark enabled so the commit step can flip the grant and load it.
    let mut config = features.config.lock_or_recover();
    config
        .extension_states
        .insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    Ok(item)
}

#[tauri::command]
pub async fn uninstall_extension(
    id: String,
    kind: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    // extensions::uninstall validates internally too — checking here as well
    // keeps this command's error message specific (frontend gets a clean
    // "invalid extension id" rather than a generic "uninstall failed").
    extensions::validate_extension_id(&id).map_err(|e| format!("Invalid extension id: {}", e))?;

    extensions::uninstall(&id, &kind).map_err(|e| format!("Uninstall failed: {}", e))?;

    // Remove from enabled states and extension config
    let mut config = features.config.lock_or_recover();
    config.extension_states.remove(&id);
    config.extensions.remove(&id);
    config.extension_grants.remove(&id);
    let _ = config.save();
    drop(config);

    crate::telemetry::track(
        &app,
        "extension_uninstalled",
        Some(serde_json::json!({ "extension_id": id, "kind": kind })),
    );

    if let Err(e) = app.emit(events::EXTENSIONS_CHANGED, ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Store window
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn open_store_window(app: tauri::AppHandle, tab: Option<String>) -> Result<(), AppError> {
    open_store_window_with_intent(app, tab, None).await
}

/// Same as `open_store_window` but with an optional install intent —
/// when present, the URL gets `&install=<id>` appended so the store JS
/// auto-prompts the install on boot. Used by the `kage://install/<id>`
/// deep-link handler. Frontend callers stick to `open_store_window`
/// without the third arg.
pub async fn open_store_window_with_intent(
    app: tauri::AppHandle,
    tab: Option<String>,
    install_id: Option<String>,
) -> Result<(), AppError> {
    use tauri::WebviewWindowBuilder;

    // Telemetry early so we count "opened" even on the path where the
    // window already exists and we just focus it.
    crate::telemetry::track(
        &app,
        "store_opened",
        tab.as_deref().map(|t| serde_json::json!({ "tab": t })),
    );

    // Sanitize the install id: only the alphabet our manifest validator
    // allows. Belt-and-suspenders — the deep-link handler already
    // checked, but we sanitise again at the JS-injection boundary.
    let safe_install_id: Option<String> = install_id.and_then(|id| {
        let s: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    });

    if let Some(w) = app.get_webview_window(window_labels::STORE) {
        let _ = w.show();
        let _ = w.set_focus();
        // Navigate to requested tab — use eval_script for reliability since
        // the window is already loaded and events can race with show()
        if let Some(ref t) = tab {
            // Sanitize: only allow alphanumeric and hyphens to prevent JS injection
            let safe_tab: String = t
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect();
            let js = format!("if(typeof switchTab==='function')switchTab('{}')", safe_tab);
            let _ = w.eval(&js);
        }
        // For an already-open store, fire the install intent via JS
        // eval too. The function is exposed by the store's boot path
        // and handles the "we don't know about this id yet" case
        // (e.g. the catalog hasn't reloaded since the deep link
        // arrived) by deferring until next render.
        if let Some(ref id) = safe_install_id {
            let js = format!(
                "if(typeof handleDeepLinkInstall==='function')handleDeepLinkInstall('{}')",
                id
            );
            let _ = w.eval(&js);
        }
        crate::setup::update_activation_policy(&app);
        return Ok(());
    }

    // Build URL. Tab + install both flow through query params so the
    // store window's bootstrap path picks them up before any event
    // listener races. This avoids the "emit before listen" timing
    // problem that biting us when we tried emitting an event right
    // after window create.
    let mut url_str = String::from("store.html");
    let mut sep = '?';
    if let Some(ref t) = tab {
        url_str.push(sep);
        url_str.push_str("tab=");
        url_str.push_str(t);
        sep = '&';
    }
    if let Some(ref id) = safe_install_id {
        url_str.push(sep);
        url_str.push_str("install=");
        url_str.push_str(id);
    }

    let w = WebviewWindowBuilder::new(
        &app,
        window_labels::STORE,
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
    crate::setup::update_activation_policy(&app);

    Ok(())
}

// ---------------------------------------------------------------------------
// Store URL config
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn save_store_url(
    url: Option<String>,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.store_url = url.filter(|s| !s.is_empty());
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    info!("Store URL updated");
    Ok(())
}

// ---------------------------------------------------------------------------
// Store API client (fetches from configured or default store URL)
// ---------------------------------------------------------------------------
//
// Wire format (schema v1) — the store is plain static JSON files served
// from `https://nachmore.github.io/Kage-Extensions/` (or any HTTPS host
// matching the same layout):
//
//   GET <base>/catalog.json
//     {
//       "schemaVersion": 1,
//       "generatedAt": "...",
//       "items": [{
//         "id", "type", "name", "version", "author", "description", "icon",
//         "tags": [...], "permissions": [...],
//         "downloadUrl": "packages/<id>-<version>.zip",   (relative to base)
//         "detailUrl":   "detail/<id>.json",              (relative to base)
//         "size", "sha256", "sourceHash", "updatedAt"
//       }]
//     }
//
//   GET <base>/detail/<id>.json   — same fields plus `manifest` and `readme`
//   GET <base>/packages/<id>-<version>.zip
//
// Pagination is client-side: the catalog is small enough to fetch in one
// shot, and a single round-trip is friendlier to GitHub Pages caching.

/// Dev server URL used as default store in dev mode.
const DEV_STORE_URL: &str = "http://localhost:1420";

/// Default production store URL — the public Kage-Extensions catalog.
const DEFAULT_STORE_URL: &str = "https://nachmore.github.io/Kage-Extensions";

/// Request timeout for store API calls.
const STORE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Resolve the store base URL: user-configured > production default > dev default.
fn resolve_store_url(config: &crate::config::Config, dev_mode: bool) -> String {
    if let Some(ref url) = config.store_url {
        if !url.is_empty() {
            return url.trim_end_matches('/').to_string();
        }
    }
    if dev_mode {
        return DEV_STORE_URL.to_string();
    }
    DEFAULT_STORE_URL.to_string()
}

/// Build a reqwest client with timeout.
fn store_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(STORE_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

/// Resolve a relative path inside the catalog (`packages/foo.zip`) to an
/// absolute URL using the store base. Strips a leading slash so the
/// result is always `<base>/<rel>` regardless of how the catalog quotes
/// it.
fn resolve_relative(base: &str, rel: &str) -> String {
    let r = rel.trim_start_matches('/');
    format!("{}/{}", base.trim_end_matches('/'), r)
}

#[tauri::command]
pub async fn store_get_catalog(
    kind: Option<String>,
    search: Option<String>,
    page: Option<u32>,
    source: Option<String>,
    force_refresh: Option<bool>,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
) -> Result<serde_json::Value, AppError> {
    let force_refresh = force_refresh.unwrap_or(false);
    let (primary_url, sources) = {
        let config = features.config.lock_or_recover();
        let primary = resolve_store_url(&config, ui.dev_mode);
        let sources = config.store_sources.clone();
        (primary, sources)
    };

    // Build list of (name, url) pairs to fetch from
    let mut store_urls: Vec<(String, String)> = Vec::new();
    if !primary_url.is_empty() {
        store_urls.push(("Default".to_string(), primary_url));
    }
    for s in &sources {
        if s.enabled
            && !s.url.is_empty()
            && store_urls.len() < 3
            && extensions::validate_store_url(&s.url).is_ok()
        {
            store_urls.push((s.name.clone(), s.url.trim_end_matches('/').to_string()));
        }
    }

    // Filter by source name if requested
    if let Some(ref src_filter) = source {
        store_urls.retain(|(name, _)| name == src_filter);
    }

    let source_names: Vec<String> = store_urls.iter().map(|(name, _)| name.clone()).collect();

    if store_urls.is_empty() {
        return Ok(serde_json::json!({
            "items": [],
            "total": 0,
            "page": 1,
            "pageSize": 20,
            "sources": [],
            "offline": false,
        }));
    }

    let client = store_client()?;
    let mut handles = Vec::new();
    for (name, base_url) in &store_urls {
        let client = client.clone();
        // Cache-bust on explicit refresh. GitHub Pages doesn't honour
        // `Cache-Control: no-cache` request headers — it serves the
        // edge-cached object regardless. A unique query param defeats
        // the cache key entirely. We intentionally don't bust on every
        // call; the catalog moves rarely and hammering with fresh
        // queries would push us into the rate-limited tier.
        let url = if force_refresh {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            format!("{}/catalog.json?_={}", base_url, now)
        } else {
            format!("{}/catalog.json", base_url)
        };
        let name = name.clone();
        let base_url = base_url.clone();
        handles.push(tokio::spawn(async move {
            match client
                .get(&url)
                // Belt-and-suspenders: also send the standard cache
                // control hints. Some intermediate proxies honour
                // these even when GitHub Pages doesn't.
                .header(reqwest::header::CACHE_CONTROL, "no-cache")
                .header(reqwest::header::PRAGMA, "no-cache")
                .send()
                .await
            {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(body) => {
                        let mut items = Vec::new();
                        if let Some(arr) = body.get("items").and_then(|v| v.as_array()) {
                            for item in arr {
                                let mut tagged = item.clone();
                                if let Some(obj) = tagged.as_object_mut() {
                                    obj.insert("_source".to_string(), serde_json::json!(name));
                                    // Resolve relative URLs to absolute so the
                                    // frontend doesn't need to know the base.
                                    if let Some(rel) = obj
                                        .get("downloadUrl")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string())
                                    {
                                        if !rel.starts_with("http") {
                                            obj.insert(
                                                "downloadUrl".to_string(),
                                                serde_json::json!(resolve_relative(
                                                    &base_url, &rel
                                                )),
                                            );
                                        }
                                    }
                                }
                                items.push(tagged);
                            }
                        }
                        Ok(items)
                    }
                    Err(e) => Err(format!("Invalid catalog from {}: {}", name, e)),
                },
                Err(e) => Err(format!("Fetch failed from {}: {}", name, e)),
            }
        }));
    }

    let mut all_items: Vec<serde_json::Value> = Vec::new();
    let mut errors = 0u32;
    for handle in handles {
        match handle.await {
            Ok(Ok(items)) => all_items.extend(items),
            Ok(Err(e)) => {
                errors += 1;
                log::warn!("{}", e);
            }
            Err(e) => {
                errors += 1;
                log::warn!("Catalog task failed: {}", e);
            }
        }
    }

    // If every source failed (typically: offline), surface that to the
    // frontend so it can render its "browse online when connected" state
    // instead of silently showing zero results.
    let offline = errors > 0 && all_items.is_empty();

    // Deduplicate by ID (first source wins)
    let mut seen = std::collections::HashSet::new();
    all_items.retain(|item| {
        if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
            seen.insert(id.to_string())
        } else {
            true
        }
    });

    // Apply kind / search filters server-side so the existing JS UI keeps
    // working unchanged. Pagination is also synthesised — there's no real
    // paging on the static layout, but returning the same envelope shape
    // means we don't have to touch the renderer.
    if let Some(ref k) = kind {
        all_items.retain(|item| item.get("type").and_then(|v| v.as_str()) == Some(k.as_str()));
    }
    if let Some(ref s) = search {
        let q = s.to_lowercase();
        all_items.retain(|item| {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let desc = item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let tags = item
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            name.contains(&q) || desc.contains(&q) || tags.contains(&q)
        });
    }

    let total = all_items.len();
    let page_size = 20usize;
    let page = page.unwrap_or(1).max(1) as usize;
    let start = (page - 1) * page_size;
    let end = (start + page_size).min(total);
    let paged: Vec<serde_json::Value> = if start < total {
        all_items[start..end].to_vec()
    } else {
        Vec::new()
    };

    Ok(serde_json::json!({
        "items": paged,
        "total": total,
        "page": page,
        "pageSize": page_size,
        "sources": source_names,
        "offline": offline,
    }))
}

#[tauri::command]
pub async fn store_get_detail(
    id: String,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
) -> Result<serde_json::Value, AppError> {
    let base_url = {
        let config = features.config.lock_or_recover();
        resolve_store_url(&config, ui.dev_mode)
    };

    if base_url.is_empty() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.extensions.no_store_url",
            &[],
        ));
    }

    extensions::validate_store_url(&base_url)?;
    extensions::validate_extension_id(&id).map_err(|e| format!("Invalid id: {}", e))?;

    let url = format!("{}/detail/{}.json", base_url, id);
    let client = store_client()?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Store request failed: {}", e))?;
    let mut body = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Invalid store response: {}", e))?;

    // Resolve relative download URL the same way the catalog endpoint does.
    if let Some(obj) = body.as_object_mut() {
        if let Some(rel) = obj
            .get("downloadUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        {
            if !rel.starts_with("http") {
                obj.insert(
                    "downloadUrl".to_string(),
                    serde_json::json!(resolve_relative(&base_url, &rel)),
                );
            }
        }
    }
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
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    app: tauri::AppHandle,
) -> Result<extensions::InstalledItem, AppError> {
    let base_url = {
        let config = features.config.lock_or_recover();
        resolve_store_url(&config, ui.dev_mode)
    };

    if base_url.is_empty() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.extensions.no_store_url",
            &[],
        ));
    }

    extensions::validate_store_url(&base_url)?;

    store_install_inner(&base_url, &id, &features, &app, false)
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
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    // Reject hostile ids before they're recorded as a grant. Even though
    // this function only writes to the in-memory config map (no direct
    // filesystem op), a stored bad id would later be consulted by code
    // paths that *do* hit the filesystem. Defense in depth.
    extensions::validate_extension_id(&extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    // Normalize the grant list. The renderer shows a modal built from
    // `CAPABILITIES` in extension-permissions.js, which should already
    // match the manifest — but Rust is authoritative. If the two ever
    // drift (bug in JS, manifest tampering between install and commit,
    // user-installed extension with a typo), we still store a clean
    // set. Dropped entries log a warning so the drift gets noticed.
    let granted = extensions::normalize_permissions(&granted, &extension_id);

    let mut config = features.config.lock_or_recover();
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

    // Anonymous install-success event. We send only the extension id,
    // which is a published identifier (visible in the extension store),
    // so this doesn't leak anything the user hasn't already chosen.
    crate::telemetry::track(
        &app,
        "extension_installed",
        Some(serde_json::json!({ "extension_id": extension_id })),
    );

    if let Err(e) = app.emit(events::EXTENSIONS_CHANGED, ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }
    Ok(())
}

/// Check for updates to installed extensions and optionally auto-install them.
/// Returns { updated: N, checked: N }
#[tauri::command]
pub async fn check_extension_updates(
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, AppError> {
    let base_url = {
        let config = features.config.lock_or_recover();
        resolve_store_url(&config, ui.dev_mode)
    };

    if base_url.is_empty() {
        return Ok(serde_json::json!({ "updated": 0, "checked": 0 }));
    }

    extensions::validate_store_url(&base_url)?;

    // Gather all installed items
    let mut installed: Vec<(String, String, String)> = Vec::new(); // (id, version, kind)
    let states = {
        let config = features.config.lock_or_recover();
        config.extension_states.clone()
    };

    for kind in &["extension", "theme", "commands"] {
        for item in extensions::discover_items(kind, &states) {
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

    // Fetch the full catalog to get all available versions
    let url = format!("{}/catalog.json", base_url);
    let client = store_client()?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Store request failed: {}", e))?;
    let catalog: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid store response: {}", e))?;

    let catalog_items = catalog
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut updated = 0u32;
    let checked = installed.len() as u32;

    for (id, local_version, _kind) in &installed {
        // Find this item in the catalog
        let catalog_item = catalog_items
            .iter()
            .find(|ci| ci.get("id").and_then(|v| v.as_str()) == Some(id.as_str()));

        if let Some(ci) = catalog_item {
            let remote_version = ci
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.0.0");

            // Compare versions using semver
            let local_sv = semver::Version::parse(local_version).ok();
            let remote_sv = semver::Version::parse(remote_version).ok();

            if let (Some(local), Some(remote)) = (local_sv, remote_sv) {
                if remote > local {
                    info!(
                        "Extension '{}' has update: {} -> {}",
                        id, local_version, remote_version
                    );
                    // Install the update in place. The existing capability
                    // grant is preserved; if the updated manifest requests
                    // more capabilities, the runtime drops them until the
                    // user re-approves. We do emit here because auto-update
                    // is an in-place refresh of already-approved software.
                    match store_install_inner(&base_url, id, &features, &app, true).await {
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
    let mut config = features.config.lock_or_recover();
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
    features: &State<'_, FeatureServices>,
    app: &tauri::AppHandle,
    emit_changed: bool,
) -> Result<extensions::InstalledItem, String> {
    // Resolve the download URL by fetching this id's detail page first.
    // We can't synthesise the zip URL from id alone because the package
    // file name embeds the version (`<id>-<version>.zip`), and we want
    // the published `sha256` for integrity verification.
    let detail_url = format!("{}/detail/{}.json", base_url, id);
    let client = store_client()?;
    let detail: serde_json::Value = client
        .get(&detail_url)
        .send()
        .await
        .map_err(|e| format!("Detail fetch failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Invalid detail JSON: {}", e))?;

    let download_rel = detail
        .get("downloadUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Detail response missing downloadUrl".to_string())?;
    let download_url = if download_rel.starts_with("http") {
        download_rel.to_string()
    } else {
        resolve_relative(base_url, download_rel)
    };
    let expected_sha = detail
        .get("sha256")
        .and_then(|v| v.as_str())
        .map(String::from);

    let zip_path = std::env::temp_dir().join(format!("kage-download-{}.zip", id));

    let resp = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download: {}", e))?;

    if bytes.len() < 4 || &bytes[0..4] != b"PK\x03\x04" {
        return Err("Invalid zip archive".into());
    }

    // Verify checksum if the catalog published one. A mismatch here means
    // the catalog and the zip are out of sync — better to refuse the install
    // than silently load tampered code.
    if let Some(expected) = expected_sha {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(format!(
                "Checksum mismatch for '{}' (expected {}, got {})",
                id, expected, actual
            ));
        }
    }

    std::fs::write(&zip_path, &bytes).map_err(|e| format!("Failed to save download: {}", e))?;

    let item = extensions::install_from_zip(&zip_path)
        .map_err(|e| format!("Installation failed: {}", e))?;

    let _ = std::fs::remove_file(&zip_path);

    let mut config = features.config.lock_or_recover();
    config
        .extension_states
        .insert(item.manifest.id.clone(), true);
    let _ = config.save();
    drop(config);

    if emit_changed {
        if let Err(e) = app.emit(events::EXTENSIONS_CHANGED, ()) {
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
// can be unit-tested without standing up a Tauri AppHandle. The host JS
// bridge force-injects extension_id from its own record before forwarding
// storage commands here, so a sandboxed caller can't spoof a different
// extension's identity.

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
    extensions::resolve_extension_data_path(&root, extension_id, key).map_err(|e| format!("{}", e))
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
    std::fs::write(&path, &data).map_err(|e| {
        format!(
            "Failed to save extension data '{}/{}': {}",
            extension_id, key, e
        )
    })?;
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
        Err(e) => Err(format!(
            "Failed to load extension data '{}/{}': {}",
            extension_id, key, e
        ))?,
    }
}

/// Delete extension data file.
#[tauri::command]
pub async fn delete_extension_data(extension_id: String, key: String) -> Result<(), AppError> {
    let path = resolve_data_path(&extension_id, &key)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "Failed to delete extension data '{}/{}': {}",
            extension_id, key, e
        ))?,
    }
}

// ---------------------------------------------------------------------------
// First-run welcome: batch provisioning
// ---------------------------------------------------------------------------
// Extensions selected on the welcome screen are pulled from the
// configured store URL (production catalog by default). If the user is
// offline at first launch, individual installs will fail and be reported
// in the WelcomeProvisionReport — the user can pick up the rest later
// from the store.

#[derive(Debug, serde::Deserialize)]
pub struct WelcomeExtensionDecision {
    pub id: String,
    /// Whether the user ticked the box on the welcome screen.
    pub checked: bool,
}

#[derive(Debug, serde::Serialize, Default)]
pub struct WelcomeProvisionReport {
    pub installed: u32,
    pub enabled: u32,
    pub disabled: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// Apply the extension decisions the user made on the welcome screen.
///
/// Fire-and-forget from the caller's perspective: the command returns
/// immediately after spawning a blocking task, so the welcome window can
/// transition to a "Launching Kage…" state and close without waiting on
/// local disk I/O.
///
/// The work runs on `spawn_blocking` because it's all synchronous disk
/// I/O (zip extraction, config writes). Putting it on the main Tokio
/// runtime would tie up a thread that other Tauri commands need.
///
/// Individual failures are logged but do not short-circuit the batch —
/// we'd rather enable 4 of 5 extensions than fail atomically on the 3rd.
/// Aggregate counts land in the `first_run_extensions_provisioned`
/// telemetry event; there's no cross-window signal because no caller
/// needs one today.
#[tauri::command]
pub async fn welcome_provision_extensions(
    decisions: Vec<WelcomeExtensionDecision>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    info!(
        "welcome_provision_extensions: dispatching {} decisions to background task",
        decisions.len()
    );

    // Clone the state we need to move into the blocking task. The Tauri
    // State can't cross the spawn boundary directly — it's tied to the
    // command invocation — so we pull the Arc<Mutex<Config>> out and
    // move that instead.
    let config = features.config.clone();
    let app_handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let report = provision_decisions(&decisions, &config, &app_handle);

        info!(
            "welcome_provision: done — installed={} enabled={} disabled={} skipped={} failed={}",
            report.installed, report.enabled, report.disabled, report.skipped, report.failed
        );

        // Aggregate telemetry. Opt-out is already respected inside track().
        crate::telemetry::track(
            &app_handle,
            "first_run_extensions_provisioned",
            Some(serde_json::json!({
                "installed": report.installed,
                "enabled": report.enabled,
                "disabled": report.disabled,
                "skipped": report.skipped,
                "failed": report.failed,
            })),
        );
    });

    Ok(())
}

/// Core provisioning loop — pulled into its own function so the
/// spawn_blocking task in [`welcome_provision_extensions`] can call it
/// with a plain Arc<Mutex<Config>> rather than needing a Tauri State.
fn provision_decisions(
    decisions: &[WelcomeExtensionDecision],
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    app: &tauri::AppHandle,
) -> WelcomeProvisionReport {
    // Snapshot what's already installed so we don't re-stage duplicates.
    let already_installed: std::collections::HashSet<String> = {
        let states = {
            let cfg = config.lock_or_recover();
            cfg.extension_states.clone()
        };
        let mut set = std::collections::HashSet::new();
        for kind in &["extension", "theme"] {
            for item in extensions::discover_items(kind, &states) {
                set.insert(item.manifest.id);
            }
        }
        set
    };

    let mut report = WelcomeProvisionReport::default();

    for decision in decisions {
        if decision.checked && !already_installed.contains(&decision.id) {
            match install_and_commit_direct(config, app, &decision.id) {
                Ok(installed_id) => {
                    report.installed += 1;
                    info!("welcome_provision: installed '{}'", installed_id);
                }
                Err(e) => {
                    report.failed += 1;
                    warn!("welcome_provision: install '{}' failed: {}", decision.id, e);
                }
            }
        } else {
            report.skipped += 1;
        }
    }

    report
}

/// Install + commit in one go for the welcome flow.
///
/// Fetches the extension package from the configured store URL
/// (`https://nachmore.github.io/Kage-Extensions/` by default), verifies
/// the SHA-256 against the catalog, extracts to the user's install dir,
/// and commits the capability grant. If the user is offline at first
/// launch, individual installs fail and surface in the
/// `WelcomeProvisionReport`; the user can install the rest later from
/// the store window.
fn install_and_commit_direct(
    config: &std::sync::Arc<std::sync::Mutex<crate::config::Config>>,
    app: &tauri::AppHandle,
    id: &str,
) -> Result<String, String> {
    extensions::validate_extension_id(id).map_err(|e| format!("Invalid extension id: {}", e))?;

    // Resolve the store URL the same way the live install path does.
    let (base_url, dev_mode) = {
        let cfg = config.lock_or_recover();
        let dev = std::env::var("KAGE_DEV").is_ok() || cfg!(debug_assertions);
        (resolve_store_url(&cfg, dev), dev)
    };
    let _ = dev_mode; // currently unused beyond the resolve call above
    if base_url.is_empty() {
        return Err("No store URL configured".to_string());
    }
    extensions::validate_store_url(&base_url).map_err(|e| format!("Bad store URL: {}", e))?;

    // Run the network install + extract on the runtime via block_on.
    let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
        format!(
            "No tokio runtime available for welcome install of '{}': {}",
            id, e
        )
    })?;

    // Fetch detail + zip + verify hash + extract.
    let item = runtime.block_on(welcome_store_install(&base_url, id))?;

    let raw_perms: Vec<String> = item.manifest.permissions.clone().unwrap_or_default();
    let granted = extensions::normalize_permissions(&raw_perms, &item.manifest.id);
    let approved_version = item.manifest.version.clone();
    let installed_id = item.manifest.id.clone();

    {
        let mut cfg = config.lock_or_recover();
        cfg.extension_states.insert(installed_id.clone(), true);
        cfg.extension_grants.insert(
            installed_id.clone(),
            crate::config::ExtensionGrant {
                granted: granted.clone(),
                approved_version,
                approved_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        cfg.save()
            .map_err(|e| format!("Failed to save config: {}", e))?;
    }

    crate::telemetry::track(
        app,
        "extension_installed",
        Some(serde_json::json!({
            "extension_id": installed_id,
            "source": "welcome_network",
        })),
    );

    if let Err(e) = app.emit(events::EXTENSIONS_CHANGED, ()) {
        error!("Failed to emit extensions_changed: {}", e);
    }

    Ok(installed_id)
}

/// Welcome-flow variant of the store install: fetches detail, downloads
/// the zip, verifies the published sha256, and extracts. Doesn't touch
/// the config — that's the caller's job (so we can fold the grant write
/// into the same critical section as the enable flag).
async fn welcome_store_install(
    base_url: &str,
    id: &str,
) -> Result<extensions::InstalledItem, String> {
    let client = store_client()?;
    let detail_url = format!("{}/detail/{}.json", base_url, id);
    let detail: serde_json::Value = client
        .get(&detail_url)
        .send()
        .await
        .map_err(|e| format!("Detail fetch failed: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Invalid detail JSON: {}", e))?;

    let download_rel = detail
        .get("downloadUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Detail response missing downloadUrl".to_string())?;
    let download_url = if download_rel.starts_with("http") {
        download_rel.to_string()
    } else {
        resolve_relative(base_url, download_rel)
    };
    let expected_sha = detail
        .get("sha256")
        .and_then(|v| v.as_str())
        .map(String::from);

    let bytes = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download: {}", e))?;
    if bytes.len() < 4 || &bytes[0..4] != b"PK\x03\x04" {
        return Err("Invalid zip archive".into());
    }
    if let Some(expected) = expected_sha {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(format!(
                "Checksum mismatch for '{}' (expected {}, got {})",
                id, expected, actual
            ));
        }
    }

    let zip_path = std::env::temp_dir().join(format!("kage-welcome-{}.zip", id));
    std::fs::write(&zip_path, &bytes).map_err(|e| format!("Failed to save download: {}", e))?;
    let item = extensions::install_from_zip(&zip_path)
        .map_err(|e| format!("Installation failed: {}", e))?;
    let _ = std::fs::remove_file(&zip_path);
    Ok(item)
}

// ---------------------------------------------------------------------------
// Extension capability grants
// ---------------------------------------------------------------------------

/// Remove a recorded grant (e.g. when uninstalling an extension).
#[tauri::command]
pub async fn remove_extension_grant(
    extension_id: String,
    features: State<'_, FeatureServices>,
) -> Result<(), AppError> {
    extensions::validate_extension_id(&extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    let mut config = features.config.lock_or_recover();
    if config.extension_grants.remove(&extension_id).is_some() {
        info!("Removed capability grant for '{}'", extension_id);
        config
            .save()
            .map_err(|e| format!("Failed to save config: {}", e))?;
    }
    Ok(())
}
