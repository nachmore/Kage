// Cross-platform application icon extraction

/// Extract an application icon as a base64-encoded data URI from a file path.
/// Returns None if icon extraction is not supported or fails.
pub fn extract_icon_base64(path: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::icon::extract_icon_base64_impl(path) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::icon::extract_icon_base64_impl(path) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::icon::extract_icon_base64_impl(path) }
}
