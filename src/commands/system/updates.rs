//! Auto-update commands. Thin wrappers around `crate::updater::*` —
//! the heavy lifting (manifest fetch, signature verification, install,
//! relaunch) lives in the `tauri-plugin-updater` plugin and our
//! `updater` module's scheduling layer. See `docs/RELEASE.md`.

use crate::error::{AppError, ErrorKind};
use crate::events;
use crate::lock_ext::LockExt;
use crate::state::{AcpHandles, FeatureServices, UiState};
use crate::window_labels;
use tauri::State;

#[tauri::command]
pub async fn check_for_update(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    let channel = features.config.lock_or_recover().updates.channel;

    let result = crate::updater::plugin_check(&app, channel)
        .await
        .map_err(|e| format!("Check failed: {}", e))?;

    let available = result.as_ref().map(|u| u.version.clone());

    // Cache the Update handle so download_and_install_update can
    // consume it without re-checking.
    if let Some(update) = result {
        if let Ok(mut v) = features.updater.available_version.lock() {
            *v = Some(update.version.clone());
        }
        if let Ok(mut p) = features.updater.pending_update.lock() {
            *p = Some(update);
        }
        features
            .updater
            .update_ready
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    // Tell chat hosts, floating, and settings — they all show
    // update-available indicators. inline-assist / store / welcome
    // / context-menu don't subscribe.
    if let Some(ref version) = available {
        crate::event_targets::emit_update_audience(&app, events::UPDATE_AVAILABLE, version);
    }

    Ok(serde_json::json!({
        "current_version": crate::updater::CURRENT_VERSION,
        "available_version": available,
    }))
}

#[tauri::command]
pub async fn fetch_changelog(features: State<'_, FeatureServices>) -> Result<String, AppError> {
    // Channel scopes prereleases — stable users see only published
    // releases, beta/dev see prereleases too. Read here rather than
    // baking it into the updater module so the command stays the
    // single channel-aware caller.
    let channel = features.config.lock_or_recover().updates.channel;
    Ok(
        tauri::async_runtime::spawn_blocking(move || crate::updater::fetch_changelog(channel))
            .await
            .map_err(|e| format!("Task error: {}", e))?
            .map_err(|e| format!("Fetch failed: {}", e))?,
    )
}

#[tauri::command]
pub async fn get_update_urls(
    features: State<'_, FeatureServices>,
) -> Result<serde_json::Value, AppError> {
    let channel = features.config.lock_or_recover().updates.channel;
    Ok(serde_json::json!({
        "channel": channel.as_str(),
        "endpoint": crate::updater::endpoint_for_channel(channel),
        "changelog_url": crate::updater::CHANGELOG_URL,
    }))
}

#[tauri::command]
pub async fn download_and_install_update(
    app: tauri::AppHandle,
    features: State<'_, FeatureServices>,
    ui: State<'_, UiState>,
    _acp: State<'_, AcpHandles>,
) -> Result<(), AppError> {
    // Prefer the Update cached from a previous check_for_update call —
    // it might carry channel-specific metadata the plugin would need to
    // re-fetch otherwise. Fall back to a fresh check if nothing cached.
    let update = {
        let mut slot = features.updater.pending_update.lock_or_recover();
        slot.take()
    };
    let update = if let Some(u) = update {
        u
    } else {
        let channel = features.config.lock_or_recover().updates.channel;
        crate::updater::plugin_check(&app, channel)
            .await
            .map_err(|e| format!("Check failed: {}", e))?
            .ok_or_else(|| {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.update.no_update_available",
                    &[],
                )
            })?
    };

    // Stamp last_updated_version so the post-restart launch can show
    // the "welcome back after update" banner. Same story as the idle
    // path in start_update_loop.
    {
        let mut cfg = features.config.lock_or_recover();
        if let Ok(v) = features.updater.available_version.lock() {
            cfg.updates.last_updated_version = v.clone();
        }
        let _ = cfg.save();
    }

    // Write the resume marker so the relaunch restores the session.
    // Prefer floating's session (post-update banner shows the floating
    // window first); fall back to main's session if floating has none.
    let session_id = ui.window_sessions.lock().ok().and_then(|m| {
        m.get(window_labels::FLOATING)
            .cloned()
            .or_else(|| m.get(window_labels::MAIN).cloned())
    });
    crate::updater::persist_resume_marker(session_id.as_deref());
    // Tag this install as user-initiated so the post-install launch
    // shows the floating window with the celebration banner. The idle
    // path writes `Idle` instead, leaving the floating window hidden
    // until the user manually summons it.
    crate::updater::persist_install_source(crate::updater::InstallSource::Interactive);

    // Bubble the plugin error up verbatim — `plugin_download_and_install`
    // now classifies failures (signature / network / disk full /
    // permission / 403 / 404 / cancelled / other) and produces a
    // user-readable string. Wrapping with another "Install failed:"
    // here would double up by the time it reaches the UI.
    crate::updater::plugin_download_and_install(&app, update)
        .await
        .map_err(|e| AppError::from(e.to_string()))?;

    // On Windows the plugin called process::exit(0) inside
    // download_and_install and we never reached this line. On macOS
    // it returned cleanly after swapping the .app on disk; relaunch
    // into the new binary seamlessly.
    crate::updater::relaunch_and_exit(&app);
    Ok(())
}

#[tauri::command]
pub async fn was_just_updated(features: State<'_, FeatureServices>) -> Result<bool, AppError> {
    let config = features.config.lock_or_recover();
    Ok(crate::updater::was_just_updated(&config))
}

#[tauri::command]
pub async fn clear_update_flag(features: State<'_, FeatureServices>) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    crate::updater::clear_update_flag(&mut config);
    Ok(config
        .save()
        .map_err(|e| format!("Failed to save: {}", e))?)
}

#[tauri::command]
pub async fn touch_floating_activity(features: State<'_, FeatureServices>) -> Result<(), AppError> {
    features.updater.touch_activity();
    Ok(())
}
