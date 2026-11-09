//! Extension view windows: on-demand Kage windows whose content an
//! extension draws (docs/design/EXTENSION_VIEWS.md §6, §7, §15, §16).
//!
//! - [`registry`] keeps the windows that exist, visible or cached hidden.
//! - [`geometry`] decides where a window opens and remembers where each
//!   view was.
//! - This module checks open requests, builds and shows the windows, and
//!   reacts to their window events. The Tauri commands in
//!   `commands::extensions::views` call into it.
//!
//! Identity always comes from the window label Kage built
//! (`ext-view:<extension>:<view>`), never from an argument the page sends.

pub mod geometry;
pub mod registry;

use crate::config::{Config, ExtensionGrant};
use crate::error::{AppError, ErrorKind};
use crate::extensions::views::{self as view_rules, ViewDecl};
use crate::extensions::{ExtensionManifest, InstalledItem};
use crate::lock_ext::LockExt;
use crate::state::{FeatureServices, UiState};
use crate::window_labels;
use geometry::{MonitorInfo, OpenerInfo, OpenerKind, Rect};
pub use registry::Registry;
use registry::{RateKind, Settle, DYING_TIMEOUT};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

/// What opened a view (§9.1). Informational: it reaches the view as
/// `source`. Anything else is read as `launcher`.
pub const VIEW_SOURCES: &[&str] = &[
    "launcher",
    "chat",
    "formatter",
    "widget",
    "toolbar",
    "settings",
    "view",
];

/// The page every view window loads. Identity comes from the label, so
/// there is no query string.
pub const VIEW_PAGE: &str = "extension-view.html";
/// A hidden view window is destroyed after this long (§7.3).
pub const TEARDOWN_AFTER: Duration = Duration::from_secs(10 * 60);
/// Trailing debounce of geometry saves on move and resize (§7.4).
pub const GEOMETRY_DEBOUNCE: Duration = Duration::from_millis(500);
/// How long `ext_view_focus_state` waits for a view to report focus.
pub const FOCUS_WAIT: Duration = Duration::from_millis(120);
/// After a view loses focus, how long to wait before telling the launcher
/// that no Kage window it cares about has focus (§15.2).
pub const FOCUS_LOST_DELAY: Duration = Duration::from_millis(150);
/// Longest text `ext_view_compose` accepts, in UTF-16 units (as JS counts).
pub const COMPOSE_MAX_CHARS: usize = 2000;
/// Longest text `ext_view_copy_text` accepts, in UTF-16 units.
pub const COPY_MAX_CHARS: usize = 8192;
/// Longest id echoed back in an error message.
const MAX_ECHOED_ID: usize = 64;
/// Background before the view page paints (no white flash).
const DARK_BACKGROUND: tauri::window::Color = tauri::window::Color(0x1e, 0x1e, 0x1e, 0xff);
const LIGHT_BACKGROUND: tauri::window::Color = tauri::window::Color(0xfa, 0xfa, 0xfa, 0xff);

/// The `source` an `ext_view_open` caller passed, checked against
/// [`VIEW_SOURCES`].
pub fn normalize_source(source: Option<&str>) -> &'static str {
    source
        .and_then(|s| VIEW_SOURCES.iter().copied().find(|known| *known == s))
        .unwrap_or("launcher")
}

/// `ext_view_open`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtViewOpened {
    /// The view window's label (`ext-view:<extension>:<view>`).
    pub label: String,
    /// The window already existed (visible or cached hidden).
    pub reused: bool,
}

/// `ext_view_boot`'s answer: what the calling view window shows. Derived
/// from the window's label, never from an argument.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtViewBoot {
    pub extension_id: String,
    pub view_id: String,
    pub params: Option<serde_json::Value>,
    /// 1 on the first open, one more on every open after it.
    pub open_seq: u64,
    /// One of [`VIEW_SOURCES`].
    pub source: String,
    /// The page booted before in this window (a reload in place).
    pub reloaded: bool,
}

/// `ext_view_focus_state`'s answer, for the launcher's blur handler.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ExtViewFocusState {
    pub view_focused: bool,
}

/// One entry of `ext_view_list`, for Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtViewInfo {
    pub extension_id: String,
    pub view_id: String,
    pub visible: bool,
    pub focused: bool,
}

// --- Pure rules ------------------------------------------------------------

/// Whether the launcher, kept open as a view's companion, should hide:
/// neither it nor any view window has focus (§15.2).
pub fn should_release_launcher(focused_labels: &[&str]) -> bool {
    !focused_labels
        .iter()
        .any(|l| *l == window_labels::FLOATING || window_labels::is_ext_view_label(l))
}

/// Whether closing a view window destroys it instead of hiding it: on
/// macOS while it is in native full screen, because hiding a full-screen
/// window leaves an empty Space behind (§7.3).
pub fn should_destroy_on_close(macos: bool, fullscreen: bool) -> bool {
    macos && fullscreen
}

/// Why `text` can't be composed or copied from a view, or `None`. The
/// rules of `copyTextError` in `ui/js/shared/settings-schema.js` with a
/// `max`: not empty, at most `max` UTF-16 units, plain text (tab and
/// newlines are fine; other control and bidi-override characters aren't).
pub fn text_error(text: &str, max: usize) -> Option<String> {
    let units = text.encode_utf16().count();
    if units == 0 {
        return Some("must not be empty".to_string());
    }
    if units > max {
        return Some(format!("must be at most {max} characters"));
    }
    let bad = text.chars().any(|c| {
        let u = c as u32;
        (u < 0x20 && !matches!(c, '\t' | '\n' | '\r'))
            || (0x7f..=0x9f).contains(&u)
            || view_rules::is_bidi_override(c)
    });
    if bad {
        return Some("must be plain text (no control or bidi-override characters)".to_string());
    }
    None
}

