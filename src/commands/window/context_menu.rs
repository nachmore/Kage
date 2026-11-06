//! The custom right-click menu: one cached popup window that renders
//! whatever menu its caller describes.
//!
//! # Protocol
//!
//! 1. A window calls `show_context_menu { x, y, menu? }`. `menu.items` is
//!    the list to show (id, i18n key or label, optional shortcut hint,
//!    enabled / danger / separator flags); without it the menu window
//!    shows its default text-editing menu. `menu.context` is opaque JSON
//!    handed back with the choice; `menu.keyboard` means the menu was
//!    opened from the keyboard (anchor at `(x, y)`, not the cursor, and
//!    highlight the first item).
//! 2. The backend sanitises the items, records the request as *pending*
//!    under a fresh token, and sends it to the menu window
//!    (`CONTEXT_MENU_RENDER`). A freshly built menu window that missed
//!    the event pulls it with `get_pending_context_menu`.
//! 3. The menu window renders the items, measures them and calls
//!    `context_menu_ready { token, width, height, rtl }`; the backend sizes
//!    the window to fit, clamps it onto the click's monitor and shows it
//!    (right-to-left: the menu's right edge at the anchor, as native RTL
//!    menus open). A stale token (a newer menu was requested meanwhile) is
//!    ignored.
//! 4. The menu window emits `context_menu_action { token, action,
//!    context }` to the requesting window — `action` is the item id, or
//!    `"dismissed"` — then asks to be hidden with `hide_context_menu
//!    { token }`. The backend ignores that hide if a newer menu has been
//!    shown since: presenting and hiding are serialised (`PRESENTED`), so a
//!    hide that was still in flight can never hide the menu that replaced
//!    the one it was for.

use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use crate::window_labels;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

/// Logical size the window is built with — only until the first menu
/// measures itself; every menu is sized to fit its items.
const CONTEXT_MENU_INITIAL_W: f64 = 160.0;
const CONTEXT_MENU_INITIAL_H: f64 = 220.0;
/// Sanity bounds on the size a menu may report (logical px).
const MENU_MIN_W: f64 = 40.0;
const MENU_MIN_H: f64 = 20.0;
const MENU_MAX_W: f64 = 640.0;
const MENU_MAX_H: f64 = 1200.0;
const MAX_ITEMS: usize = 40;
const MAX_ID_LEN: usize = 64;
const MAX_KEY_LEN: usize = 128;
const MAX_LABEL_CHARS: usize = 200;
const MAX_SHORTCUT_CHARS: usize = 32;
const MAX_LABEL_VARS: usize = 8;
const MAX_CONTEXT_BYTES: usize = 16 * 1024;

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuItem {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_vars: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub danger: bool,
    #[serde(default)]
    pub separator: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuRequest {
    #[serde(default)]
    pub items: Option<Vec<MenuItem>>,
    #[serde(default)]
    pub context: Option<serde_json::Value>,
    #[serde(default)]
    pub keyboard: bool,
}

/// What the menu window renders (`CONTEXT_MENU_RENDER` payload).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuRender {
    pub token: u64,
    /// Label of the window that asked; the choice is emitted to it.
    pub owner: String,
    /// `None`: the default text menu.
    pub items: Option<Vec<MenuItem>>,
    pub context: Option<serde_json::Value>,
    pub keyboard: bool,
}

/// Where the menu's top-left goes before clamping: points on macOS,
/// physical pixels elsewhere (see `context_menu_anchor`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Anchor {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Logical(f64, f64),
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Physical(f64, f64),
}

struct PendingMenu {
    anchor: Anchor,
    render: MenuRender,
    presented: bool,
}

static PENDING: Mutex<Option<PendingMenu>> = Mutex::new(None);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// The newest menu token shown. Held across a show's or a hide's window
/// calls, so the two are applied in the order they were decided.
static PRESENTED: tokio::sync::Mutex<Presented> = tokio::sync::Mutex::const_new(Presented(0));

/// The newest token presented so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Presented(u64);

