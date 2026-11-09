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
    redirect_installer_staging_dir();
    crate::app_log::flush();

    if let Err(error) = update.install(bytes) {
        restore_after_failed_install(app, &teardown);
        return Err(install_failed(app, &update, &error));
    }
    Ok(())
}

/// Stage the installer under our own config dir instead of `%TEMP%`.
///
/// The plugin writes the verified installer to a `tempfile` directory and
/// launches it from there. "Unsigned PE written to %TEMP% and executed
/// silently by the process that downloaded it" is the canonical dropper
/// shape, and endpoint protection (CrowdStrike Falcon among others) scores
/// execution-from-temp heavily — it was killing the installer mid-run.
///
/// `tempfile` honours `std::env::temp_dir()`, which on Windows reads `TMP`
/// then `TEMP`, so pointing those at `<config>/updates` moves the staging
/// directory without reimplementing the plugin's launch (arg construction,
/// NSIS escaping and `ShellExecuteW` stay the plugin's job — see the
/// "never call run_installer" note in CLAUDE.md).
///
/// Process-wide, but this runs microseconds before the installer handoff
/// and `process::exit(0)`, so the only thing that inherits it is the
/// installer itself (NSIS uses it for `$PLUGINSDIR`, which we also want out
/// of `%TEMP%`). Best-effort: on any failure we leave the env alone and the
/// plugin falls back to `%TEMP%` exactly as before.
fn redirect_installer_staging_dir() {
    if !cfg!(windows) {
        return;
    }
    let Some(dir) = crate::paths::config_dir().map(|d| d.join("kage").join("updates")) else {
        warn!("Could not resolve config dir; staging the installer in %TEMP%");
        return;
    };
    // Clear any installer a previous run left behind (the plugin keeps its
    // temp dir so the installer outlives us, so nothing else prunes these).
    if dir.exists() {
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            warn!("Could not clear stale installer staging dir: {error}");
        }
    }
    if let Err(error) = std::fs::create_dir_all(&dir) {
        warn!("Could not create {}: {error}; using %TEMP%", dir.display());
        return;
    }
    info!("Staging installer in {}", dir.display());
    std::env::set_var("TMP", &dir);
    std::env::set_var("TEMP", &dir);
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
    // graceful_shutdown handed any taken-over system shortcut (⌘Space) back
    // to macOS and latched the takeover off; we're staying up, so take it
    // again — otherwise Kage's ⌘Space would go to Spotlight until restart.
    crate::system_shortcut_takeover::resume();
    // Teardown stopped the host-command bridge (agents' commands outside
    // their sandbox); listen again.
    if let Some(procs) = app.try_state::<crate::state::ChildProcesses>() {
        procs.host_commands.resume(app);
    }
    crate::commands::system::register_all_hotkeys(app);
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

/// Relaunch into the freshly installed bundle, then exit.
///
/// The relaunch must happen *after* this process is gone. Running
/// `open -a Kage.app` while we're still alive (the previous approach) makes
/// LaunchServices merely re-activate the running instance — dropping
/// `--args` — and then we exit, leaving nothing running ("updated but
/// didn't relaunch"). Launching a second copy straight away doesn't work
/// either: tauri-plugin-single-instance in the new process would find the
/// dying one, hand it the argv and quit.
///
/// So we spawn a small detached `/bin/sh` helper that waits for our PID to
/// disappear (bounded), then launches the bundle through LaunchServices.
///
/// `foreground`: true for a user-clicked install (the post-update floating
/// window should take focus); false for a silent idle install, which
/// relaunches with `open -g` so it doesn't steal focus from whatever the
/// user is typing in.
#[cfg(target_os = "macos")]
pub fn relaunch_and_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>, foreground: bool) {
    match std::env::current_exe() {
        Ok(executable) => {
            let target = relaunch_target(&executable);
            let mut cmd =
                relaunch_helper_command(std::process::id(), &target, OPEN_BIN, foreground);
            // The helper outlives our log writer; give it its own file so a
            // failed relaunch is diagnosable instead of silent.
            if let Some(log) = open_relaunch_log() {
                cmd.stderr(log);
            }
            match cmd.spawn() {
                Ok(child) => info!(
                    "Relaunch helper spawned (pid {}) — will start {:?} once pid {} exits",
                    child.id(),
                    target,
                    std::process::id()
                ),
                Err(error) => error!("Failed to spawn relaunch helper: {error}"),
            }
        }
        Err(error) => error!("Cannot resolve exe path for relaunch: {error}"),
    }
    crate::app_log::flush();
    app.exit(0);
}

