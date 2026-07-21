//! Floating-window placement and visibility behavior.

use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::os;
use crate::window_labels;
use log::{error, info, warn};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{Manager, WebviewWindow};
use tauri_plugin_notification::NotificationExt;

/// Get cursor position via OS abstraction
pub(super) fn get_cursor_position() -> Option<(i32, i32)> {
    os::get_cursor_position()
}

/// Last time we surfaced a "floating window won't show" notification.
/// Used to throttle the toast — without this, hotkey mashing into a
/// stuck WebView2 would queue dozens of notifications that the user
/// would only see when the system finally caught up.
static LAST_SHOW_FAILURE_NOTIFY: LazyLock<Mutex<Option<Instant>>> =
    LazyLock::new(|| Mutex::new(None));

/// Minimum gap between consecutive "show failed" toasts. Long enough
/// that a frustrated user mashing the hotkey only sees one toast per
/// burst, short enough that if the situation goes south again later
/// they get told.
const SHOW_FAILURE_NOTIFY_THROTTLE: Duration = Duration::from_secs(60);

/// Surface a system notification when a floating-window `show()` call
/// returns Err. The HRESULT 0x8007139F family ("Class not registered" /
/// "group or resource is not in the correct state") usually means the
/// WebView2 controller is wedged and the only fix the user has is to
/// restart kage. Without this, the failure is silent: the hotkey logs
/// say "show() result: Err(...)" but nothing surfaces to the human.
///
/// Rate-limited so a user mashing Alt+Space at a stuck window doesn't
/// queue up a stack of toasts; the throttle is per-process state so
/// every fresh launch starts unmuted.
fn notify_show_failed(app: &tauri::AppHandle, err: &tauri::Error) {
    {
        let mut guard = LAST_SHOW_FAILURE_NOTIFY.lock_or_recover();
        if let Some(prev) = *guard {
            if prev.elapsed() < SHOW_FAILURE_NOTIFY_THROTTLE {
                return;
            }
        }
        *guard = Some(Instant::now());
    }

    let body = format!(
        "Something's gone wrong with the webview ({}). Restart Kage from the tray to recover.",
        err
    );
    if let Err(e) = app
        .notification()
        .builder()
        .title("Kage's floating window won't show")
        .body(body)
        .show()
    {
        warn!("Failed to send recovery notification: {}", e);
    }
}

/// Find which monitor contains the given point
pub(super) fn find_monitor_at_position(
    window: &WebviewWindow,
    x: i32,
    y: i32,
) -> Option<tauri::Monitor> {
    if let Ok(monitors) = window.available_monitors() {
        for monitor in monitors {
            let pos = monitor.position();
            let size = monitor.size();

            if x >= pos.x
                && x < pos.x + size.width as i32
                && y >= pos.y
                && y < pos.y + size.height as i32
            {
                return Some(monitor);
            }
        }
    }
    None
}

/// Clamp a window's top-left `(x, y)` so a `w`×`h` window stays inside
/// `monitor`: if it would overflow the right/bottom edge, shift it back, but
/// never past the monitor's top-left origin. Unit-agnostic — pass x/y/w/h in
/// the same coordinate space (physical for inline-assist, logical for the
/// context menu); the monitor bounds are read in physical pixels, matching the
/// existing call sites.
pub(super) fn clamp_into_monitor(
    monitor: &tauri::Monitor,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> (i32, i32) {
    let mon_pos = monitor.position();
    let mon_size = monitor.size();
    let mon_right = mon_pos.x + mon_size.width as i32;
    let mon_bottom = mon_pos.y + mon_size.height as i32;
    let cx = if x + w > mon_right {
        (mon_right - w).max(mon_pos.x)
    } else {
        x
    };
    let cy = if y + h > mon_bottom {
        (mon_bottom - h).max(mon_pos.y)
    } else {
        y
    };
    (cx, cy)
}

/// Get the active monitor (where cursor is) or fall back to primary
pub fn get_active_monitor(window: &WebviewWindow) -> Option<tauri::Monitor> {
    if let Some((cursor_x, cursor_y)) = get_cursor_position() {
        info!("Cursor position: ({}, {})", cursor_x, cursor_y);

        if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
            info!("Found active monitor at cursor position");
            return Some(monitor);
        }
    }

    info!("Falling back to primary monitor");
    window.primary_monitor().ok().flatten()
}

