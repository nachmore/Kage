use anyhow::Result;
use log::info;
#[cfg(target_os = "macos")]
use log::{error, warn};
use tauri::Manager;
use tauri_plugin_updater::Update;

/// Download, verify, and install a previously checked update.
pub async fn plugin_download_and_install(app: &tauri::AppHandle, update: Update) -> Result<()> {
    info!(
        "Downloading update v{} (body: {:?})",
        update.version, update.body
    );
    let app_for_finish = app.clone();
    let result = update
        .download_and_install(
            |_, _| {},
            move || {
                info!("Update downloaded, starting installer");
                crate::commands::system::graceful_shutdown(&app_for_finish);
                if let Some(acp) = app_for_finish.try_state::<crate::state::AcpHandles>() {
                    acp.client.disconnect();
                }
                crate::os::release_kill_on_exit_job();
                crate::app_log::flush();
            },
        )
        .await;
    if let Err(error) = result {
        let reason = classify_install_error(&error);
        crate::telemetry::track(
            app,
            "update_install_failed",
            Some(serde_json::json!({ "reason": reason })),
        );
        return Err(format_install_error(&error, reason));
    }
    Ok(())
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
pub fn relaunch_and_exit(app: &tauri::AppHandle) {
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
pub fn relaunch_and_exit(app: &tauri::AppHandle) {
    app.exit(0);
}
