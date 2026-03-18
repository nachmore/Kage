//! Windows UI Automation accessibility provider using the `uiautomation` crate.

use log::{info, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use uiautomation::controls::ControlType;
use uiautomation::core::UIAutomation;
use uiautomation::core::UIElement as UiaElement;
use uiautomation::core::UITreeWalker;
use uiautomation::patterns::*;
use uiautomation::types::{ExpandCollapseState, ScrollAmount, ToggleState};

use crate::computer_control::tree::{self, UIElement};
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

const MAX_ELEMENTS: usize = 500;
const TREE_WALK_TIMEOUT_SECS: f64 = 8.0;
const SEARCH_TIMEOUT_SECS: f64 = 10.0;

// Thread-local native handle registry (UiaElement is !Send)
thread_local! {
    static NATIVE_REGISTRY: RefCell<HashMap<String, UiaElement>> = RefCell::new(HashMap::new());
}

fn register_native(elem: &UiaElement) -> String {
    let handle = elem.get_runtime_id().unwrap_or_default()
        .iter().fold(0u64, |acc, &v| acc.wrapping_mul(31).wrapping_add(v as u64));
    let id = tree::register_element(handle);
    NATIVE_REGISTRY.with(|r| r.borrow_mut().insert(id.clone(), elem.clone()));
    id
}

fn resolve_native(eid: &str) -> Result<UiaElement, String> {
    NATIVE_REGISTRY.with(|r| {
        r.borrow().get(eid).cloned().ok_or_else(|| {
            format!("Element '{}' not found. IDs are ephemeral — call get_ui_tree() to get fresh IDs.", eid)
        })
    })
}

fn clear_native() {
    NATIVE_REGISTRY.with(|r| r.borrow_mut().clear());
    tree::clear_registry();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
fn get_role(elem: &UiaElement) -> String {
    match elem.get_control_type() {
        Ok(ct) => match ct {
            ControlType::Button => "button", ControlType::Calendar => "calendar",
            ControlType::CheckBox => "checkbox", ControlType::ComboBox => "combobox",
            ControlType::Edit => "edit", ControlType::Hyperlink => "link",
            ControlType::Image => "image", ControlType::List => "list",
            ControlType::ListItem => "listitem", ControlType::Menu => "menu",
            ControlType::MenuBar => "menubar", ControlType::MenuItem => "menuitem",
            ControlType::ProgressBar => "progressbar", ControlType::RadioButton => "radiobutton",
            ControlType::ScrollBar => "scrollbar", ControlType::Slider => "slider",
            ControlType::Spinner => "spinner", ControlType::StatusBar => "statusbar",
            ControlType::Tab => "tab", ControlType::TabItem => "tabitem",
            ControlType::Text => "text", ControlType::ToolBar => "toolbar",
            ControlType::ToolTip => "tooltip", ControlType::Tree => "tree",
            ControlType::TreeItem => "treeitem", ControlType::Window => "window",
            ControlType::Pane => "pane", ControlType::Group => "group",
            ControlType::Thumb => "thumb", ControlType::DataGrid => "datagrid",
            ControlType::DataItem => "dataitem", ControlType::Document => "document",
            ControlType::SplitButton => "splitbutton", ControlType::Header => "header",
            ControlType::HeaderItem => "headeritem", ControlType::Table => "table",
            ControlType::TitleBar => "titlebar", ControlType::Separator => "separator",
            _ => "unknown",
        }.to_string(),
        Err(_) => "unknown".to_string(),
    }
}

fn safe_name(elem: &UiaElement) -> String { elem.get_name().unwrap_or_default() }
fn safe_automation_id(elem: &UiaElement) -> String { elem.get_automation_id().unwrap_or_default() }
fn safe_pid(elem: &UiaElement) -> u32 { elem.get_process_id().unwrap_or(0) as u32 }

fn get_value(elem: &UiaElement) -> String {
    if let Ok(vp) = elem.get_pattern::<UIValuePattern>() {
        if let Ok(v) = vp.get_value() { if !v.is_empty() { return v; } }
    }
    String::new()
}

fn get_bounds(elem: &UiaElement) -> Option<(i32, i32, i32, i32)> {
    if let Ok(rect) = elem.get_bounding_rectangle() {
        let w = rect.get_right() - rect.get_left();
        let h = rect.get_bottom() - rect.get_top();
        if w > 0 && h > 0 {
            return Some((rect.get_left() as i32, rect.get_top() as i32, w as i32, h as i32));
        }
    }
    None
}

fn get_actions(elem: &UiaElement) -> Vec<String> {
    let mut a = Vec::new();
    if elem.get_pattern::<UIInvokePattern>().is_ok() { a.push("invoke".into()); }
    if elem.get_pattern::<UIValuePattern>().is_ok() { a.push("set_value".into()); }
    if elem.get_pattern::<UITogglePattern>().is_ok() { a.push("toggle".into()); }
    if elem.get_pattern::<UISelectionItemPattern>().is_ok() { a.push("select".into()); }
    if elem.get_pattern::<UIExpandCollapsePattern>().is_ok() { a.push("expand_collapse".into()); }
    if elem.get_pattern::<UIScrollPattern>().is_ok() { a.push("scroll".into()); }
    if elem.get_pattern::<UITextPattern>().is_ok() { a.push("get_text".into()); }
    a
}

fn get_states(elem: &UiaElement) -> Vec<String> {
    let mut s = Vec::new();
    if let Ok(false) = elem.is_enabled() { s.push("disabled".into()); }
    if let Ok(true) = elem.is_offscreen() { s.push("offscreen".into()); }
    if let Ok(tp) = elem.get_pattern::<UITogglePattern>() {
        if let Ok(ts) = tp.get_toggle_state() {
            match ts {
                ToggleState::On => s.push("checked".into()),
                ToggleState::Off => s.push("unchecked".into()),
                _ => {}
            }
        }
    }
    if let Ok(ecp) = elem.get_pattern::<UIExpandCollapsePattern>() {
        if let Ok(es) = ecp.get_state() {
            match es {
                ExpandCollapseState::Expanded => s.push("expanded".into()),
                ExpandCollapseState::Collapsed => s.push("collapsed".into()),
                _ => {}
            }
        }
    }
    s
}

fn get_process_name(pid: u32) -> String {
    crate::os::process::get_process_name(pid).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Electron detection
// ---------------------------------------------------------------------------
const ELECTRON_PROCESSES: &[&str] = &[
    "code", "slack", "discord", "teams", "spotify", "notion",
    "obsidian", "figma", "postman", "signal", "whatsapp", "telegram", "bitwarden",
];

fn detect_electron_hint(elem: &UiaElement, count: usize) -> Option<String> {
    if count > 5 { return None; }
    let pname = get_process_name(safe_pid(elem));
    if pname.is_empty() { return None; }
    let lower = pname.to_lowercase();
    if ELECTRON_PROCESSES.iter().any(|&e| lower.contains(e)) {
        Some(format!("⚠️ Electron app ({}) with sparse tree ({} elements). Try --force-renderer-accessibility.", pname, count))
    } else { None }
}

// ---------------------------------------------------------------------------
// Tree walk state
// ---------------------------------------------------------------------------
struct WalkState { count: usize, truncated: bool, deadline: Instant }
impl WalkState {
    fn new(timeout: f64) -> Self {
        Self { count: 0, truncated: false, deadline: Instant::now() + std::time::Duration::from_secs_f64(timeout) }
    }
    fn exhausted(&mut self) -> bool {
        if self.count >= MAX_ELEMENTS || Instant::now() > self.deadline { self.truncated = true; true } else { false }
    }
}

// ---------------------------------------------------------------------------
// Tree building
// ---------------------------------------------------------------------------
fn build_element(walker: &UITreeWalker, elem: &UiaElement, depth: usize, max_depth: usize, include_invisible: bool, state: &mut WalkState) -> Option<UIElement> {
    if state.exhausted() { return None; }
    if !include_invisible { if let Ok(true) = elem.is_offscreen() { return None; } }

    let eid = register_native(elem);
    state.count += 1;

    let mut ui = UIElement::new(eid, get_role(elem));
    ui.name = safe_name(elem);
    ui.value = get_value(elem);
    ui.automation_id = safe_automation_id(elem);
    ui.states = get_states(elem);
    ui.actions = get_actions(elem);
    ui.bounds = get_bounds(elem);

    if depth < max_depth && !state.exhausted() {
        if let Ok(child) = walker.get_first_child(elem) {
            if let Some(c) = build_element(walker, &child, depth + 1, max_depth, include_invisible, state) {
                ui.children.push(c);
            }
            let mut next = child;
            while let Ok(sib) = walker.get_next_sibling(&next) {
                if state.exhausted() { break; }
                if let Some(c) = build_element(walker, &sib, depth + 1, max_depth, include_invisible, state) {
                    ui.children.push(c);
                }
                next = sib;
            }
        }
    }
    Some(ui)
}

fn find_window(automation: &UIAutomation, title: Option<&str>) -> Result<UiaElement, String> {
    if let Some(t) = title {
        let root = automation.get_root_element().map_err(|e| format!("Root: {}", e))?;
        let walker = automation.get_control_view_walker().map_err(|e| format!("Walker: {}", e))?;
        if let Ok(child) = walker.get_first_child(&root) {
            let mut cur = child;
            loop {
                if safe_name(&cur).to_lowercase().contains(&t.to_lowercase()) { return Ok(cur); }
                match walker.get_next_sibling(&cur) { Ok(n) => cur = n, Err(_) => break }
            }
        }
        Err(format!("No window matching '{}'. Use list_windows() to see available windows.", t))
    } else {
        automation.get_focused_element().map_err(|e| format!("No focused window: {}", e))
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------
pub fn get_ui_tree_impl(window_title: Option<&str>, max_depth: usize, include_invisible: bool) -> Result<UIElement, String> {
    let auto = UIAutomation::new().map_err(|e| format!("UIA init: {}", e))?;
    let walker = auto.get_control_view_walker().map_err(|e| format!("Walker: {}", e))?;
    clear_native();
    let win = find_window(&auto, window_title)?;
    let mut st = WalkState::new(TREE_WALK_TIMEOUT_SECS);
    let mut elem = build_element(&walker, &win, 0, max_depth, include_invisible, &mut st)
        .ok_or("Failed to build UI tree")?;
    let mut meta = Vec::new();
    if st.truncated { meta.push(format!("⚠️ Tree truncated at {} elements (limit={}).", st.count, MAX_ELEMENTS)); }
    if let Some(h) = detect_electron_hint(&win, st.count) { meta.push(h); }
    elem.meta = meta.join("\n");
    info!("get_ui_tree: {} elements, truncated={}", st.count, st.truncated);
    Ok(elem)
}

pub fn find_elements_impl(params: &FindElementsParams) -> Result<Vec<UIElement>, String> {
    let auto = UIAutomation::new().map_err(|e| format!("UIA init: {}", e))?;
    let walker = auto.get_control_view_walker().map_err(|e| format!("Walker: {}", e))?;
    clear_native();
    let win = find_window(&auto, params.window_title.as_deref())?;
    let mut results = Vec::new();
    let mut st = WalkState::new(SEARCH_TIMEOUT_SECS);
    search_recursive(&walker, &win, params, &mut results, 0, 10, &mut st);
    if st.truncated { warn!("find_elements: truncated at {} elements ({} matches)", st.count, results.len()); }
    Ok(results)
}

fn search_recursive(walker: &UITreeWalker, elem: &UiaElement, params: &FindElementsParams, results: &mut Vec<UIElement>, depth: usize, max_depth: usize, st: &mut WalkState) {
    if depth > max_depth || st.exhausted() { return; }
    st.count += 1;
    let mut matched = true;
    if let Some(ref r) = params.role { if get_role(elem) != r.to_lowercase() { matched = false; } }
    if matched { if let Some(ref n) = params.name { if !safe_name(elem).to_lowercase().contains(&n.to_lowercase()) { matched = false; } } }
    if matched { if let Some(ref a) = params.automation_id { if safe_automation_id(elem) != *a { matched = false; } } }
    if matched { if let Some(ref v) = params.value { if !get_value(elem).to_lowercase().contains(&v.to_lowercase()) { matched = false; } } }
    if matched && depth > 0 {
        let eid = register_native(elem);
        let mut ui = UIElement::new(eid, get_role(elem));
        ui.name = safe_name(elem); ui.value = get_value(elem); ui.automation_id = safe_automation_id(elem);
        ui.states = get_states(elem); ui.actions = get_actions(elem); ui.bounds = get_bounds(elem);
        results.push(ui);
    }
    if let Ok(child) = walker.get_first_child(elem) {
        search_recursive(walker, &child, params, results, depth + 1, max_depth, st);
        let mut next = child;
        while let Ok(sib) = walker.get_next_sibling(&next) {
            if st.exhausted() { break; }
            search_recursive(walker, &sib, params, results, depth + 1, max_depth, st);
            next = sib;
        }
    }
}

pub fn get_focused_element_impl() -> Result<Option<UIElement>, String> {
    let auto = UIAutomation::new().map_err(|e| format!("UIA init: {}", e))?;
    clear_native();
    match auto.get_focused_element() {
        Ok(f) => {
            let eid = register_native(&f);
            let mut ui = UIElement::new(eid, get_role(&f));
            ui.name = safe_name(&f); ui.value = get_value(&f); ui.automation_id = safe_automation_id(&f);
            ui.states = get_states(&f); ui.actions = get_actions(&f); ui.bounds = get_bounds(&f);
            Ok(Some(ui))
        }
        Err(_) => Ok(None),
    }
}

pub fn list_accessible_windows_impl(title_filter: Option<&str>) -> Result<Vec<AccessibleWindowInfo>, String> {
    let auto = UIAutomation::new().map_err(|e| format!("UIA init: {}", e))?;
    let root = auto.get_root_element().map_err(|e| format!("Root: {}", e))?;
    let walker = auto.get_control_view_walker().map_err(|e| format!("Walker: {}", e))?;
    let mut results = Vec::new();
    let Ok(child) = walker.get_first_child(&root) else { return Ok(results) };
    let mut cur = child;
    loop {
        if let Ok(ct) = cur.get_control_type() {
            if ct == ControlType::Window {
                let title = safe_name(&cur);
                let dominated = title_filter.map_or(false, |f| !title.to_lowercase().contains(&f.to_lowercase()));
                if !dominated {
                    if let Some(b) = get_bounds(&cur) {
                        if b.2 > 50 && b.3 > 50 {
                            let pid = safe_pid(&cur);
                            results.push(AccessibleWindowInfo { title, bounds: Some(b), process_id: pid, process_name: get_process_name(pid) });
                        }
                    }
                }
            }
        }
        match walker.get_next_sibling(&cur) { Ok(n) => cur = n, Err(_) => break }
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------
pub fn click_element_impl(element_id: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if let Ok(ip) = elem.get_pattern::<UIInvokePattern>() { if ip.invoke().is_ok() { return Ok(format!("Invoked [{}] '{}'", role, name)); } }
    if let Ok(tp) = elem.get_pattern::<UITogglePattern>() { if tp.toggle().is_ok() { let s = tp.get_toggle_state().map(|s| format!("{:?}", s)).unwrap_or_default(); return Ok(format!("Toggled [{}] '{}' → {}", role, name, s)); } }
    if let Ok(sp) = elem.get_pattern::<UISelectionItemPattern>() { if sp.select().is_ok() { return Ok(format!("Selected [{}] '{}'", role, name)); } }
    if let Ok(ecp) = elem.get_pattern::<UIExpandCollapsePattern>() {
        if let Ok(es) = ecp.get_state() {
            match es {
                ExpandCollapseState::Collapsed => { if ecp.expand().is_ok() { return Ok(format!("Expanded [{}] '{}'", role, name)); } }
                _ => { if ecp.collapse().is_ok() { return Ok(format!("Collapsed [{}] '{}'", role, name)); } }
            }
        }
    }
    if elem.click().is_ok() { return Ok(format!("Clicked [{}] '{}' (coordinate fallback)", role, name)); }
    Err(format!("Failed to click [{}] '{}'", role, name))
}
pub fn focus_element_impl(element_id: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if elem.set_focus().is_ok() {
        Ok(format!("Focused [{}] '{}'", role, name))
    } else {
        Err(format!("Failed to focus [{}] '{}'", role, name))
    }
}

pub fn set_element_value_impl(element_id: &str, value: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if let Ok(vp) = elem.get_pattern::<UIValuePattern>() { if vp.set_value(value).is_ok() { return Ok(format!("Set value on [{}] '{}'", role, name)); } }
    if elem.set_focus().is_ok() {
        let _ = elem.send_keys("{Ctrl}a", 20);
        let _ = elem.send_keys(value, 10);
        return Ok(format!("Typed value into [{}] '{}' (keyboard fallback)", role, name));
    }
    Err(format!("Failed to set value on [{}] '{}'", role, name))
}

pub fn toggle_element_impl(element_id: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if let Ok(tp) = elem.get_pattern::<UITogglePattern>() {
        if tp.toggle().is_ok() { let s = tp.get_toggle_state().map(|s| format!("{:?}", s)).unwrap_or_default(); return Ok(format!("Toggled [{}] '{}' → {}", role, name, s)); }
    }
    Err(format!("[{}] '{}' does not support toggle", role, name))
}

pub fn select_element_impl(element_id: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if let Ok(sp) = elem.get_pattern::<UISelectionItemPattern>() { if sp.select().is_ok() { return Ok(format!("Selected [{}] '{}'", role, name)); } }
    if let Ok(ip) = elem.get_pattern::<UIInvokePattern>() { if ip.invoke().is_ok() { return Ok(format!("Invoked [{}] '{}' (select fallback)", role, name)); } }
    Err(format!("[{}] '{}' does not support selection", role, name))
}

pub fn expand_element_impl(element_id: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if let Ok(ecp) = elem.get_pattern::<UIExpandCollapsePattern>() { if ecp.expand().is_ok() { return Ok(format!("Expanded [{}] '{}'", role, name)); } }
    Err(format!("[{}] '{}' does not support expand", role, name))
}

pub fn collapse_element_impl(element_id: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if let Ok(ecp) = elem.get_pattern::<UIExpandCollapsePattern>() { if ecp.collapse().is_ok() { return Ok(format!("Collapsed [{}] '{}'", role, name)); } }
    Err(format!("[{}] '{}' does not support collapse", role, name))
}

pub fn scroll_element_impl(element_id: &str, direction: &str, _amount: f64) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    let (role, name) = (get_role(&elem), safe_name(&elem));
    if let Ok(sp) = elem.get_pattern::<UIScrollPattern>() {
        let res = match direction {
            "up" => sp.scroll(ScrollAmount::NoAmount, ScrollAmount::SmallDecrement),
            "down" => sp.scroll(ScrollAmount::NoAmount, ScrollAmount::SmallIncrement),
            "left" => sp.scroll(ScrollAmount::SmallDecrement, ScrollAmount::NoAmount),
            "right" => sp.scroll(ScrollAmount::SmallIncrement, ScrollAmount::NoAmount),
            _ => return Err(format!("Invalid scroll direction: {}", direction)),
        };
        if res.is_ok() { return Ok(format!("Scrolled {} on [{}] '{}'", direction, role, name)); }
    }
    Err(format!("[{}] '{}' does not support scrolling", role, name))
}

pub fn get_element_text_impl(element_id: &str) -> Result<String, String> {
    let elem = resolve_native(element_id)?;
    if let Ok(tp) = elem.get_pattern::<UITextPattern>() {
        if let Ok(range) = tp.get_document_range() {
            if let Ok(text) = range.get_text(-1) { if !text.is_empty() { return Ok(text); } }
        }
    }
    if let Ok(vp) = elem.get_pattern::<UIValuePattern>() {
        if let Ok(v) = vp.get_value() { if !v.is_empty() { return Ok(v); } }
    }
    Ok(safe_name(&elem))
}

pub fn get_element_children_impl(element_id: &str, max_depth: usize) -> Result<UIElement, String> {
    let elem = resolve_native(element_id)?;
    let auto = UIAutomation::new().map_err(|e| format!("UIA init: {}", e))?;
    let walker = auto.get_control_view_walker().map_err(|e| format!("Walker: {}", e))?;
    let mut st = WalkState::new(TREE_WALK_TIMEOUT_SECS);
    build_element(&walker, &elem, 0, max_depth, false, &mut st)
        .ok_or_else(|| format!("Failed to build subtree for {}", element_id))
}
