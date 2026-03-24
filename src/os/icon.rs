// Cross-platform application icon extraction

/// Extract an application icon as a base64-encoded data URI from a file path.
/// Returns None if icon extraction is not supported or fails.
pub fn extract_icon_base64(path: &str) -> Option<String> {
    crate::os::platform::icon::extract_icon_base64_impl(path)
}
