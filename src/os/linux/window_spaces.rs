// macOS-only concept (Spaces / full-screen auxiliary windows); no-op here.

#[allow(dead_code)]
pub fn make_overlay_window_impl(_ns_window: *mut std::ffi::c_void) {}

#[allow(dead_code)]
pub fn is_panel_impl(_ns_window: *mut std::ffi::c_void) -> bool {
    false
}

#[allow(dead_code)]
pub fn present_without_activating_impl(_ns_window: *mut std::ffi::c_void) {}

#[allow(dead_code)]
pub fn use_popup_menu_level_impl(_ns_window: *mut std::ffi::c_void) {}
