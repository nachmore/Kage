use crate::error::{AppError, ErrorKind};
use crate::events;
use crate::lock_ext::LockExt;
use crate::os;
use crate::window_labels::{self, is_chat_label, is_session_host_label};
use log::{error, info, warn};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{Manager, WebviewWindow};
use tauri_plugin_notification::NotificationExt;

/// Get cursor position via OS abstraction
fn get_cursor_position() -> Option<(i32, i32)> {
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
fn find_monitor_at_position(window: &WebviewWindow, x: i32, y: i32) -> Option<tauri::Monitor> {
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

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub async fn open_chat_window(app: tauri::AppHandle) -> Result<(), AppError> {
    info!("Opening chat window");

    if let Some(floating_window) = app.get_webview_window(window_labels::FLOATING) {
        let _ = floating_window.hide();
    }

    if let Some(window) = app.get_webview_window(window_labels::MAIN) {
        let features: tauri::State<'_, crate::state::FeatureServices> = app.state();
        let config = features.config.lock_or_recover();
        let saved_w = config.ui.chat_window_width;
        let saved_h = config.ui.chat_window_height;
        let saved_x = config.ui.chat_window_x;
        let saved_y = config.ui.chat_window_y;
        drop(config);

        let scale = window.scale_factor().unwrap_or(1.0);

        // Get active monitor bounds
        let monitor = get_active_monitor(&window);
        let (mon_x, mon_y, mon_w, mon_h) = monitor
            .as_ref()
            .map(|m| {
                let p = m.position();
                let s = m.size();
                (p.x, p.y, s.width as i32, s.height as i32)
            })
            .unwrap_or((0, 0, 1920, 1080));

        if saved_w > 0 && saved_h > 0 {
            // Clamp size to monitor dimensions
            let phys_w = ((saved_w as f64 * scale) as i32).min(mon_w);
            let phys_h = ((saved_h as f64 * scale) as i32).min(mon_h);
            let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: phys_w as u32,
                height: phys_h as u32,
            }));

            // Restore position if saved, clamped to screen
            if let (Some(x), Some(y)) = (saved_x, saved_y) {
                let win_w = phys_w;
                let win_h = phys_h;
                let cx = x.max(mon_x).min(mon_x + mon_w - win_w);
                let cy = y.max(mon_y).min(mon_y + mon_h - win_h);
                let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                    x: cx,
                    y: cy,
                }));
            } else {
                center_window_on_active_monitor(&window);
            }
        } else {
            // Default: 800x600 centered
            let def_w = (800.0 * scale) as u32;
            let def_h = (600.0 * scale) as u32;
            let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: def_w.min(mon_w as u32),
                height: def_h.min(mon_h as u32),
            }));
            center_window_on_active_monitor(&window);
        }

        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        // Showing main is a chat-experience activation — cancel any
        // pending agent shutdown so the user's first message-send
        // hits a live agent.
        cancel_chat_shutdown(&app);
        crate::setup::update_activation_policy(&app);
    } else {
        warn!("Main window not found");
    }

    Ok(())
}

/// Record this chat window as the most-recently-focused. Read by the
/// single-instance handler and "show chat" affordances to decide which
/// window to surface when the user has multiple open.
pub fn mark_focused_chat(app: &tauri::AppHandle, label: &str) {
    if !is_session_host_label(label) {
        return;
    }
    if let Some(ui) = app.try_state::<crate::state::UiState>() {
        if let Ok(mut last) = ui.last_focused_chat.lock() {
            *last = Some(label.to_string());
        }
    }
}

