//! Linux accessibility provider — not yet implemented.

use crate::computer_control::tree::UIElement;
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

const NOT_IMPL: &str = "Linux accessibility provider not yet implemented";

pub fn get_ui_tree_impl(_title: Option<&str>, _depth: usize, _invisible: bool) -> Result<UIElement, String> { Err(NOT_IMPL.into()) }
pub fn find_elements_impl(_p: &FindElementsParams) -> Result<Vec<UIElement>, String> { Err(NOT_IMPL.into()) }
pub fn get_focused_element_impl() -> Result<Option<UIElement>, String> { Err(NOT_IMPL.into()) }
pub fn list_accessible_windows_impl(_filter: Option<&str>) -> Result<Vec<AccessibleWindowInfo>, String> { Err(NOT_IMPL.into()) }
pub fn click_element_impl(_id: &str) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn set_element_value_impl(_id: &str, _val: &str) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn toggle_element_impl(_id: &str) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn select_element_impl(_id: &str) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn expand_element_impl(_id: &str) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn collapse_element_impl(_id: &str) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn scroll_element_impl(_id: &str, _dir: &str, _amt: f64) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn get_element_text_impl(_id: &str) -> Result<String, String> { Err(NOT_IMPL.into()) }
pub fn get_element_children_impl(_id: &str, _depth: usize) -> Result<UIElement, String> { Err(NOT_IMPL.into()) }
