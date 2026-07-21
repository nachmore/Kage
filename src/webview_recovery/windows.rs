use super::trigger_recovery;
use log::{error, info, warn};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();
static LAST_PROCESS_FAILURE: Mutex<Option<ProcessFailureEvent>> = Mutex::new(None);
static SOFT_RECOVERY_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

#[cfg(target_os = "windows")]
const SOFT_RECOVERY_GRACE: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
struct ProcessFailureEvent {
    label: String,
    kind: String,
    at: Instant,
}

pub(super) fn set_app_handle(app: tauri::AppHandle) {
    let _ = APP_HANDLE.set(app);
}

pub(super) fn within_soft_recovery_grace() -> bool {
    SOFT_RECOVERY_UNTIL
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .is_some_and(|until| Instant::now() < until)
}

pub(super) fn log_recent_process_failure() {
    let recent_failure = LAST_PROCESS_FAILURE
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|failure| failure.at.elapsed() < Duration::from_secs(15));
    if let Some(failure) = recent_failure {
        info!(
            "webview-recovery: correlated with ProcessFailed on '{}' kind={} \
             ({}s before wedge log)",
            failure.label,
            failure.kind,
            failure.at.elapsed().as_secs()
        );
    }
}

pub(super) fn log_webview_snapshot() {
    let Some(app) = APP_HANDLE.get() else {
        warn!("webview-recovery: no app handle stashed; can't snapshot webviews");
        return;
    };
    use tauri::Manager;
    for (label, window) in app.webview_windows() {
        info!(
            "webview-recovery: snapshot label={} visible={:?} pos={:?} size={:?} focused={:?}",
            label,
            window.is_visible().ok(),
            window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            window
                .inner_size()
                .ok()
                .map(|size| (size.width, size.height)),
            window.is_focused().ok(),
        );
    }
}

#[cfg(target_os = "windows")]
fn record_process_failure(label: &str, kind: &str) {
    let event = ProcessFailureEvent {
        label: label.to_string(),
        kind: kind.to_string(),
        at: Instant::now(),
    };
    error!(
        "webview-recovery: ProcessFailed observed on window '{}' kind={}",
        label, kind
    );
    if let Ok(mut slot) = LAST_PROCESS_FAILURE.lock() {
        *slot = Some(event);
    }

    match action_for_process_failed_kind(kind) {
        ProcessFailedAction::ReloadWindow => attempt_window_reload(label),
        ProcessFailedAction::RestartApp => {
            error!(
                "webview-recovery: browser process for '{}' exited — controller is dead, \
                 driving recovery restart immediately",
                label
            );
            trigger_recovery();
        }
        ProcessFailedAction::Ignore => {}
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessFailedAction {
    ReloadWindow,
    RestartApp,
    Ignore,
}

#[cfg(target_os = "windows")]
fn action_for_process_failed_kind(kind: &str) -> ProcessFailedAction {
    match kind {
        "render_process_exited" | "render_process_unresponsive" | "frame_render_process_exited" => {
            ProcessFailedAction::ReloadWindow
        }
        "browser_process_exited" => ProcessFailedAction::RestartApp,
        _ => ProcessFailedAction::Ignore,
    }
}

#[cfg(target_os = "windows")]
fn attempt_window_reload(label: &str) {
    let Some(app) = APP_HANDLE.get() else {
        warn!(
            "webview-recovery: can't reload '{}' — no AppHandle stashed",
            label
        );
        return;
    };
    use tauri::Manager;
    let Some(window) = app.get_webview_window(label) else {
        warn!(
            "webview-recovery: can't reload '{}' — window not in manager",
            label
        );
        return;
    };
    match window.reload() {
        Ok(()) => {
            info!(
                "webview-recovery: in-place reload triggered on '{}' (renderer-only failure)",
                label
            );
            if let Ok(mut slot) = SOFT_RECOVERY_UNTIL.lock() {
                *slot = Some(Instant::now() + SOFT_RECOVERY_GRACE);
            }
        }
        Err(error) => warn!("webview-recovery: reload of '{}' failed: {}", label, error),
    }
}

#[cfg(target_os = "windows")]
pub(super) fn install_process_failed_for<R: tauri::Runtime>(webview: &tauri::Webview<R>) {
    let label = webview.label().to_string();
    {
        let installed = match LISTENER_INSTALLED.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if installed.contains(&label) {
            return;
        }
    }
    install_for_webview(label, webview.clone());
}

#[cfg(not(target_os = "windows"))]
pub(super) fn install_process_failed_for<R: tauri::Runtime>(_webview: &tauri::Webview<R>) {}

#[cfg(target_os = "windows")]
static LISTENER_INSTALLED: LazyLock<Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

#[cfg(target_os = "windows")]
fn install_for_webview<R: tauri::Runtime>(label: String, webview: tauri::Webview<R>) {
    use webview2_com::ProcessFailedEventHandler;

    let label_for_handler = label.clone();
    let label_for_log = label.clone();
    if let Err(error) = webview.with_webview(move |window| {
        // The closure runs on Tauri's main thread, matching the WebView2 COM apartment.
        let core = match unsafe { window.controller().CoreWebView2() } {
            Ok(core) => core,
            Err(error) => {
                warn!(
                    "webview-recovery: '{}' couldn't fetch CoreWebView2 for ProcessFailed listener: {}",
                    label_for_handler, error
                );
                return;
            }
        };

        let label_for_callback = label_for_handler.clone();
        let handler = ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
            let kind = args
                .as_ref()
                .map(|args| {
                    let mut kind = Default::default();
                    if unsafe { args.ProcessFailedKind(&mut kind) }.is_ok() {
                        process_failed_kind_str(kind)
                    } else {
                        "unknown_kind"
                    }
                })
                .unwrap_or("no_args");
            record_process_failure(&label_for_callback, kind);
            Ok(())
        }));
        let mut token = Default::default();
        unsafe {
            if let Err(error) = core.add_ProcessFailed(&handler, &mut token) {
                warn!(
                    "webview-recovery: '{}' add_ProcessFailed failed: {}",
                    label_for_handler, error
                );
                return;
            }
        }
        info!(
            "webview-recovery: ProcessFailed listener installed on window '{}'",
            label_for_handler
        );
        if let Ok(mut installed) = LISTENER_INSTALLED.lock() {
            installed.insert(label_for_handler);
        }
    }) {
        warn!(
            "webview-recovery: with_webview failed for '{}': {}",
            label_for_log, error
        );
    }
}