fn has_cap(list: &[String], cap: &str) -> bool {
    list.iter().any(|c| c.trim().eq_ignore_ascii_case(cap))
}

/// Whether the extension may show its views: the manifest asks for
/// `views` and the user's grant includes it (requested ∩ granted, the
/// rule the JS hosts apply).
pub fn holds_views(manifest: &ExtensionManifest, grant: Option<&ExtensionGrant>) -> bool {
    let requested = manifest.permissions.as_deref().unwrap_or_default();
    grant.is_some_and(|g| has_cap(requested, "views") && has_cap(&g.granted, "views"))
}

fn echo(id: &str) -> String {
    id.chars().take(MAX_ECHOED_ID).collect()
}

fn keyed(kind: ErrorKind, key: &str, params: &[(&str, &str)]) -> AppError {
    AppError::keyed(kind, key, params)
}

pub fn not_a_view_window() -> AppError {
    keyed(
        ErrorKind::NotPermitted,
        "errors.ext_view.not_a_view_window",
        &[],
    )
}

fn create_failed(reason: &str) -> AppError {
    keyed(
        ErrorKind::Internal,
        "errors.window.create_failed",
        &[("reason", reason)],
    )
}

fn show_failed(reason: &str) -> AppError {
    keyed(
        ErrorKind::Internal,
        "errors.window.show_failed",
        &[("reason", reason)],
    )
}

/// The opener rules (§6.2): only the launcher, chat windows, Settings and
/// view windows open views, and a view window only views of its own
/// extension. Checked before anything is looked up, so a view can't learn
/// whether another extension is installed.
pub fn check_opener(opener: &str, extension_id: &str) -> Result<(), AppError> {
    if !window_labels::is_extension_host_label(opener) {
        return Err(keyed(
            ErrorKind::NotPermitted,
            "errors.ext_view.opener_refused",
            &[],
        ));
    }
    if let Some((own, _)) = window_labels::parse_ext_view_label(opener) {
        if own != extension_id {
            return Err(keyed(
                ErrorKind::NotPermitted,
                "errors.ext_view.wrong_extension",
                &[],
            ));
        }
    }
    Ok(())
}

/// The extension is installed and enabled, holds `views`, and declares
/// `view_id`. Returns the declaration. Applied on every open and every
/// boot.
pub fn validate_installed_view(
    item: Option<&InstalledItem>,
    grant: Option<&ExtensionGrant>,
    extension_id: &str,
    view_id: &str,
) -> Result<ViewDecl, AppError> {
    let Some(item) = item.filter(|i| i.manifest.id == extension_id) else {
        return Err(keyed(
            ErrorKind::InvalidInput,
            "errors.ext_view.unknown_extension",
            &[("id", &echo(extension_id))],
        ));
    };
    let name = crate::config_export::sensitive::display_name(item);
    if !item.enabled {
        return Err(keyed(
            ErrorKind::NotPermitted,
            "errors.ext_view.disabled",
            &[("extension", &name)],
        ));
    }
    if !holds_views(&item.manifest, grant) {
        return Err(keyed(
            ErrorKind::NotPermitted,
            "errors.ext_view.not_granted",
            &[("extension", &name)],
        ));
    }
    item.manifest
        .views()
        .into_iter()
        .find(|v| v.id == view_id)
        .ok_or_else(|| {
            keyed(
                ErrorKind::InvalidInput,
                "errors.ext_view.unknown_view",
                &[("extension", &name), ("view", &echo(view_id))],
            )
        })
}

/// Every check of `ext_view_open` (§6.2), in order: the opener, the
/// installed extension and its grant, the declared view, then the params.
pub fn validate_open_request(
    item: Option<&InstalledItem>,
    grant: Option<&ExtensionGrant>,
    opener: &str,
    extension_id: &str,
    view_id: &str,
    params: Option<&serde_json::Value>,
) -> Result<ViewDecl, AppError> {
    check_opener(opener, extension_id)?;
    let decl = validate_installed_view(item, grant, extension_id, view_id)?;
    view_rules::validate_params(params)?;
    Ok(decl)
}

fn background_for(theme: &str, os_dark: bool) -> tauri::window::Color {
    let dark = match theme {
        "light" => false,
        "system" | "" => os_dark,
        // "dark" and theme extensions (which are dark unless they say
        // otherwise; the page repaints within its first frame anyway).
        _ => true,
    };
    if dark {
        DARK_BACKGROUND
    } else {
        LIGHT_BACKGROUND
    }
}

// --- Lookups ---------------------------------------------------------------

/// The installed extension `extension_id` and its grant. Reads the
/// config under its lock, then scans the install directory without it.
fn lookup(
    config: &std::sync::Mutex<Config>,
    extension_id: &str,
) -> (Option<InstalledItem>, Option<ExtensionGrant>) {
    if crate::extensions::validate_extension_id(extension_id).is_err() {
        return (None, None);
    }
    let (states, grant): (HashMap<String, bool>, Option<ExtensionGrant>) = {
        let config = config.lock_or_recover();
        (
            config.extension_states.clone(),
            config.extension_grants.get(extension_id).cloned(),
        )
    };
    let item = crate::extensions::discover_items("extension", &states)
        .into_iter()
        .find(|i| i.manifest.id == extension_id);
    (item, grant)
}

