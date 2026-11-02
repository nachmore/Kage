//! Spaces / full-screen behaviour for Kage's overlay windows (floating
//! launcher, inline assist, context menu).
//!
//! On macOS a window only appears on another app's full-screen Space if it
//! opts in: `alwaysOnTop` alone raises it within the current Space, so
//! summoning Kage over a full-screen app either did nothing visible or
//! yanked the user back to the desktop Space. Spotlight-style overlays set
//! `CanJoinAllSpaces | FullScreenAuxiliary`; this module does the same.
//! Windows / Linux have no equivalent concept and are no-ops.

/// Let the native window identified by `ns_window` (an `NSWindow*` on
/// macOS; ignored elsewhere) appear on every Space, including over
/// full-screen apps. Must be called on the main thread.
// Only the macOS cfg branch of `overlay_windows::configure` calls this;
// keep the cross-platform surface without tripping dead_code elsewhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn make_overlay_window(ns_window: *mut std::ffi::c_void) {
    crate::os::platform::window_spaces::make_overlay_window_impl(ns_window)
}

/// True when the native window is an `NSPanel` (i.e. it was converted by
/// `crate::overlay_windows`). Always false off macOS.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn is_panel(ns_window: *mut std::ffi::c_void) -> bool {
    crate::os::platform::window_spaces::is_panel_impl(ns_window)
}

/// Bring the window to the front and make it key **without activating the
/// app** (`orderFrontRegardless` + `makeKeyWindow`). For a non-activating
/// panel this is what lets it take keyboard input over a full-screen app
/// without macOS switching Spaces. Main thread only.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn present_without_activating(ns_window: *mut std::ffi::c_void) {
    crate::os::platform::window_spaces::present_without_activating_impl(ns_window)
}