/// Position the floating window at the mouse cursor, regardless of user config.
/// Show the floating window at the mouse cursor without selection capture.
/// Used by the clipboard hotkey — we don't want to send Ctrl+C when the user
/// just wants to browse their clipboard history.
pub fn show_floating_at_mouse(window: &WebviewWindow) {
    let app = window.app_handle();
    let ui: tauri::State<'_, crate::state::UiState> = app.state();
    let features: tauri::State<'_, crate::state::FeatureServices> = app.state();

    // Don't show during first-run wizard
    if !features.config.lock_or_recover().first_run_completed {
        info!("Ignoring show_floating_at_mouse — first run not completed");
        return;
    }

    // If already visible, hide it (toggle behavior). A wedge here (the
    // WebView2 host died across sleep/resume) surfaces as an Err from
    // this getter; `show()` below is a setter and would silently no-op,
    // so this getter is our only chance to notice. Route it to recovery.
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
            return;
        }
        Ok(false) => {}
        Err(e) => {
            error!("[show_at_mouse] is_visible() failed: {}", e);
            if crate::webview_recovery::note_window_error(window.label(), &e) {
                return;
            }
        }
    }

    // Position before showing so the window appears in its final spot —
    // showing first and snapping creates a visible jump.
    position_floating_window_with_height(window, "mouse", None, None, Some(500));
    if let Err(e) = window.show() {
        error!("[show_at_mouse] show() failed: {}", e);
        notify_show_failed(app, &e);
        return;
    }
    let _ = window.set_focus();
    features.updater.touch_activity();

    // Clear selection — clipboard mode doesn't use it
    if let Ok(mut sel) = ui.last_selection.lock() {
        *sel = None;
    }
    crate::event_targets::emit_to_floating(app, "selection_captured", &false);
}