#[cfg(target_os = "macos")]
const OPEN_BIN: &str = "/usr/bin/open";

/// `<logs>/relaunch.log` (next to app.jsonl), append mode.
#[cfg(target_os = "macos")]
fn open_relaunch_log() -> Option<std::fs::File> {
    let dir = crate::paths::data_local_dir()?.join("kage").join("logs");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("relaunch.log"))
        .ok()
}

/// What to start after we exit: the enclosing `.app` bundle when there is
/// one (so LaunchServices launches it properly), else the bare executable
/// (`cargo run` / dev builds).
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
enum RelaunchTarget {
    Bundle(std::path::PathBuf),
    Executable(std::path::PathBuf),
}

#[cfg(target_os = "macos")]
fn relaunch_target(executable: &std::path::Path) -> RelaunchTarget {
    // <bundle>.app/Contents/MacOS/<exe>
    let bundle = executable
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "MacOS"))
        .and_then(|p| p.parent())
        .filter(|p| p.file_name().is_some_and(|n| n == "Contents"))
        .and_then(|p| p.parent())
        .filter(|p| p.extension().is_some_and(|e| e == "app"));
    match bundle {
        Some(b) => RelaunchTarget::Bundle(b.to_path_buf()),
        None => RelaunchTarget::Executable(executable.to_path_buf()),
    }
}

/// Wait-for-exit-then-launch helper. PID and paths are passed as
/// positional parameters (never interpolated into the script), so bundle
/// paths with spaces or quotes are safe. Waits up to 60 s — exit can block
/// on telemetry flushes with 10 s network timeouts — then SIGKILLs the old
/// process (teardown already ran; only the flush is lost) so the new
/// instance can't hand off to a wedged one via the single-instance socket.
/// Runs in its own process group with null stdin/stdout so a SIGTERM to
/// our group, or our stdout closing, can't take it down with us; stderr
/// carries one status line (the caller points it at relaunch.log).
#[cfg(target_os = "macos")]
fn relaunch_helper_command(
    pid: u32,
    target: &RelaunchTarget,
    open_bin: &str,
    foreground: bool,
) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    const SCRIPT: &str = r#"pid=$1; mode=$2; target=$3; open_bin=$4; bg=$5; i=0
while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 600 ]; do sleep 0.1; i=$((i+1)); done
note=""
if kill -0 "$pid" 2>/dev/null; then
  kill -KILL "$pid" 2>/dev/null; note=" (timed out; killed old pid)"; j=0
  while kill -0 "$pid" 2>/dev/null && [ "$j" -lt 20 ]; do sleep 0.1; j=$((j+1)); done
fi
if [ "$mode" = bundle ]; then
  if [ "$bg" = 1 ]; then "$open_bin" -g -n -a "$target" --args --restart; else "$open_bin" -n -a "$target" --args --restart; fi
else
  "$target" --restart </dev/null >/dev/null 2>&1 &
fi
rc=$?
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) relaunch $mode '$target' after ${i}x100ms$note: exit=$rc" >&2"#;
    let (mode, path) = match target {
        RelaunchTarget::Bundle(p) => ("bundle", p),
        RelaunchTarget::Executable(p) => ("exe", p),
    };
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(SCRIPT)
        .arg("kage-relaunch") // $0
        .arg(pid.to_string())
        .arg(mode)
        .arg(path)
        .arg(open_bin)
        .arg(if foreground { "0" } else { "1" })
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    cmd
}