/// Drop a chat window's `window_sessions` and originator-routing
/// entries. Called from the window's `Destroyed` event listener so a
/// user closing a chat window (OS chrome or `close_chat_window`) leaves
/// the backend state clean. Safe to call when the entries don't exist.
fn cleanup_chat_window_state(app: &tauri::AppHandle, label: &str) {
    use tauri::Manager;
    let Some(ui) = app.try_state::<crate::state::UiState>() else {
        return;
    };
    if let Ok(mut ws) = ui.window_sessions.lock() {
        ws.remove(label);
    }
    if let Ok(mut originators) = ui.pending_prompt_originators.lock() {
        originators.retain(|_, owner| owner != label);
    }
    if let Ok(mut last) = ui.last_focused_chat.lock() {
        if last.as_deref() == Some(label) {
            *last = None;
        }
    }
    // The Destroyed event fires after webview_windows() has dropped
    // the entry, so the count we read here is post-close.
    let remaining = count_open_chat_windows(app);
    crate::telemetry::track(
        app,
        "chat_window_closed",
        Some(serde_json::json!({
            "remaining_bucket": chat_window_count_bucket(remaining.max(1)),
        })),
    );
    info!(
        "Cleaned up window_sessions for closed chat window: {}",
        label
    );

    // Maybe disconnect the agent. The full chat experience holds the
    // agent open; once the last chat window closes (and floating isn't
    // preloaded with a session), we want to release the kiro-cli
    // subprocess to be respectful of memory.
    schedule_chat_shutdown_check(app);
}

/// How long to wait after the last chat window closes before checking
/// whether the agent should be disconnected. Long enough to be
/// forgiving (user closes the X then realises they wanted to copy
/// something — within 30s they reopen, agent stays); short enough
/// that closing for real frees memory promptly.
const CHAT_SHUTDOWN_DELAY_SECS: u64 = 30;

/// Public entry point for the same logic — used by `handle_window_close`
/// in setup.rs when the user closes main via the OS chrome (which hides
/// rather than destroys). The internal `cleanup_chat_window_state` path
/// is for chat-* peer windows that actually destroy.
pub fn schedule_chat_shutdown_check_public(app: &tauri::AppHandle) {
    schedule_chat_shutdown_check(app);
}

/// Schedule a check to disconnect the agent if no chat window is open
/// and floating doesn't have a pinned session. Bumps the generation
/// counter so any in-flight task from a previous close observes the
/// change and exits early. The actual decision (and the disconnect)
/// is in [`run_chat_shutdown_check`].
fn schedule_chat_shutdown_check(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    use tauri::Manager;
    let Some(ui) = app.try_state::<crate::state::UiState>() else {
        return;
    };
    let gen = ui.chat_shutdown_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(CHAT_SHUTDOWN_DELAY_SECS)).await;
        run_chat_shutdown_check(&app, gen);
    });
}

/// Cancel any pending chat-shutdown task by bumping the generation.
/// Called when a chat window opens (or any path that should keep the
/// agent alive) — the in-flight sleep wakes up and finds its
/// generation stale.
pub fn cancel_chat_shutdown(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    use tauri::Manager;
    let Some(ui) = app.try_state::<crate::state::UiState>() else {
        return;
    };
    ui.chat_shutdown_generation.fetch_add(1, Ordering::SeqCst);
}

fn run_chat_shutdown_check(app: &tauri::AppHandle, my_generation: u64) {
    use std::sync::atomic::Ordering;
    use tauri::Manager;
    let Some(ui) = app.try_state::<crate::state::UiState>() else {
        return;
    };
    let current = ui.chat_shutdown_generation.load(Ordering::SeqCst);
    if current != my_generation {
        // A newer chat-shutdown task superseded us, OR a chat window
        // opened (cancel_chat_shutdown bumps too). Stand down.
        return;
    }

    // Any chat windows still visible? If so, leave the agent alone —
    // they need it. Hidden main counts as closed (the user could have
    // typed Cmd+Q on macOS or hidden via tray).
    if count_visible_chat_windows(app) > 0 {
        return;
    }

    // Floating preloaded with a session means the agent persists for
    // the app lifetime — don't disconnect.
    let floating_has_session = ui
        .window_sessions
        .lock()
        .ok()
        .map(|m| m.get(window_labels::FLOATING).is_some())
        .unwrap_or(false);
    if floating_has_session {
        return;
    }

    // No chat windows, no preloaded floating session — disconnect.
    if let Some(acp) = app.try_state::<crate::state::AcpHandles>() {
        if acp.client.is_connected() {
            info!(
                "No chat windows open and no floating session pinned — disconnecting agent after {}s idle",
                CHAT_SHUTDOWN_DELAY_SECS
            );
            acp.client.disconnect();
        }
    }
}

