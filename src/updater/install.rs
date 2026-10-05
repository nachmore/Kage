use crate::lock_ext::LockExt;
use anyhow::Result;
#[cfg(target_os = "macos")]
use log::error;
use log::{info, warn};
use tauri::Manager;
use tauri_plugin_updater::Update;

/// Download, verify, and install a previously checked update.
pub async fn plugin_download_and_install<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    update: Update,
) -> Result<()> {
    info!(
        "Downloading update v{} (body: {:?})",
        update.version, update.body
    );
    // Split download from install: the plugin's `download` fires its
    // finish callback *before* verifying the signature, so tearing down
    // there left a corrupt download with a headless, disconnected app.
    // `download` returns only verified bytes, so teardown happens after.
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(error) => return Err(install_failed(app, &update, &error)),
    };

    info!("Update downloaded and verified, starting installer");
    let visible_windows: Vec<&str> = RESTORABLE_WINDOWS
        .into_iter()
        .filter(|label| {
            app.get_webview_window(label)
                .is_some_and(|window| window.is_visible().unwrap_or(false))
        })
        .collect();
    // Order matters: explicit child cleanup while the Job Object still
    // safety-nets, THEN release the kill flag so the installer survives us.
    crate::commands::system::graceful_shutdown(app);
    if let Some(acp) = app.try_state::<crate::state::AcpHandles>() {
        acp.client.disconnect();
    }
    crate::os::release_kill_on_exit_job();
    crate::app_log::flush();

    if let Err(error) = update.install(bytes) {
        restore_after_failed_install(app, &visible_windows);
        return Err(install_failed(app, &update, &error));
    }
    Ok(())
}

/// Windows hidden by `graceful_shutdown` that we re-show if install fails
/// (only those that were visible beforehand).
const RESTORABLE_WINDOWS: [&str; 3] = [
    crate::window_labels::FLOATING,
    crate::window_labels::MAIN,
    crate::window_labels::SETTINGS,
];

/// Undo the visible parts of the pre-install teardown so the user can see
/// the error and keep using the app. The agent stays disconnected (the next
/// send / reconnect re-spawns it).
fn restore_after_failed_install<R: tauri::Runtime>(app: &tauri::AppHandle<R>, windows: &[&str]) {
    warn!("Update install failed after teardown; restoring UI");
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_visible(true);
    }
    for label in windows {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.show();
        }
    }
}

/// Record a failed download/install and roll back the relaunch markers both
/// callers wrote beforehand, so the next ordinary launch doesn't resume a
/// stale session or show the post-update banner for an update that never
/// landed.
fn install_failed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    update: &Update,
    error: &tauri_plugin_updater::Error,
) -> anyhow::Error {
    super::markers::clear_install_markers();
    if let Some(features) = app.try_state::<crate::state::FeatureServices>() {
        let mut cfg = features.config.lock_or_recover();
        if cfg.updates.last_updated_version.as_deref() == Some(update.version.as_str()) {
            cfg.updates.last_updated_version = None;
            if let Err(save_error) = cfg.save() {
                warn!("Failed to save config (update rollback): {save_error}");
            }
        }
    }
    let reason = classify_install_error(error);
    crate::telemetry::track(
        app,
        "update_install_failed",
        Some(serde_json::json!({ "reason": reason })),
    );
    format_install_error(error, reason)
}

/// Stable telemetry category for an installer failure.
pub fn classify_install_error(error: &tauri_plugin_updater::Error) -> &'static str {
    let message = error.to_string().to_lowercase();
    if ["signature", "verify", "public key", "minisign"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        "signature"
    } else if message.contains("403") || message.contains("forbidden") {
        "forbidden"
    } else if message.contains("404") || message.contains("not found") {
        "not_found"
    } else if ["disk", "space", "os error 112", "os error 28"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        "disk_full"
    } else if ["denied", "permission", "os error 5", "os error 13"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        "permission"
    } else if ["dns", "connect", "network", "timeout", "transport"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        "network"
    } else if message.contains("cancel") || message.contains("interrupt") {
        "cancelled"
    } else {
        "other"
    }
}

fn format_install_error(error: &tauri_plugin_updater::Error, reason: &str) -> anyhow::Error {
    let detail = error.to_string();
    let message = match reason {
        "signature" => "Update signature didn't verify. The download may be corrupted; try again.",
        "forbidden" => "Server refused the download (HTTP 403). If you're behind a proxy or filter, that's the most likely cause.",
        "not_found" => "Update file is missing on the server (HTTP 404). The release may have been pulled - try again later or check the channel in Settings -> Updates.",
        "disk_full" => "Not enough disk space to download or install the update.",
        "permission" => "Kage doesn't have permission to write the installer file. Close any antivirus / EDR holding the directory and try again.",
        "network" => "Network error while downloading the update. Check your connection and try again.",
        "cancelled" => "Update was cancelled.",
        _ => "Update install failed.",
    };
    anyhow::anyhow!("{message} ({detail})")
}

#[cfg(target_os = "macos")]
pub fn relaunch_and_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            error!("Cannot resolve exe path for relaunch: {error}");
            app.exit(0);
            return;
        }
    };
    let bundle = executable
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent());
    if let Some(bundle) = bundle {
        info!("Relaunching from bundle: {bundle:?}");
        let _ = std::process::Command::new("open")
            .arg("-a")
            .arg(bundle)
            .arg("--args")
            .arg("--restart")
            .spawn();
    } else {
        warn!("Could not resolve .app bundle path; spawning exe directly");
        let _ = std::process::Command::new(&executable)
            .arg("--restart")
            .spawn();
    }
    app.exit(0);
}

#[cfg(not(target_os = "macos"))]
pub fn relaunch_and_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    app.exit(0);
}