/// Toggle the floating window visibility and position it
pub fn toggle_floating_window(window: &WebviewWindow) {
    let app = window.app_handle();
    let ui_state: tauri::State<'_, crate::state::UiState> = app.state();
    let features: tauri::State<'_, crate::state::FeatureServices> = app.state();

    // Don't show during first-run wizard — user should complete onboarding first
    if !features.config.lock_or_recover().first_run_completed {
        info!("Ignoring hotkey — first run not completed");
        return;
    }

    let is_showing = !window.is_visible().unwrap_or(true);
    info!(
        "[toggle] entry: is_visible={:?}, taking={} branch",
        window.is_visible(),
        if is_showing { "show" } else { "hide" }
    );

    let config = features.config.lock_or_recover();
    let capture_enabled = config.system.capture_selection;
    let capture_blocklist = config.system.capture_selection_blocklist.clone();
    let start_pos = config.ui.window_start_position.clone();
    let last_x = config.ui.last_window_x;
    let last_y = config.ui.last_window_y;
    drop(config);

    // Capture the foreground window info before we steal focus (~1ms).
    // Grab it ahead of the Ctrl+C injection so we can also consult the
    // blocklist without a second foreground lookup.
    let source_window_info = if is_showing {
        crate::os::window_list::get_foreground_window_info()
    } else {
        None
    };

    // Phase 1: send Ctrl+C while the source window is still focused (~20ms).
    // This must happen before we show our window. Skip the injection when
    // the foreground app is blocklisted (terminals etc.) — sending Ctrl+C
    // there interrupts commands instead of copying text.
    let capture_token = if is_showing && capture_enabled {
        let fg_process = source_window_info
            .as_ref()
            .map(|(_, proc)| proc.as_str())
            .unwrap_or("");
        if crate::os::clipboard::is_process_blocklisted(fg_process, &capture_blocklist) {
            info!("[selection] Skipping capture — foreground app '{fg_process}' is blocklisted");
            None
        } else {
            Some(crate::os::clipboard::begin_selection_capture())
        }
    } else {
        None
    };

    match window.is_visible() {
        Ok(is_visible) => {
            if is_visible {
                // Save position before hiding if "remember" mode
                if start_pos == "remember" {
                    if let Ok(pos) = window.outer_position() {
                        let mut config = features.config.lock_or_recover();
                        config.ui.last_window_x = Some(pos.x);
                        config.ui.last_window_y = Some(pos.y);
                        let _ = config.save();
                    }
                }
                let hide_res = window.hide();
                info!("[toggle] hide() result: {:?}", hide_res);
                // Clear source window info when hiding
                if let Ok(mut sw) = ui_state.source_window.lock() {
                    *sw = None;
                }
            } else {
                // Restore saved launcher size if enabled
                {
                    let config = features.config.lock_or_recover();
                    if config.ui.remember_launcher_size {
                        if let (Some(w), Some(h)) =
                            (config.ui.launcher_width, config.ui.launcher_height)
                        {
                            let scale = window.scale_factor().unwrap_or(1.0);
                            let phys_w = (w as f64 * scale) as u32;
                            let phys_h = (h as f64 * scale) as u32;
                            let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                                width: phys_w,
                                height: phys_h,
                            }));
                        }
                    }
                }
                // Position the window before showing so it appears at
                // its final location — showing first and then moving
                // produces a visible jump.
                position_floating_window(window, &start_pos, last_x, last_y);
                let show_res = window.show();
                let focus_res = window.set_focus();
                info!(
                    "[toggle] show() result: {:?}, set_focus() result: {:?}",
                    show_res, focus_res
                );
                if let Err(e) = show_res {
                    notify_show_failed(app, &e);
                    return;
                }

                // Record floating window activity for the updater idle check
                features.updater.touch_activity();

                // Store the source window info for screen context
                if let Ok(mut sw) = ui_state.source_window.lock() {
                    *sw = source_window_info;
                }

                // Phase 2: poll clipboard in background and deliver result via event.
                // The Ctrl+C was already sent, so this just waits for the clipboard change.
                if let Some(token) = capture_token {
                    let last_selection = ui_state.last_selection.clone();
                    let app_handle = app.clone();
                    std::thread::spawn(move || {
                        let selection = crate::os::clipboard::finish_selection_capture(token);
                        let has_sel = selection.as_ref().is_some_and(|s| !s.is_empty());
                        if let Ok(mut sel) = last_selection.lock() {
                            *sel = selection;
                        }
                        crate::event_targets::emit_to_floating(
                            &app_handle,
                            "selection_captured",
                            &has_sel,
                        );
                    });
                } else {
                    if let Ok(mut sel) = ui_state.last_selection.lock() {
                        *sel = None;
                    }
                    crate::event_targets::emit_to_floating(app, "selection_captured", &false);
                }
            }
        }
        Err(e) => {
            error!("Failed to check visibility: {}", e);
            // A `FailedToReceiveMessage` here means the floating window's
            // WebView2 host has died (typically across a sleep/resume
            // cycle) and every getter against it will fail forever. The
            // HRESULT-log recovery path never sees this — it's a typed
            // runtime error, not a wry `log::error!` line — so route it
            // into the recovery state machine ourselves. If recovery
            // fires we're about to restart; nothing left to do here.
            crate::webview_recovery::note_window_error(window.label(), &e);
        }
    }
}

/// Position the floating window based on the configured strategy
fn position_floating_window(
    window: &WebviewWindow,
    strategy: &str,
    last_x: Option<i32>,
    last_y: Option<i32>,
) {
    position_floating_window_with_height(window, strategy, last_x, last_y, None);
}