fn registry_of<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<Registry>> {
    app.try_state::<UiState>().map(|ui| ui.ext_views.clone())
}

// --- Open ------------------------------------------------------------------

/// `ext_view_open` (§6.3): check, then show the existing window with new
/// params, or build it.
#[allow(clippy::too_many_arguments)]
pub async fn open<R: Runtime>(
    app: &AppHandle<R>,
    opener: &WebviewWindow<R>,
    features: &FeatureServices,
    registry: &Registry,
    extension_id: &str,
    view_id: &str,
    params: Option<serde_json::Value>,
    source: Option<&str>,
) -> Result<ExtViewOpened, AppError> {
    let opener_label = opener.label().to_string();
    let source = normalize_source(source);
    let refuse = |e: AppError| {
        log::info!(
            "[ext {}] open_view refused: {} (from {})",
            echo(extension_id),
            e.key,
            opener_label
        );
        e
    };
    check_opener(&opener_label, extension_id).map_err(refuse)?;
    let (item, grant) = lookup(&features.config, extension_id);
    let decl = validate_open_request(
        item.as_ref(),
        grant.as_ref(),
        &opener_label,
        extension_id,
        view_id,
        params.as_ref(),
    )
    .map_err(refuse)?;
    let label = window_labels::ext_view_label(extension_id, &decl.id)
        .ok_or_else(|| create_failed("invalid window label"))?;

    let _guard = registry.open_guard().await;
    wait_until_settled(app, registry, &label).await?;

    let existing = app
        .get_webview_window(&label)
        .filter(|_| registry.contains(&label));
    let reused = match existing {
        Some(window) => {
            show_again(app, registry, &window, &label, params, source)?;
            true
        }
        None => {
            build(
                app,
                opener,
                features,
                registry,
                &label,
                &decl,
                extension_id,
                params,
                source,
            )?;
            false
        }
    };
    crate::setup::update_activation_policy(app);
    crate::telemetry::track(
        app,
        "extension_view_opened",
        Some(serde_json::json!({ "source": source, "reused": u8::from(reused) })),
    );
    log::info!(
        "[ext {}] opened view '{}' from {} (reused={})",
        extension_id,
        decl.id,
        opener_label,
        reused
    );
    Ok(ExtViewOpened { label, reused })
}

