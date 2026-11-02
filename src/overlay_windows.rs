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

use tauri::{Manager, Runtime, WebviewWindow};

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
            if use_panel && !crate::os::window_spaces::is_panel(ns_window) {
                convert_to_panel(&target);
            }
            crate::os::window_spaces::make_overlay_window(ns_window);
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
    {
        if let Ok(ptr) = window.ns_window() {
            if crate::os::window_spaces::is_panel(ptr) {
                let target = window.clone();
                return window.run_on_main_thread(move || match target.ns_window() {
                    Ok(p) => crate::os::window_spaces::present_without_activating(p),
                    Err(e) => log::error!("[overlay] present {}: {}", target.label(), e),
                });
            }
        }
    }
    window.show()?;
    window.set_focus()
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
            log::info!("[overlay] {} is now a non-activating panel", window.label());
            // Dropping the handle only releases its extra retain; the native
            // window stays owned by tao.
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
