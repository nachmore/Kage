//! UIA window lookup, tree construction, and element search.

use std::time::{Duration, Instant};

use log::{info, warn};
use uiautomation::controls::ControlType;
use uiautomation::core::{UIAutomation, UIElement as UiaElement, UITreeWalker};

use crate::computer_control::tree::UIElement;
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

use super::element;
use super::native_registry;
use super::WorkerState;

const MAX_ELEMENTS: usize = 500;
const TREE_WALK_TIMEOUT_SECS: f64 = 8.0;
const SEARCH_TIMEOUT_SECS: f64 = 10.0;

const ELECTRON_PROCESSES: &[&str] = &[
    "code",
    "slack",
    "discord",
    "teams",
    "spotify",
    "notion",
    "obsidian",
    "figma",
    "postman",
    "signal",
    "whatsapp",
    "telegram",
    "bitwarden",
];

struct WalkState {
    count: usize,
    truncated: bool,
    deadline: Instant,
}

impl WalkState {
    fn new(timeout: f64) -> Self {
        Self {
            count: 0,
            truncated: false,
            deadline: Instant::now() + Duration::from_secs_f64(timeout),
        }
    }

    fn exhausted(&mut self) -> bool {
        if self.count >= MAX_ELEMENTS || Instant::now() > self.deadline {
            self.truncated = true;
            true
        } else {
            false
        }
    }
}

fn process_name(pid: u32) -> String {
    crate::os::process::get_process_name(pid).unwrap_or_default()
}

fn electron_hint(elem: &UiaElement, count: usize) -> Option<String> {
    if count > 5 {
        return None;
    }
    let process_name = process_name(element::process_id(elem));
    if process_name.is_empty() {
        return None;
    }
    if ELECTRON_PROCESSES
        .iter()
        .any(|process| process_name.to_lowercase().contains(process))
    {
        Some(format!(
            "⚠️ Electron app ({}) with sparse tree ({} elements). Try --force-renderer-accessibility.",
            process_name, count
        ))
    } else {
        None
    }
}

fn build_element(
    walker: &UITreeWalker,
    elem: &UiaElement,
    depth: usize,
    max_depth: usize,
    include_invisible: bool,
    state: &mut WalkState,
) -> Option<UIElement> {
    if state.exhausted() {
        return None;
    }
    if !include_invisible && matches!(elem.is_offscreen(), Ok(true)) {
        return None;
    }

    let mut ui = element::to_ui_element(elem);
    state.count += 1;

    if depth < max_depth && !state.exhausted() {
        if let Ok(child) = walker.get_first_child(elem) {
            if let Some(child_ui) = build_element(
                walker,
                &child,
                depth + 1,
                max_depth,
                include_invisible,
                state,
            ) {
                ui.children.push(child_ui);
            }
            let mut next = child;
            while let Ok(sibling) = walker.get_next_sibling(&next) {
                if state.exhausted() {
                    break;
                }
                if let Some(sibling_ui) = build_element(
                    walker,
                    &sibling,
                    depth + 1,
                    max_depth,
                    include_invisible,
                    state,
                ) {
                    ui.children.push(sibling_ui);
                }
                next = sibling;
            }
        }
    }
    Some(ui)
}

fn find_window(automation: &UIAutomation, title: Option<&str>) -> Result<UiaElement, String> {
    let Some(title) = title else {
        return automation
            .get_focused_element()
            .map_err(|error| format!("No focused window: {}", error));
    };

    let root = automation
        .get_root_element()
        .map_err(|error| format!("Root: {}", error))?;
    let walker = automation
        .get_control_view_walker()
        .map_err(|error| format!("Walker: {}", error))?;
    if let Ok(child) = walker.get_first_child(&root) {
        let mut current = child;
        loop {
            if element::name(&current)
                .to_lowercase()
                .contains(&title.to_lowercase())
            {
                return Ok(current);
            }
            match walker.get_next_sibling(&current) {
                Ok(next) => current = next,
                Err(_) => break,
            }
        }
    }
    Err(format!(
        "No window matching '{}'. Use list_windows() to see available windows.",
        title
    ))
}

