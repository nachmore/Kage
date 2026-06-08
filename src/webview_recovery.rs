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
//! # Two detection paths
//!
//! There are two distinct ways the wedge surfaces, and we catch both:
//!
//!   1. **Log line** (`record_indicates_wedge`): wry sometimes logs
//!      `0x8007139F` via `log::error!`. Our log adapter
//!      (`logger.rs::LogShim`) sees every record before it lands, giving
//!      us a chokepoint to detect that variant.
//!   2. **Typed getter error** (`error_indicates_wedge` /
//!      `note_window_error`): the field-dominant case — after a
//!      sleep/resume cycle the WebView2 host backing a *pre-existing*
//!      window dies, wry drops that window's `inner`, and every getter
//!      (`is_visible`, `outer_position`, …) thereafter returns
//!      `Err(Runtime(FailedToReceiveMessage))`. This NEVER emits the
//!      `0x8007139F` log line, so path 1 is blind to it. Window code
//!      that calls a getter routes the `Err` through `note_window_error`,
//!      which feeds the same state machine below.
//!
//! Both paths converge on [`trigger_recovery`].
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

/// Stashed at app startup so the log shim's wedge detector can interrogate
/// the manager when a wedge fires. Set once via `set_app_handle`; never
/// cleared. Held by clone so the recovery thread can outlive any scope
/// that might otherwise drop it.
static APP_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// Wire the running [`tauri::AppHandle`] into the recovery module.
/// Called from `setup()` after the manager is up so the wedge detector
/// can list webviews + probe their state when a wedge fires.
///
/// Failing to call this doesn't break the recovery state machine — the
/// per-window diagnostics just degrade to "we don't know which window
/// wedged." The auto-restart path still runs.
pub fn set_app_handle(app: tauri::AppHandle) {
    let _ = APP_HANDLE.set(app);
}

/// Records the most recent ProcessFailed event observed across all
/// webviews — written by the per-webview `add_ProcessFailed` listener,
/// read by `trigger_recovery` so the diagnostic snapshot can name the
/// dead webview alongside the bare wry log line. Wrapped in `Mutex`
/// rather than atomics because the payload is two strings.
static LAST_PROCESS_FAILURE: std::sync::Mutex<Option<ProcessFailureEvent>> =
    std::sync::Mutex::new(None);

/// Set by `attempt_window_reload` when it kicks off an in-place reload
/// (option 3, renderer-only recovery). Reads as "we're trying a soft
/// recovery; suppress the heavy-handed process restart for a few
/// seconds so the wry log line we may emit during the reload doesn't
/// bounce us into a full relaunch." Cleared after [`SOFT_RECOVERY_GRACE`].
static SOFT_RECOVERY_UNTIL: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// How long to suppress process-level restart after a per-window reload.
/// The reload itself can spray a few `evaluate_script` errors as queued
/// IPC drains against the briefly-detached webview — those would hit
/// `record_indicates_wedge` and trigger a process restart we don't want.
/// Long enough to cover the reload + first paint, short enough that a
/// real wedge after the reload still triggers the bigger hammer.
///
/// Only consumed on Windows where the ProcessFailed listener actually
/// installs and can kick off `attempt_window_reload`. macOS / Linux
/// never set `SOFT_RECOVERY_UNTIL`, so `within_soft_recovery_grace`
/// always returns false there and the const isn't referenced.
#[cfg(target_os = "windows")]
const SOFT_RECOVERY_GRACE: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
struct ProcessFailureEvent {
    /// Window label whose webview emitted ProcessFailed.
    label: String,
    /// `kind` value from `ICoreWebView2ProcessFailedEventArgs::ProcessFailedKind`,
    /// converted to a human string via `process_failed_kind_str`.
    kind: String,
    /// Wall-clock instant the event arrived. Used to decide whether a
    /// subsequent wry-log wedge correlates (events older than a few
    /// seconds aren't the cause of *this* wedge).
    at: std::time::Instant,
}