impl Presented {
    /// Whether a hide requested by menu `token` may hide the window: not if
    /// a newer menu has been presented since.
    fn may_hide(self, token: u64) -> bool {
        token >= self.0
    }

    fn record(&mut self, token: u64) {
        self.0 = self.0.max(token);
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

fn valid_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_KEY_LEN
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.'))
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Make caller-supplied items safe to render: at most `MAX_ITEMS`; ids,
/// i18n keys, labels and hints bounded; label variables limited to a few
/// strings/numbers; an item without a usable id and label dropped; and
/// separators collapsed so none leads, trails or doubles up.
pub fn sanitize_menu_items(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut out: Vec<MenuItem> = Vec::new();
    for item in items.into_iter().take(MAX_ITEMS) {
        if item.separator {
            if out.last().is_some_and(|last| !last.separator) {
                out.push(MenuItem {
                    separator: true,
                    enabled: false,
                    ..Default::default()
                });
            }
            continue;
        }
        if !valid_id(&item.id) {
            continue;
        }
        let label_key = item.label_key.filter(|k| valid_key(k));
        let label = item
            .label
            .map(|l| truncate_chars(l.trim(), MAX_LABEL_CHARS))
            .filter(|l| !l.is_empty());
        if label_key.is_none() && label.is_none() {
            continue;
        }
        let label_vars = item.label_vars.map(|vars| {
            vars.into_iter()
                .filter(|(k, v)| valid_id(k) && (v.is_string() || v.is_number()))
                .take(MAX_LABEL_VARS)
                .map(|(k, v)| match v {
                    serde_json::Value::String(s) => (
                        k,
                        serde_json::Value::String(truncate_chars(&s, MAX_LABEL_CHARS)),
                    ),
                    other => (k, other),
                })
                .collect()
        });
        out.push(MenuItem {
            id: item.id,
            label_key,
            label,
            label_vars,
            shortcut: item
                .shortcut
                .map(|s| truncate_chars(s.trim(), MAX_SHORTCUT_CHARS))
                .filter(|s| !s.is_empty()),
            enabled: item.enabled,
            danger: item.danger,
            separator: false,
        });
    }
    while out.last().is_some_and(|i| i.separator) {
        out.pop();
    }
    out
}

/// The size the menu reported, bounded to sane values (logical px).
fn bounded_menu_size(width: f64, height: f64) -> Option<(f64, f64)> {
    (width.is_finite() && height.is_finite()).then(|| {
        (
            width.clamp(MENU_MIN_W, MENU_MAX_W),
            height.clamp(MENU_MIN_H, MENU_MAX_H),
        )
    })
}

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
    .inner_size(CONTEXT_MENU_INITIAL_W, CONTEXT_MENU_INITIAL_H)
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

/// Request a menu; returns its token (echoed in `context_menu_action`).
pub async fn show_context_menu<R: tauri::Runtime>(
    x: i32,
    y: i32,
    menu: Option<MenuRequest>,
    caller: tauri::WebviewWindow<R>,
    app: tauri::AppHandle<R>,
) -> Result<u64, AppError> {
    let invalid = || AppError::keyed(ErrorKind::Internal, "errors.window.menu_invalid", &[]);
    let request = menu.unwrap_or_default();
    let items = match request.items {
        Some(items) => {
            let items = sanitize_menu_items(items);
            if items.is_empty() {
                return Err(invalid());
            }
            Some(items)
        }
        None => None,
    };
    if request
        .context
        .as_ref()
        .is_some_and(|c| c.to_string().len() > MAX_CONTEXT_BYTES)
    {
        return Err(invalid());
    }
    crate::telemetry::track(&app, "context_menu_shown", None);

    let window = ensure_context_menu_window(&app)?;
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let render = MenuRender {
        token,
        owner: caller.label().to_string(),
        items,
        context: request.context,
        keyboard: request.keyboard,
    };
    *PENDING.lock_or_recover() = Some(PendingMenu {
        anchor: context_menu_anchor(&caller, x, y, !request.keyboard),
        render: render.clone(),
        presented: false,
    });
    window
        .emit_to(
            window_labels::CONTEXT_MENU,
            crate::events::CONTEXT_MENU_RENDER,
            &render,
        )
        .map_err(|e| AppError::internal(format!("Failed to send the context menu: {e}")))?;
    Ok(token)
}

/// The menu request the menu window hasn't shown yet — for a window that
/// was still loading when `CONTEXT_MENU_RENDER` went out.
pub fn get_pending_context_menu() -> Option<MenuRender> {
    PENDING
        .lock_or_recover()
        .as_ref()
        .filter(|p| !p.presented)
        .map(|p| p.render.clone())
}

/// The menu window measured menu `token` at `width`×`height` (logical px):
/// size the window to fit, place it at the anchor clamped onto that
/// monitor, and show it. `Ok(false)` for a stale or repeated token.
pub async fn context_menu_ready<R: tauri::Runtime>(
    token: u64,
    width: f64,
    height: f64,
    rtl: bool,
    app: tauri::AppHandle<R>,
) -> Result<bool, AppError> {
    let Some((w, h)) = bounded_menu_size(width, height) else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.window.menu_invalid",
            &[],
        ));
    };
    // Ordered against `hide_context_menu`: whichever takes this first
    // applies first.
    let mut presented = PRESENTED.lock().await;
    let anchor = {
        let mut pending = PENDING.lock_or_recover();
        match pending.as_mut() {
            Some(p) if p.render.token == token && !p.presented => {
                p.presented = true;
                p.anchor
            }
            _ => return Ok(false),
        }
    };
    let window = app
        .get_webview_window(window_labels::CONTEXT_MENU)
        .ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.window.not_found",
                &[("label", window_labels::CONTEXT_MENU)],
            )
        })?;
    let (size, position) = context_menu_frame(&window, anchor, w, h, rtl);
    window
        .set_position(position)
        .map_err(|e| format!("Failed to position context menu: {}", e))?;
    window
        .set_size(size)
        .map_err(|e| format!("Failed to size context menu: {}", e))?;
    presented.record(token);
    crate::overlay_windows::present(&window)
        .map_err(|e| format!("Failed to show context menu: {}", e))?;
    Ok(true)
}

