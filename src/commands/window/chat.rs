//! Chat-window lifecycle and shutdown coordination.

use super::floating::{center_window_on_active_monitor, get_active_monitor};
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::window_labels::{self, is_chat_label, is_session_host_label};
use log::{info, warn};
use tauri::Manager;

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