pub(crate) fn get_ui_tree_inner(
    state: &WorkerState,
    window_title: Option<&str>,
    max_depth: usize,
    include_invisible: bool,
) -> Result<UIElement, String> {
    native_registry::clear();
    let window = find_window(&state.automation, window_title)?;
    let mut walk_state = WalkState::new(TREE_WALK_TIMEOUT_SECS);
    let mut element = build_element(
        &state.walker,
        &window,
        0,
        max_depth,
        include_invisible,
        &mut walk_state,
    )
    .ok_or("Failed to build UI tree")?;
    let mut meta = Vec::new();
    if walk_state.truncated {
        meta.push(format!(
            "⚠️ Tree truncated at {} elements (limit={}).",
            walk_state.count, MAX_ELEMENTS
        ));
    }
    if let Some(hint) = electron_hint(&window, walk_state.count) {
        meta.push(hint);
    }
    element.meta = meta.join("\n");
    info!(
        "get_ui_tree: {} elements, truncated={}",
        walk_state.count, walk_state.truncated
    );
    Ok(element)
}

pub(crate) fn find_elements_inner(
    state: &WorkerState,
    params: &FindElementsParams,
) -> Result<Vec<UIElement>, String> {
    let window = find_window(&state.automation, params.window_title.as_deref())?;
    let mut results = Vec::new();
    let mut walk_state = WalkState::new(SEARCH_TIMEOUT_SECS);
    search_recursive(
        &state.walker,
        &window,
        params,
        &mut results,
        0,
        10,
        &mut walk_state,
    );
    if walk_state.truncated {
        warn!(
            "find_elements: truncated at {} elements ({} matches)",
            walk_state.count,
            results.len()
        );
    }
    Ok(results)
}

fn search_recursive(
    walker: &UITreeWalker,
    elem: &UiaElement,
    params: &FindElementsParams,
    results: &mut Vec<UIElement>,
    depth: usize,
    max_depth: usize,
    state: &mut WalkState,
) {
    if depth > max_depth || state.exhausted() {
        return;
    }
    state.count += 1;
    let matched = params
        .role
        .as_ref()
        .is_none_or(|role| element::role(elem) == role.to_lowercase())
        && params.name.as_ref().is_none_or(|name| {
            element::name(elem)
                .to_lowercase()
                .contains(&name.to_lowercase())
        })
        && params
            .automation_id
            .as_ref()
            .is_none_or(|automation_id| element::automation_id(elem) == *automation_id)
        && params.value.as_ref().is_none_or(|value| {
            element::value(elem)
                .to_lowercase()
                .contains(&value.to_lowercase())
        });
    if matched && depth > 0 {
        results.push(element::to_ui_element(elem));
    }
    if let Ok(child) = walker.get_first_child(elem) {
        search_recursive(walker, &child, params, results, depth + 1, max_depth, state);
        let mut next = child;
        while let Ok(sibling) = walker.get_next_sibling(&next) {
            if state.exhausted() {
                break;
            }
            search_recursive(
                walker,
                &sibling,
                params,
                results,
                depth + 1,
                max_depth,
                state,
            );
            next = sibling;
        }
    }
}

pub(crate) fn get_focused_element_inner(state: &WorkerState) -> Result<Option<UIElement>, String> {
    Ok(state
        .automation
        .get_focused_element()
        .ok()
        .map(|element| element::to_ui_element(&element)))
}

pub(crate) fn list_accessible_windows_inner(
    state: &WorkerState,
    title_filter: Option<&str>,
) -> Result<Vec<AccessibleWindowInfo>, String> {
    let root = state
        .automation
        .get_root_element()
        .map_err(|error| format!("Root: {}", error))?;
    let mut results = Vec::new();
    let Ok(child) = state.walker.get_first_child(&root) else {
        return Ok(results);
    };
    let mut current = child;
    loop {
        if matches!(current.get_control_type(), Ok(ControlType::Window)) {
            let title = element::name(&current);
            let excluded = title_filter
                .is_some_and(|filter| !title.to_lowercase().contains(&filter.to_lowercase()));
            if !excluded {
                if let Some(bounds) = element::bounds(&current) {
                    if bounds.2 > 50 && bounds.3 > 50 {
                        let process_id = element::process_id(&current);
                        results.push(AccessibleWindowInfo {
                            title,
                            bounds: Some(bounds),
                            process_id,
                            process_name: process_name(process_id),
                        });
                    }
                }
            }
        }
        match state.walker.get_next_sibling(&current) {
            Ok(next) => current = next,
            Err(_) => break,
        }
    }
    Ok(results)
}

pub(crate) fn get_element_children_inner(
    state: &WorkerState,
    element_id: &str,
    max_depth: usize,
) -> Result<UIElement, String> {
    let elem = native_registry::resolve(element_id)?;
    let mut walk_state = WalkState::new(TREE_WALK_TIMEOUT_SECS);
    build_element(&state.walker, &elem, 0, max_depth, false, &mut walk_state)
        .ok_or_else(|| format!("Failed to build subtree for {}", element_id))
}
