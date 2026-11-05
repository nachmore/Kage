//! Settings, context-menu, and window-state commands.

use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::window_labels;
use log::{debug, info, warn};
use tauri::{Manager, WebviewWindow};

pub async fn resize_floating_window<R: tauri::Runtime>(
    window: WebviewWindow<R>,
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

pub async fn open_settings_window<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    section: Option<String>,
    sub_section: Option<String>,
) -> Result<(), AppError> {
    use crate::navigation_guard::NewWindowGuardExt;
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
        .guard_new_windows(&app)
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

/// Get the context-menu window, building it if needed. Excluded from
/// tauri.conf.json's initial windows so Windows doesn't pay for a WebView2
/// process at every launch. On macOS it's preloaded during setup instead
/// (`preload_context_menu`): wry activates the app whenever a WKWebView is
/// created, and doing that on the first right-click over a full-screen app
/// would pull the user back to the desktop Space.
pub fn ensure_context_menu_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<tauri::WebviewWindow<R>, AppError> {
    use crate::navigation_guard::NewWindowGuardExt;
    use tauri::WebviewWindowBuilder;
    if let Some(w) = app.get_webview_window(window_labels::CONTEXT_MENU) {
        return Ok(w);
    }
    let w = WebviewWindowBuilder::new(
        app,
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
    .guard_new_windows(app)
    .build()
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.window.create_failed",
            &[("reason", &e.to_string())],
        )
    })?;
    // Match what configure_transparent_windows did for the preloaded
    // overlays. set_shadow is Windows-only on the trait.
    let _ = w.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    #[cfg(target_os = "windows")]
    let _ = w.set_shadow(false);
    // Right-clicking the launcher over a full-screen app must show the
    // menu there too, not on the desktop Space.
    crate::overlay_windows::configure(&w);
    Ok(w)
}

