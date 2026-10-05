//! UIA element property extraction and conversion to the portable tree type.

use uiautomation::controls::ControlType;
use uiautomation::core::{UIAutomation, UICacheRequest, UIElement as UiaElement};
use uiautomation::patterns::*;
use uiautomation::types::{ExpandCollapseState, Rect, ToggleState, UIProperty};

use crate::computer_control::tree::UIElement;

use super::native_registry;

/// Everything `to_ui_element` and element search read. Prefetching these
/// through a cache request turns ~20 cross-process UIA calls per node into
/// the walker step plus the runtime-id lookup in `native_registry::register`,
/// which is what keeps big Electron/browser trees inside the walk timeout.
const CACHED_PROPERTIES: &[UIProperty] = &[
    UIProperty::ControlType,
    UIProperty::Name,
    UIProperty::AutomationId,
    UIProperty::BoundingRectangle,
    UIProperty::IsEnabled,
    UIProperty::IsOffscreen,
    UIProperty::ValueValue,
    UIProperty::ToggleToggleState,
    UIProperty::ExpandCollapseExpandCollapseState,
    UIProperty::IsInvokePatternAvailable,
    UIProperty::IsValuePatternAvailable,
    UIProperty::IsTogglePatternAvailable,
    UIProperty::IsSelectionItemPatternAvailable,
    UIProperty::IsExpandCollapsePatternAvailable,
    UIProperty::IsScrollPatternAvailable,
    UIProperty::IsTextPatternAvailable,
];

/// Build the worker's cache request. Element mode stays Full (the default)
/// so registered elements remain live references for later actions.
pub(crate) fn create_cache_request(automation: &UIAutomation) -> Result<UICacheRequest, String> {
    let request = automation
        .create_cache_request()
        .map_err(|error| format!("Cache request: {}", error))?;
    for &property in CACHED_PROPERTIES {
        request
            .add_property(property)
            .map_err(|error| format!("Cache property {:?}: {}", property, error))?;
    }
    Ok(request)
}

/// True when `elem` came from a build-cache call. Uncached elements (window
/// roots, the focused element) fail this locally, without a round trip.
fn is_cached(elem: &UiaElement) -> bool {
    elem.get_cached_control_type().is_ok()
}

fn cached_bool(elem: &UiaElement, property: UIProperty) -> Option<bool> {
    elem.get_cached_property_value(property)
        .ok()?
        .try_into()
        .ok()
}

fn cached_i32(elem: &UiaElement, property: UIProperty) -> Option<i32> {
    elem.get_cached_property_value(property)
        .ok()?
        .try_into()
        .ok()
}

fn cached_string(elem: &UiaElement, property: UIProperty) -> Option<String> {
    elem.get_cached_property_value(property)
        .ok()?
        .try_into()
        .ok()
}

fn has_cached_pattern(elem: &UiaElement, availability: UIProperty) -> bool {
    cached_bool(elem, availability).unwrap_or(false)
}

pub(super) fn role(elem: &UiaElement) -> String {
    role_from(elem.get_control_type().ok())
}

