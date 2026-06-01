//! Auto-recovery from a wedged WebView2 host process.
//!
//! # The bug we're catching
//!
//! WebView2's host process can drift into an unrecoverable state after long
//! uptime — typically because Microsoft pushed an Edge / WebView2 runtime
//! update underneath us, or because a sleep/hibernate cycle corrupted the
//! controller's COM bindings. When that happens:
//!
//!   - `WebviewWindow::show()` and friends still return `Ok(())` (so our
//!     existing show-failure notification path doesn't fire).
//!   - The OS-level window state flips visible, but the embedded webview
//!     surface emits `0x8007139F` (`ERROR_INVALID_STATE`,
//!     "the group or resource is not in the correct state to perform the
//!     requested operation") asynchronously from `tauri_runtime_wry` and
//!     never paints any content.
//!   - The user sees empty/transparent windows and an unresponsive
//!     floating launcher; the only fix is a full app relaunch.
//!
//! The runtime emits the error via `log::error!`, which means our log
//! adapter (`logger.rs::LogShim`) sees every record before it lands.
//! That gives us a single chokepoint for detection.
//!
//! # Recovery state machine
//!
//! We do NOT immediately notify the user — they noticed the symptom long
//! before we did, and the only useful answer is "restart Kage." So we
//! restart automatically, with a guard against an infinite loop if the
//! wedged state survives the relaunch:
//!
//!   1. **Clean run**: marker absent. First wedge → write marker `Restarted`,
//!      shut down + relaunch immediately.
//!   2. **Just-restarted run**: marker `Restarted`. If we wedge AGAIN within
//!      [`MARKER_TTL`] of startup, escalate to `EscalatedDelayed` and
//!      relaunch after [`ESCALATION_DELAY`] (gives any transient
//!      WebView2 / OS state time to drain).
//!   3. **Already-escalated run**: marker `EscalatedDelayed`. If we STILL
//!      wedge, give up — show a notification with a "Quit" instruction
//!      and exit. Looping past two retries means the wedged state is in
//!      WebView2 itself or the GPU process, not something our process can
//!      fix.
//!
//! After [`MARKER_TTL`] of stable runtime, the marker is cleared so the
//! next wedge starts fresh from step 1. That handles the realistic
//! pattern where the user's machine wedges WebView2 once a day; we
//! shouldn't escalate just because the same problem happened 8 hours
//! apart.
//!
//! # Why a file marker (not in-memory state)
//!
//! The state has to survive process exit — that's the whole point. The
//! marker file lives next to `install-source.txt` and friends in the
//! config dir, and follows the same write-on-trigger / consume-on-read
//! pattern.
//!
//! # Debounce
//!
//! WebView2 errors burst — each subsequent `show()` call against the
//! wedged controller produces a new error line. We only act on the
//! first detection; subsequent records within the same process are
//! dropped via [`AtomicBool`] so we don't kick off N restarts in
//! parallel.

use crate::os;
use log::{error, info, warn};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Substring matched against incoming log records to detect the wedge.
/// `0x8007139F` is `ERROR_INVALID_STATE` on Windows; pairing it with the
/// `tauri_runtime_wry` source filters out incidental matches in
/// unrelated logs.
const WEDGE_HRESULT: &str = "0x8007139F";
const WEDGE_TARGET: &str = "tauri_runtime_wry";

/// How long the marker is considered "fresh." If a process runs cleanly
/// past this, the next wedge restarts the cycle from scratch instead of
/// counting against an ancient retry budget.
const MARKER_TTL: Duration = Duration::from_secs(5 * 60);

/// Pause inserted before the second restart attempt. Gives the
/// WebView2 runtime / GPU process time to drain whatever transient
/// state was screwing the previous controller. 30s is the smallest
/// number where we've observed reliable recovery in the field
/// reports collected so far; shorter intervals tend to wedge again.
const ESCALATION_DELAY: Duration = Duration::from_secs(30);

const MARKER_FILE: &str = "webview-recovery.txt";

/// Flag flipped on first successful detection so subsequent records
/// don't kick off parallel restart attempts.
static RECOVERY_TRIGGERED: AtomicBool = AtomicBool::new(false);

/// Snapshot of process startup so the marker's TTL can be evaluated
/// against "we wedged X seconds after start" rather than wall-clock
/// time on the marker file (which is harder to read robustly across
/// platforms). Set by `init_at_startup` from `main()`.
static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryState {
    /// Marker absent / unreadable / older than MARKER_TTL. Treat as a
    /// clean run.
    Clean,
    /// Previous run wrote `Restarted` and we're now booted from that
    /// restart. The next wedge escalates.
    Restarted,
    /// Previous run wrote `EscalatedDelayed`. Another wedge from here
    /// is the give-up signal.
    EscalatedDelayed,
}