#[cfg(target_os = "windows")]
fn record_process_failure(label: &str, kind: &str) {
    let evt = ProcessFailureEvent {
        label: label.to_string(),
        kind: kind.to_string(),
        at: std::time::Instant::now(),
    };
    log::error!(
        "webview-recovery: ProcessFailed observed on window '{}' kind={}",
        label,
        kind
    );
    if let Ok(mut slot) = LAST_PROCESS_FAILURE.lock() {
        *slot = Some(evt);
    }

    // Per-window recovery (option 3). For renderer-process failures the
    // controller itself is healthy — only the page-rendering subprocess
    // died and WebView2 will respawn it on the next navigation. A
    // `reload()` is the cheapest way to trigger that without losing the
    // window or restarting Kage.
    //
    // For *browser* process or GPU process failures the controller's
    // backing process is gone; reload would race against an already-
    // dead host. Those drop through to the existing process-level
    // restart path (via the wry log line, which fires shortly after the
    // browser process actually exits).
    if matches!(
        kind,
        "render_process_exited" | "render_process_unresponsive" | "frame_render_process_exited"
    ) {
        attempt_window_reload(label);
    }
}

/// Try to reload a single webview in place. Logged at info on success
/// and warn on failure; the failure path doesn't escalate because the
/// later wry-log-driven process restart will catch the case where the
/// reload itself triggers the wedge log.
#[cfg(target_os = "windows")]
fn attempt_window_reload(label: &str) {
    let Some(app) = APP_HANDLE.get() else {
        log::warn!(
            "webview-recovery: can't reload '{}' — no AppHandle stashed",
            label
        );
        return;
    };
    use tauri::Manager;
    let Some(window) = app.get_webview_window(label) else {
        log::warn!(
            "webview-recovery: can't reload '{}' — window not in manager",
            label
        );
        return;
    };
    match window.reload() {
        Ok(()) => {
            log::info!(
                "webview-recovery: in-place reload triggered on '{}' (renderer-only failure)",
                label
            );
            // Suppress the bigger-hammer process restart during the
            // reload's settle window — see SOFT_RECOVERY_GRACE.
            if let Ok(mut slot) = SOFT_RECOVERY_UNTIL.lock() {
                *slot = Some(std::time::Instant::now() + SOFT_RECOVERY_GRACE);
            }
        }
        Err(e) => log::warn!("webview-recovery: reload of '{}' failed: {}", label, e),
    }
}

fn within_soft_recovery_grace() -> bool {
    if let Ok(slot) = SOFT_RECOVERY_UNTIL.lock() {
        if let Some(until) = *slot {
            return std::time::Instant::now() < until;
        }
    }
    false
}

/// Install the ProcessFailed listener for one webview. Wired into
/// `tauri::Builder::on_page_load` so it fires every time a webview
/// completes navigation — that's the earliest reliable point where
/// `WebviewWindow::with_webview` returns a live controller for both
/// pre-declared windows (main/floating/inline-assist) and on-demand
/// ones (settings/store/welcome/chat-*).
///
/// Why this matters: the wry log line (`tauri_runtime_wry: WebView2
/// error: 0x8007139F`) is what we currently catch in `record_indicates_wedge`,
/// but it doesn't carry the failing webview's label and only fires after
/// the wedge has already manifested as eval failures. ProcessFailed
/// fires when the WebView2 renderer / GPU / browser process itself dies —
/// the upstream root cause — and gives us both the kind of failure
/// (BrowserProcessExited, RenderProcessExited, RenderProcessUnresponsive)
/// and direct access to the webview that owns the dead process.
///
/// Windows-only — wry's macOS / Linux backends don't expose an analogous
/// hook. On non-Windows builds this is a no-op.
#[cfg(target_os = "windows")]
pub fn install_process_failed_for<R: tauri::Runtime>(webview: &tauri::Webview<R>) {
    let label = webview.label().to_string();
    {
        let installed = match LISTENER_INSTALLED.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if installed.contains(&label) {
            return;
        }
    }
    install_for_webview(label, webview.clone());
}