/// Spawn a new chat window, peer to `main`. The new window has a
/// label of the form `chat-<uuid>` and loads the same `index.html` as
/// main; the frontend reads its label on boot via
/// `getCurrentWebviewWindow().label`, calls `switch_acp_session` with
/// the optional `resume_session_id` URL param to either load that
/// session or create a fresh one, and writes the result into
/// `window_sessions[label]`.
///
/// Returns the new window's label so the caller can route follow-up
/// invocations.
#[tauri::command]
pub async fn open_new_chat_window(
    resume_session_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, AppError> {
    use tauri::WebviewWindowBuilder;

    let label = window_labels::chat_label(&uuid::Uuid::new_v4().simple().to_string());
    info!("Opening new chat window: {}", label);

    let url = if let Some(ref sid) = resume_session_id {
        let qs = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("resumeSessionId", sid)
            .finish();
        format!("index.html?{}", qs)
    } else {
        "index.html".to_string()
    };

    let window = WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App(url.into()))
        .title("Kage Chat")
        .inner_size(800.0, 600.0)
        .min_inner_size(400.0, 300.0)
        .resizable(true)
        .center()
        .visible(false)
        .build()
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.create_failed",
                &[("reason", &e.to_string())],
            )
        })?;

    // Per-window event handler: track focus for "most recent chat
    // window" routing, and clean up bookkeeping on close.
    let label_for_event = label.clone();
    let app_for_event = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Focused(true) => {
            mark_focused_chat(&app_for_event, &label_for_event);
        }
        tauri::WindowEvent::Destroyed => {
            cleanup_chat_window_state(&app_for_event, &label_for_event);
        }
        _ => {}
    });

    let _ = window.show();
    let _ = window.set_focus();

    // Opening a chat window cancels any pending agent-shutdown task
    // — the user came back within the grace window, agent stays.
    cancel_chat_shutdown(&app);

    crate::setup::update_activation_policy(&app);
    let open_count = count_open_chat_windows(&app);
    crate::telemetry::track(
        &app,
        "chat_window_opened",
        Some(serde_json::json!({
            "resumed": resume_session_id.is_some(),
            "open_count_bucket": chat_window_count_bucket(open_count),
        })),
    );

    Ok(label)
}

/// Bucket the simultaneously-open chat-window count for telemetry.
/// Raw counts can fingerprint power users (someone opening 17 windows
/// is identifiable from one event); buckets keep the dashboard useful
/// without exposing individual behaviour.
fn chat_window_count_bucket(n: usize) -> &'static str {
    match n {
        0 | 1 => "1",
        2 => "2",
        3 => "3",
        4..=5 => "4-5",
        _ => "6+",
    }
}

fn count_open_chat_windows(app: &tauri::AppHandle) -> usize {
    use tauri::Manager;
    app.webview_windows()
        .keys()
        .filter(|label| is_session_host_label(label.as_str()))
        .count()
}

/// Number of chat windows currently *visible* (not just constructed).
/// `main` is created at app launch but typically hidden until the user
/// brings it up; for the agent-shutdown decision we treat hidden the
/// same as closed. Falls back to "exists" if Tauri can't report
/// visibility (treat as visible to err on the side of keeping the
/// agent up).
fn count_visible_chat_windows(app: &tauri::AppHandle) -> usize {
    use tauri::Manager;
    app.webview_windows()
        .iter()
        .filter(|(label, _)| is_session_host_label(label.as_str()))
        .filter(|(_, w)| w.is_visible().unwrap_or(true))
        .count()
}