fn role_from(control_type: Option<ControlType>) -> String {
    match control_type {
        Some(ControlType::Button) => "button",
        Some(ControlType::Calendar) => "calendar",
        Some(ControlType::CheckBox) => "checkbox",
        Some(ControlType::ComboBox) => "combobox",
        Some(ControlType::Edit) => "edit",
        Some(ControlType::Hyperlink) => "link",
        Some(ControlType::Image) => "image",
        Some(ControlType::List) => "list",
        Some(ControlType::ListItem) => "listitem",
        Some(ControlType::Menu) => "menu",
        Some(ControlType::MenuBar) => "menubar",
        Some(ControlType::MenuItem) => "menuitem",
        Some(ControlType::ProgressBar) => "progressbar",
        Some(ControlType::RadioButton) => "radiobutton",
        Some(ControlType::ScrollBar) => "scrollbar",
        Some(ControlType::Slider) => "slider",
        Some(ControlType::Spinner) => "spinner",
        Some(ControlType::StatusBar) => "statusbar",
        Some(ControlType::Tab) => "tab",
        Some(ControlType::TabItem) => "tabitem",
        Some(ControlType::Text) => "text",
        Some(ControlType::ToolBar) => "toolbar",
        Some(ControlType::ToolTip) => "tooltip",
        Some(ControlType::Tree) => "tree",
        Some(ControlType::TreeItem) => "treeitem",
        Some(ControlType::Window) => "window",
        Some(ControlType::Pane) => "pane",
        Some(ControlType::Group) => "group",
        Some(ControlType::Thumb) => "thumb",
        Some(ControlType::DataGrid) => "datagrid",
        Some(ControlType::DataItem) => "dataitem",
        Some(ControlType::Document) => "document",
        Some(ControlType::SplitButton) => "splitbutton",
        Some(ControlType::Header) => "header",
        Some(ControlType::HeaderItem) => "headeritem",
        Some(ControlType::Table) => "table",
        Some(ControlType::TitleBar) => "titlebar",
        Some(ControlType::Separator) => "separator",
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
    rect_bounds(elem.get_bounding_rectangle().ok()?)
}

fn rect_bounds(rect: Rect) -> Option<(i32, i32, i32, i32)> {
    let width = rect.get_right() - rect.get_left();
    let height = rect.get_bottom() - rect.get_top();
    if width > 0 && height > 0 {
        Some((rect.get_left(), rect.get_top(), width, height))
    } else {
        None
    }
}

/// Offscreen check for the tree walk: cached when the element was fetched
/// with the cache request, a live call otherwise.
pub(super) fn is_offscreen(elem: &UiaElement) -> bool {
    match elem.is_cached_offscreen() {
        Ok(offscreen) => offscreen,
        Err(_) => matches!(elem.is_offscreen(), Ok(true)),
    }
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

fn cached_value(elem: &UiaElement) -> String {
    if has_cached_pattern(elem, UIProperty::IsValuePatternAvailable) {
        cached_string(elem, UIProperty::ValueValue).unwrap_or_default()
    } else {
        String::new()
    }
}

fn cached_actions(elem: &UiaElement) -> Vec<String> {
    [
        (UIProperty::IsInvokePatternAvailable, "invoke"),
        (UIProperty::IsValuePatternAvailable, "set_value"),
        (UIProperty::IsTogglePatternAvailable, "toggle"),
        (UIProperty::IsSelectionItemPatternAvailable, "select"),
        (
            UIProperty::IsExpandCollapsePatternAvailable,
            "expand_collapse",
        ),
        (UIProperty::IsScrollPatternAvailable, "scroll"),
        (UIProperty::IsTextPatternAvailable, "get_text"),
    ]
    .into_iter()
    .filter(|&(availability, _)| has_cached_pattern(elem, availability))
    .map(|(_, action)| action.to_string())
    .collect()
}

fn cached_states(elem: &UiaElement) -> Vec<String> {
    let mut states = Vec::new();
    if let Ok(false) = elem.is_cached_enabled() {
        states.push("disabled".into());
    }
    if let Ok(true) = elem.is_cached_offscreen() {
        states.push("offscreen".into());
    }
    if has_cached_pattern(elem, UIProperty::IsTogglePatternAvailable) {
        match cached_i32(elem, UIProperty::ToggleToggleState) {
            Some(state) if state == ToggleState::On as i32 => states.push("checked".into()),
            Some(state) if state == ToggleState::Off as i32 => states.push("unchecked".into()),
            _ => {}
        }
    }
    if has_cached_pattern(elem, UIProperty::IsExpandCollapsePatternAvailable) {
        match cached_i32(elem, UIProperty::ExpandCollapseExpandCollapseState) {
            Some(state) if state == ExpandCollapseState::Expanded as i32 => {
                states.push("expanded".into())
            }
            Some(state) if state == ExpandCollapseState::Collapsed as i32 => {
                states.push("collapsed".into())
            }
            _ => {}
        }
    }
    states
}

/// Search-match accessors: cached reads when available, live otherwise.
/// (The plain accessors above stay live: actions use them on registered
/// elements whose cache may be stale by then.)
pub(super) fn match_role(elem: &UiaElement) -> String {
    if is_cached(elem) {
        role_from(elem.get_cached_control_type().ok())
    } else {
        role(elem)
    }
}

pub(super) fn match_name(elem: &UiaElement) -> String {
    elem.get_cached_name().unwrap_or_else(|_| name(elem))
}

pub(super) fn match_automation_id(elem: &UiaElement) -> String {
    elem.get_cached_automation_id()
        .unwrap_or_else(|_| automation_id(elem))
}

pub(super) fn match_value(elem: &UiaElement) -> String {
    if is_cached(elem) {
        cached_value(elem)
    } else {
        value(elem)
    }
}

pub(super) fn to_ui_element(elem: &UiaElement) -> UIElement {
    if is_cached(elem) {
        return to_ui_element_cached(elem);
    }
    let mut ui = UIElement::new(native_registry::register(elem), role(elem));
    ui.name = name(elem);
    ui.value = value(elem);
    ui.automation_id = automation_id(elem);
    ui.states = states(elem);
    ui.actions = actions(elem);
    ui.bounds = bounds(elem);
    ui
}

fn to_ui_element_cached(elem: &UiaElement) -> UIElement {
    let mut ui = UIElement::new(
        native_registry::register(elem),
        role_from(elem.get_cached_control_type().ok()),
    );
    ui.name = elem.get_cached_name().unwrap_or_default();
    ui.value = cached_value(elem);
    ui.automation_id = elem.get_cached_automation_id().unwrap_or_default();
    ui.states = cached_states(elem);
    ui.actions = cached_actions(elem);
    ui.bounds = elem
        .get_cached_bounding_rectangle()
        .ok()
        .and_then(rect_bounds);
    ui
}
