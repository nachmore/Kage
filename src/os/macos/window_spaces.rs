//! macOS: opt an overlay window into every Space, including full-screen ones.

use objc2::ClassType;
use objc2_app_kit::{NSPanel, NSPopUpMenuWindowLevel, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::NSObjectProtocol;

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

pub fn is_panel_impl(ns_window: *mut std::ffi::c_void) -> bool {
    if ns_window.is_null() {
        return false;
    }
    // SAFETY: live NSWindow* from Tauri; isKindOfClass: is a read-only
    // runtime query.
    let window: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };
    window.isKindOfClass(NSPanel::class())
}

pub fn use_popup_menu_level_impl(ns_window: *mut std::ffi::c_void) {
    if ns_window.is_null() {
        return;
    }
    // SAFETY: live NSWindow* from Tauri, called on the main thread.
    let window: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };
    window.setLevel(NSPopUpMenuWindowLevel);
}

pub fn present_without_activating_impl(ns_window: *mut std::ffi::c_void) {
    if ns_window.is_null() {
        return;
    }
    // SAFETY: live NSWindow* from Tauri, called on the main thread.
    let window: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };
    window.orderFrontRegardless();
    window.makeKeyWindow();
}