/// Wait until `label` is either a live window with a record or nothing at
/// all: a window being destroyed (or one left without a record) must be
/// gone before a new one with the same label is built.
async fn wait_until_settled<R: Runtime>(
    app: &AppHandle<R>,
    registry: &Registry,
    label: &str,
) -> Result<(), AppError> {
    let started = Instant::now();
    let mut destroy_sent = false;
    loop {
        let window = app.get_webview_window(label);
        let Some(window) = window else {
            return Ok(());
        };
        if registry.contains(label) {
            return Ok(());
        }
        if !destroy_sent && !registry.is_dying(label, Instant::now()) {
            log::warn!(
                "[ext-view] {label} exists without a record; destroying it before reopening"
            );
            let _ = window.destroy();
        }
        destroy_sent = true;
        if started.elapsed() >= DYING_TIMEOUT {
            return Err(create_failed("the previous window is still closing"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Show an existing window again with new params.
fn show_again<R: Runtime>(
    app: &AppHandle<R>,
    registry: &Registry,
    window: &WebviewWindow<R>,
    label: &str,
    params: Option<serde_json::Value>,
    source: &str,
) -> Result<(), AppError> {
    let reopened = registry
        .reopen(label, params.clone(), source)
        .ok_or_else(|| create_failed("the window went away"))?;
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    window.show().map_err(|e| show_failed(&e.to_string()))?;
    let _ = window.set_focus();
    if reopened.was_hidden {
        crate::event_targets::emit_to_ext_view(
            app,
            label,
            crate::events::EXT_VIEW_VISIBILITY,
            &serde_json::json!({ "visible": true }),
        );
    }
    if reopened.booted {
        crate::event_targets::emit_to_ext_view(
            app,
            label,
            crate::events::EXT_VIEW_REOPEN,
            &serde_json::json!({
                "params": params,
                "open_seq": reopened.open_seq,
                "source": source,
            }),
        );
    }
    Ok(())
}

/// Build, place, show and focus a new view window.
#[allow(clippy::too_many_arguments)]
fn build<R: Runtime>(
    app: &AppHandle<R>,
    opener: &WebviewWindow<R>,
    features: &FeatureServices,
    registry: &Registry,
    label: &str,
    decl: &ViewDecl,
    extension_id: &str,
    params: Option<serde_json::Value>,
    source: &str,
) -> Result<(), AppError> {
    use crate::navigation_guard::NewWindowGuardExt;

    let now = Instant::now();
    let evict = registry
        .reserve(label, extension_id, &decl.id, params, source, now)
        .map_err(|_| {
            log::info!("[ext {extension_id}] open_view refused: every view window is visible");
            keyed(
                ErrorKind::RateLimited,
                "errors.ext_view.too_many_open",
                &[("limit", &registry::MAX_WINDOWS.to_string())],
            )
        })?;
    for old in evict {
        log::info!("[ext-view] evicting {old} to make room for {label}");
        destroy_window_only(app, &old);
    }

    let can_position = crate::os::window_spaces::can_position_windows();
    let placement = placement_for(opener, extension_id, decl, can_position);
    let theme = features.config.lock_or_recover().ui.theme.clone();
    let os_dark = opener
        .theme()
        .map(|t| t == tauri::Theme::Dark)
        .unwrap_or(true);

    let built =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(VIEW_PAGE.into()))
            .title("Kage")
            .inner_size(
                decl.default_size.width as f64,
                decl.default_size.height as f64,
            )
            .min_inner_size(decl.min_size.width as f64, decl.min_size.height as f64)
            .resizable(true)
            .maximizable(true)
            .minimizable(true)
            .closable(true)
            .decorations(true)
            .always_on_top(false)
            .skip_taskbar(false)
            .visible(false)
            .background_color(background_for(&theme, os_dark))
            .guard_new_windows(app)
            .build();
    let window = match built {
        Ok(window) => window,
        Err(e) => {
            registry.remove(label);
            log::warn!("[ext-view] building {label} failed: {e}");
            return Err(create_failed(&e.to_string()));
        }
    };

    let position = placement
        .position
        .map(|(x, y)| tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
    if let Some(pos) = position {
        let _ = window.set_position(pos);
    }
    let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
        width: placement.width,
        height: placement.height,
    }));
    match position {
        // Again after the resize: moving a window to a monitor with another
        // scale can rescale it on the way.
        Some(pos) => {
            let _ = window.set_position(pos);
        }
        None if can_position => {
            let _ = window.center();
        }
        None => {}
    }
    if let Err(e) = window.show() {
        registry.take_for_destroy(label, Instant::now(), false);
        let _ = window.destroy();
        return Err(show_failed(&e.to_string()));
    }
    if placement.maximized {
        let _ = window.maximize();
    }
    let _ = window.set_focus();
    Ok(())
}

/// Monitors as the opener window sees them.
fn monitor_infos<R: Runtime>(window: &WebviewWindow<R>) -> Vec<MonitorInfo> {
    let primary = window.primary_monitor().ok().flatten();
    let same = |a: &tauri::Monitor, b: &tauri::Monitor| {
        a.name() == b.name() && a.position() == b.position() && a.size() == b.size()
    };
    window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| monitor_info(m, primary.as_ref().is_some_and(|p| same(p, m))))
        .collect()
}

fn monitor_info(m: &tauri::Monitor, primary: bool) -> MonitorInfo {
    let work = m.work_area();
    MonitorInfo {
        name: m.name().cloned().unwrap_or_default(),
        rect: Rect::new(
            m.position().x,
            m.position().y,
            m.size().width,
            m.size().height,
        ),
        work_area: Rect::new(
            work.position.x,
            work.position.y,
            work.size.width,
            work.size.height,
        ),
        scale: m.scale_factor(),
        primary,
    }
}

fn monitor_index(monitors: &[MonitorInfo], m: &MonitorInfo) -> Option<usize> {
    monitors
        .iter()
        .position(|x| x.name == m.name && x.rect == m.rect)
}

/// Where the new window goes (§7.5), from the opener, the cursor and the
/// saved geometry.
fn placement_for<R: Runtime>(
    opener: &WebviewWindow<R>,
    extension_id: &str,
    decl: &ViewDecl,
    can_position: bool,
) -> geometry::Placement {
    let monitors = monitor_infos(opener);
    let opener_monitor = opener
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| monitor_info(&m, false))
        .and_then(|m| monitor_index(&monitors, &m));
    let opener_info = match (opener.outer_position(), opener.outer_size()) {
        (Ok(pos), Ok(size)) => Some(OpenerInfo {
            kind: if opener.label() == window_labels::FLOATING {
                OpenerKind::Floating
            } else {
                OpenerKind::Other
            },
            rect: Rect::new(pos.x, pos.y, size.width, size.height),
            monitor: opener_monitor,
        }),
        _ => None,
    };
    let cursor_monitor = opener
        .cursor_position()
        .ok()
        .and_then(|p| geometry::monitor_at(&monitors, p.x, p.y));
    let saved = geometry::load_entry(extension_id, &decl.id);
    geometry::place(
        saved.as_ref(),
        &monitors,
        opener_info.as_ref(),
        cursor_monitor,
        decl,
        can_position,
    )
}

// --- Boot, compose, copy -----------------------------------------------------

/// The label of the calling view window, if it is one Kage knows.
fn calling_view<R: Runtime>(
    window: &WebviewWindow<R>,
    registry: &Registry,
) -> Result<String, AppError> {
    let label = window.label();
    if window_labels::is_ext_view_label(label) && registry.contains(label) {
        Ok(label.to_string())
    } else {
        Err(not_a_view_window())
    }
}

/// `ext_view_boot`: the calling window's boot record, after checking again
/// that the extension is installed, enabled, granted and declares the view.
pub fn boot<R: Runtime>(
    window: &WebviewWindow<R>,
    features: &FeatureServices,
    registry: &Registry,
) -> Result<ExtViewBoot, AppError> {
    let label = calling_view(window, registry)?;
    let (extension_id, view_id) =
        window_labels::parse_ext_view_label(&label).ok_or_else(not_a_view_window)?;
    let (item, grant) = lookup(&features.config, extension_id);
    validate_installed_view(item.as_ref(), grant.as_ref(), extension_id, view_id).inspect_err(
        |e| {
            log::info!(
                "[ext {extension_id}] view '{view_id}' can't boot: {}",
                e.key
            )
        },
    )?;
    let record = registry.boot(&label).ok_or_else(not_a_view_window)?;
    Ok(ExtViewBoot {
        extension_id: record.extension_id,
        view_id: record.view_id,
        params: record.params,
        open_seq: record.open_seq,
        source: record.source,
        reloaded: record.reloaded,
    })
}