#[cfg(target_os = "windows")]
fn process_failed_kind_str(
    kind: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_PROCESS_FAILED_KIND,
) -> &'static str {
    use webview2_com::Microsoft::Web::WebView2::Win32 as webview2;
    match kind {
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => {
            "browser_process_exited"
        }
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => "render_process_exited",
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
            "render_process_unresponsive"
        }
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED => {
            "frame_render_process_exited"
        }
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED => {
            "utility_process_exited"
        }
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_SANDBOX_HELPER_PROCESS_EXITED => {
            "sandbox_helper_process_exited"
        }
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED => "gpu_process_exited",
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_PLUGIN_PROCESS_EXITED => {
            "ppapi_plugin_process_exited"
        }
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_BROKER_PROCESS_EXITED => {
            "ppapi_broker_process_exited"
        }
        webview2::COREWEBVIEW2_PROCESS_FAILED_KIND_UNKNOWN_PROCESS_EXITED => {
            "unknown_process_exited"
        }
        _ => "unrecognized_kind",
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use super::{action_for_process_failed_kind, ProcessFailedAction};

    #[cfg(target_os = "windows")]
    #[test]
    fn process_failed_kind_maps_to_correct_action() {
        use ProcessFailedAction::*;

        assert_eq!(
            action_for_process_failed_kind("render_process_exited"),
            ReloadWindow
        );
        assert_eq!(
            action_for_process_failed_kind("render_process_unresponsive"),
            ReloadWindow
        );
        assert_eq!(
            action_for_process_failed_kind("frame_render_process_exited"),
            ReloadWindow
        );
        assert_eq!(
            action_for_process_failed_kind("browser_process_exited"),
            RestartApp
        );
        assert_eq!(action_for_process_failed_kind("gpu_process_exited"), Ignore);
        assert_eq!(
            action_for_process_failed_kind("utility_process_exited"),
            Ignore
        );
        assert_eq!(action_for_process_failed_kind("unrecognized_kind"), Ignore);
    }
}
