//! Startup-time pure helpers extracted from main.rs.
//!
//! Everything here is framework-agnostic (no Tauri types, no global state)
//! so it can be unit-tested without spinning up the app. The intent is to
//! keep main.rs thin and make startup logic regression-testable.

use crate::acp_client::AcpConnectionMode;
use crate::config::AcpMode;
use std::path::{Path, PathBuf};

/// Command-line flags parsed from `std::env::args()`. All fields have
/// defaults so callers can rely on construction never failing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CliFlags {
    /// /dev or --dev — extra startup timing logs and tray dev menu.
    pub dev_mode: bool,
    /// /debug or --debug — ACP message tracing on console.
    pub debug_mode: bool,
    /// --restart or /restart — signals we were launched by the updater
    /// or by a user-initiated restart. Triggers the WebView2 wait loop.
    pub is_restart: bool,
}

impl CliFlags {
    /// Parse the flags we care about from a slice of arguments (typically
    /// `std::env::args().collect::<Vec<_>>()`). Unknown arguments are
    /// silently ignored so future subcommands don't trip the launcher.
    pub fn parse(args: &[String]) -> Self {
        let mut flags = CliFlags::default();
        for a in args {
            match a.as_str() {
                "/dev" | "--dev" => flags.dev_mode = true,
                "/debug" | "--debug" => flags.debug_mode = true,
                "/restart" | "--restart" => flags.is_restart = true,
                _ => {}
            }
        }
        flags
    }
}

/// If the binary was launched as the `/capture-hotkey <timeout_ms>` helper,
/// return the parsed timeout. A missing or invalid timeout yields the
/// default of 10 seconds, which matches the in-process behaviour.
///
/// Returns `None` if this isn't the capture-hotkey subcommand at all.
pub fn detect_capture_hotkey_subcommand(args: &[String]) -> Option<u64> {
    if args.len() < 2 {
        return None;
    }
    if args[1] != "/capture-hotkey" {
        return None;
    }
    let timeout = args
        .get(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10_000);
    Some(timeout)
}

/// Resolve the session id to restore on startup. Looks at explicit
/// `--resume-session`/`/resume-session <id>` args first, then falls back
/// to reading (and consuming) a `last-session.txt` file inside
/// `config_dir`. The file is deleted whether or not the read succeeded
/// so we never resume the same session twice.
///
/// Returns `None` when no resume signal was present or both sources
/// yielded an empty/whitespace string.
pub fn resolve_resume_session_id(args: &[String], config_dir: &Path) -> Option<String> {
    // 1. Explicit CLI argument
    if let Some(pos) = args.iter().position(|a| a == "/resume-session" || a == "--resume-session") {
        if let Some(id) = args.get(pos + 1) {
            let trimmed = id.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    // 2. last-session.txt marker written by the updater
    let path = config_dir.join("last-session.txt");
    let contents = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

/// Turn a user-facing `AcpMode` (from config) into the transport-level
/// `AcpConnectionMode` the client constructor needs, plus a short
/// descriptive string for startup logs. Extracted so we can test the
/// mapping without needing to spin up an ACP client.
pub fn acp_mode_for(mode: &AcpMode) -> (AcpConnectionMode, String) {
    match mode {
        AcpMode::Local { spawn_command } => (
            AcpConnectionMode::Local { spawn_command: spawn_command.clone() },
            format!("ACP Mode: Local with spawn command: {}", spawn_command),
        ),
        AcpMode::Remote { host, port, timeout_ms } => (
            AcpConnectionMode::Remote { host: host.clone(), port: *port },
            format!("ACP Mode: Remote at {}:{} (timeout: {}ms)", host, port, timeout_ms),
        ),
    }
}

/// Outcome of waiting for the WebView2 directory to be released after a
/// restart. The variants exist for tests; the main startup path just
/// cares that it eventually returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebviewWaitResult {
    /// Directory didn't exist (first run) — nothing to wait for.
    NotPresent,
    /// Successfully created and removed a probe file; the previous
    /// process has released its lock.
    Released { waited_ms: u64 },
    /// Reached the attempt limit without getting a writable handle.
    /// The caller should log and continue; nothing we can do.
    TimedOut { waited_ms: u64 },
}

/// Poll the WebView2 user data directory until we can create a probe
/// file inside it. This is the pure, testable form of the loop in
/// main.rs — pass a tempdir in tests, `data_local_dir()/kage/EBWebView`
/// in the real path.
///
/// `sleep_fn` is dependency-injected so tests can avoid real waits.
/// The real caller passes `std::thread::sleep`.
pub fn wait_for_webview_release<F: FnMut(std::time::Duration)>(
    dir: &Path,
    max_attempts: u32,
    delay: std::time::Duration,
    mut sleep_fn: F,
) -> WebviewWaitResult {
    if !dir.exists() {
        return WebviewWaitResult::NotPresent;
    }
    for i in 0..max_attempts {
        sleep_fn(delay);
        let probe = dir.join(".restart-test");
        match std::fs::File::create(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                let waited_ms = (i as u64 + 1) * delay.as_millis() as u64;
                return WebviewWaitResult::Released { waited_ms };
            }
            Err(_) => {
                // Keep waiting — the lock might release soon.
            }
        }
    }
    let waited_ms = max_attempts as u64 * delay.as_millis() as u64;
    WebviewWaitResult::TimedOut { waited_ms }
}

/// Resolve the full path to the WebView2 user data directory. Factored
/// out so tests and the startup path share a single source of truth.
/// Returns None when `dirs::data_local_dir()` itself fails, which is
/// extremely rare on real systems but worth handling defensively.
pub fn webview_user_data_dir() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("kage").join("EBWebView"))
}


