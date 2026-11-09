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

/// GTK runs on Wayland when the session is Wayland and nothing forces the
/// X11 backend (`GDK_BACKEND=x11`).
pub fn can_position_windows_impl() -> bool {
    !is_wayland(
        std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
        std::env::var("GDK_BACKEND").ok().as_deref(),
    )
}

/// `GDK_BACKEND` is a comma-separated preference list; GTK tries its
/// first entry. Unset or empty: Wayland when `WAYLAND_DISPLAY` is set.
fn is_wayland(wayland_display: bool, gdk_backend: Option<&str>) -> bool {
    match gdk_backend.map(str::trim) {
        Some(backend) if !backend.is_empty() => backend
            .split(',')
            .next()
            .is_some_and(|first| first.trim() == "wayland"),
        _ => wayland_display,
    }
}

#[cfg(test)]
mod tests {
    use super::is_wayland;

    #[test]
    fn wayland_detection() {
        assert!(is_wayland(true, None));
        assert!(is_wayland(true, Some("")));
        assert!(!is_wayland(false, None));
        assert!(!is_wayland(true, Some("x11")));
        assert!(is_wayland(true, Some("wayland,x11")));
        assert!(!is_wayland(true, Some("x11,wayland")));
        assert!(is_wayland(false, Some("wayland")));
    }
}
