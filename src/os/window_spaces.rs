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
pub fn make_overlay_window(ns_window: *mut std::ffi::c_void) {
    crate::os::platform::window_spaces::make_overlay_window_impl(ns_window)
}