#[cfg(not(target_os = "windows"))]
pub fn install_process_failed_for<R: tauri::Runtime>(_webview: &tauri::Webview<R>) {}

#[cfg(target_os = "windows")]
static LISTENER_INSTALLED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<String>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(target_os = "windows")]
fn install_for_webview<R: tauri::Runtime>(label: String, window: tauri::Webview<R>) {
    use webview2_com::ProcessFailedEventHandler;

    let label_for_handler = label.clone();
    let label_for_log = label.clone();
    if let Err(e) = window.with_webview(move |w| {
        // SAFETY: `controller()` returns the live ICoreWebView2Controller
        // owned by wry. The closure runs on the Tauri main thread, where
        // WebView2 callbacks are also dispatched, so the COM apartment
        // matches and the resulting registration is valid.
        let controller = w.controller();
        let core = match unsafe { controller.CoreWebView2() } {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "webview-recovery: '{}' couldn't fetch CoreWebView2 for ProcessFailed listener: {}",
                    label_for_handler,
                    e
                );
                return;
            }
        };

        let label_for_callback = label_for_handler.clone();
        let handler = ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
            // `args` is an `Option<ICoreWebView2ProcessFailedEventArgs>`;
            // a None here would mean the event fired with no detail,
            // which the API doesn't actually do but the type allows.
            let kind_str = args
                .as_ref()
                .map(|a| {
                    let mut k = Default::default();
                    // SAFETY: callback runs on the same COM apartment
                    // that owns `args`; the out-pointer points to a
                    // stack local owned by this frame.
                    if unsafe { a.ProcessFailedKind(&mut k) }.is_ok() {
                        process_failed_kind_str(k)
                    } else {
                        "unknown_kind"
                    }
                })
                .unwrap_or("no_args");
            record_process_failure(&label_for_callback, kind_str);
            Ok(())
        }));
        let mut token = Default::default();
        unsafe {
            if let Err(e) = core.add_ProcessFailed(&handler, &mut token) {
                log::warn!(
                    "webview-recovery: '{}' add_ProcessFailed failed: {}",
                    label_for_handler,
                    e
                );
                return;
            }
        }
        log::info!(
            "webview-recovery: ProcessFailed listener installed on window '{}'",
            label_for_handler
        );
        if let Ok(mut set) = LISTENER_INSTALLED.lock() {
            set.insert(label_for_handler);
        }
    }) {
        log::warn!(
            "webview-recovery: with_webview failed for '{}': {}",
            label_for_log,
            e
        );
    }
}

#[cfg(target_os = "windows")]
fn process_failed_kind_str(
    kind: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_PROCESS_FAILED_KIND,
) -> &'static str {
    use webview2_com::Microsoft::Web::WebView2::Win32 as wv;
    match kind {
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => "browser_process_exited",
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => "render_process_exited",
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
            "render_process_unresponsive"
        }
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED => {
            "frame_render_process_exited"
        }
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED => "utility_process_exited",
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_SANDBOX_HELPER_PROCESS_EXITED => {
            "sandbox_helper_process_exited"
        }
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED => "gpu_process_exited",
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_PLUGIN_PROCESS_EXITED => {
            "ppapi_plugin_process_exited"
        }
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_BROKER_PROCESS_EXITED => {
            "ppapi_broker_process_exited"
        }
        wv::COREWEBVIEW2_PROCESS_FAILED_KIND_UNKNOWN_PROCESS_EXITED => "unknown_process_exited",
        _ => "unrecognized_kind",
    }
}

