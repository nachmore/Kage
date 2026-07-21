//! macOS accessibility provider.
//!
//! The AX worker owns the native element registry. This facade submits public
//! requests to that worker; focused modules contain native decoding, traversal,
//! and mutating operations.

#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

mod actions;
mod core;
mod tree;

use crate::computer_control::tree::UIElement;
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

use super::ax_worker::{self, Job};

pub(super) use actions::{
    click_element_inner, collapse_element_inner, expand_element_inner, focus_element_inner,
    scroll_element_inner, select_element_inner, set_element_value_inner, toggle_element_inner,
};
pub(super) use tree::{
    find_elements_inner, get_element_children_inner, get_element_text_inner,
    get_focused_element_inner, get_ui_tree_inner, list_accessible_windows_inner,
};
// Public dispatch — every _impl submits a job to the worker and blocks
// ---------------------------------------------------------------------------

pub fn get_ui_tree_impl(
    title: Option<&str>,
    depth: usize,
    invisible: bool,
) -> Result<UIElement, String> {
    let title = title.map(String::from);
    ax_worker::submit(
        |reply| Job::GetUiTree {
            window_title: title,
            max_depth: depth,
            include_invisible: invisible,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn list_accessible_windows_impl(
    filter: Option<&str>,
) -> Result<Vec<AccessibleWindowInfo>, String> {
    let filter = filter.map(String::from);
    ax_worker::submit(
        |reply| Job::ListAccessibleWindows {
            title_filter: filter,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn get_focused_element_impl() -> Result<Option<UIElement>, String> {
    ax_worker::submit(
        |reply| Job::GetFocusedElement { reply },
        || Err("AX worker not running".into()),
    )
}

pub fn get_element_children_impl(id: &str, depth: usize) -> Result<UIElement, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::GetElementChildren {
            element_id: id,
            max_depth: depth,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn find_elements_impl(p: &FindElementsParams) -> Result<Vec<UIElement>, String> {
    // Clone into an owned copy so we can move it across the channel.
    let params = FindElementsParams {
        name: p.name.clone(),
        role: p.role.clone(),
        automation_id: p.automation_id.clone(),
        value: p.value.clone(),
        window_title: p.window_title.clone(),
    };
    ax_worker::submit(
        |reply| Job::FindElements { params, reply },
        || Err("AX worker not running".into()),
    )
}

pub fn click_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::ClickElement {
            element_id: id,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn focus_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::FocusElement {
            element_id: id,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn set_element_value_impl(id: &str, val: &str) -> Result<String, String> {
    let (id, val) = (id.to_string(), val.to_string());
    ax_worker::submit(
        |reply| Job::SetElementValue {
            element_id: id,
            value: val,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn toggle_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::ToggleElement {
            element_id: id,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn select_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::SelectElement {
            element_id: id,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn expand_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::ExpandElement {
            element_id: id,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn collapse_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::CollapseElement {
            element_id: id,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn scroll_element_impl(id: &str, dir: &str, amt: f64) -> Result<String, String> {
    let (id, dir) = (id.to_string(), dir.to_string());
    ax_worker::submit(
        |reply| Job::ScrollElement {
            element_id: id,
            direction: dir,
            amount: amt,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

pub fn get_element_text_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::GetElementText {
            element_id: id,
            reply,
        },
        || Err("AX worker not running".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::core::normalize_role;

    #[test]
    fn maps_common_ax_roles_to_cross_platform_tokens() {
        // Buttons, inputs, containers — the tokens the LLM sees must match
        // the Windows provider's output so prompts and examples are portable.
        assert_eq!(normalize_role("AXButton", ""), "button");
        assert_eq!(normalize_role("AXCheckBox", ""), "checkbox");
        assert_eq!(normalize_role("AXRadioButton", ""), "radiobutton");
        assert_eq!(normalize_role("AXTextField", ""), "edit");
        assert_eq!(normalize_role("AXTextArea", ""), "edit");
        assert_eq!(normalize_role("AXStaticText", ""), "text");
        assert_eq!(normalize_role("AXList", ""), "list");
        assert_eq!(normalize_role("AXOutline", ""), "tree");
        assert_eq!(normalize_role("AXWindow", ""), "window");
        assert_eq!(normalize_role("AXGroup", ""), "group");
        assert_eq!(normalize_role("AXSplitGroup", ""), "group");
        assert_eq!(normalize_role("AXPopUpButton", ""), "combobox");
        assert_eq!(normalize_role("AXMenuBarItem", ""), "menuitem");
    }

    #[test]
    fn subrole_does_not_override_core_role() {
        // AX close/minimize buttons are all just `button` to the LLM —
        // subrole info doesn't leak into the normalised token.
        assert_eq!(normalize_role("AXButton", "AXCloseButton"), "button");
        assert_eq!(normalize_role("AXButton", "AXMinimizeButton"), "button");
        assert_eq!(normalize_role("AXButton", "AXZoomButton"), "button");
    }

    #[test]
    fn unknown_roles_fall_back_to_lowercased_stripped_prefix() {
        // Unmapped AX roles should still produce readable tokens rather
        // than empty strings, so the LLM sees *something* to reason about.
        assert_eq!(normalize_role("AXMatte", ""), "matte");
        assert_eq!(normalize_role("AXRuler", ""), "ruler");
        // Missing AX prefix still lowercases sensibly.
        assert_eq!(normalize_role("Something", ""), "something");
    }

    #[test]
    fn empty_role_returns_unknown() {
        assert_eq!(normalize_role("", ""), "unknown");
    }
}