/// Close a chat window by its label and clear its session bookkeeping.
/// Refuses to close `main` or `floating` — those persist for the app's
/// lifetime; the caller should hide them instead.
#[tauri::command]
pub async fn close_chat_window(
    label: String,
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    if label == window_labels::MAIN || label == window_labels::FLOATING {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.window.privileged_close_refused",
            &[("label", &label)],
        ));
    }
    if !is_chat_label(&label) {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.window.non_chat_close_refused",
            &[("label", &label)],
        ));
    }

    if let Some(window) = app.get_webview_window(&label) {
        window.close().map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.show_failed",
                &[("reason", &e.to_string())],
            )
        })?;
    }

    if let Ok(mut ws) = ui.window_sessions.lock() {
        ws.remove(&label);
    }
    if let Ok(mut originators) = ui.pending_prompt_originators.lock() {
        originators.retain(|_, owner| owner != &label);
    }

    info!("Closed chat window: {}", label);
    Ok(())
}

#[derive(serde::Serialize)]
pub struct ChatWindowInfo {
    pub label: String,
    pub session_id: Option<String>,
}

/// Enumerate all chat-* windows and main, with their pinned session
/// ids (if any). Used by the tray submenu and by the single-instance
/// handler to decide which window to focus.
#[tauri::command]
pub async fn list_chat_windows(
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Vec<ChatWindowInfo>, AppError> {
    let sessions = ui
        .window_sessions
        .lock()
        .map(|m| m.clone())
        .unwrap_or_default();

    let mut out: Vec<ChatWindowInfo> = app
        .webview_windows()
        .keys()
        .filter(|label| is_session_host_label(label.as_str()))
        .map(|label| ChatWindowInfo {
            label: label.to_string(),
            session_id: sessions.get(label.as_str()).cloned(),
        })
        .collect();

    // Stable order: main first, then chat-* by label (insertion-time
    // ordering would be nicer but Tauri's webview_windows() doesn't
    // expose creation order).
    out.sort_by(|a, b| {
        let a_is_main = a.label == window_labels::MAIN;
        let b_is_main = b.label == window_labels::MAIN;
        match (a_is_main, b_is_main) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.label.cmp(&b.label),
        }
    });
    Ok(out)
}

#[tauri::command]
pub async fn resize_floating_window(
    window: WebviewWindow,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(), AppError> {
    let current_size = window.inner_size().map_err(|e| e.to_string())?;

    let target_width = width.unwrap_or(current_size.width);
    let target_height = height.unwrap_or(current_size.height);

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: target_width,
            height: target_height,
        }))
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.show_failed",
                &[("reason", &e.to_string())],
            )
        })
}