impl RecoveryState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Restarted => "restarted",
            Self::EscalatedDelayed => "escalated_delayed",
        }
    }
    fn parse(s: &str) -> Self {
        match s.trim() {
            "restarted" => Self::Restarted,
            "escalated_delayed" => Self::EscalatedDelayed,
            _ => Self::Clean,
        }
    }
}

fn marker_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("kage").join(MARKER_FILE))
}

fn read_marker() -> RecoveryState {
    let Some(path) = marker_path() else {
        return RecoveryState::Clean;
    };
    if !path.exists() {
        return RecoveryState::Clean;
    }
    // Marker mtime + content together are the truth. We don't trust the
    // mtime alone (someone could touch the file) but if it's older than
    // MARKER_TTL the previous run survived long enough that we should
    // start fresh — anything otherwise would compound retries across
    // unrelated wedges.
    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = modified.elapsed() {
                if age > MARKER_TTL {
                    let _ = std::fs::remove_file(&path);
                    return RecoveryState::Clean;
                }
            }
        }
    }
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    // Always consume on read: a marker is a one-shot signal across the
    // boundary between two processes. The new process re-writes the
    // appropriate state if it triggers another restart.
    let _ = std::fs::remove_file(&path);
    RecoveryState::parse(&contents)
}

fn write_marker(state: RecoveryState) {
    let Some(path) = marker_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, state.as_str()) {
        warn!("webview-recovery: failed to write marker: {}", e);
    }
}

/// Capture the startup time and clear any stale marker if the previous
/// run survived past MARKER_TTL (handled inside `read_marker`).
///
/// Called from `main()` once early in startup. The state read here is
/// stashed via the static below so log-shim detection can branch on it
/// without re-reading the file each time.
pub fn init_at_startup() {
    let _ = PROCESS_START.set(std::time::Instant::now());
    let state = read_marker();
    if state != RecoveryState::Clean {
        info!(
            "webview-recovery: previous run flagged state '{}' — \
             will escalate if WebView2 wedges again",
            state.as_str()
        );
    }
    let _ = STARTUP_STATE.set(state);
}

static STARTUP_STATE: std::sync::OnceLock<RecoveryState> = std::sync::OnceLock::new();

/// Predicate the log shim asks for every record. Returns `true` if the
/// caller should hand this record to `trigger_recovery_async`. Lifted
/// out of the shim so it can be unit-tested without faking the
/// `log::Record` type.
pub fn record_indicates_wedge(target: &str, message: &str) -> bool {
    target == WEDGE_TARGET && message.contains(WEDGE_HRESULT)
}

/// Kick off the recovery flow. Called from the log shim when a wedge
/// record is observed. Idempotent — only the first call per process
/// does work; subsequent calls return immediately.
///
/// Spawns the slow path (graceful shutdown + delay + relaunch) onto a
/// background thread so the calling thread (the log writer) doesn't
/// block.
pub fn trigger_recovery() {
    if RECOVERY_TRIGGERED.swap(true, Ordering::SeqCst) {
        // Already triggered — debounce subsequent error bursts.
        return;
    }
    let prior = STARTUP_STATE.get().copied().unwrap_or(RecoveryState::Clean);

    error!(
        "webview-recovery: WebView2 invalid-state error detected \
         (prior state: {})",
        prior.as_str()
    );

    match prior {
        RecoveryState::Clean => {
            info!("webview-recovery: triggering immediate restart (attempt 1/2)");
            write_marker(RecoveryState::Restarted);
            std::thread::spawn(move || {
                spawn_restart_then_exit(Duration::from_secs(0));
            });
        }
        RecoveryState::Restarted => {
            info!(
                "webview-recovery: webview wedged again post-restart; \
                 escalating with a {}s delay (attempt 2/2)",
                ESCALATION_DELAY.as_secs()
            );
            write_marker(RecoveryState::EscalatedDelayed);
            std::thread::spawn(move || {
                spawn_restart_then_exit(ESCALATION_DELAY);
            });
        }
        RecoveryState::EscalatedDelayed => {
            error!(
                "webview-recovery: webview wedged a third time even after \
                 a {}s wait — giving up. Showing a notification and exiting.",
                ESCALATION_DELAY.as_secs()
            );
            // Don't clear the marker — leaving it lets the next manual
            // launch see we're in a degraded state and log accordingly.
            // The marker's own MARKER_TTL drains it after 5 minutes
            // anyway, so a user who walks away and comes back gets a
            // fresh slate.
            std::thread::spawn(move || {
                notify_and_exit();
            });
        }
    }
}