#[cfg(all(test, target_os = "macos"))]
mod relaunch_tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    #[test]
    fn target_is_the_enclosing_app_bundle() {
        assert_eq!(
            relaunch_target(Path::new("/Applications/Kage.app/Contents/MacOS/kage")),
            RelaunchTarget::Bundle(PathBuf::from("/Applications/Kage.app"))
        );
        assert_eq!(
            relaunch_target(Path::new(
                "/Users/me/My Apps/Kage Dev.app/Contents/MacOS/kage"
            )),
            RelaunchTarget::Bundle(PathBuf::from("/Users/me/My Apps/Kage Dev.app"))
        );
    }

    #[test]
    fn bare_executables_relaunch_directly() {
        // cargo run / target/debug — no bundle around the binary.
        let exe = Path::new("/Users/me/Kage/target/debug/kage");
        assert_eq!(
            relaunch_target(exe),
            RelaunchTarget::Executable(exe.to_path_buf())
        );
        // Looks bundle-ish but isn't one.
        let odd = Path::new("/tmp/Contents/MacOS/kage");
        assert_eq!(
            relaunch_target(odd),
            RelaunchTarget::Executable(odd.to_path_buf())
        );
    }

    fn wait_for(path: &Path, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// End-to-end on the real /bin/sh: the helper must not launch anything
    /// until the "old Kage" process is gone, then launch exactly once with
    /// `--restart`. A fake opener records its argv instead of opening an app.
    #[test]
    fn helper_waits_for_old_process_then_launches_bundle() {
        let dir = std::env::temp_dir().join(format!("kage-relaunch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("launched");
        let opener = dir.join("fake open");
        std::fs::write(
            &opener,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut old = std::process::Command::new("/bin/sleep")
            .arg("1")
            .spawn()
            .unwrap();
        let bundle = RelaunchTarget::Bundle(PathBuf::from("/Applications/Kage Test.app"));
        let mut helper = relaunch_helper_command(old.id(), &bundle, opener.to_str().unwrap(), true)
            .spawn()
            .unwrap();

        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !marker.exists(),
            "launched while the old process was still alive"
        );
        old.wait().unwrap();
        assert!(
            wait_for(&marker, Duration::from_secs(5)),
            "never launched after exit"
        );
        let _ = helper.wait();
        let argv = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(
            argv,
            "-n\n-a\n/Applications/Kage Test.app\n--args\n--restart\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn background_relaunch_does_not_take_focus() {
        let dir = std::env::temp_dir().join(format!("kage-relaunch-bg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("launched");
        let opener = dir.join("open");
        std::fs::write(
            &opener,
            format!("#!/bin/sh\nprintf '%s ' \"$@\" > '{}'\n", marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut gone = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = gone.id();
        gone.wait().unwrap();
        let bundle = RelaunchTarget::Bundle(PathBuf::from("/Applications/Kage.app"));
        let mut helper = relaunch_helper_command(pid, &bundle, opener.to_str().unwrap(), false)
            .spawn()
            .unwrap();
        assert!(wait_for(&marker, Duration::from_secs(5)));
        let _ = helper.wait();
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "-g -n -a /Applications/Kage.app --args --restart "
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_runs_bare_executable_with_restart_flag() {
        let dir = std::env::temp_dir().join(format!("kage-relaunch-exe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("ran");
        let exe = dir.join("kage");
        std::fs::write(
            &exe,
            format!("#!/bin/sh\nprintf '%s' \"$*\" > '{}'\n", marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        // PID of a process that has already exited → launches immediately.
        let mut gone = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let gone_pid = gone.id();
        gone.wait().unwrap();
        let mut helper = relaunch_helper_command(
            gone_pid,
            &RelaunchTarget::Executable(exe.clone()),
            "/nonexistent-open",
            true,
        )
        .spawn()
        .unwrap();
        assert!(wait_for(&marker, Duration::from_secs(5)));
        let _ = helper.wait();
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "--restart");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn relaunch_and_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>, _foreground: bool) {
    app.exit(0);
}
