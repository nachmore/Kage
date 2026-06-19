//! Store window plus the catalog/detail/install HTTP surface.
//!
//! Wire format (schema v1) — the store is plain static JSON files served
//! from `https://nachmore.github.io/Kage-Extensions/` (or any HTTPS host
//! matching the same layout):
//!
//!   GET <base>/catalog.json
//!     {
//!       "schemaVersion": 1,
//!       "generatedAt": "...",
//!       "items": [{
//!         "id", "type", "name", "version", "author", "description", "icon",
//!         "tags": [...], "permissions": [...],
//!         "downloadUrl": "packages/<id>-<version>.zip",   (relative to base)
//!         "detailUrl":   "detail/<id>.json",              (relative to base)
//!         "size", "sha256", "sourceHash", "updatedAt"
//!       }]
//!     }
//!
//!   GET <base>/detail/<id>.json   — same fields plus `manifest` and `readme`
//!   GET <base>/packages/<id>-<version>.zip
//!
//! Pagination is client-side: the catalog is small enough to fetch in one
//! shot, and a single round-trip is friendlier to GitHub Pages caching.
//!
//! The base-URL resolution and HTTP client helpers (`resolve_store_url`,
//! `store_client`, `resolve_relative`) live in the parent module so the
//! welcome-flow installer can share them.

use super::*;

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
                    // user re-approves.
                    //
                    // Crucially we pass emit_changed=false here and fire a
                    // SINGLE extensions_changed AFTER the whole loop. Emitting
                    // per-extension means N updates trigger N reloads in every
                    // window; those reloads are async and reenter
                    // (`sandbox already loaded`, duplicate installs), and each
                    // one re-mounts widgets that spawn OS processes (calendar's
                    // PowerShell, app-scan). A batch of 9 updates melted down a
                    // user's machine into process-exhaustion this way.
                    match store_install_inner(&base_url, id, &features, &app, false).await {
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

    // Emit a SINGLE refresh after all updates land (see the emit_changed=false
    // note in the loop). One reload per window picks up every updated
    // extension at once instead of a reload storm.
    if updated > 0 {
        if let Err(e) = app.emit(events::EXTENSIONS_CHANGED, ()) {
            error!("Failed to emit extensions_changed: {}", e);
        }
    }

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