#[tauri::command]
pub async fn open_settings_window(
    app: tauri::AppHandle,
    section: Option<String>,
    sub_section: Option<String>,
) -> Result<(), AppError> {
    use tauri::WebviewWindowBuilder;
    info!(
        "Opening settings window (section: {:?}, sub: {:?})",
        section, sub_section
    );
    // Reuse an existing settings window if it's already up (user
    // already opened it, then dismissed without closing). Otherwise
    // build a fresh one — settings is excluded from the initial
    // tauri.conf.json windows array so we don't pay for the WebView2
    // process + its JS init at every app launch (it calls
    // detect_agents on startup, which used to flash DOS windows for
    // each preset's `where`/`--version` probe).
    let already_open = app.get_webview_window(window_labels::SETTINGS).is_some();
    let window = if let Some(w) = app.get_webview_window(window_labels::SETTINGS) {
        w
    } else {
        // Encode section/subsection in the URL so the fresh window's
        // boot path can apply them synchronously. Going through the
        // event channel races: we'd `emit()` immediately after `show()`
        // but the new webview's listener isn't registered until its
        // JS finishes booting, so the event is silently lost. Query
        // params survive that race because they're attached to the
        // initial document URL, available to JS the moment it runs.
        // Section + subsection IDs are caller-supplied but always
        // simple ASCII identifiers (e.g. 'updates', 'changelog') —
        // they're code-defined, not user-input. Filter to that shape
        // defensively so a future caller passing user content can't
        // smuggle URL syntax. Anything outside [A-Za-z0-9_-] is
        // dropped silently.
        fn safe_id(s: &str) -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect()
        }
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(s) = section.as_deref() {
            let safe = safe_id(s);
            if !safe.is_empty() {
                params.push(("section", safe));
            }
        }
        if let Some(s) = sub_section.as_deref() {
            let safe = safe_id(s);
            if !safe.is_empty() {
                params.push(("subsection", safe));
            }
        }
        let mut url = String::from("settings.html");
        for (i, (k, v)) in params.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(k);
            url.push('=');
            url.push_str(v);
        }
        WebviewWindowBuilder::new(
            &app,
            window_labels::SETTINGS,
            tauri::WebviewUrl::App(url.into()),
        )
        .title("Settings - Kage")
        .inner_size(800.0, 700.0)
        .min_inner_size(600.0, 450.0)
        .resizable(true)
        .center()
        .visible(false) // shown below after we know it built
        .build()
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.create_failed",
                &[("reason", &e.to_string())],
            )
        })?
    };

    let _ = window.show();
    let _ = window.set_focus();
    // Only emit nav events when the window already existed — the
    // freshly-built case picked them up via URL params above. Doing
    // both would re-invoke switchSection redundantly (harmless but
    // noisy) and could trigger a flash of a different section.
    if already_open {
        // The settings window is the only listener — `WebviewWindow::emit`
        // would fan out to every webview, but the cost adds up if a user
        // re-opens settings repeatedly. Scope it.
        if let Some(ref s) = section {
            crate::event_targets::emit_to_settings(&app, "navigate_settings_section", s);
        }
        if let Some(ref sub) = sub_section {
            crate::event_targets::emit_to_settings(&app, "navigate_settings_subsection", sub);
        }
    }
    crate::setup::update_activation_policy(&app);
    crate::telemetry::track(
        &app,
        "settings_opened",
        section
            .as_deref()
            .map(|s| serde_json::json!({ "section": s })),
    );
    Ok(())
}

/// Logical (DIP) size of the context-menu window. Kept here as a single
/// source of truth — used both for the initial WebviewWindowBuilder and
/// for clamping math when the window's outer_size() isn't available
/// yet (the very first show, before the WebView2 process has reported
/// its rendered size).
const CONTEXT_MENU_LOGICAL_W: f64 = 160.0;
const CONTEXT_MENU_LOGICAL_H: f64 = 220.0;

