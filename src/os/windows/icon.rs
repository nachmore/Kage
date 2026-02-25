// Windows icon extraction

pub fn extract_icon_base64_impl(path: &str) -> Option<String> {
    windows_icons::get_icon_base64_by_path(path).ok()
}
