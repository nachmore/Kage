//! Cross-platform accessibility API for computer control.
//!
//! Each function delegates to the platform-specific `accessibility_impl()` in
//! `src/os/{windows,macos,linux}/accessibility.rs`.
//!
//! Consumed by the `computer-control-mcp` binary via the lib crate.

use crate::computer_control::tree::UIElement;

/// Parameters for searching elements.
pub struct FindElementsParams {
    pub name: Option<String>,
    pub role: Option<String>,
    pub automation_id: Option<String>,
    pub value: Option<String>,
    pub window_title: Option<String>,
}

/// Window info returned by list_windows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccessibleWindowInfo {
    pub title: String,
    pub bounds: Option<(i32, i32, i32, i32)>,
    pub process_id: u32,
    pub process_name: String,
}

// ---------------------------------------------------------------------------
// Cross-platform dispatch
// ---------------------------------------------------------------------------

pub fn get_ui_tree(
    window_title: Option<&str>,
    max_depth: usize,
    include_invisible: bool,
) -> Result<UIElement, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::get_ui_tree_impl(window_title, max_depth, include_invisible) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::get_ui_tree_impl(window_title, max_depth, include_invisible) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::get_ui_tree_impl(window_title, max_depth, include_invisible) }
}

pub fn find_elements(params: &FindElementsParams) -> Result<Vec<UIElement>, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::find_elements_impl(params) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::find_elements_impl(params) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::find_elements_impl(params) }
}

pub fn get_focused_element() -> Result<Option<UIElement>, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::get_focused_element_impl() }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::get_focused_element_impl() }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::get_focused_element_impl() }
}

pub fn list_accessible_windows(title_filter: Option<&str>) -> Result<Vec<AccessibleWindowInfo>, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::list_accessible_windows_impl(title_filter) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::list_accessible_windows_impl(title_filter) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::list_accessible_windows_impl(title_filter) }
}

pub fn click_element(element_id: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::click_element_impl(element_id) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::click_element_impl(element_id) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::click_element_impl(element_id) }
}

pub fn set_element_value(element_id: &str, value: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::set_element_value_impl(element_id, value) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::set_element_value_impl(element_id, value) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::set_element_value_impl(element_id, value) }
}

pub fn toggle_element(element_id: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::toggle_element_impl(element_id) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::toggle_element_impl(element_id) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::toggle_element_impl(element_id) }
}

pub fn select_element(element_id: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::select_element_impl(element_id) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::select_element_impl(element_id) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::select_element_impl(element_id) }
}

pub fn expand_element(element_id: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::expand_element_impl(element_id) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::expand_element_impl(element_id) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::expand_element_impl(element_id) }
}

pub fn collapse_element(element_id: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::collapse_element_impl(element_id) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::collapse_element_impl(element_id) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::collapse_element_impl(element_id) }
}

pub fn scroll_element(element_id: &str, direction: &str, amount: f64) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::scroll_element_impl(element_id, direction, amount) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::scroll_element_impl(element_id, direction, amount) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::scroll_element_impl(element_id, direction, amount) }
}

pub fn get_element_text(element_id: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::get_element_text_impl(element_id) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::get_element_text_impl(element_id) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::get_element_text_impl(element_id) }
}

pub fn get_element_children(element_id: &str, max_depth: usize) -> Result<UIElement, String> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::accessibility::get_element_children_impl(element_id, max_depth) }

    #[cfg(target_os = "macos")]
    { crate::os::macos::accessibility::get_element_children_impl(element_id, max_depth) }

    #[cfg(target_os = "linux")]
    { crate::os::linux::accessibility::get_element_children_impl(element_id, max_depth) }
}
