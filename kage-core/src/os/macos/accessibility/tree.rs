//! Accessibility trust, window discovery, tree construction, and searches.

use accessibility_sys as ax;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use log::{info, warn};
use std::time::Instant;

use crate::computer_control::tree::UIElement;
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

use super::core::*;

const MAX_ELEMENTS: usize = 500;
const TREE_WALK_TIMEOUT_SECS: f64 = 5.0;
const SEARCH_TIMEOUT_SECS: f64 = 8.0;
/// Wall-clock cap on probing running apps for windows. With the messaging
/// timeout set, only several hung apps at once can approach this.
const ENUMERATE_WINDOWS_TIMEOUT_SECS: f64 = 5.0;
// Electron detection (port of the Windows provider's logic)
// ---------------------------------------------------------------------------
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

fn detect_electron_hint(pname: &str, count: usize) -> Option<String> {
    if count > 5 {
        return None;
    }
    if pname.is_empty() {
        return None;
    }
    let lower = pname.to_lowercase();
    if ELECTRON_PROCESSES.iter().any(|&e| lower.contains(e)) {
        Some(format!(
            "⚠️ Electron app ({}) with sparse tree ({} elements). \
             May need the app's own accessibility flag enabled.",
            pname, count
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// TCC permission check
// ---------------------------------------------------------------------------

/// One-time check on first provider entry: returns Ok(()) if
/// Accessibility permission is granted, Err(msg) otherwise. Using a
/// once-per-process check rather than per-call so we don't spam the
/// trust check (which is a syscall).
pub(crate) fn ensure_trusted() -> Result<(), String> {
    use std::sync::OnceLock;
    static TRUSTED: OnceLock<bool> = OnceLock::new();
    let ok = *TRUSTED.get_or_init(|| {
        let dict: CFDictionary<CFString, CFBoolean> = CFDictionary::from_CFType_pairs(&[(
            unsafe { CFString::wrap_under_get_rule(ax::kAXTrustedCheckOptionPrompt) },
            CFBoolean::false_value(),
        )]);
        unsafe { ax::AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef()) }
    });
    if ok {
        Ok(())
    } else {
        Err("Kage does not have Accessibility permission. Grant it in \
             System Settings → Privacy & Security → Accessibility and \
             restart Kage."
            .to_string())
    }
}

// ---------------------------------------------------------------------------
// Tree walk
// ---------------------------------------------------------------------------
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
            deadline: Instant::now() + std::time::Duration::from_secs_f64(timeout),
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

fn build_element(
    elem: ax::AXUIElementRef,
    depth: usize,
    max_depth: usize,
    include_invisible: bool,
    state: &mut WalkState,
) -> Option<UIElement> {
    if state.exhausted() {
        return None;
    }
    // We don't have a direct `IsOffscreen` AX attribute. Windows filters
    // invisible via UIA's IsOffscreen; on macOS, zero-sized or negative
    // bounds are the best proxy. If include_invisible is false and the
    // element has no usable bounds, include it anyway (menus, focus-only
    // roots legitimately have no bounds).
    // Fetched once and reused for the element's own bounds below.
    let bounds = get_bounds(elem);
    if !include_invisible {
        if let Some((_, _, w, h)) = bounds {
            if w <= 0 || h <= 0 {
                return None;
            }
        }
    }

    let eid = register_native(elem);
    state.count += 1;

    let mut ui = describe_element(elem, eid, bounds);

    if depth < max_depth && !state.exhausted() {
        let children = copy_elements_attr(elem, ax::kAXChildrenAttribute);
        for child in children {
            if state.exhausted() {
                break;
            }
            if let Some(c) = build_element(
                child.as_ref(),
                depth + 1,
                max_depth,
                include_invisible,
                state,
            ) {
                ui.children.push(c);
            }
        }
    }

    Some(ui)
}

/// Fill a UIElement's fields with one AX read per attribute. Each read is
/// a cross-process IPC into the target app, so role and value are fetched
/// once and shared by the state/action derivations instead of re-read.
fn describe_element(
    elem: ax::AXUIElementRef,
    eid: String,
    bounds: Option<(i32, i32, i32, i32)>,
) -> UIElement {
    let ax_role = get_ax_role(elem);
    let value = get_value(elem);
    let mut ui = UIElement::new(eid, normalize_role(&ax_role, ""));
    ui.name = safe_name(elem);
    ui.automation_id = safe_automation_id(elem);
    ui.states = get_states(elem, &ax_role, &value);
    ui.actions = get_actions(elem, &ax_role, &value);
    ui.value = value;
    ui.bounds = bounds;
    ui
}

// ---------------------------------------------------------------------------
// Window discovery — application enumeration via NSWorkspace
// ---------------------------------------------------------------------------

/// Enumerate AXWindowRefs belonging to running apps, optionally filtering
/// by window title substring (case-insensitive). `first_only` stops at the
/// first match — every app probed costs IPCs, so a title lookup shouldn't
/// pay for the whole process list.
fn enumerate_windows(
    title_filter: Option<&str>,
    first_only: bool,
) -> Vec<(AxElem, String, u32, String)> {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};

    let mut out = Vec::new();
    let workspace = NSWorkspace::sharedWorkspace();
    let apps: Retained<objc2_foundation::NSArray<objc2_app_kit::NSRunningApplication>> =
        workspace.runningApplications();
    let filter_lower = title_filter.map(str::to_lowercase);
    let deadline =
        Instant::now() + std::time::Duration::from_secs_f64(ENUMERATE_WINDOWS_TIMEOUT_SECS);

    for app in &apps {
        if Instant::now() > deadline {
            warn!(
                "enumerate_windows: deadline hit after {} windows; results partial",
                out.len()
            );
            break;
        }
        // Prohibited = background agents/daemons with no UI. They make up
        // most of runningApplications and can never own a window.
        if app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
            continue;
        }
        let pid = app.processIdentifier() as u32;
        if pid == 0 {
            continue;
        }
        let process_name = app
            .localizedName()
            .map(|s| s.to_string())
            .unwrap_or_default();

        let app_elem = unsafe { ax::AXUIElementCreateApplication(pid as ax::pid_t) };
        if app_elem.is_null() {
            continue;
        }
        let app_elem = AxElem::take(app_elem);

        let windows = copy_elements_attr(app_elem.as_ref(), ax::kAXWindowsAttribute);
        for win in windows {
            let title = safe_name(win.as_ref());
            if let Some(filter) = &filter_lower {
                if !title.to_lowercase().contains(filter.as_str()) {
                    continue;
                }
            }
            out.push((win, title, pid, process_name.clone()));
            if first_only {
                return out;
            }
        }
    }
    out
}

/// Set the process-wide AX messaging timeout. Setting it on the
/// system-wide element changes the default for every AX element, so a hung
/// (beachballing) app costs this per IPC instead of the ~6s system default
/// — which would otherwise stall the single AX worker and every job queued
/// behind it. Not so short that big trees in slow apps (Xcode, Electron)
/// fail legitimate reads. Call once, on the AX worker thread.
pub(crate) fn configure_messaging_timeout() {
    const AX_MESSAGING_TIMEOUT_SECS: f32 = 1.0;
    let system = unsafe { ax::AXUIElementCreateSystemWide() };
    if system.is_null() {
        warn!("AXUIElementCreateSystemWide returned null; AX messaging timeout not set");
        return;
    }
    let system = AxElem::take(system);
    let err =
        unsafe { ax::AXUIElementSetMessagingTimeout(system.as_ref(), AX_MESSAGING_TIMEOUT_SECS) };
    if err != ax::kAXErrorSuccess {
        warn!("AXUIElementSetMessagingTimeout failed: {}", err);
    }
}

fn find_window(title: Option<&str>) -> Result<AxElem, String> {
    if let Some(t) = title {
        let wins = enumerate_windows(Some(t), true);
        if let Some((win, _, _, _)) = wins.into_iter().next() {
            return Ok(win);
        }
        Err(format!(
            "No window matching '{}'. Use list_windows() to see available windows.",
            t
        ))
    } else {
        // No filter → return the focused window of the frontmost app.
        let system = unsafe { ax::AXUIElementCreateSystemWide() };
        if system.is_null() {
            return Err("AXUIElementCreateSystemWide returned null".into());
        }
        let system = AxElem::take(system);
        copy_element_attr(system.as_ref(), ax::kAXFocusedUIElementAttribute)
            .and_then(|focused| {
                // Walk up to AXWindow if we're on a descendant.
                let mut cur = focused;
                for _ in 0..10 {
                    let role = copy_string_attr(cur.as_ref(), ax::kAXRoleAttribute);
                    if role == "AXWindow" {
                        return Some(cur);
                    }
                    match copy_element_attr(cur.as_ref(), ax::kAXParentAttribute) {
                        Some(p) => cur = p,
                        None => break,
                    }
                }
                None
            })
            .ok_or_else(|| "No focused window found".to_string())
    }
}

// ---------------------------------------------------------------------------
// Worker-thread entry points (F5a — tree + windows + focused + children)
//
// These run on the acp-ax-worker thread; the public `*_impl` functions
// at the bottom submit jobs and wait. Each `_inner` performs the real
// AX work.
// ---------------------------------------------------------------------------

pub(crate) fn get_ui_tree_inner(
    window_title: Option<&str>,
    max_depth: usize,
    include_invisible: bool,
) -> Result<UIElement, String> {
    ensure_trusted()?;
    clear_native();
    let win = find_window(window_title)?;
    let mut st = WalkState::new(TREE_WALK_TIMEOUT_SECS);
    let mut elem = build_element(win.as_ref(), 0, max_depth, include_invisible, &mut st)
        .ok_or("Failed to build UI tree")?;

    let mut meta = Vec::new();
    if st.truncated {
        meta.push(format!(
            "⚠️ Tree truncated at {} elements (limit={}).",
            st.count, MAX_ELEMENTS
        ));
    }
    let pid = safe_pid(win.as_ref());
    let pname = get_process_name(pid);
    if let Some(h) = detect_electron_hint(&pname, st.count) {
        meta.push(h);
    }
    elem.meta = meta.join("\n");
    info!(
        "get_ui_tree: {} elements, truncated={}",
        st.count, st.truncated
    );
    Ok(elem)
}

pub(crate) fn list_accessible_windows_inner(
    title_filter: Option<&str>,
) -> Result<Vec<AccessibleWindowInfo>, String> {
    ensure_trusted()?;
    let wins = enumerate_windows(title_filter, false);
    let mut out = Vec::with_capacity(wins.len());
    for (win, title, pid, pname) in wins {
        let bounds = get_bounds(win.as_ref());
        out.push(AccessibleWindowInfo {
            title,
            bounds,
            process_id: pid,
            process_name: pname,
        });
    }
    Ok(out)
}

pub(crate) fn get_focused_element_inner() -> Result<Option<UIElement>, String> {
    ensure_trusted()?;
    let system = unsafe { ax::AXUIElementCreateSystemWide() };
    if system.is_null() {
        return Err("AXUIElementCreateSystemWide returned null".into());
    }
    let system = AxElem::take(system);
    let focused = match copy_element_attr(system.as_ref(), ax::kAXFocusedUIElementAttribute) {
        Some(e) => e,
        None => return Ok(None),
    };
    let eid = register_native(focused.as_ref());
    let bounds = get_bounds(focused.as_ref());
    Ok(Some(describe_element(focused.as_ref(), eid, bounds)))
}

pub(crate) fn get_element_children_inner(
    element_id: &str,
    max_depth: usize,
) -> Result<UIElement, String> {
    ensure_trusted()?;
    // NOTE: unlike `get_ui_tree_inner`, we deliberately DON'T clear the
    // registry here. The LLM uses this to drill into a subtree after
    // calling `get_ui_tree`; clearing would invalidate every ID it
    // already holds. Matches the Windows provider's behaviour — the
    // registry growth across many drill-downs in one session is
    // bounded by MAX_ELEMENTS per call and the next `get_ui_tree` will
    // wipe the slate.
    let elem = resolve_native(element_id)?;
    let mut st = WalkState::new(TREE_WALK_TIMEOUT_SECS);
    build_element(elem.as_ref(), 0, max_depth, true, &mut st).ok_or_else(|| {
        format!(
            "Failed to build subtree for element {} (truncated at {})",
            element_id, st.count
        )
    })
}

// ---------------------------------------------------------------------------
// F5b — read ops: find_elements, get_element_text
// ---------------------------------------------------------------------------

fn matches_predicate(elem: ax::AXUIElementRef, params: &FindElementsParams) -> bool {
    if let Some(ref r) = params.role {
        if get_role(elem) != r.to_lowercase() {
            return false;
        }
    }
    if let Some(ref n) = params.name {
        if !safe_name(elem).to_lowercase().contains(&n.to_lowercase()) {
            return false;
        }
    }
    if let Some(ref a) = params.automation_id {
        if safe_automation_id(elem) != *a {
            return false;
        }
    }
    if let Some(ref v) = params.value {
        if !get_value(elem).to_lowercase().contains(&v.to_lowercase()) {
            return false;
        }
    }
    true
}

fn search_recursive(
    elem: ax::AXUIElementRef,
    params: &FindElementsParams,
    results: &mut Vec<UIElement>,
    depth: usize,
    max_depth: usize,
    st: &mut WalkState,
) {
    if depth > max_depth || st.exhausted() {
        return;
    }
    st.count += 1;

    // Only accumulate matches from depth > 0 to match the Windows
    // provider — the window root itself is excluded from results.
    if depth > 0 && matches_predicate(elem, params) {
        let eid = register_native(elem);
        results.push(describe_element(elem, eid, get_bounds(elem)));
    }

    let children = copy_elements_attr(elem, ax::kAXChildrenAttribute);
    for child in children {
        if st.exhausted() {
            break;
        }
        search_recursive(child.as_ref(), params, results, depth + 1, max_depth, st);
    }
}

pub(crate) fn find_elements_inner(p: &FindElementsParams) -> Result<Vec<UIElement>, String> {
    ensure_trusted()?;
    let win = find_window(p.window_title.as_deref())?;
    let mut results = Vec::new();
    let mut st = WalkState::new(SEARCH_TIMEOUT_SECS);
    // Matches Windows' max-depth of 10 for find_elements — deep enough to
    // cover real UIs without letting pathological trees burn the timeout.
    search_recursive(win.as_ref(), p, &mut results, 0, 10, &mut st);
    if st.truncated {
        warn!(
            "find_elements: truncated at {} elements ({} matches)",
            st.count,
            results.len()
        );
    }
    Ok(results)
}

pub(crate) fn get_element_text_inner(element_id: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    // Primary source: the AXValue attribute (text field contents, static
    // text label value, etc.). Fall back to selected-text when the widget
    // exposes a selection rather than a full value (e.g. AXTextArea with
    // a selected range but an empty direct value).
    let v = copy_value_as_string(elem.as_ref());
    if !v.is_empty() {
        return Ok(v);
    }
    let selected = copy_string_attr(elem.as_ref(), ax::kAXSelectedTextAttribute);
    if !selected.is_empty() {
        return Ok(selected);
    }
    // Last-ditch: the element's displayed title/description. Useful for
    // static-text-only elements whose value attribute is intentionally empty.
    Ok(safe_name(elem.as_ref()))
}
