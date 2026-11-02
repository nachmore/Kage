//! macOS: opt an overlay window into every Space, including full-screen ones.

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

pub fn make_overlay_window_impl(ns_window: *mut std::ffi::c_void) {
    if ns_window.is_null() {
        return;
    }
    // SAFETY: the caller passes the live NSWindow* from Tauri's
    // `ns_window()` and runs us on the main thread (NSWindow is
    // main-thread-only).
    let window: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };
    let wanted = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    // CanJoinAllSpaces and MoveToActiveSpace are mutually exclusive.
    let behavior =
        (window.collectionBehavior() - NSWindowCollectionBehavior::MoveToActiveSpace) | wanted;
    window.setCollectionBehavior(behavior);
    log::info!(
        "[spaces] overlay window collectionBehavior = {:#x}",
        window.collectionBehavior().0
    );
}
