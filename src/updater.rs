//! Auto-update system.
//!
//! Checks for new versions, downloads installers, and applies updates
//! when the user is idle (floating window not shown for 5+ minutes).
//!
//! Update URLs are compiled in from [package.metadata.update] in Cargo.toml.

use crate::config::Config;
use anyhow::{Context, Result};
use log::{error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::Emitter;

/// Compile-time update URLs from Cargo.toml [package.metadata.update]
pub const VERSION_URL: &str = env!("UPDATE_VERSION_URL");
pub const INSTALLER_URL: &str = env!("UPDATE_INSTALLER_URL");
pub const CHANGELOG_URL: &str = env!("UPDATE_CHANGELOG_URL");
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Shared state for the updater
pub struct UpdaterState {
    /// Timestamp of the last time the floating window was shown
    pub last_floating_activity: std::sync::Mutex<Instant>,
    /// Whether an update has been downloaded and is ready to install
    pub update_ready: AtomicBool,
    /// Path to the downloaded installer
    pub installer_path: std::sync::Mutex<Option<String>>,
    /// The new version available
    pub available_version: std::sync::Mutex<Option<String>>,
}

impl UpdaterState {
    pub fn new() -> Self {
        Self {
            last_floating_activity: std::sync::Mutex::new(Instant::now()),
            update_ready: AtomicBool::new(false),
            installer_path: std::sync::Mutex::new(None),
            available_version: std::sync::Mutex::new(None),
        }
    }

    /// Record that the floating window was just shown
    pub fn touch_activity(&self) {
        if let Ok(mut t) = self.last_floating_activity.lock() {
            *t = Instant::now();
        }
    }

    /// Check if the user has been idle (no floating window activity) for 5+ minutes
    pub fn is_idle(&self) -> bool {
        self.last_floating_activity
            .lock()
            .map(|t| t.elapsed().as_secs() >= 300)
            .unwrap_or(false)
    }
}

/// Check if a newer version is available.
/// Returns Some(version_string) if newer, None otherwise.
pub fn check_for_update() -> Result<Option<String>> {
    if VERSION_URL.is_empty() {
        return Ok(None);
    }

    info!("Checking for updates at {}", VERSION_URL);

    let response = reqwest::blocking::Client::new()
        .get(VERSION_URL)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .context("Failed to fetch version")?
        .text()
        .context("Failed to read version response")?;

    let remote_version = response.trim();
    info!("Remote version: {}, current: {}", remote_version, CURRENT_VERSION);

    let remote = semver::Version::parse(remote_version)
        .with_context(|| format!("Invalid remote version: {}", remote_version))?;
    let current = semver::Version::parse(CURRENT_VERSION)
        .with_context(|| format!("Invalid current version: {}", CURRENT_VERSION))?;

    if remote > current {
        info!("Update available: {} -> {}", CURRENT_VERSION, remote_version);
        Ok(Some(remote_version.to_string()))
    } else {
        info!("Already up to date (current: {}, remote: {})", CURRENT_VERSION, remote_version);
        Ok(None)
    }
}

/// Download the installer to a temp file. Returns the path.
pub fn download_installer() -> Result<String> {
    if INSTALLER_URL.is_empty() {
        anyhow::bail!("No installer URL configured");
    }

    info!("Downloading installer from {}", INSTALLER_URL);

    let response = reqwest::blocking::Client::new()
        .get(INSTALLER_URL)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .context("Failed to download installer")?;

    let bytes = response.bytes().context("Failed to read installer bytes")?;

    let download_dir = dirs::cache_dir()
        .or_else(|| dirs::home_dir())
        .context("Failed to get cache directory")?
        .join("kiro-assistant");

    std::fs::create_dir_all(&download_dir)?;

    let ext = if cfg!(windows) { ".exe" } else if cfg!(target_os = "macos") { ".dmg" } else { ".AppImage" };
    let installer_path = download_dir.join(format!("kiro-assistant-update{}", ext));

    std::fs::write(&installer_path, &bytes)
        .context("Failed to write installer")?;

    info!("Installer downloaded to {:?} ({} bytes)", installer_path, bytes.len());
    Ok(installer_path.to_string_lossy().to_string())
}

/// Fetch the changelog markdown (first 10KB).
pub fn fetch_changelog() -> Result<String> {
    if CHANGELOG_URL.is_empty() {
        return Ok("No changelog URL configured.".to_string());
    }

    let response = reqwest::blocking::Client::new()
        .get(CHANGELOG_URL)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .context("Failed to fetch changelog")?
        .text()
        .context("Failed to read changelog")?;

    // Limit to first 10KB
    let truncated = if response.len() > 10240 {
        let mut end = 10240;
        // Don't cut in the middle of a UTF-8 char
        while end > 0 && !response.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n\n---\n*Changelog truncated. Full version available online.*", &response[..end])
    } else {
        response
    };

    Ok(truncated)
}

/// Run the installer silently and exit.
/// On Windows: runs NSIS installer with /S flag.
/// On macOS: opens the .dmg.
/// On Linux: makes the AppImage executable and runs it.
pub fn run_installer_and_exit(installer_path: &str, session_id: Option<&str>) -> Result<()> {
    info!("Running installer: {}", installer_path);

    // Write session ID to the lock file so the new instance can resume
    if let Some(sid) = session_id {
        if let Ok(lock_dir) = dirs::config_dir().context("config dir") {
            let session_file = lock_dir.join("kiro-assistant").join("last-session.txt");
            let _ = std::fs::write(&session_file, sid);
            info!("Wrote session ID to {:?}", session_file);
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        use std::os::windows::process::CommandExt;
        Command::new(installer_path)
            .arg("/S")
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .context("Failed to run installer")?;
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("open")
            .arg(installer_path)
            .spawn()
            .context("Failed to open installer")?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(installer_path, std::fs::Permissions::from_mode(0o755));
        Command::new(installer_path)
            .spawn()
            .context("Failed to run installer")?;
    }

    // Give the installer a moment to start, then exit
    std::thread::sleep(std::time::Duration::from_millis(500));
    std::process::exit(0);
}

/// Start the background update checker loop.
/// Runs on a tokio task, checks once per hour if auto_check is enabled,
/// but only actually hits the network once per day.
pub fn start_update_loop(
    updater_state: Arc<UpdaterState>,
    config: Arc<std::sync::Mutex<Config>>,
    app_handle: tauri::AppHandle,
    floating_session_id: Arc<std::sync::Mutex<Option<String>>>,
    acp_client: Arc<tokio::sync::Mutex<crate::acp_client::AcpClient>>,
) {
    let updater_for_idle = updater_state.clone();
    let config_for_idle = config.clone();
    let floating_session_for_idle = floating_session_id;
    let acp_client_for_idle = acp_client;

    tauri::async_runtime::spawn(async move {
        // Initial delay — let the app finish starting
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;

        let mut first_check = true;

        loop {
            let (auto_check, should_check, silent_update) = {
                let cfg = config.lock().unwrap();
                let auto = cfg.updates.auto_check;
                let should = if !auto {
                    false
                } else if first_check {
                    true
                } else {
                    cfg.updates.last_check_time.as_ref().map_or(true, |t| {
                        chrono::DateTime::parse_from_rfc3339(t)
                            .map(|dt| chrono::Utc::now().signed_duration_since(dt).num_hours() >= 24)
                            .unwrap_or(true)
                    })
                };
                let silent = cfg.updates.silent_update;
                (auto, should, silent)
            };

            if !auto_check {
                first_check = false;
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                continue;
            }

            if !should_check {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                continue;
            }

            first_check = false;

            // Check for update (blocking HTTP in spawn_blocking)
            let update_result = tauri::async_runtime::spawn_blocking(check_for_update).await;

            match update_result {
                Ok(Ok(Some(new_version))) => {
                    info!("Update available: {}", new_version);
                    if let Ok(mut v) = updater_state.available_version.lock() {
                        *v = Some(new_version.clone());
                    }

                    // Emit event so UI can show indicator
                    let _ = app_handle.emit("update_available", &new_version);

                    // Update last check time
                    if let Ok(mut cfg) = config.try_lock() {
                        cfg.updates.last_check_time = Some(chrono::Utc::now().to_rfc3339());
                        let _ = cfg.save();
                    }

                    // If silent update enabled, download
                    if silent_update {
                        let dl_result = tauri::async_runtime::spawn_blocking(download_installer).await;
                        match dl_result {
                            Ok(Ok(path)) => {
                                info!("Installer downloaded, waiting for idle to install");
                                if let Ok(mut p) = updater_state.installer_path.lock() {
                                    *p = Some(path);
                                }
                                updater_state.update_ready.store(true, Ordering::SeqCst);
                            }
                            Ok(Err(e)) => error!("Failed to download installer: {}", e),
                            Err(e) => error!("Download task failed: {}", e),
                        }
                    }
                }
                Ok(Ok(None)) => {
                    // Up to date — record check time
                    if let Ok(mut cfg) = config.try_lock() {
                        cfg.updates.last_check_time = Some(chrono::Utc::now().to_rfc3339());
                        let _ = cfg.save();
                    }
                }
                Ok(Err(e)) => warn!("Update check failed: {}", e),
                Err(e) => warn!("Update check task failed: {}", e),
            }

            // Wait before next check
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });

    // Separate loop: check if idle and update is ready → install
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            if !updater_for_idle.update_ready.load(Ordering::SeqCst) {
                continue;
            }

            if !updater_for_idle.is_idle() {
                continue;
            }

            let silent = {
                let cfg = config_for_idle.lock().unwrap();
                cfg.updates.silent_update
            };
            if !silent {
                continue;
            }

            if let Ok(path) = updater_for_idle.installer_path.lock() {
                if let Some(ref installer) = *path {
                    info!("User is idle, applying update...");

                    // Save the version we're updating to
                    if let Ok(mut cfg) = config_for_idle.try_lock() {
                        if let Ok(v) = updater_for_idle.available_version.lock() {
                            cfg.updates.last_updated_version = v.clone();
                        }
                        let _ = cfg.save();
                    }

                    // Resolve session ID: prefer floating session, fall back to ACP client's current session
                    let session_id = floating_session_for_idle
                        .lock()
                        .ok()
                        .and_then(|s| s.clone())
                        .or_else(|| {
                            acp_client_for_idle
                                .try_lock()
                                .ok()
                                .and_then(|c| c.get_session_id())
                        });

                    if let Err(e) = run_installer_and_exit(installer, session_id.as_deref()) {
                        error!("Failed to run installer: {}", e);
                        updater_for_idle.update_ready.store(false, Ordering::SeqCst);
                    }
                }
            }
        }
    });
}

/// Check if the app was just updated (current version matches last_updated_version
/// but differs from what was running before).
pub fn was_just_updated(config: &Config) -> bool {
    config.updates.last_updated_version.as_ref()
        .map(|v| v == CURRENT_VERSION)
        .unwrap_or(false)
}

/// Clear the "just updated" flag after the user has been notified.
pub fn clear_update_flag(config: &mut Config) {
    config.updates.last_updated_version = None;
}