/// The calling view window must be in front (§13.4). The window system is
/// the authority; the focus events are the fallback when it can't answer.
fn ensure_focused<R: Runtime>(
    window: &WebviewWindow<R>,
    registry: &Registry,
    label: &str,
) -> Result<(), AppError> {
    let focused = window
        .is_focused()
        .unwrap_or_else(|_| registry.is_focused(label));
    if focused {
        Ok(())
    } else {
        Err(keyed(
            ErrorKind::NotPermitted,
            "errors.ext_view.not_focused",
            &[],
        ))
    }
}

/// The checks `compose` and `copy_text` share: a known view window, in
/// front, plain text within `max`, within the rate limit.
fn check_text_request<R: Runtime>(
    window: &WebviewWindow<R>,
    registry: &Registry,
    text: &str,
    max: usize,
    kind: RateKind,
) -> Result<String, AppError> {
    let label = calling_view(window, registry)?;
    ensure_focused(window, registry, &label)?;
    if let Some(reason) = text_error(text, max) {
        return Err(keyed(
            ErrorKind::InvalidInput,
            "errors.ext_view.text_invalid",
            &[("reason", &reason)],
        ));
    }
    if !registry.allow(&label, kind, Instant::now()) {
        return Err(keyed(
            ErrorKind::RateLimited,
            "errors.ext_view.too_fast",
            &[],
        ));
    }
    Ok(label)
}

/// `ext_view_compose`: put `text` in the launcher's input and show the
/// launcher. Never sends.
pub fn compose<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    registry: &Registry,
    text: String,
) -> Result<(), AppError> {
    let label = check_text_request(
        window,
        registry,
        &text,
        COMPOSE_MAX_CHARS,
        RateKind::Compose,
    )?;
    let floating = app
        .get_webview_window(window_labels::FLOATING)
        .ok_or_else(|| {
            keyed(
                ErrorKind::Internal,
                "errors.window.not_found",
                &[("label", window_labels::FLOATING)],
            )
        })?;
    if !floating.is_visible().unwrap_or(false) {
        // A host-initiated show: centred on the active monitor, as the
        // welcome and update banners do. No selection capture: the
        // foreground app is Kage's own view.
        crate::commands::window::center_floating_on_active_monitor(&floating);
    }
    crate::overlay_windows::present(&floating).map_err(|e| show_failed(&e.to_string()))?;
    if let Some(features) = app.try_state::<FeatureServices>() {
        features.updater.touch_activity();
    }
    crate::event_targets::emit_to_floating(
        app,
        crate::events::FLOATING_COMPOSE,
        &serde_json::json!({ "text": text }),
    );
    log::info!(
        "[ext-view] {label} composed {} characters in the launcher",
        text.encode_utf16().count()
    );
    Ok(())
}

/// `ext_view_copy_text`: copy plain text for the focused view window.
pub async fn copy_text<R: Runtime>(
    window: &WebviewWindow<R>,
    registry: &Registry,
    text: String,
) -> Result<(), AppError> {
    let label = check_text_request(window, registry, &text, COPY_MAX_CHARS, RateKind::Copy)?;
    let written = tokio::task::spawn_blocking(move || crate::os::write_clipboard(&text))
        .await
        .unwrap_or(false);
    if !written {
        log::warn!("[ext-view] {label}: clipboard write failed");
        return Err(keyed(
            ErrorKind::Internal,
            "errors.ext_view.clipboard_failed",
            &[],
        ));
    }
    Ok(())
}

// --- Close, hide, destroy ----------------------------------------------------

/// Destroy a window without touching the registry (the caller already did).
fn destroy_window_only<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        if let Err(e) = window.destroy() {
            log::warn!("[ext-view] destroying {label} failed: {e}");
        }
    }
}

/// Destroy one view window and forget its record.
pub fn destroy_view<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(registry) = registry_of(app) {
        registry.take_for_destroy(label, Instant::now(), true);
    }
    destroy_window_only(app, label);
}

/// Destroy every view window of `extension_id`, visible or hidden (§7.3):
/// on disable, uninstall, or a grant without `views`. Returns how many.
pub fn close_for_extension<R: Runtime>(app: &AppHandle<R>, extension_id: &str) -> usize {
    let mut labels = registry_of(app)
        .map(|r| r.labels_for_extension(extension_id))
        .unwrap_or_default();
    // A window that is closing already has no record; it goes too.
    for label in app.webview_windows().keys() {
        let ours =
            window_labels::parse_ext_view_label(label).is_some_and(|(e, _)| e == extension_id);
        if ours && !labels.contains(label) {
            labels.push(label.clone());
        }
    }
    for label in &labels {
        destroy_view(app, label);
    }
    if !labels.is_empty() {
        log::info!(
            "[ext {extension_id}] closed {} view window(s)",
            labels.len()
        );
    }
    labels.len()
}

/// A view window closed in native full screen is about to be destroyed by
/// the window system. The caller refreshes the activation policy itself.
pub fn forget_closing<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(registry) = registry_of(app) {
        registry.take_for_destroy(label, Instant::now(), false);
    }
}

