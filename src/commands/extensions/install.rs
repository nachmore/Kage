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

#[tauri::command]
pub async fn uninstall_extension<R: tauri::Runtime>(
    id: String,
    kind: String,
    features: State<'_, FeatureServices>,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    // extensions::uninstall validates internally too — checking here as well
    // keeps this command's error message specific (frontend gets a clean
    // "invalid extension id" rather than a generic "uninstall failed").
    extensions::validate_extension_id(&id).map_err(|e| format!("Invalid extension id: {}", e))?;

    // A declined upgrade prompt rolls back through here. Drop only the
    // parked files: the previous version, its settings, grant and stored
    // data all stay, and nothing loaded changed so there's nothing to emit.
    if extensions::discard_pending(&id, &kind).map_err(|e| format!("Uninstall failed: {}", e))? {
        return Ok(());
    }

    extensions::uninstall(&id, &kind).map_err(|e| format!("Uninstall failed: {}", e))?;

    // Remove from enabled states and extension config
    let mut config = features.config.lock_or_recover();
    config.extension_states.remove(&id);
    config.extensions.remove(&id);
    config.extension_grants.remove(&id);
    let _ = config.save();
    drop(config);

    // Stored data is keyed only by id, so leaving it would hand an old
    // extension's tokens to any later extension installed under the same id.
    // Safe now that a declined upgrade returns above instead of landing here.
    if let Err(e) = super::files::purge_extension_data(&id) {
        warn!("Failed to remove stored data for '{}': {}", id, e);
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
