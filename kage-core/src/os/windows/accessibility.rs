//! Windows UI Automation accessibility provider.
//!
//! Public `*_impl` functions submit work to the dedicated UIA worker. Native
//! UIA handles and all inner operations stay on that thread because
//! `UiaElement` is not `Send`.

mod actions;
mod element;
mod native_registry;
mod traversal;

use crate::computer_control::tree::UIElement;
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

use super::uia_worker::{self, Job, WorkerState};

pub(super) use actions::{
    click_element_inner, collapse_element_inner, expand_element_inner, focus_element_inner,
    get_element_text_inner, scroll_element_inner, select_element_inner, set_element_value_inner,
    toggle_element_inner,
};
pub(super) use traversal::{
    find_elements_inner, get_element_children_inner, get_focused_element_inner, get_ui_tree_inner,
    list_accessible_windows_inner,
};

/// Submit a tree snapshot request to the UIA worker.
pub fn get_ui_tree_impl(
    window_title: Option<&str>,
    max_depth: usize,
    include_invisible: bool,
) -> Result<UIElement, String> {
    let window_title = window_title.map(|s| s.to_string());
    uia_worker::submit(
        |reply| Job::GetUiTree {
            window_title,
            max_depth,
            include_invisible,
            reply,
        },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn find_elements_impl(params: &FindElementsParams) -> Result<Vec<UIElement>, String> {
    let params = FindElementsParams {
        name: params.name.clone(),
        role: params.role.clone(),
        automation_id: params.automation_id.clone(),
        value: params.value.clone(),
        window_title: params.window_title.clone(),
    };
    uia_worker::submit(
        |reply| Job::FindElements { params, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn get_focused_element_impl() -> Result<Option<UIElement>, String> {
    uia_worker::submit(
        |reply| Job::GetFocusedElement { reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn list_accessible_windows_impl(
    title_filter: Option<&str>,
) -> Result<Vec<AccessibleWindowInfo>, String> {
    let title_filter = title_filter.map(|s| s.to_string());
    uia_worker::submit(
        |reply| Job::ListAccessibleWindows {
            title_filter,
            reply,
        },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn click_element_impl(element_id: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::ClickElement { element_id, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn focus_element_impl(element_id: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::FocusElement { element_id, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn set_element_value_impl(element_id: &str, value: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    let value = value.to_string();
    uia_worker::submit(
        |reply| Job::SetElementValue {
            element_id,
            value,
            reply,
        },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn toggle_element_impl(element_id: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::ToggleElement { element_id, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn select_element_impl(element_id: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::SelectElement { element_id, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn expand_element_impl(element_id: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::ExpandElement { element_id, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn collapse_element_impl(element_id: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::CollapseElement { element_id, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn scroll_element_impl(
    element_id: &str,
    direction: &str,
    amount: f64,
) -> Result<String, String> {
    let element_id = element_id.to_string();
    let direction = direction.to_string();
    uia_worker::submit(
        |reply| Job::ScrollElement {
            element_id,
            direction,
            amount,
            reply,
        },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn get_element_text_impl(element_id: &str) -> Result<String, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::GetElementText { element_id, reply },
        || Err("UIA worker not running".to_string()),
    )
}

pub fn get_element_children_impl(element_id: &str, max_depth: usize) -> Result<UIElement, String> {
    let element_id = element_id.to_string();
    uia_worker::submit(
        |reply| Job::GetElementChildren {
            element_id,
            max_depth,
            reply,
        },
        || Err("UIA worker not running".to_string()),
    )
}