/// Snapshot every webview's visible/outer-state at trigger time. The wry
/// log line that fires `0x8007139F` doesn't carry the failing webview's
/// label — it just dumps wry's `Error` Display. Without this snapshot we
/// have no idea which window class (main / floating / settings / chat-*)
/// is the one whose WebView2 host has gone south, and the rebuild path
/// needs that.
///
/// We don't ask each webview to eval-ping back: that would be the most
/// authoritative health check but it's async and the recovery flow has
/// to be synchronous (we're about to spawn a restart thread). Synchronous
/// visibility / position queries go straight to the OS via Win32, bypassing
/// wry's IPC, so a wedged WebView2 controller can't hide them.
///
/// Combined with option 4 (`ProcessFailed` listener) this pins the
/// culprit accurately enough to act on.
fn snapshot_webview_state() -> Vec<WebviewSnapshot> {
    let Some(app) = APP_HANDLE.get() else {
        return Vec::new();
    };
    use tauri::Manager;
    app.webview_windows()
        .into_iter()
        .map(|(label, window)| WebviewSnapshot {
            label,
            is_visible: window.is_visible().ok(),
            outer_position: window.outer_position().ok().map(|p| (p.x, p.y)),
            inner_size: window.inner_size().ok().map(|s| (s.width, s.height)),
            is_focused: window.is_focused().ok(),
        })
        .collect()
}

#[derive(Debug)]
struct WebviewSnapshot {
    label: String,
    is_visible: Option<bool>,
    outer_position: Option<(i32, i32)>,
    inner_size: Option<(u32, u32)>,
    is_focused: Option<bool>,
}

fn log_snapshot(snapshots: &[WebviewSnapshot]) {
    if snapshots.is_empty() {
        warn!("webview-recovery: no app handle stashed; can't snapshot webviews");
        return;
    }
    for s in snapshots {
        info!(
            "webview-recovery: snapshot label={} visible={:?} pos={:?} size={:?} focused={:?}",
            s.label, s.is_visible, s.outer_position, s.inner_size, s.is_focused
        );
    }
}

/// Predicate the log shim asks for every record. Returns `true` if the
/// caller should hand this record to `trigger_recovery_async`. Lifted
/// out of the shim so it can be unit-tested without faking the
/// `log::Record` type.
pub fn record_indicates_wedge(target: &str, message: &str) -> bool {
    target == WEDGE_TARGET && message.contains(WEDGE_HRESULT)
}

/// Classify a `tauri::Error` returned by a window *getter*
/// (`is_visible`, `outer_position`, …) as a webview wedge.
///
/// # Why this exists separately from `record_indicates_wedge`
///
/// The HRESULT-in-the-log detector catches the case where wry itself
/// logs `0x8007139F`. But the wedge we actually see in the field after
/// a sleep/resume cycle never emits that line. Instead the WebView2 host
/// process backing a *pre-existing* window dies, wry drops that window's
/// `inner`, and every getter call thereafter returns
/// `Err(Runtime(FailedToReceiveMessage))` — the reply channel's sender
/// is dropped without a value (see tauri-runtime-wry's `getter!` macro).
/// That error never round-trips through `log::error!`, so the log-shim
/// path is blind to it and recovery never fires; the window stays dead
/// for the life of the process.
///
/// We treat three runtime variants as a wedge:
///   - `FailedToReceiveMessage` — the canonical "host died, inner gone"
///     signal described above.
///   - `FailedToSendMessage` / `EventLoopClosed` — the loop itself has
///     stopped servicing the user-event channel. Rarer and arguably
///     more terminal, but the user-visible symptom (window operations
///     no longer work) and the only fix (relaunch) are identical, so we
///     fold them into the same recovery path.
///
/// `WindowNotFound` is deliberately *not* treated as a wedge: that's the
/// benign "this label was closed" case our own code hits routinely.
pub fn error_indicates_wedge(err: &tauri::Error) -> bool {
    matches!(
        err,
        tauri::Error::Runtime(
            tauri_runtime::Error::FailedToReceiveMessage
                | tauri_runtime::Error::FailedToSendMessage
                | tauri_runtime::Error::EventLoopClosed
        )
    )
}