/// The menu window is done with menu `token` (answered or dismissed):
/// hide it — unless a newer menu has been presented meanwhile, which a
/// late hide must not take down. `Ok(false)` when ignored.
pub async fn hide_context_menu<R: tauri::Runtime>(
    token: u64,
    app: tauri::AppHandle<R>,
) -> Result<bool, AppError> {
    let presented = PRESENTED.lock().await;
    if !presented.may_hide(token) {
        log::debug!("[context-menu] ignoring the hide for menu {token}: a newer one is up");
        return Ok(false);
    }
    let Some(window) = app.get_webview_window(window_labels::CONTEXT_MENU) else {
        return Ok(false);
    };
    window
        .hide()
        .map_err(|e| AppError::internal(format!("Failed to hide context menu: {e}")))?;
    drop(presented);
    Ok(true)
}

/// Where a menu opened at screen point `(x, y)` starts. The caller
/// computes that point as its window's position divided by its own scale
/// plus the offset inside the window (`ui/js/floating/context-menu.js`):
/// logical units *of the caller's monitor*.
///
/// macOS: global window coordinates *are* points, the same on every
/// monitor whatever its scale — stay logical. Windows/Linux: physical
/// pixels are native, and "logical" differs per monitor, so a point can't
/// be placed by testing it against each monitor's own logical rectangle
/// (on a 100% + 200% pair those overlap). A mouse-opened menu uses the OS
/// cursor position (physical and exact); a keyboard-opened one, or a
/// failed cursor query, undoes the caller's own division: × the caller
/// window's scale.
fn context_menu_anchor<R: tauri::Runtime>(
    caller: &tauri::WebviewWindow<R>,
    x: i32,
    y: i32,
    at_cursor: bool,
) -> Anchor {
    #[cfg(target_os = "macos")]
    {
        let _ = (caller, at_cursor);
        Anchor::Logical(x as f64, y as f64)
    }
    #[cfg(not(target_os = "macos"))]
    {
        if at_cursor {
            match caller.cursor_position() {
                Ok(p) => return Anchor::Physical(p.x.round(), p.y.round()),
                Err(e) => {
                    log::debug!("[context-menu] cursor_position failed ({e}); using the click")
                }
            }
        }
        let scale = caller.scale_factor().unwrap_or(1.0);
        let (px, py) = caller_point_to_physical(x as f64, y as f64, scale);
        Anchor::Physical(px, py)
    }
}