fn position_floating_window_with_height(
    window: &WebviewWindow,
    strategy: &str,
    last_x: Option<i32>,
    last_y: Option<i32>,
    estimated_height: Option<u32>,
) {
    match strategy {
        "mouse" => {
            // Position near cursor, but ensure fully on-screen
            if let Some((cursor_x, cursor_y)) = get_cursor_position() {
                if let Some(monitor) = find_monitor_at_position(window, cursor_x, cursor_y) {
                    let mon_pos = monitor.position();
                    let mon_size = monitor.size();
                    let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize {
                        width: 500,
                        height: 60,
                    });
                    // Use estimated height if provided (e.g. clipboard dropdown)
                    let effective_height = estimated_height.unwrap_or(win_size.height) as i32;

                    // Start at cursor, offset slightly down-right
                    let mut x = cursor_x;
                    let mut y = cursor_y + 20;

                    // Clamp to monitor bounds using the effective height
                    let max_x = mon_pos.x + mon_size.width as i32 - win_size.width as i32;
                    let max_y = mon_pos.y + mon_size.height as i32 - effective_height;
                    x = x.max(mon_pos.x).min(max_x);
                    y = y.max(mon_pos.y).min(max_y);

                    let _ = window
                        .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
                    return;
                }
            }
            // Fallback to center
            center_floating_on_active_monitor(window);
        }
        "remember" => {
            // Use saved position if it's still on a valid monitor
            if let (Some(x), Some(y)) = (last_x, last_y) {
                if let Some(_monitor) = find_monitor_at_position(window, x, y) {
                    // Verify the window would be mostly visible
                    let win_size = window.inner_size().unwrap_or(tauri::PhysicalSize {
                        width: 500,
                        height: 60,
                    });
                    if find_monitor_at_position(window, x + win_size.width as i32 / 2, y + 30)
                        .is_some()
                    {
                        let _ = window.set_position(tauri::Position::Physical(
                            tauri::PhysicalPosition { x, y },
                        ));
                        return;
                    }
                }
            }
            // Fallback to center if saved position is off-screen
            center_floating_on_active_monitor(window);
        }
        _ => {
            // "center" — center on active monitor at 1/3 height
            center_floating_on_active_monitor(window);
        }
    }
}

/// Center a window on the active monitor.
/// `vertical_fraction` controls vertical placement (0.33 = 1/3 down, 0.5 = centered).
/// `default_size` is used if the window's inner size can't be determined.
pub fn center_on_active_monitor(
    window: &WebviewWindow,
    vertical_fraction: f64,
    default_size: tauri::PhysicalSize<u32>,
) {
    if let Some(monitor) = get_active_monitor(window) {
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let win_size = window.inner_size().unwrap_or(default_size);
        let x = mon_pos.x + (mon_size.width as i32 - win_size.width as i32) / 2;
        let y = mon_pos.y
            + ((mon_size.height as f64 - win_size.height as f64) * vertical_fraction) as i32;
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
    }
}

/// Center the floating window horizontally on the active monitor, 1/3 down
pub fn center_floating_on_active_monitor(window: &WebviewWindow) {
    center_on_active_monitor(
        window,
        0.33,
        tauri::PhysicalSize {
            width: 500,
            height: 60,
        },
    );
}

pub async fn test_floating_window(app: tauri::AppHandle) -> Result<String, AppError> {
    info!("Testing floating window visibility");

    if let Some(window) = app.get_webview_window(window_labels::FLOATING) {
        let is_visible = window.is_visible().unwrap_or(false);

        if is_visible {
            window
                .hide()
                .map_err(|e| format!("Failed to hide: {}", e))?;
            Ok("Window was visible, now hidden".to_string())
        } else {
            window
                .show()
                .map_err(|e| format!("Failed to show: {}", e))?;
            window
                .set_focus()
                .map_err(|e| format!("Failed to focus: {}", e))?;
            center_floating_on_active_monitor(&window);
            Ok("Window was hidden, now visible and positioned".to_string())
        }
    } else {
        Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.window.not_found",
            &[("label", "floating")],
        ))
    }
}

pub async fn start_drag_window(window: WebviewWindow) -> Result<(), AppError> {
    info!("Starting window drag");
    window.start_dragging().map_err(|e| {
        error!("Failed to start dragging: {}", e);
        AppError::keyed(
            ErrorKind::Internal,
            "errors.window.show_failed",
            &[("reason", &e.to_string())],
        )
    })
}

/// Center a window on the active monitor (where the cursor is)
pub fn center_window_on_active_monitor(window: &WebviewWindow) {
    center_on_active_monitor(
        window,
        0.5,
        tauri::PhysicalSize {
            width: 800,
            height: 600,
        },
    );
}