/// Call-site hook for the typed-error wedge path. Window code that calls
/// a getter (e.g. `toggle_floating_window`'s `window.is_visible()`) and
/// gets back an `Err` passes it here; if it classifies as a wedge we
/// kick off the same recovery state machine the log-shim path uses.
///
/// `label` is logged for the post-mortem so we know which window's
/// getter surfaced the wedge — useful because the HRESULT log line never
/// carried it. Returns `true` if recovery was triggered, so the caller
/// can bail out of whatever it was doing rather than press on against a
/// dead window.
pub fn note_window_error(label: &str, err: &tauri::Error) -> bool {
    if error_indicates_wedge(err) {
        error!(
            "webview-recovery: window '{}' getter returned a wedge error ({}); \
             routing to recovery state machine",
            label, err
        );
        trigger_recovery();
        true
    } else {
        false
    }
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

    // If we're inside the soft-recovery grace window (a per-window
    // reload was just kicked off in response to ProcessFailed), the
    // wry log line is almost certainly a tail-end queued eval error
    // from the reload itself, NOT a fresh wedge. Suppress the process
    // restart and clear the trigger flag so a genuine wedge later can
    // still escalate.
    if within_soft_recovery_grace() {
        info!(
            "webview-recovery: wedge log fired inside soft-recovery grace — \
             skipping process restart (per-window reload owns this incident)"
        );
        RECOVERY_TRIGGERED.store(false, Ordering::SeqCst);
        return;
    }

    let prior = STARTUP_STATE.get().copied().unwrap_or(RecoveryState::Clean);

    error!(
        "webview-recovery: WebView2 invalid-state error detected \
         (prior state: {})",
        prior.as_str()
    );

    // If a ProcessFailed callback fired in the last few seconds, the
    // wedge we just observed is almost certainly the same incident —
    // log them side by side so the post-mortem doesn't need to guess.
    let recent_failure = LAST_PROCESS_FAILURE
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|f| f.at.elapsed() < Duration::from_secs(15));
    if let Some(f) = &recent_failure {
        info!(
            "webview-recovery: correlated with ProcessFailed on '{}' kind={} \
             ({}s before wedge log)",
            f.label,
            f.kind,
            f.at.elapsed().as_secs()
        );
    }

    // Diagnostic: what does the manager think every webview's state is
    // right now? On a clean wedge one window's `is_visible()` will hang
    // or stale-read while the others remain healthy. The data here is
    // also what the rebuild path (option 3) keys off to decide which
    // single window to destroy + recreate, so logging it now is both
    // forensics and a checkpoint.
    let snapshot = snapshot_webview_state();
    log_snapshot(&snapshot);

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
    fn typed_error_wedge_variants_are_detected() {
        // The field symptom: a getter on a window whose WebView2 host
        // died returns this. The HRESULT-log path never sees it, so this
        // typed predicate is the only thing that catches it.
        assert!(error_indicates_wedge(&tauri::Error::Runtime(
            tauri_runtime::Error::FailedToReceiveMessage
        )));
        // The loop-side failures share the symptom and the fix.
        assert!(error_indicates_wedge(&tauri::Error::Runtime(
            tauri_runtime::Error::FailedToSendMessage
        )));
        assert!(error_indicates_wedge(&tauri::Error::Runtime(
            tauri_runtime::Error::EventLoopClosed
        )));
    }

    #[test]
    fn benign_window_errors_are_not_wedges() {
        // `WindowNotFound` is the routine "label was closed" case — our
        // own code hits it and it must NOT trigger an app restart.
        assert!(!error_indicates_wedge(&tauri::Error::Runtime(
            tauri_runtime::Error::WindowNotFound
        )));
        // An unrelated runtime failure (creating a window) isn't a wedge
        // of an existing one.
        assert!(!error_indicates_wedge(&tauri::Error::Runtime(
            tauri_runtime::Error::CreateWindow
        )));
        // A non-Runtime tauri error never indicates a webview wedge.
        assert!(!error_indicates_wedge(
            &tauri::Error::WindowLabelAlreadyExists("floating".into())
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