/// macOS: build the context menu at startup (see
/// `ensure_context_menu_window`). No-op elsewhere.
pub fn preload_context_menu<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    #[cfg(target_os = "macos")]
    if let Err(e) = ensure_context_menu_window(app) {
        log::warn!("[overlay] context menu preload failed: {}", e);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

pub async fn show_context_menu<R: tauri::Runtime>(
    x: i32,
    y: i32,
    app: tauri::AppHandle<R>,
) -> Result<(), AppError> {
    crate::telemetry::track(&app, "context_menu_shown", None);

    let window = ensure_context_menu_window(&app)?;

    // `(x, y)` is the click in *logical* screen coordinates. Positioning
    // with those as physical pixels put the menu at half the distance from
    // the screen origin on a 2× display — up and to the left of the cursor.
    let position = context_menu_position(&window, x, y);
    window
        .set_position(position)
        .map_err(|e| format!("Failed to position context menu: {}", e))?;
    crate::overlay_windows::present(&window)
        .map_err(|e| format!("Failed to show context menu: {}", e))?;
    Ok(())
}

/// Where to put the context menu's top-left for a click at logical screen
/// point `(x, y)`, flipped left/up to stay on the click's monitor.
///
/// macOS: global window coordinates *are* points, so position logically —
/// exact on every monitor regardless of its scale (a physical position is
/// converted back with the menu window's current scale, which is wrong
/// whenever the menu last sat on a display with a different one).
/// Windows/Linux: physical pixels are native, and the OS cursor position is
/// already physical and exact; the click is the fallback.
fn context_menu_position<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    x: i32,
    y: i32,
) -> tauri::Position {
    let monitors = window.available_monitors().unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        let (lx, ly) = (x as f64, y as f64);
        let mut pos = (lx, ly);
        if let Some(m) = monitors.iter().find(|m| logical_rect(m).contains(lx, ly)) {
            let r = logical_rect(m);
            let s = m.scale_factor();
            let (w, h) = menu_size_logical(window, s);
            pos = clamp_rect(r, lx, ly, w, h);
        }
        tauri::Position::Logical(tauri::LogicalPosition { x: pos.0, y: pos.1 })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (px, py) = match window.cursor_position() {
            Ok(p) => (p.x.round(), p.y.round()),
            Err(e) => {
                log::debug!("[context-menu] cursor_position failed ({e}); using the click");
                let s = monitors
                    .iter()
                    .find(|m| logical_rect(m).contains(x as f64, y as f64))
                    .map(|m| m.scale_factor())
                    .or_else(|| window.scale_factor().ok())
                    .unwrap_or(1.0);
                ((x as f64 * s).round(), (y as f64 * s).round())
            }
        };
        let mut pos = (px, py);
        if let Some(m) = monitors.iter().find(|m| physical_rect(m).contains(px, py)) {
            let s = m.scale_factor();
            let (w, h) = menu_size_logical(window, s);
            pos = clamp_rect(physical_rect(m), px, py, w * s, h * s);
        }
        tauri::Position::Physical(tauri::PhysicalPosition {
            x: pos.0.round() as i32,
            y: pos.1.round() as i32,
        })
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// A monitor's rect in its own logical units (physical ÷ its scale).
fn logical_rect(m: &tauri::Monitor) -> Rect {
    let s = m.scale_factor();
    Rect {
        x: m.position().x as f64 / s,
        y: m.position().y as f64 / s,
        w: m.size().width as f64 / s,
        h: m.size().height as f64 / s,
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn physical_rect(m: &tauri::Monitor) -> Rect {
    Rect {
        x: m.position().x as f64,
        y: m.position().y as f64,
        w: m.size().width as f64,
        h: m.size().height as f64,
    }
}

/// The menu's logical size: its rendered size if known, else the
/// configured one (a freshly built window can still report 0×0).
fn menu_size_logical<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    scale: f64,
) -> (f64, f64) {
    match window.outer_size() {
        Ok(s) if s.width > 0 && s.height > 0 => (s.width as f64 / scale, s.height as f64 / scale),
        _ => (CONTEXT_MENU_LOGICAL_W, CONTEXT_MENU_LOGICAL_H),
    }
}

/// Keep a `w`×`h` box whose top-left is `(x, y)` inside `r`: shift it back
/// from the right/bottom edge, never past the top-left.
fn clamp_rect(r: Rect, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    let cx = if x + w > r.x + r.w {
        (r.x + r.w - w).max(r.x)
    } else {
        x
    };
    let cy = if y + h > r.y + r.h {
        (r.y + r.h - h).max(r.y)
    } else {
        y
    };
    (cx, cy)
}

pub async fn set_floating_opacity<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    opacity: f64,
) -> Result<(), AppError> {
    // Opacity is applied via CSS in the frontend (body opacity).
    // This command exists so the frontend can trigger it via config_updated.
    // The actual application happens in floating-theme.js loadAndApplyTheme().
    let _ = app; // Acknowledge the handle
    let _ = opacity;
    Ok(())
}

pub async fn apply_chat_window_size<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
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
pub fn notify_frontend_ready(ui: tauri::State<'_, crate::state::UiState>) {
    info!("Frontend signaled ready");
    ui.frontend_ready
        .store(true, std::sync::atomic::Ordering::Release);
}

/// Show a "response ready" style notification whose click foregrounds the
/// window named by `target_label` (e.g. "floating", "main", "chat-<uuid>").
///
/// Returns `Ok(true)` if a *clickable* notification was shown (Windows,
/// installed build). Returns `Ok(false)` when the platform can't deliver a
/// click — the frontend then falls back to the plain `sendNotification` plugin
/// toast so the user still sees the message, just without click-to-open.
///
/// Why this exists: `tauri-plugin-notification` has no desktop click callback
/// (its `onAction` is mobile-only), so clicking a completion toast did nothing.
/// See src/os/notification.rs and src/os/windows/notification.rs.
pub async fn notify_response_ready<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    title: String,
    body: String,
    target_label: String,
) -> Result<bool, AppError> {
    // WinRT toast activation only routes to an app with a registered AUMID.
    // The installed build has one (config.identifier); dev builds run under
    // the PowerShell AUMID, so a click wouldn't reach us — mirror the
    // notification plugin's own "installed vs dev" gate and bail to the
    // fallback path in dev. (The plugin checks the exe dir; we reuse the same
    // rule so both agree.)
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.display().to_string()));
    let is_installed = exe_dir
        .as_deref()
        .map(|dir| {
            let sep = std::path::MAIN_SEPARATOR;
            !(dir.ends_with(&format!("{sep}target{sep}debug"))
                || dir.ends_with(&format!("{sep}target{sep}release")))
        })
        .unwrap_or(false);
    debug!(
        "notify_response_ready: target='{}' installed={} exe_dir={:?}",
        target_label, is_installed, exe_dir
    );
    if !is_installed {
        debug!(
            "notify_response_ready: dev build — falling back to plugin toast (no clickable path)"
        );
        return Ok(false);
    }

    let aumid = app.config().identifier.clone();
    debug!("notify_response_ready: showing clickable WinRT toast (aumid='{aumid}')");

    // The click callback runs on a WinRT callback thread; window ops are
    // thread-affine, so hop to the main thread before touching them.
    let app_for_click = app.clone();
    let shown = crate::os::notification::show_clickable(
        &aumid,
        &title,
        &body,
        Box::new(move || {
            info!("notify_response_ready: toast activated (clicked) — foregrounding window");
            let app = app_for_click.clone();
            let label = target_label.clone();
            if let Err(e) = app.clone().run_on_main_thread(move || {
                // Prefer the requested window; fall back to main so a click
                // never dead-ends (e.g. a closed chat-<uuid> window).
                let resolved = app
                    .get_webview_window(&label)
                    .map(|w| (label.clone(), w))
                    .or_else(|| {
                        app.get_webview_window(window_labels::MAIN)
                            .map(|w| (window_labels::MAIN.to_string(), w))
                    });
                match resolved {
                    Some((resolved_label, win)) => {
                        info!(
                            "notify_response_ready: activation foregrounding window '{resolved_label}' (requested '{label}')"
                        );
                        if let Err(e) = win.unminimize() {
                            warn!("notify_response_ready: unminimize failed: {e}");
                        }
                        if let Err(e) = win.show() {
                            warn!("notify_response_ready: show failed: {e}");
                        }
                        if let Err(e) = win.set_focus() {
                            warn!("notify_response_ready: set_focus failed: {e}");
                        }
                    }
                    None => {
                        warn!(
                            "notify_response_ready: activation found no window for '{label}' or main — nothing to foreground"
                        );
                    }
                }
                crate::setup::update_activation_policy(&app);
            }) {
                warn!("notify_response_ready: run_on_main_thread failed on activation: {e}");
            }
        }),
    );

    debug!("notify_response_ready: show_clickable returned shown={shown}");
    Ok(shown)
}