/// Snapshot of args + exe needed by `shutdown_and_exit_with_restart`.
/// Built once at module-init time so the recovery thread doesn't have
/// to call `std::env::current_exe()` (which can fail) inside the
/// time-critical wedge handler.
fn restart_command() -> Option<(PathBuf, Vec<String>)> {
    let exe = std::env::current_exe().ok()?;
    // Mirror restart_app's arg-filter so we don't propagate cargo /
    // dev-runner flags through a recovery restart. Skip the helper's
    // `/restart` flag — recovery has its own meaning, and we don't
    // want the new process to think the user pressed restart.
    let mut args: Vec<String> = Vec::new();
    let mut skip_next = false;
    for arg in std::env::args().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--" {
            break;
        }
        if arg.starts_with("--no-default") || arg.starts_with("--color") {
            if arg == "--color" {
                skip_next = true;
            }
            continue;
        }
        args.push(arg);
    }
    Some((exe, args))
}

fn spawn_restart_then_exit(delay: Duration) {
    if !delay.is_zero() {
        info!(
            "webview-recovery: waiting {}s before relaunch",
            delay.as_secs()
        );
        std::thread::sleep(delay);
    }
    let Some((exe, args)) = restart_command() else {
        error!("webview-recovery: couldn't resolve current_exe; exiting without restart");
        force_exit();
        return;
    };
    info!("webview-recovery: spawning replacement process: {:?}", exe);
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args)
        .current_dir(std::env::current_dir().unwrap_or_default());
    os::configure_breakaway_from_job(&mut cmd);
    match cmd.spawn() {
        Ok(child) => {
            info!(
                "webview-recovery: replacement spawned (pid {}); exiting",
                child.id()
            );
        }
        Err(e) => {
            error!("webview-recovery: failed to spawn replacement: {}", e);
        }
    }
    // Best-effort log flush before tearing the process down. We don't
    // call `graceful_shutdown` here because the floating window is
    // already wedged and trying to coordinate with it can hang.
    crate::app_log::flush();
    force_exit();
}

fn notify_and_exit() {
    // Notification is best-effort: the WebView2-broken state can also
    // have broken the notification plugin's surface. We log unconditionally
    // so the failure is at least visible from app.jsonl.
    error!(
        "webview-recovery: WebView2 unrecoverable. Quit Kage from the tray \
         and relaunch manually. (HRESULT 0x8007139F)"
    );
    crate::app_log::flush();
    force_exit();
}

fn force_exit() {
    // `std::process::exit` skips destructors but our state has already
    // been written to disk where it needs to be (marker, app log) and
    // child cleanup is handled by the Job Object's KILL_ON_JOB_CLOSE.
    // Tauri's app.exit(0) would be cleaner but we may not have a handle
    // here, and on a wedged webview the message-loop teardown can hang.
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_exact_hresult_from_the_runtime() {
        // The literal log we saw in the field — verbatim.
        let target = "tauri_runtime_wry";
        let msg = "WebView2 error: WindowsError(Error { code: HRESULT(0x8007139F), \
                   message: \"The group or resource is not in the correct state \
                   to perform the requested operation.\" })";
        assert!(record_indicates_wedge(target, msg));
    }

    #[test]
    fn rejects_unrelated_log_lines() {
        // Right HRESULT, wrong target — could be any other Windows API
        // throwing the same code in unrelated code paths.
        assert!(!record_indicates_wedge(
            "kage::os::windows::shell",
            "ShellExecuteW failed: 0x8007139F"
        ));
        // Right target, wrong content.
        assert!(!record_indicates_wedge(
            "tauri_runtime_wry",
            "regular debug output"
        ));
    }

    #[test]
    fn parses_marker_state_round_trip() {
        for state in [
            RecoveryState::Clean,
            RecoveryState::Restarted,
            RecoveryState::EscalatedDelayed,
        ] {
            assert_eq!(RecoveryState::parse(state.as_str()), state);
        }
        // Unknown values land on Clean — defensive against a partial
        // write or hand-edit.
        assert_eq!(RecoveryState::parse("garbage"), RecoveryState::Clean);
        assert_eq!(RecoveryState::parse(""), RecoveryState::Clean);
    }
}