// -------------------------------------------------------------------
// Post-config helpers
// -------------------------------------------------------------------

/// Load the on-disk config, applying CLI overrides. On load failure
/// this falls back to `Config::default()` and emits a warning so the
/// app can still start. Returns the final Config the app should use.
///
/// Extracted from main() so we can cover the override-merge logic
/// with a unit test. The actual I/O (Config::load) is injected via
/// the `loader` closure so tests can drive the fallback path without
/// touching dirs::config_dir().
pub fn load_config_with_overrides<F>(debug_mode: bool, loader: F) -> crate::config::Config
where
    F: FnOnce() -> anyhow::Result<crate::config::Config>,
{
    let mut config = loader().unwrap_or_else(|e| {
        log::error!("Failed to load config, using defaults: {}", e);
        eprintln!("Failed to load config, using defaults: {}", e);
        crate::config::Config::default()
    });
    // A --debug CLI flag should force debug-mode on even when the
    // persisted config has it off. We never flip it off based on CLI
    // absence — that would toggle away a user preference.
    if debug_mode {
        config.debug_mode = true;
    }
    config
}


/// If the app was launched with `--restart`, poll the WebView2 user
/// data directory until the previous process releases its lock.
/// Silent no-op on first run or when we're not restarting.
///
/// Extracted so main() can call one line. The testable piece
/// (`wait_for_webview_release`) is above; this wraps it with the
/// real filesystem and sleep.
pub fn wait_for_previous_instance_if_restart(is_restart: bool) {
    if !is_restart {
        return;
    }
    log::info!("Restart mode: waiting for previous instance resources to release...");
    let Some(webview_dir) = webview_user_data_dir() else { return };
    match wait_for_webview_release(
        &webview_dir,
        20,
        std::time::Duration::from_millis(500),
        std::thread::sleep,
    ) {
        WebviewWaitResult::NotPresent => {}
        WebviewWaitResult::Released { waited_ms } => {
            log::info!("WebView2 resources released after {}ms", waited_ms);
        }
        WebviewWaitResult::TimedOut { waited_ms } => {
            log::warn!("WebView2 lock still held after {}ms — continuing anyway", waited_ms);
        }
    }
}