#[tauri::command]
pub async fn show_context_menu(x: i32, y: i32, app: tauri::AppHandle) -> Result<(), AppError> {
    use tauri::WebviewWindowBuilder;
    crate::telemetry::track(&app, "context_menu_shown", None);

    // Build the window on first show. Excluded from tauri.conf.json's
    // initial windows so we don't pay for a WebView2 process at every
    // launch — the menu is opened rarely and via explicit user action.
    let window = if let Some(w) = app.get_webview_window(window_labels::CONTEXT_MENU) {
        w
    } else {
        let w = WebviewWindowBuilder::new(
            &app,
            window_labels::CONTEXT_MENU,
            tauri::WebviewUrl::App("context-menu.html".into()),
        )
        .title("")
        .inner_size(CONTEXT_MENU_LOGICAL_W, CONTEXT_MENU_LOGICAL_H)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.create_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        // Match what configure_transparent_windows did for the
        // preloaded version. set_shadow is Windows-only on the trait.
        let _ = w.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        #[cfg(target_os = "windows")]
        let _ = w.set_shadow(false);
        w
    };

    let mut final_x = x;
    let mut final_y = y;

    // Find which monitor the click is on and clamp to its bounds
    if let Some(monitor) = find_monitor_at_position(&window, x, y) {
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let scale = monitor.scale_factor();

        let mon_right = mon_pos.x + mon_size.width as i32;
        let mon_bottom = mon_pos.y + mon_size.height as i32;
        // outer_size() can be 0×0 on a freshly-built window that hasn't
        // rendered yet. Fall back to the configured logical size in that
        // case so first-show clamping still works.
        let outer = window.outer_size().unwrap_or(tauri::PhysicalSize {
            width: 0,
            height: 0,
        });
        let menu_w = if outer.width > 0 {
            (outer.width as f64 / scale) as i32
        } else {
            CONTEXT_MENU_LOGICAL_W as i32
        };
        let menu_h = if outer.height > 0 {
            (outer.height as f64 / scale) as i32
        } else {
            CONTEXT_MENU_LOGICAL_H as i32
        };

        // If menu would overflow right edge, flip to left of cursor
        if final_x + menu_w > mon_right {
            final_x = (mon_right - menu_w).max(mon_pos.x);
        }
        // If menu would overflow bottom edge, flip upward
        if final_y + menu_h > mon_bottom {
            final_y = (mon_bottom - menu_h).max(mon_pos.y);
        }
    }

    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: final_x,
            y: final_y,
        }))
        .map_err(|e| format!("Failed to position context menu: {}", e))?;
    window
        .show()
        .map_err(|e| format!("Failed to show context menu: {}", e))?;
    window
        .set_focus()
        .map_err(|e| format!("Failed to focus context menu: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn set_floating_opacity(app: tauri::AppHandle, opacity: f64) -> Result<(), AppError> {
    // Opacity is applied via CSS in the frontend (body opacity).
    // This command exists so the frontend can trigger it via config_updated.
    // The actual application happens in floating-theme.js loadAndApplyTheme().
    let _ = app; // Acknowledge the handle
    let _ = opacity;
    Ok(())
}

#[tauri::command]
pub async fn apply_chat_window_size(
    app: tauri::AppHandle,
    width: u32,
    height: u32,
) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window(window_labels::MAIN) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let phys_width = (width as f64 * scale) as u32;
        let phys_height = (height as f64 * scale) as u32;
        window
            .set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: phys_width,
                height: phys_height,
            }))
            .map_err(|e| format!("Failed to resize chat window: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn save_window_position(
    features: tauri::State<'_, crate::state::FeatureServices>,
    x: i32,
    y: i32,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.ui.last_window_x = Some(x);
    config.ui.last_window_y = Some(y);
    config
        .save()
        .map_err(|e| format!("Failed to save window position: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn save_chat_window_geometry(
    features: tauri::State<'_, crate::state::FeatureServices>,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
) -> Result<(), AppError> {
    let mut config = features.config.lock_or_recover();
    config.ui.chat_window_width = width;
    config.ui.chat_window_height = height;
    config.ui.chat_window_x = Some(x);
    config.ui.chat_window_y = Some(y);
    config
        .save()
        .map_err(|e| format!("Failed to save chat window geometry: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn get_last_selection(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let sel = ui.last_selection.lock().map_err(|e| e.to_string())?;
    Ok(sel.clone())
}

// `set_notification_source` and `show_notification_source_window`
// existed under the single-session model where one global string told
// the backend which window owned the in-flight prompt. Permission
// routing is now done per-session via
// `UiState.pending_prompt_originators` — see
// `commands::messaging::handle_permission_notification`.
/// Called by the floating window frontend once `init()` completes.
/// Used purely as a diagnostic / telemetry beacon — the backend log line
/// "Frontend signaled ready" is the canonical signal that the floating
/// JS bundle loaded and ran end-to-end. If you ever see hotkey presses
/// without this line preceding them, the JS module graph aborted (e.g.
/// a top-level `window.__TAURI__` access racing the global injection).
///
/// We used to gate `show_floating_at_mouse` / `toggle_floating_window`
/// on the boolean this command sets, to suppress a brief flash of an
/// unthemed window during the first ~200ms of startup. That gate was
/// removed because Tauri doesn't load the floating window's HTML+JS
/// until the window first shows (the window is configured
/// `visible: false`), which made the gate a permanent deadlock if the
/// JS init ever failed: window never shows → JS never runs →
/// frontend_ready never flips → window never shows. A one-frame flash
/// is fine; a permanently dead app is not.
#[tauri::command]
pub fn notify_frontend_ready(ui: tauri::State<'_, crate::state::UiState>) {
    info!("Frontend signaled ready");
    ui.frontend_ready
        .store(true, std::sync::atomic::Ordering::Release);
}

/// Get the source window info (title, process_name) captured when the hotkey was pressed.
#[tauri::command]
pub async fn get_source_window(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<serde_json::Value>, AppError> {
    let sw = ui.source_window.lock().map_err(|e| e.to_string())?;
    match sw.as_ref() {
        Some((title, process_name)) => Ok(Some(serde_json::json!({
            "title": title,
            "processName": process_name,
        }))),
        None => Ok(None),
    }
}

/// Get a shallow accessibility tree snapshot of the source window for screen context.
/// Uses depth 2 to keep it fast and small. Returns a compact text representation.
#[tauri::command]
pub async fn get_screen_context(
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<Option<String>, AppError> {
    let title = {
        let sw = ui.source_window.lock().map_err(|e| e.to_string())?;
        match sw.as_ref() {
            Some((title, _)) => title.clone(),
            None => return Ok(None),
        }
    }; // MutexGuard dropped here

    // Spawn on a blocking thread since UI Automation is COM-based
    let result = tokio::task::spawn_blocking(move || {
        crate::os::accessibility::get_ui_tree(Some(&title), 2, false)
    })
    .await
    .map_err(|e| format!("Join error: {}", e))?;

    match result {
        Ok(tree) => {
            let text = tree.to_text(0, 2);
            // Cap at 4KB to keep token usage reasonable
            if text.len() > 4096 {
                Ok(Some(text[..4096].to_string() + "\n... (truncated)"))
            } else {
                Ok(Some(text))
            }
        }
        Err(e) => {
            log::warn!("Screen context capture failed: {}", e);
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Inline Assist — context-aware AI popup at cursor position
// ---------------------------------------------------------------------------

/// Show the inline assist popup at the cursor position.
/// Captures the selected text and foreground window info first.
#[tauri::command]
pub async fn show_inline_assist(app: tauri::AppHandle) -> Result<(), AppError> {
    // When called as a Tauri command (not from hotkey), capture here
    let source_info = crate::os::window_list::get_foreground_window_info();
    let features: tauri::State<'_, crate::state::FeatureServices> = app.state();
    let blocklist = features
        .config
        .lock_or_recover()
        .system
        .capture_selection_blocklist
        .clone();
    let fg_process = source_info
        .as_ref()
        .map(|(_, proc)| proc.as_str())
        .unwrap_or("");
    let selection = if crate::os::clipboard::is_process_blocklisted(fg_process, &blocklist) {
        info!("[inline-assist] Skipping capture — foreground app '{fg_process}' is blocklisted");
        None
    } else {
        let capture_token = crate::os::clipboard::begin_selection_capture();
        crate::os::clipboard::finish_selection_capture(capture_token)
    };
    let cursor_pos = get_cursor_position().unwrap_or((500, 500));

    show_inline_assist_with_context(app, source_info, selection, cursor_pos).await
}

/// Show the inline assist popup with pre-captured context.
/// Called from the hotkey handler where capture happens synchronously.
pub async fn show_inline_assist_with_context(
    app: tauri::AppHandle,
    source_info: Option<(String, String)>,
    selection: Option<String>,
    cursor_pos: (i32, i32),
) -> Result<(), AppError> {
    info!(
        "show_inline_assist_with_context: source={:?}, selection_len={}, cursor=({},{})",
        source_info,
        selection.as_ref().map(|s| s.len()).unwrap_or(0),
        cursor_pos.0,
        cursor_pos.1
    );

    // Single firing point for both the command-driven and hotkey-driven
    // entry. has_selection lets us tell whether inline-assist is being
    // used to operate on highlighted text vs as a freeform prompt.
    crate::telemetry::track(
        &app,
        "inline_assist_shown",
        Some(serde_json::json!({
            "has_selection": selection.is_some(),
        })),
    );

    let ui: tauri::State<'_, crate::state::UiState> = app.state();

    // Store source window info for paste-back
    if let Ok(mut sw) = ui.source_window.lock() {
        *sw = source_info.clone();
    }

    // Show the inline assist window at cursor
    if let Some(window) = app.get_webview_window(window_labels::INLINE_ASSIST) {
        info!(
            "Found inline-assist window, positioning at ({}, {})",
            cursor_pos.0, cursor_pos.1
        );
        // Position near cursor, with screen edge clamping
        let mut x = cursor_pos.0;
        let mut y = cursor_pos.1;

        if let Some(monitor) = find_monitor_at_position(&window, x, y) {
            let mon_pos = monitor.position();
            let mon_size = monitor.size();
            let scale = monitor.scale_factor();
            let win_w = (300.0 * scale) as i32;
            let win_h = (320.0 * scale) as i32;
            let mon_right = mon_pos.x + mon_size.width as i32;
            let mon_bottom = mon_pos.y + mon_size.height as i32;

            if x + win_w > mon_right {
                x = (mon_right - win_w).max(mon_pos.x);
            }
            if y + win_h > mon_bottom {
                y = (mon_bottom - win_h).max(mon_pos.y);
            }
        }

        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
        let _ = window.show();
        let _ = window.set_focus();

        // Send the context to the frontend (delay to ensure JS module is loaded)
        let (app_name, title) = source_info.unwrap_or_default();
        let sel = selection.unwrap_or_default();
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let payload = serde_json::json!({
                "selection": sel,
                "app": app_name,
                "title": title,
            });
            crate::event_targets::emit_to_self(&window_clone, events::INLINE_ASSIST_SHOW, &payload);
        });
    } else {
        warn!("inline-assist window not found — is it defined in tauri.conf.json?");
    }

    Ok(())
}

/// Apply inline assist result: hide popup, focus source window, write to clipboard, paste.
#[tauri::command]
pub async fn inline_assist_apply(
    text: String,
    app: tauri::AppHandle,
    ui: tauri::State<'_, crate::state::UiState>,
) -> Result<(), AppError> {
    // Hide the inline assist window FIRST so it doesn't receive the paste
    if let Some(window) = app.get_webview_window(window_labels::INLINE_ASSIST) {
        let _ = window.hide();
    }

    // Small delay to let the window fully hide
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Focus the source window
    let source_title = {
        let sw = ui.source_window.lock().map_err(|e| e.to_string())?;
        sw.as_ref().map(|(title, _)| title.clone())
    };

    if let Some(title) = source_title {
        let windows = crate::os::list_windows();
        if let Some(win) = windows.iter().find(|w| w.title.contains(&title)) {
            let _ = crate::os::window_list::focus_window(win.handle);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    // Write result to clipboard and paste
    crate::os::clipboard::write_clipboard(&text);
    std::thread::sleep(std::time::Duration::from_millis(30));
    crate::os::simulate_paste();

    Ok(())
}
