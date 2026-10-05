use crate::lock_ext::LockExt;
use anyhow::Result;
#[cfg(target_os = "macos")]
use log::error;
use log::{info, warn};
use tauri::{Emitter, Manager};
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
    let visible_windows: Vec<&'static str> = RESTORABLE_WINDOWS
        .into_iter()
        .filter(|label| {
            app.get_webview_window(label)
                .is_some_and(|window| window.is_visible().unwrap_or(false))
        })
        .collect();
    // Snapshot what teardown is about to stop, so a failed install restores
    // exactly that (and doesn't start anything the user had stopped).
    let mut teardown = Teardown {
        windows: visible_windows,
        agent_was_connected: false,
        tts_was_running: false,
    };
    if let Some(acp) = app.try_state::<crate::state::AcpHandles>() {
        teardown.agent_was_connected = acp.client.is_connected();
    }
    if let Some(procs) = app.try_state::<crate::state::ChildProcesses>() {
        teardown.tts_was_running = procs.pocket_tts.lock_or_recover().is_some();
    }
    // Order matters: explicit child cleanup while the Job Object still
    // safety-nets, THEN release the kill flag so the installer survives us.
    crate::commands::system::graceful_shutdown(app);
    if let Some(acp) = app.try_state::<crate::state::AcpHandles>() {
        acp.client.disconnect();
    }
    crate::os::release_kill_on_exit_job();
    crate::app_log::flush();

    if let Err(error) = update.install(bytes) {
        restore_after_failed_install(app, &teardown);
        return Err(install_failed(app, &update, &error));
    }
    Ok(())
}

/// What the pre-install teardown stopped, captured just before it ran.
struct Teardown {
    windows: Vec<&'static str>,
    agent_was_connected: bool,
    tts_was_running: bool,
}

/// Windows hidden by `graceful_shutdown` that we re-show if install fails
/// (only those that were visible beforehand).
const RESTORABLE_WINDOWS: [&str; 3] = [
    crate::window_labels::FLOATING,
    crate::window_labels::MAIN,
    crate::window_labels::SETTINGS,
];

/// Undo the pre-install teardown so the user can see the error and keep
/// using the app: re-arm the Job Object, re-show the tray and windows, and
/// bring back the Pocket TTS server and agent connection if they were up.
/// An in-flight Pocket TTS pip install that teardown cancelled stays
/// cancelled; the user can re-run it from Settings.
fn restore_after_failed_install<R: tauri::Runtime>(app: &tauri::AppHandle<R>, teardown: &Teardown) {
    warn!("Update install failed after teardown; restoring app state");
    // First, so the children respawned below are reaped with us again if
    // we later crash (teardown released the flag for the installer).
    crate::os::rearm_kill_on_exit_job();
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_visible(true);
    }
    for label in &teardown.windows {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.show();
        }
    }
    if teardown.tts_was_running {
        crate::setup::spawn_pocket_tts_server(app);
    }
    if teardown.agent_was_connected {
        reconnect_agent(app);
    }
}

/// Re-spawn the agent that teardown disconnected. Done eagerly rather than
/// on the next send because `disconnect` bumped the transport generation,
/// so no `agent_disconnected` was emitted and windows still show the agent
/// as connected.
fn reconnect_agent<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(acp) = app.try_state::<crate::state::AcpHandles>() else {
        return;
    };
    let client = acp.client.clone();
    let app = app.clone();
    // connect() is a blocking spawn + handshake; keep it off async workers.
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = client.connect() {
            warn!("Agent reconnect after failed update install failed: {error}");
            // Tell windows now rather than leaving a stale "connected"
            // header (same reasoning as the config-change reconnect).
            if let Err(error) = app.emit(crate::events::AGENT_DISCONNECTED, ()) {
                warn!("Failed to emit agent_disconnected event: {error}");
            }
        } else {
            info!("Agent reconnected after failed update install");
        }
    });
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
