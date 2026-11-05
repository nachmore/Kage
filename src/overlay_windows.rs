//! Kage's overlay windows — the floating launcher, inline assist and the
//! launcher's context menu — and how they're shown.
//!
//! # Why panels (macOS)
//!
//! Spotlight-style overlays must appear over *other apps' full-screen
//! Spaces*. Setting `CanJoinAllSpaces | FullScreenAuxiliary` on a plain
//! `NSWindow` is not enough (verified on macOS 26.7: the launcher was
//! ordered in and focused, yet never appeared over a full-screen app), and
//! activating the app — which `WebviewWindow::set_focus` does via
//! `activateIgnoringOtherApps` — can pull the user back to the desktop
//! Space. Launchers solve this with a **non-activating `NSPanel`**: it can
//! become key and take typing without activating the app, so macOS keeps
//! it on the current (full-screen) Space.
//!
//! `configure` converts the window's class to an `NSPanel` subclass (via
//! `tauri-nspanel`), adds `NonactivatingPanel`, disables
//! hide-on-deactivate and sets the Spaces behaviour. `present` is the one
//! way to show an overlay: on a panel it does `orderFrontRegardless` +
//! `makeKeyWindow` (no app activation); everywhere else it's the usual
//! `show()` + `set_focus()`.
//!
//! Escape hatch: `config.ui.macos_overlay_panels = false` (restart) keeps
//! plain windows.
//!
//! # Invariants (the class swap depends on them)
//!
//! * **Overlays are never destroyed or really closed.** `object_setClass`
//!   drops the window's KVO subclass (WebKit observes the window before
//!   we convert it), so AppKit can no longer remove those observers:
//!   destroying the window, detaching its webview, or WebKit reparenting
//!   it (element full-screen) aborts the process with NSRangeException.
//!   Today every CloseRequested is turned into a hide
//!   (`setup::handle_window_close` → `prevent_close`) and quitting goes
//!   through `process::exit`. If an overlay ever needs tearing down,
//!   restore its original class first.
//! * **Never call tao's `set_focusable` on an overlay.** It reads TaoWindow's
//!   `focusable` ivar, which the panel class doesn't have → panic. Change
//!   focusability through the panel config instead.
//! * AppKit is only touched on the main thread: `present` decides from the
//!   label (recorded at conversion), not by querying the window.

use tauri::{Manager, Runtime, WebviewWindow};

/// Labels successfully converted to panels (macOS). Lets `present` choose
/// the path without an AppKit call on the caller's thread.
#[cfg(target_os = "macos")]
static PANEL_LABELS: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

#[cfg(target_os = "macos")]
fn is_converted(label: &str) -> bool {
    PANEL_LABELS
        .lock()
        .map(|s| s.contains(label))
        .unwrap_or(false)
}

/// Labels treated as overlays.
pub const OVERLAY_LABELS: [&str; 3] = [
    crate::window_labels::FLOATING,
    crate::window_labels::INLINE_ASSIST,
    crate::window_labels::CONTEXT_MENU,
];

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(KageOverlayPanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false,
            is_floating_panel: true,
            hides_on_deactivate: false
        }
    })
}

/// Prepare an overlay window. Idempotent; safe from any thread (hops to
/// the main thread, which AppKit requires).
pub fn configure<R: Runtime>(window: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    {
        let use_panel = window
            .app_handle()
            .try_state::<crate::state::FeatureServices>()
            .map(|f| {
                use crate::lock_ext::LockExt;
                f.config.lock_or_recover().ui.macos_overlay_panels
            })
            .unwrap_or(true);
        let target = window.clone();
        if let Err(e) = window.run_on_main_thread(move || {
            let ns_window = match target.ns_window() {
                Ok(ptr) => ptr,
                Err(e) => {
                    log::error!("[overlay] no NSWindow for {}: {}", target.label(), e);
                    return;
                }
            };
            if use_panel {
                if crate::os::window_spaces::is_panel(ns_window) {
                    mark_converted(target.label());
                } else {
                    convert_to_panel(&target);
                }
            }
            crate::os::window_spaces::make_overlay_window(ns_window);
            // After the panel conversion, which sets the floating level.
            if target.label() == crate::window_labels::CONTEXT_MENU {
                crate::os::window_spaces::use_popup_menu_level(ns_window);
            }
        }) {
            log::error!("[overlay] failed to configure {}: {}", window.label(), e);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// `configure` every preloaded overlay window (the context menu is built
/// on demand and configured where it's built).
pub fn configure_all<R: Runtime>(app: &tauri::App<R>) {
    for label in OVERLAY_LABELS {
        if let Some(window) = app.get_webview_window(label) {
            configure(&window);
        }
    }
}

/// Show an overlay and give it keyboard focus. Use this instead of
/// `show()` + `set_focus()` for every overlay window.
pub fn present<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    if is_converted(window.label()) {
        let target = window.clone();
        return window.run_on_main_thread(move || match target.ns_window() {
            Ok(p) => crate::os::window_spaces::present_without_activating(p),
            Err(e) => log::error!("[overlay] present {}: {}", target.label(), e),
        });
    }
    window.show()?;
    window.set_focus()
}

#[cfg(target_os = "macos")]
fn mark_converted(label: &str) {
    if let Ok(mut s) = PANEL_LABELS.lock() {
        s.insert(label.to_string());
    }
}

/// Main thread only.
#[cfg(target_os = "macos")]
fn convert_to_panel<R: Runtime>(window: &WebviewWindow<R>) {
    use tauri_nspanel::Panel;
    match KageOverlayPanel::from_window(window) {
        Ok(panel) => {
            if let Err(e) =
                panel.add_style_mask(objc2_app_kit::NSWindowStyleMask::NonactivatingPanel)
            {
                log::warn!(
                    "[overlay] {}: couldn't add NonactivatingPanel: {:?}",
                    window.label(),
                    e
                );
            }
            // NSPanel defaults to hiding when the app deactivates; Kage's
            // own blur handling decides when overlays hide.
            panel.set_hides_on_deactivate(false);
            mark_converted(window.label());
            log::info!("[overlay] {} is now a non-activating panel", window.label());
            // Dropping the handle only releases its extra retain; the native
            // window stays owned by tao. See the module invariants: it must
            // never be destroyed while it has the panel class.
        }
        Err(e) => log::error!(
            "[overlay] {}: panel conversion failed, staying a window: {}",
            window.label(),
            e
        ),
    }
}

/// Frontend entry point: overlays call this instead of
/// `appWindow.show(); appWindow.setFocus()` so a panel is shown without
/// activating Kage (see module docs). Non-overlay windows get the plain
/// show + focus.
#[tauri::command]
pub async fn present_overlay_window<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<(), crate::error::AppError> {
    let result = if OVERLAY_LABELS.contains(&window.label()) {
        present(&window)
    } else {
        window.show().and_then(|_| window.set_focus())
    };
    result.map_err(|e| crate::error::AppError::internal(format!("present_overlay_window: {e}")))
}