/// A view window was hidden by its close button (§7.3): tell the page,
/// start the teardown timer, and evict hidden windows over the cap.
pub fn after_hide<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let Some(registry) = registry_of(app) else {
        return;
    };
    let Some(hidden) = registry.mark_hidden(label, Instant::now()) else {
        return;
    };
    crate::event_targets::emit_to_ext_view(
        app,
        label,
        crate::events::EXT_VIEW_VISIBILITY,
        &serde_json::json!({ "visible": false }),
    );
    for old in hidden.evict {
        log::info!("[ext-view] evicting hidden {old}: too many hidden view windows");
        destroy_window_only(app, &old);
    }
    let app = app.clone();
    let label = label.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(TEARDOWN_AFTER).await;
        if registry.teardown_due(&label, hidden.token) {
            log::info!("[ext-view] tearing down {label}: hidden for 10 minutes");
            destroy_view(&app, &label);
        }
    });
}

// --- Geometry ----------------------------------------------------------------

/// What to save about a window now, or `None` while it is minimised, in
/// native full screen, or on no known monitor.
fn snapshot<R: Runtime>(window: &WebviewWindow<R>) -> Option<(Rect, MonitorInfo, bool)> {
    if window.is_minimized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
        return None;
    }
    let monitor = window.current_monitor().ok().flatten()?;
    let pos = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    if size.width == 0 || size.height == 0 {
        return None;
    }
    Some((
        Rect::new(pos.x, pos.y, size.width, size.height),
        monitor_info(&monitor, false),
        window.is_maximized().unwrap_or(false),
    ))
}

/// Save the window's geometry now (the write happens off this thread).
pub fn save_geometry_now<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let Some((extension_id, view_id)) =
        window_labels::parse_ext_view_label(label).map(|(e, v)| (e.to_string(), v.to_string()))
    else {
        return;
    };
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let Some((rect, monitor, maximized)) = snapshot(&window) else {
        return;
    };
    let saved_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    tauri::async_runtime::spawn_blocking(move || {
        let result = geometry::update_entry(&extension_id, &view_id, |prev| {
            Some(geometry::captured(
                prev, rect, &monitor, maximized, saved_at,
            ))
        });
        if let Err(e) = result {
            log::warn!("[ext-view] saving geometry of {extension_id}/{view_id} failed: {e}");
        }
    });
}

fn schedule_geometry_save<R: Runtime>(app: &AppHandle<R>, registry: Arc<Registry>, label: &str) {
    let Some((token, start)) = registry.bump_geometry(label) else {
        return;
    };
    if !start {
        return;
    }
    let app = app.clone();
    let label = label.to_string();
    tauri::async_runtime::spawn(async move {
        let mut token = token;
        loop {
            tokio::time::sleep(GEOMETRY_DEBOUNCE).await;
            match registry.settle_geometry(&label, token) {
                Settle::Changed(newer) => token = newer,
                Settle::Save => {
                    save_geometry_now(&app, &label);
                    break;
                }
                Settle::Skip => break,
            }
        }
    });
}

// --- Focus -------------------------------------------------------------------

/// After a view lost focus: if neither the launcher nor any view has it a
/// moment later, tell the launcher, which hides if it stayed open as the
/// view's companion (§15.2).
fn schedule_focus_check<R: Runtime>(app: &AppHandle<R>, registry: Arc<Registry>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FOCUS_LOST_DELAY).await;
        let mut focused = registry.focused_labels();
        let floating = app.get_webview_window(window_labels::FLOATING);
        if let Some(f) = &floating {
            if f.is_focused().unwrap_or(false) {
                focused.push(window_labels::FLOATING.to_string());
            }
        }
        let refs: Vec<&str> = focused.iter().map(String::as_str).collect();
        let launcher_shown = floating.is_some_and(|f| f.is_visible().unwrap_or(false));
        if launcher_shown && should_release_launcher(&refs) {
            crate::event_targets::emit_to_floating(&app, crate::events::EXT_VIEW_FOCUS_LOST, &());
        }
    });
}

/// `ext_view_focus_state`.
pub async fn focus_state(registry: &Registry) -> ExtViewFocusState {
    ExtViewFocusState {
        view_focused: registry.wait_for_view_focus(FOCUS_WAIT).await,
    }
}

// --- Window events -------------------------------------------------------------