/// Get the source window info (title, process_name) captured when the hotkey was pressed.
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

#[cfg(test)]
mod context_menu_tests {
    use super::{clamp_rect, Rect};

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1512.0,
        h: 982.0,
    };

    #[test]
    fn stays_at_the_click_when_it_fits() {
        assert_eq!(
            clamp_rect(SCREEN, 300.0, 200.0, 160.0, 220.0),
            (300.0, 200.0)
        );
    }

    #[test]
    fn flips_back_from_the_right_and_bottom_edges() {
        assert_eq!(
            clamp_rect(SCREEN, 1500.0, 900.0, 160.0, 220.0),
            (1352.0, 762.0)
        );
    }

    #[test]
    fn never_moves_past_the_monitor_origin() {
        let tiny = Rect {
            x: 100.0,
            y: 50.0,
            w: 120.0,
            h: 100.0,
        };
        assert_eq!(clamp_rect(tiny, 150.0, 60.0, 160.0, 220.0), (100.0, 50.0));
    }

    #[test]
    fn works_on_a_secondary_monitor_with_negative_origin() {
        let left = Rect {
            x: -1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert!(left.contains(-10.0, 500.0));
        assert_eq!(
            clamp_rect(left, -10.0, 500.0, 160.0, 220.0),
            (-160.0, 500.0)
        );
    }
}
