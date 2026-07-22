//! UIA element property extraction and conversion to the portable tree type.

use uiautomation::controls::ControlType;
use uiautomation::core::UIElement as UiaElement;
use uiautomation::patterns::*;
use uiautomation::types::{ExpandCollapseState, ToggleState};

use crate::computer_control::tree::UIElement;

use super::native_registry;

pub(super) fn role(elem: &UiaElement) -> String {
    match elem.get_control_type() {
        Ok(ControlType::Button) => "button",
        Ok(ControlType::Calendar) => "calendar",
        Ok(ControlType::CheckBox) => "checkbox",
        Ok(ControlType::ComboBox) => "combobox",
        Ok(ControlType::Edit) => "edit",
        Ok(ControlType::Hyperlink) => "link",
        Ok(ControlType::Image) => "image",
        Ok(ControlType::List) => "list",
        Ok(ControlType::ListItem) => "listitem",
        Ok(ControlType::Menu) => "menu",
        Ok(ControlType::MenuBar) => "menubar",
        Ok(ControlType::MenuItem) => "menuitem",
        Ok(ControlType::ProgressBar) => "progressbar",
        Ok(ControlType::RadioButton) => "radiobutton",
        Ok(ControlType::ScrollBar) => "scrollbar",
        Ok(ControlType::Slider) => "slider",
        Ok(ControlType::Spinner) => "spinner",
        Ok(ControlType::StatusBar) => "statusbar",
        Ok(ControlType::Tab) => "tab",
        Ok(ControlType::TabItem) => "tabitem",
        Ok(ControlType::Text) => "text",
        Ok(ControlType::ToolBar) => "toolbar",
        Ok(ControlType::ToolTip) => "tooltip",
        Ok(ControlType::Tree) => "tree",
        Ok(ControlType::TreeItem) => "treeitem",
        Ok(ControlType::Window) => "window",
        Ok(ControlType::Pane) => "pane",
        Ok(ControlType::Group) => "group",
        Ok(ControlType::Thumb) => "thumb",
        Ok(ControlType::DataGrid) => "datagrid",
        Ok(ControlType::DataItem) => "dataitem",
        Ok(ControlType::Document) => "document",
        Ok(ControlType::SplitButton) => "splitbutton",
        Ok(ControlType::Header) => "header",
        Ok(ControlType::HeaderItem) => "headeritem",
        Ok(ControlType::Table) => "table",
        Ok(ControlType::TitleBar) => "titlebar",
        Ok(ControlType::Separator) => "separator",
        _ => "unknown",
    }
    .to_string()
}

pub(super) fn name(elem: &UiaElement) -> String {
    elem.get_name().unwrap_or_default()
}

pub(super) fn automation_id(elem: &UiaElement) -> String {
    elem.get_automation_id().unwrap_or_default()
}

pub(super) fn process_id(elem: &UiaElement) -> u32 {
    elem.get_process_id().unwrap_or(0)
}

pub(super) fn value(elem: &UiaElement) -> String {
    if let Ok(pattern) = elem.get_pattern::<UIValuePattern>() {
        if let Ok(value) = pattern.get_value() {
            if !value.is_empty() {
                return value;
            }
        }
    }
    String::new()
}

pub(super) fn bounds(elem: &UiaElement) -> Option<(i32, i32, i32, i32)> {
    if let Ok(rect) = elem.get_bounding_rectangle() {
        let width = rect.get_right() - rect.get_left();
        let height = rect.get_bottom() - rect.get_top();
        if width > 0 && height > 0 {
            return Some((rect.get_left(), rect.get_top(), width, height));
        }
    }
    None
}

fn actions(elem: &UiaElement) -> Vec<String> {
    let mut actions = Vec::new();
    if elem.get_pattern::<UIInvokePattern>().is_ok() {
        actions.push("invoke".into());
    }
    if elem.get_pattern::<UIValuePattern>().is_ok() {
        actions.push("set_value".into());
    }
    if elem.get_pattern::<UITogglePattern>().is_ok() {
        actions.push("toggle".into());
    }
    if elem.get_pattern::<UISelectionItemPattern>().is_ok() {
        actions.push("select".into());
    }
    if elem.get_pattern::<UIExpandCollapsePattern>().is_ok() {
        actions.push("expand_collapse".into());
    }
    if elem.get_pattern::<UIScrollPattern>().is_ok() {
        actions.push("scroll".into());
    }
    if elem.get_pattern::<UITextPattern>().is_ok() {
        actions.push("get_text".into());
    }
    actions
}

fn states(elem: &UiaElement) -> Vec<String> {
    let mut states = Vec::new();
    if let Ok(false) = elem.is_enabled() {
        states.push("disabled".into());
    }
    if let Ok(true) = elem.is_offscreen() {
        states.push("offscreen".into());
    }
    if let Ok(pattern) = elem.get_pattern::<UITogglePattern>() {
        match pattern.get_toggle_state() {
            Ok(ToggleState::On) => states.push("checked".into()),
            Ok(ToggleState::Off) => states.push("unchecked".into()),
            _ => {}
        }
    }
    if let Ok(pattern) = elem.get_pattern::<UIExpandCollapsePattern>() {
        match pattern.get_state() {
            Ok(ExpandCollapseState::Expanded) => states.push("expanded".into()),
            Ok(ExpandCollapseState::Collapsed) => states.push("collapsed".into()),
            _ => {}
        }
    }
    states
}

pub(super) fn to_ui_element(elem: &UiaElement) -> UIElement {
    let mut ui = UIElement::new(native_registry::register(elem), role(elem));
    ui.name = name(elem);
    ui.value = value(elem);
    ui.automation_id = automation_id(elem);
    ui.states = states(elem);
    ui.actions = actions(elem);
    ui.bounds = bounds(elem);
    ui
}