/// Window events of a view window (wired in `main.rs`). `CloseRequested`
/// goes through `setup::handle_window_close`.
pub fn handle_window_event<R: Runtime>(window: &tauri::Window<R>, event: &tauri::WindowEvent) {
    let label = window.label();
    if !window_labels::is_ext_view_label(label) {
        return;
    }
    let app = window.app_handle();
    let Some(registry) = registry_of(app) else {
        return;
    };
    match event {
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
            schedule_geometry_save(app, registry, label);
        }
        tauri::WindowEvent::Focused(focused) => {
            registry.set_focused(label, *focused);
            if !focused {
                schedule_focus_check(app, registry);
            }
        }
        tauri::WindowEvent::Destroyed => {
            let refresh = registry.on_destroyed(label, Instant::now());
            crate::webview_recovery::forget_webview(label);
            if refresh {
                crate::setup::update_activation_policy(app);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn source_is_one_of_the_closed_list() {
        for source in VIEW_SOURCES {
            assert_eq!(normalize_source(Some(source)), *source);
        }
        assert_eq!(normalize_source(None), "launcher");
        assert_eq!(normalize_source(Some("")), "launcher");
        assert_eq!(normalize_source(Some("Chat")), "launcher");
        assert_eq!(normalize_source(Some("deep-link")), "launcher");
    }

    #[test]
    fn wire_shapes() {
        let opened = ExtViewOpened {
            label: "ext-view:mindmap:explorer".into(),
            reused: false,
        };
        assert_eq!(
            serde_json::to_value(&opened).unwrap(),
            json!({ "label": "ext-view:mindmap:explorer", "reused": false })
        );
        let boot = ExtViewBoot {
            extension_id: "mindmap".into(),
            view_id: "explorer".into(),
            params: None,
            open_seq: 1,
            source: "launcher".into(),
            reloaded: false,
        };
        assert_eq!(
            serde_json::to_value(&boot).unwrap(),
            json!({
                "extension_id": "mindmap", "view_id": "explorer", "params": null,
                "open_seq": 1, "source": "launcher", "reloaded": false
            })
        );
        assert_eq!(
            serde_json::to_value(ExtViewFocusState::default()).unwrap(),
            json!({ "view_focused": false })
        );
        let info = ExtViewInfo {
            extension_id: "mindmap".into(),
            view_id: "explorer".into(),
            visible: true,
            focused: false,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            json!({
                "extension_id": "mindmap", "view_id": "explorer",
                "visible": true, "focused": false
            })
        );
    }

    #[test]
    fn launcher_is_released_only_when_no_kage_surface_it_cares_about_has_focus() {
        assert!(should_release_launcher(&[]));
        assert!(should_release_launcher(&["main"]));
        assert!(should_release_launcher(&["settings", "chat-abc"]));
        assert!(should_release_launcher(&["ext-view:bad:Label"]));
        assert!(!should_release_launcher(&["floating"]));
        assert!(!should_release_launcher(&["ext-view:mindmap:explorer"]));
        assert!(!should_release_launcher(&[
            "main",
            "ext-view:mindmap:explorer"
        ]));
    }

    #[test]
    fn full_screen_close_destroys_only_on_macos() {
        assert!(should_destroy_on_close(true, true));
        assert!(!should_destroy_on_close(true, false));
        assert!(!should_destroy_on_close(false, true));
        assert!(!should_destroy_on_close(false, false));
    }

    #[test]
    fn text_rules_mirror_copy_text_error() {
        assert_eq!(text_error("", 10).as_deref(), Some("must not be empty"));
        assert_eq!(text_error("hello", 10), None);
        assert_eq!(text_error("tab\tand\nnew\r\nlines", 100), None);
        assert_eq!(
            text_error("12345678901", 10).as_deref(),
            Some("must be at most 10 characters")
        );
        // Lengths are UTF-16 units, as JS counts: an emoji is two.
        assert_eq!(text_error("🙂🙂🙂🙂🙂", 10), None);
        assert!(text_error("🙂🙂🙂🙂🙂🙂", 10).is_some());
        for bad in [
            "a\u{1b}[31m",
            "a\u{0}",
            "a\u{7f}",
            "a\u{85}",
            "a\u{202e}b",
            "\u{2066}x",
        ] {
            assert_eq!(
                text_error(bad, 100).as_deref(),
                Some("must be plain text (no control or bidi-override characters)"),
                "{bad:?}"
            );
        }
        assert_eq!(COMPOSE_MAX_CHARS, 2000);
        assert_eq!(COPY_MAX_CHARS, 8192);
    }

    #[test]
    fn background_follows_the_theme() {
        assert_eq!(background_for("dark", false), DARK_BACKGROUND);
        assert_eq!(background_for("light", true), LIGHT_BACKGROUND);
        assert_eq!(background_for("system", true), DARK_BACKGROUND);
        assert_eq!(background_for("system", false), LIGHT_BACKGROUND);
        assert_eq!(background_for("", false), LIGHT_BACKGROUND);
        assert_eq!(background_for("nord", false), DARK_BACKGROUND);
    }

    fn item(permissions: &[&str], enabled: bool) -> InstalledItem {
        let manifest: ExtensionManifest = serde_json::from_value(json!({
            "id": "mindmap",
            "name": "Mind Map",
            "version": "1.0.0",
            "type": "extension",
            "icon": "🧠",
            "permissions": permissions,
            "contributes": { "views": [
                { "id": "explorer", "title": "Explorer", "module": "./explorer.js" }
            ] }
        }))
        .unwrap();
        InstalledItem {
            manifest,
            path: String::new(),
            enabled,
        }
    }

    fn grant(caps: &[&str]) -> ExtensionGrant {
        ExtensionGrant {
            granted: caps.iter().map(|c| c.to_string()).collect(),
            approved_version: "1.0.0".into(),
            approved_at: String::new(),
        }
    }

    fn key(r: Result<ViewDecl, AppError>) -> String {
        match r {
            Ok(decl) => format!("ok:{}", decl.id),
            Err(e) => e.key,
        }
    }

    #[test]
    fn open_request_validation() {
        let ok = item(&["storage", "views"], true);
        let g = grant(&["storage", "views"]);
        let open = |item: Option<&InstalledItem>,
                    grant: Option<&ExtensionGrant>,
                    opener: &str,
                    ext: &str,
                    view: &str,
                    params: Option<serde_json::Value>| {
            key(validate_open_request(
                item,
                grant,
                opener,
                ext,
                view,
                params.as_ref(),
            ))
        };
        for opener in [
            "floating",
            "main",
            "chat-1234",
            "settings",
            "ext-view:mindmap:board",
        ] {
            assert_eq!(
                open(Some(&ok), Some(&g), opener, "mindmap", "explorer", None),
                "ok:explorer",
                "{opener}"
            );
        }
        for opener in [
            "store",
            "welcome",
            "context-menu",
            "inline-assist",
            "ext-view:bad:X",
            "",
        ] {
            assert_eq!(
                open(Some(&ok), Some(&g), opener, "mindmap", "explorer", None),
                "errors.ext_view.opener_refused",
                "{opener}"
            );
        }
        assert_eq!(
            open(
                Some(&ok),
                Some(&g),
                "ext-view:other:main",
                "mindmap",
                "explorer",
                None
            ),
            "errors.ext_view.wrong_extension"
        );
        // A view of another extension is refused before anything is looked
        // up, so it can't probe what's installed.
        assert_eq!(
            open(
                None,
                None,
                "ext-view:other:main",
                "mindmap",
                "explorer",
                None
            ),
            "errors.ext_view.wrong_extension"
        );
        assert_eq!(
            open(None, None, "floating", "mindmap", "explorer", None),
            "errors.ext_view.unknown_extension"
        );
        assert_eq!(
            open(Some(&ok), Some(&g), "floating", "other", "explorer", None),
            "errors.ext_view.unknown_extension"
        );
        assert_eq!(
            open(
                Some(&item(&["views"], false)),
                Some(&g),
                "floating",
                "mindmap",
                "explorer",
                None
            ),
            "errors.ext_view.disabled"
        );
        // Not granted: no grant, a grant without views, or a manifest that
        // no longer asks for views.
        assert_eq!(
            open(Some(&ok), None, "floating", "mindmap", "explorer", None),
            "errors.ext_view.not_granted"
        );
        assert_eq!(
            open(
                Some(&ok),
                Some(&grant(&["storage"])),
                "floating",
                "mindmap",
                "explorer",
                None
            ),
            "errors.ext_view.not_granted"
        );
        assert_eq!(
            open(
                Some(&item(&["storage"], true)),
                Some(&g),
                "floating",
                "mindmap",
                "explorer",
                None
            ),
            "errors.ext_view.not_granted"
        );
        assert_eq!(
            open(Some(&ok), Some(&g), "floating", "mindmap", "board", None),
            "errors.ext_view.unknown_view"
        );
        assert_eq!(
            open(
                Some(&ok),
                Some(&g),
                "floating",
                "mindmap",
                "explorer",
                Some(json!([1]))
            ),
            "errors.ext_view.params_invalid"
        );
        assert_eq!(
            open(
                Some(&ok),
                Some(&g),
                "floating",
                "mindmap",
                "explorer",
                Some(json!({ "a": { "b": { "c": { "d": {} } } } }))
            ),
            "errors.ext_view.params_invalid"
        );
        let big = "x".repeat(view_rules::MAX_PARAMS_BYTES);
        assert_eq!(
            open(
                Some(&ok),
                Some(&g),
                "floating",
                "mindmap",
                "explorer",
                Some(json!({ "s": big }))
            ),
            "errors.ext_view.params_too_large"
        );
        assert_eq!(
            open(
                Some(&ok),
                Some(&g),
                "floating",
                "mindmap",
                "explorer",
                Some(json!({ "focus": "e1a2b3", "path": [1, 2, 3] }))
            ),
            "ok:explorer"
        );
        // Capability names are matched the way normalize_permissions reads
        // them: trimmed, any case.
        assert_eq!(
            open(
                Some(&item(&[" Views "], true)),
                Some(&grant(&["VIEWS"])),
                "floating",
                "mindmap",
                "explorer",
                None
            ),
            "ok:explorer"
        );
    }

    #[test]
    fn error_kinds_and_params() {
        let ok = item(&["views"], true);
        let err = validate_open_request(Some(&ok), None, "store", "mindmap", "explorer", None)
            .unwrap_err();
        assert!(matches!(err.kind, ErrorKind::NotPermitted));
        let err =
            validate_open_request(None, None, "floating", "mindmap", "explorer", None).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidInput));
        assert_eq!(err.params.get("id").map(String::as_str), Some("mindmap"));
        let long = "a".repeat(200);
        let err =
            validate_open_request(None, None, "floating", &long, "explorer", None).unwrap_err();
        assert_eq!(err.params.get("id").map(|s| s.len()), Some(MAX_ECHOED_ID));
        let err = validate_open_request(Some(&ok), None, "floating", "mindmap", "explorer", None)
            .unwrap_err();
        assert!(matches!(err.kind, ErrorKind::NotPermitted));
        assert_eq!(
            err.params.get("extension").map(String::as_str),
            Some("Mind Map")
        );
        let err = validate_open_request(
            Some(&ok),
            Some(&grant(&["views"])),
            "floating",
            "mindmap",
            "nope",
            None,
        )
        .unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidInput));
        assert_eq!(err.params.get("view").map(String::as_str), Some("nope"));
        assert_eq!(
            err.params.get("extension").map(String::as_str),
            Some("Mind Map")
        );
    }

    #[test]
    fn boot_checks_reject_the_same_states() {
        let g = grant(&["views"]);
        assert_eq!(
            key(validate_installed_view(
                None,
                Some(&g),
                "mindmap",
                "explorer"
            )),
            "errors.ext_view.unknown_extension"
        );
        assert_eq!(
            key(validate_installed_view(
                Some(&item(&["views"], false)),
                Some(&g),
                "mindmap",
                "explorer"
            )),
            "errors.ext_view.disabled"
        );
        assert_eq!(
            key(validate_installed_view(
                Some(&item(&["views"], true)),
                Some(&grant(&[])),
                "mindmap",
                "explorer"
            )),
            "errors.ext_view.not_granted"
        );
        assert_eq!(
            key(validate_installed_view(
                Some(&item(&["views"], true)),
                Some(&g),
                "mindmap",
                "gone"
            )),
            "errors.ext_view.unknown_view"
        );
    }
}
