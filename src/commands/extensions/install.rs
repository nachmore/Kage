//! Local install/uninstall, install commit, and capability-grant removal.
//! Network-backed store installs live in [`super::store`]; the welcome-screen
//! batch path lives in [`super::welcome`].

use super::*;

/// Stage a manual install from a local zip or directory path.
/// Like `store_install`, this does NOT emit `extensions_changed` — the
/// caller must show the permission prompt and call
/// `commit_extension_install` before the extension will load.
#[tauri::command]
pub async fn install_extension_from_path<R: tauri::Runtime>(
    source_path: String,
    features: State<'_, FeatureServices>,
    _app: tauri::AppHandle<R>,
) -> Result<extensions::InstalledItem, AppError> {
    let source = std::path::PathBuf::from(&source_path);
    // An upgrade of an installed item is parked until the prompt resolves,
    // so declining it keeps the working version.
    let mode = extensions::InstallMode::DeferUpgrade;

    let installed = if source.extension().map(|e| e == "zip").unwrap_or(false) {
        // Install from zip file
        extensions::install_from_zip(&source, mode)
            .map_err(|e| format!("Installation failed: {}", e))?
    } else {
        // Install from directory
        extensions::install_from_directory(&source, mode)
            .map_err(|e| format!("Installation failed: {}", e))?
    };

    // Mark a fresh install enabled so the commit step can flip the grant
    // and load it. A parked upgrade is enabled by the commit itself.
    if !installed.deferred {
        let mut config = features.config.lock_or_recover();
        record_install_state(
            &mut config.extension_states,
            &installed.item.manifest.id,
            mode,
        );
        let _ = config.save();
        drop(config);
    }

    Ok(installed.item)
}

/// Uninstall an item. `rollback` is set only by a declined (or failed)
/// install prompt; without it this is always a real uninstall, so a parked
/// upgrade left behind by an abandoned prompt can't swallow one.
#[tauri::command]
pub async fn uninstall_extension<R: tauri::Runtime>(
    id: String,
    kind: String,
    rollback: Option<bool>,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    // extensions::uninstall validates internally too — checking here as well
    // keeps this command's error message specific (frontend gets a clean
    // "invalid extension id" rather than a generic "uninstall failed").
    extensions::validate_extension_id(&id).map_err(|e| format!("Invalid extension id: {}", e))?;

    if rollback.unwrap_or(false) {
        // A declined upgrade: drop only the parked files. The previous
        // version, its settings, grant and stored data all stay, and nothing
        // loaded changed so there's nothing to emit.
        if extensions::discard_pending(&id, &kind)
            .map_err(|e| format!("Uninstall failed: {}", e))?
        {
            return Ok(());
        }
        // Nothing parked, yet the item was approved before this stage: its
        // parked copy was superseded (e.g. an auto-update swapped a version
        // in while the prompt was open). Declining must not uninstall it.
        if features
            .config
            .lock_or_recover()
            .extension_grants
            .contains_key(&id)
        {
            info!(
                "Declined update for '{}' has no parked copy; keeping the installed version",
                id
            );
            return Ok(());
        }
        // Otherwise this was a fresh install: roll it back fully below.
    }

    // Its view windows go first, so none is left showing files that are
    // about to be deleted.
    if kind == "extension" {
        crate::ext_views::close_for_extension(&app, &id);
    }

    extensions::uninstall(&id, &kind).map_err(|e| format!("Uninstall failed: {}", e))?;

    // Remove from enabled states and extension config
    let mut config = features.config.lock_or_recover();
    config.extension_states.remove(&id);
    config.extensions.remove(&id);
    config.extension_grants.remove(&id);
    // Background AI consent named this install; a later one asks again.
    config.extension_agent_access.remove(&id);
    let _ = config.save();
    drop(config);
    // Its background agent calls end (`cancelled_access_off`).
    features.agent_calls.on_extension_changed(&id);

    // Stored data is keyed only by id, so leaving it would hand an old
    // extension's tokens to any later extension installed under the same id.
    // Safe now that a declined upgrade of an approved item returns above.
    if let Err(e) = super::files::purge_extension_data(&app, &id) {
        warn!("Failed to remove stored data for '{}': {}", id, e);
    }
    // Saved view window positions go with it (they're keyed by id too).
    if kind == "extension" {
        if let Err(e) = crate::ext_views::geometry::remove_entries(&id, None) {
            warn!("Failed to remove view window positions for '{}': {}", id, e);
        }
    }

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

/// Finalize a staged install: save the user-approved capability grant and
/// emit `extensions_changed` so the loader picks it up.
///
/// Call this only after the user has approved the extension's capability
/// set via the install-time permission prompt. Granting without user
/// consent is a direct security violation.
#[tauri::command]
pub async fn commit_extension_install<R: tauri::Runtime>(
    extension_id: String,
    granted: Vec<String>,
    approved_version: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
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

    // Apply a parked upgrade before recording the grant, so a failed swap
    // leaves the old version and its grant exactly as they were.
    let upgraded = extensions::commit_pending(&extension_id)
        .map_err(|e| format!("Installation failed: {}", e))?;

    let mut config = features.config.lock_or_recover();
    if upgraded {
        // The user approved this (re)install, so enable it — the same thing
        // staging does for a fresh install.
        config.extension_states.insert(extension_id.clone(), true);
    }
    let record = crate::config::ExtensionGrant {
        granted,
        approved_version,
        approved_at: chrono::Utc::now().to_rfc3339(),
        origin: None,
    };
    info!(
        "Committing install for '{}': capabilities {:?}",
        extension_id, record.granted
    );
    config
        .extension_grants
        .insert(extension_id.clone(), record.clone());
    let states = config.extension_states.clone();
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    drop(config);

    // A grant (or an updated manifest) without `agent_background` or its
    // `agentBackground` block ends the extension's background agent calls.
    features.agent_calls.on_extension_changed(&extension_id);

    // A grant (or an updated manifest) without `views` closes the
    // extension's view windows. With `views` still held they stay, and
    // their pages reload in place on `extensions_changed`.
    let holds_views = extensions::discover_items("extension", &states)
        .iter()
        .find(|i| i.manifest.id == extension_id)
        .is_some_and(|i| crate::ext_views::holds_views(&i.manifest, Some(&record)));
    if !holds_views {
        crate::ext_views::close_for_extension(&app, &extension_id);
    }

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

/// Remove a recorded grant (e.g. when uninstalling an extension). Its
/// view windows close: without a grant it holds no `views`. Its background
/// AI access goes with the grant, and its calls end.
#[tauri::command]
pub async fn remove_extension_grant<R: tauri::Runtime>(
    extension_id: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    extensions::validate_extension_id(&extension_id)
        .map_err(|e| format!("Invalid extension id: {}", e))?;

    let mut config = features.config.lock_or_recover();
    let had_grant = config.extension_grants.remove(&extension_id).is_some();
    let had_access = config
        .extension_agent_access
        .remove(&extension_id)
        .is_some();
    if had_grant || had_access {
        info!("Removed capability grant for '{}'", extension_id);
        config
            .save()
            .map_err(|e| format!("Failed to save config: {}", e))?;
    }
    drop(config);
    features.agent_calls.on_extension_changed(&extension_id);
    crate::ext_views::close_for_extension(&app, &extension_id);
    Ok(())
}