/// A point in the caller's logical units (`position / scale + offset`)
/// as physical pixels: `position + offset × scale`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn caller_point_to_physical(x: f64, y: f64, caller_scale: f64) -> (f64, f64) {
    let s = if caller_scale.is_finite() && caller_scale > 0.0 {
        caller_scale
    } else {
        1.0
    };
    ((x * s).round(), (y * s).round())
}

/// The first of `rects` containing `(x, y)`.
fn monitor_at(rects: &[Rect], x: f64, y: f64) -> Option<usize> {
    rects.iter().position(|r| r.contains(x, y))
}

/// Where the top-left of a `w`-wide menu goes for anchor x `x`: at the
/// anchor, or — right-to-left — with its right edge there.
fn start_x(x: f64, w: f64, rtl: bool) -> f64 {
    if rtl {
        x - w
    } else {
        x
    }
}

/// The window size and position for a `w`×`h` (logical) menu at `anchor`,
/// flipped left/up to stay on the anchor's monitor.
fn context_menu_frame<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    anchor: Anchor,
    w: f64,
    h: f64,
    rtl: bool,
) -> (tauri::Size, tauri::Position) {
    let monitors = window.available_monitors().unwrap_or_default();
    match anchor {
        Anchor::Logical(ax, y) => {
            // Find the monitor by the anchor itself, then clamp the box.
            let x = start_x(ax, w, rtl);
            let mut pos = (x, y);
            let rects: Vec<Rect> = monitors.iter().map(logical_rect).collect();
            if let Some(i) = monitor_at(&rects, ax, y) {
                pos = clamp_rect(rects[i], x, y, w, h);
            }
            (
                tauri::Size::Logical(tauri::LogicalSize::new(w, h)),
                tauri::Position::Logical(tauri::LogicalPosition { x: pos.0, y: pos.1 }),
            )
        }
        Anchor::Physical(ax, y) => {
            // Physical rectangles never overlap, whatever each monitor's
            // scale; the menu is sized for the monitor it lands on.
            let rects: Vec<Rect> = monitors.iter().map(physical_rect).collect();
            let monitor = monitor_at(&rects, ax, y);
            let s = monitor
                .map(|i| monitors[i].scale_factor())
                .or_else(|| window.scale_factor().ok())
                .unwrap_or(1.0);
            let (pw, ph) = (w * s, h * s);
            let x = start_x(ax, pw, rtl);
            let pos = match monitor {
                Some(i) => clamp_rect(rects[i], x, y, pw, ph),
                None => (x, y),
            };
            (
                tauri::Size::Physical(tauri::PhysicalSize::new(
                    pw.round() as u32,
                    ph.round() as u32,
                )),
                tauri::Position::Physical(tauri::PhysicalPosition {
                    x: pos.0.round() as i32,
                    y: pos.1.round() as i32,
                }),
            )
        }
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

/// A monitor's rect in its own logical units (physical ÷ its scale) —
/// global points on macOS. Meaningless across monitors elsewhere.
fn logical_rect(m: &tauri::Monitor) -> Rect {
    let s = m.scale_factor();
    Rect {
        x: m.position().x as f64 / s,
        y: m.position().y as f64 / s,
        w: m.size().width as f64 / s,
        h: m.size().height as f64 / s,
    }
}

fn physical_rect(m: &tauri::Monitor) -> Rect {
    Rect {
        x: m.position().x as f64,
        y: m.position().y as f64,
        w: m.size().width as f64,
        h: m.size().height as f64,
    }
}

/// Keep a `w`×`h` box whose top-left is `(x, y)` inside `r`: shift it back
/// from the right/bottom edge (or, for a right-to-left menu, in from the
/// left edge), never past the top-left.
fn clamp_rect(r: Rect, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    let cx = if x + w > r.x + r.w {
        (r.x + r.w - w).max(r.x)
    } else {
        x.max(r.x)
    };
    let cy = if y + h > r.y + r.h {
        (r.y + r.h - h).max(r.y)
    } else {
        y
    };
    (cx, cy)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn right_to_left_menus_end_at_the_anchor_and_stay_on_screen() {
        assert_eq!(start_x(500.0, 160.0, true), 340.0);
        assert_eq!(start_x(500.0, 160.0, false), 500.0);
        // Near the left edge an RTL menu is pushed back on screen.
        assert_eq!(
            clamp_rect(SCREEN, start_x(40.0, 160.0, true), 200.0, 160.0, 220.0),
            (0.0, 200.0)
        );
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

    /// A 100% monitor with a 200% one to its right, in physical pixels.
    const MIXED: [Rect; 2] = [
        Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        },
        Rect {
            x: 1920.0,
            y: 0.0,
            w: 3840.0,
            h: 2160.0,
        },
    ];
    const MIXED_SCALES: [f64; 2] = [1.0, 2.0];

    /// What the floating window sends: its position ÷ its scale + the
    /// row's offset in CSS px.
    fn caller_point(window_physical: (f64, f64), scale: f64, css: (f64, f64)) -> (f64, f64) {
        (
            window_physical.0 / scale + css.0,
            window_physical.1 / scale + css.1,
        )
    }

    #[test]
    fn keyboard_anchor_lands_on_the_callers_monitor_at_mixed_dpi() {
        // The launcher on the 200% monitor, a row 100×50 CSS px in.
        let (lx, ly) = caller_point((2000.0, 100.0), 2.0, (100.0, 50.0));
        assert_eq!((lx, ly), (1100.0, 100.0));
        // Per-monitor "logical" rectangles overlap: (1100, 100) is inside
        // the 100% monitor's too, which is how the old lookup misplaced it.
        let logical: Vec<Rect> = MIXED
            .iter()
            .zip(MIXED_SCALES)
            .map(|(r, s)| Rect {
                x: r.x / s,
                y: r.y / s,
                w: r.w / s,
                h: r.h / s,
            })
            .collect();
        assert!(logical[0].contains(lx, ly) && logical[1].contains(lx, ly));
        assert_eq!(monitor_at(&logical, lx, ly), Some(0));
        // × the caller's own scale: exactly position + offset × scale.
        let (px, py) = caller_point_to_physical(lx, ly, 2.0);
        assert_eq!((px, py), (2200.0, 200.0));
        assert_eq!(monitor_at(&MIXED, px, py), Some(1));
    }

    #[test]
    fn keyboard_anchor_on_the_100_percent_monitor() {
        let (lx, ly) = caller_point((500.0, 300.0), 1.0, (100.0, 50.0));
        let (px, py) = caller_point_to_physical(lx, ly, 1.0);
        assert_eq!((px, py), (600.0, 350.0));
        assert_eq!(monitor_at(&MIXED, px, py), Some(0));
        // Just right of the boundary is the other monitor.
        assert_eq!(monitor_at(&MIXED, 1920.0, 10.0), Some(1));
        assert_eq!(monitor_at(&MIXED, -1.0, 10.0), None);
    }

    #[test]
    fn a_bad_caller_scale_is_treated_as_100_percent() {
        assert_eq!(caller_point_to_physical(10.4, 20.6, 0.0), (10.0, 21.0));
        assert_eq!(caller_point_to_physical(10.0, 20.0, f64::NAN), (10.0, 20.0));
        assert_eq!(caller_point_to_physical(10.0, 20.0, 1.5), (15.0, 30.0));
    }

    fn item(id: &str, key: &str) -> MenuItem {
        MenuItem {
            id: id.into(),
            label_key: Some(key.into()),
            enabled: true,
            ..Default::default()
        }
    }

    fn sep() -> MenuItem {
        MenuItem {
            separator: true,
            ..Default::default()
        }
    }

    #[test]
    fn collapses_leading_trailing_and_double_separators() {
        let items = sanitize_menu_items(vec![
            sep(),
            item("open", "result_menu.open"),
            sep(),
            sep(),
            item("reveal", "result_menu.reveal_macos"),
            sep(),
        ]);
        let shape: Vec<&str> = items
            .iter()
            .map(|i| if i.separator { "-" } else { i.id.as_str() })
            .collect();
        assert_eq!(shape, vec!["open", "-", "reveal"]);
    }

    #[test]
    fn drops_items_without_a_valid_id_or_label() {
        let items = sanitize_menu_items(vec![
            item("", "result_menu.open"),
            item("bad id!", "result_menu.open"),
            MenuItem {
                id: "nolabel".into(),
                label_key: Some("Not A Key".into()),
                ..Default::default()
            },
            MenuItem {
                id: "raw".into(),
                label: Some("  Raw label  ".into()),
                ..Default::default()
            },
        ]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "raw");
        assert_eq!(items[0].label.as_deref(), Some("Raw label"));
    }

    #[test]
    fn bounds_labels_hints_vars_and_count() {
        let mut vars = serde_json::Map::new();
        vars.insert("name".into(), serde_json::json!("x".repeat(500)));
        vars.insert("count".into(), serde_json::json!(3));
        vars.insert("obj".into(), serde_json::json!({ "nested": true }));
        let items = sanitize_menu_items(vec![MenuItem {
            id: "a".into(),
            label: Some("y".repeat(500)),
            label_vars: Some(vars),
            shortcut: Some("Ctrl+Shift+Alt+Super+Hyper+Meta+K".repeat(3)),
            ..Default::default()
        }]);
        let it = &items[0];
        assert_eq!(it.label.as_ref().unwrap().chars().count(), MAX_LABEL_CHARS);
        assert_eq!(
            it.shortcut.as_ref().unwrap().chars().count(),
            MAX_SHORTCUT_CHARS
        );
        let vars = it.label_vars.as_ref().unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars["name"].as_str().unwrap().len(), MAX_LABEL_CHARS);
        assert!(!vars.contains_key("obj"));

        let many = sanitize_menu_items((0..100).map(|i| item(&format!("i{i}"), "k")).collect());
        assert_eq!(many.len(), MAX_ITEMS);
    }

    #[test]
    fn deserializes_with_enabled_by_default() {
        let req: MenuRequest = serde_json::from_value(serde_json::json!({
            "items": [{ "id": "copy", "labelKey": "context_menu.copy" }, { "separator": true }],
            "context": { "kind": "app" },
            "keyboard": true
        }))
        .unwrap();
        let items = req.items.unwrap();
        assert!(items[0].enabled && !items[0].danger);
        assert!(items[1].separator);
        assert!(req.keyboard);
        // A bare request is the default text menu.
        let bare: MenuRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(bare.items.is_none() && !bare.keyboard);
    }

    #[test]
    fn a_late_hide_never_hides_a_newer_menu() {
        // Menu 1 is up; its answer's hide is dispatched, menu 2 is
        // presented, then the hide for 1 lands: ignored.
        let mut p = Presented(0);
        p.record(1);
        assert!(p.may_hide(1));
        p.record(2);
        assert!(!p.may_hide(1));
        assert!(p.may_hide(2));
        // A newer menu requested but not yet presented doesn't block the
        // current one's hide: it'll present after it.
        assert!(p.may_hide(3));
        // Tokens only move forward.
        p.record(1);
        assert_eq!(p, Presented(2));
    }

    #[test]
    fn menu_sizes_are_bounded() {
        assert_eq!(bounded_menu_size(180.0, 240.0), Some((180.0, 240.0)));
        assert_eq!(bounded_menu_size(1.0, 1.0), Some((MENU_MIN_W, MENU_MIN_H)));
        assert_eq!(bounded_menu_size(1e9, 1e9), Some((MENU_MAX_W, MENU_MAX_H)));
        assert_eq!(bounded_menu_size(f64::NAN, 10.0), None);
    }
}
