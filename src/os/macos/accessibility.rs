//! macOS accessibility provider.
//!
//! Mirrors the Windows UIA provider's shape: a dedicated worker thread
//! (`super::ax_worker`) owns a `thread_local!` registry mapping ephemeral
//! `e{N}` IDs to retained `AXUIElementRef` handles. Every public
//! `*_impl` function is a thin wrapper that builds a `Job` and submits
//! it; the worker runs the matching `*_inner` function that contains
//! the real AX logic.
//!
//! Requires the **Accessibility** TCC permission at runtime. The first
//! AX call checks `AXIsProcessTrustedWithOptions` and returns a clear
//! "grant Accessibility in System Settings" error if the permission is
//! missing, instead of letting every individual AX call fail with the
//! opaque `kAXErrorAPIDisabled`.

#![allow(non_upper_case_globals)] // kAX* constants from accessibility-sys
#![allow(non_snake_case)]

use accessibility_sys as ax;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGSize};
use log::{info, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::time::Instant;

use crate::computer_control::tree::{self, UIElement};
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

use super::ax_worker::{self, Job};

// ---------------------------------------------------------------------------
// Constants — match the Windows provider's tuning
// ---------------------------------------------------------------------------
const MAX_ELEMENTS: usize = 500;
const TREE_WALK_TIMEOUT_SECS: f64 = 5.0;
const SEARCH_TIMEOUT_SECS: f64 = 8.0;

// ---------------------------------------------------------------------------
// Retained AXUIElementRef wrapper — CFRetain in `new`, CFRelease in `Drop`
// ---------------------------------------------------------------------------
/// Owned `AXUIElementRef` — adds one retain, drops one release.
struct AxElem(ax::AXUIElementRef);

impl AxElem {
    /// Retain and wrap. Caller must have a valid, non-null ref.
    fn from_ref(r: ax::AXUIElementRef) -> Self {
        unsafe {
            CFRetain(r as CFTypeRef);
        }
        Self(r)
    }

    /// Take ownership of a ref from a `Copy*` function (which already did
    /// the retain on our behalf). Use this only for AX functions documented
    /// as returning an owned reference.
    fn take(r: ax::AXUIElementRef) -> Self {
        Self(r)
    }

    fn as_ref(&self) -> ax::AXUIElementRef {
        self.0
    }
}

impl Drop for AxElem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0 as CFTypeRef);
            }
        }
    }
}

impl Clone for AxElem {
    fn clone(&self) -> Self {
        AxElem::from_ref(self.0)
    }
}

// `AxElem` holds a raw `AXUIElementRef` (`*mut _`), so it's not `Send` by
// default. We never cross thread boundaries with it: the registry is
// `thread_local!` on the worker thread, and `enumerate_windows` /
// `find_window` are called only from the same worker. No `unsafe impl`
// needed — if a future change starts sending `AxElem` through the Job
// channel, the compiler will flag it here and force a deliberate decision.

// ---------------------------------------------------------------------------
// Native element registry (thread-local, lives on the worker thread)
// ---------------------------------------------------------------------------
thread_local! {
    static NATIVE_REGISTRY: RefCell<HashMap<String, AxElem>> = RefCell::new(HashMap::new());
}

fn register_native(elem: ax::AXUIElementRef) -> String {
    // Opaque u64 identity: pointer value itself is fine for disambiguation.
    let handle = elem as usize as u64;
    let id = tree::register_element(handle);
    NATIVE_REGISTRY.with(|r| {
        r.borrow_mut()
            .insert(id.clone(), AxElem::from_ref(elem));
    });
    id
}

fn resolve_native(eid: &str) -> Result<AxElem, String> {
    NATIVE_REGISTRY.with(|r| {
        r.borrow().get(eid).cloned().ok_or_else(|| {
            format!(
                "Element '{}' not found. IDs are ephemeral — \
                 call get_ui_tree() to get fresh IDs.",
                eid
            )
        })
    })
}

fn clear_native() {
    // `AxElem` Drop releases each retained handle.
    NATIVE_REGISTRY.with(|r| r.borrow_mut().clear());
    tree::clear_registry();
}

// ---------------------------------------------------------------------------
// AX attribute helpers — safe wrappers around AXUIElementCopyAttributeValue
// ---------------------------------------------------------------------------

/// Read an attribute whose value is a CFStringRef. Returns empty string
/// on any failure (missing attribute, wrong type, etc.) to match the
/// Windows helpers' "fall back quietly" behavior.
fn copy_string_attr(elem: ax::AXUIElementRef, attr: &str) -> String {
    let cf_attr = CFString::new(attr);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return String::new();
    }
    // Take ownership — the Copy function retained.
    unsafe {
        let cfstr = value as CFStringRef;
        let result = cfstring_to_string(cfstr);
        CFRelease(value);
        result
    }
}

/// Read an attribute whose value is a CFBooleanRef. Returns None on any
/// failure (missing, wrong type, etc.). Verifies the returned CFType is
/// actually a CFBoolean before casting — AX occasionally returns
/// `CFNumber(0|1)` for attributes that *should* be booleans.
fn copy_bool_attr(elem: ax::AXUIElementRef, attr: &str) -> Option<bool> {
    let cf_attr = CFString::new(attr);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return None;
    }
    unsafe {
        let type_id = core_foundation::base::CFGetTypeID(value);
        if type_id == core_foundation::boolean::CFBoolean::type_id() {
            let cfbool: CFBoolean = CFBoolean::wrap_under_create_rule(value as _);
            Some(bool::from(cfbool))
        } else if type_id == core_foundation::number::CFNumber::type_id() {
            // Some Cocoa widgets return CFNumber for "boolean" attributes.
            // Treat non-zero as true. `CFNumber::wrap_under_create_rule`
            // takes the retain so we don't need a manual CFRelease.
            let cfnum: CFNumber = CFNumber::wrap_under_create_rule(value as _);
            cfnum.to_i64().map(|n| n != 0)
        } else {
            CFRelease(value);
            None
        }
    }
}

/// Read an attribute whose value is an AXUIElementRef. Returns None on failure.
/// Caller owns the returned element (takes one retain).
fn copy_element_attr(elem: ax::AXUIElementRef, attr: &str) -> Option<AxElem> {
    let cf_attr = CFString::new(attr);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return None;
    }
    Some(AxElem::take(value as ax::AXUIElementRef))
}

/// Read an attribute whose value is a CFArrayRef of AXUIElementRef. Each
/// element is retained into the returned Vec (AxElem owns).
fn copy_elements_attr(elem: ax::AXUIElementRef, attr: &str) -> Vec<AxElem> {
    let cf_attr = CFString::new(attr);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return Vec::new();
    }
    unsafe {
        let array_ref = value as CFArrayRef;
        let array: CFArray<*const c_void> = CFArray::wrap_under_create_rule(array_ref);
        let mut out = Vec::with_capacity(array.len() as usize);
        for item in array.iter() {
            let ptr = *item as ax::AXUIElementRef;
            if !ptr.is_null() {
                out.push(AxElem::from_ref(ptr));
            }
        }
        out
    }
}

/// Read a `kAXPositionAttribute` / `kAXSizeAttribute` AXValue. Returns
/// None if the attribute isn't present or isn't the expected AXValue type.
fn copy_axvalue_point(elem: ax::AXUIElementRef) -> Option<CGPoint> {
    let cf_attr = CFString::new(ax::kAXPositionAttribute);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return None;
    }
    unsafe {
        let mut point = CGPoint { x: 0.0, y: 0.0 };
        let ok = ax::AXValueGetValue(
            value as ax::AXValueRef,
            ax::kAXValueTypeCGPoint,
            &mut point as *mut _ as *mut c_void,
        );
        CFRelease(value);
        if ok {
            Some(point)
        } else {
            None
        }
    }
}

fn copy_axvalue_size(elem: ax::AXUIElementRef) -> Option<CGSize> {
    let cf_attr = CFString::new(ax::kAXSizeAttribute);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return None;
    }
    unsafe {
        let mut size = CGSize {
            width: 0.0,
            height: 0.0,
        };
        let ok = ax::AXValueGetValue(
            value as ax::AXValueRef,
            ax::kAXValueTypeCGSize,
            &mut size as *mut _ as *mut c_void,
        );
        CFRelease(value);
        if ok {
            Some(size)
        } else {
            None
        }
    }
}

/// Read the value of an attribute as a CFType and format it as a String.
/// Used for `kAXValueAttribute` which can be a string, number, or bool
/// depending on the widget.
fn copy_value_as_string(elem: ax::AXUIElementRef) -> String {
    let cf_attr = CFString::new(ax::kAXValueAttribute);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        ax::AXUIElementCopyAttributeValue(elem, cf_attr.as_concrete_TypeRef(), &mut value)
    };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return String::new();
    }
    unsafe {
        let type_id = core_foundation::base::CFGetTypeID(value);
        let result = if type_id == core_foundation::string::CFString::type_id() {
            cfstring_to_string(value as CFStringRef)
        } else if type_id == core_foundation::number::CFNumber::type_id() {
            let cfnum: CFNumber = CFNumber::wrap_under_get_rule(value as _);
            cfnum
                .to_i64()
                .map(|n| n.to_string())
                .or_else(|| cfnum.to_f64().map(|f| f.to_string()))
                .unwrap_or_default()
        } else if type_id == core_foundation::boolean::CFBoolean::type_id() {
            let cfbool: CFBoolean = CFBoolean::wrap_under_get_rule(value as _);
            if bool::from(cfbool) {
                "true".to_string()
            } else {
                "false".to_string()
            }
        } else {
            String::new()
        };
        CFRelease(value);
        result
    }
}

fn copy_action_names(elem: ax::AXUIElementRef) -> Vec<String> {
    let mut value: CFArrayRef = ptr::null();
    let err = unsafe { ax::AXUIElementCopyActionNames(elem, &mut value) };
    if err != ax::kAXErrorSuccess || value.is_null() {
        return Vec::new();
    }
    unsafe {
        let array: CFArray<*const c_void> = CFArray::wrap_under_create_rule(value);
        let mut out = Vec::with_capacity(array.len() as usize);
        for item in array.iter() {
            let s = cfstring_to_string(*item as CFStringRef);
            if !s.is_empty() {
                out.push(s);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// CFString → String
// ---------------------------------------------------------------------------
unsafe fn cfstring_to_string(cfstr: CFStringRef) -> String {
    if cfstr.is_null() {
        return String::new();
    }
    let s: CFString = CFString::wrap_under_get_rule(cfstr);
    s.to_string()
}

// ---------------------------------------------------------------------------
// Role / name / value / bounds / states / actions — per-field helpers
// ---------------------------------------------------------------------------

/// Normalise an AX role token to the same shape the Windows provider
/// uses. Matches on `kAXRoleAttribute` values; subrole takes a second
/// pass for things AX collapses into AXButton (e.g. `AXCloseButton`).
fn get_role(elem: ax::AXUIElementRef) -> String {
    let ax_role = copy_string_attr(elem, ax::kAXRoleAttribute);
    let subrole = copy_string_attr(elem, ax::kAXSubroleAttribute);
    normalize_role(&ax_role, &subrole)
}

/// Pure role-normalisation logic, extracted so it's unit-testable
/// without needing a live AXUIElementRef.
fn normalize_role(ax_role: &str, _subrole: &str) -> String {
    if ax_role.is_empty() {
        return "unknown".to_string();
    }
    match ax_role {
        "AXButton" => "button".to_string(),
        "AXCheckBox" => "checkbox".to_string(),
        "AXRadioButton" => "radiobutton".to_string(),
        "AXRadioGroup" => "radiogroup".to_string(),
        "AXSlider" => "slider".to_string(),
        "AXComboBox" => "combobox".to_string(),
        "AXPopUpButton" => "combobox".to_string(),
        "AXMenu" => "menu".to_string(),
        "AXMenuBar" => "menubar".to_string(),
        "AXMenuItem" => "menuitem".to_string(),
        "AXMenuBarItem" => "menuitem".to_string(),
        "AXTextField" | "AXTextArea" => "edit".to_string(),
        "AXStaticText" => "text".to_string(),
        "AXList" => "list".to_string(),
        "AXOutline" => "tree".to_string(),
        "AXRow" => "treeitem".to_string(),
        "AXScrollArea" => "pane".to_string(),
        "AXScrollBar" => "scrollbar".to_string(),
        "AXWindow" => "window".to_string(),
        "AXGroup" | "AXSplitGroup" => "group".to_string(),
        "AXTabGroup" => "tab".to_string(),
        "AXImage" => "image".to_string(),
        "AXToolbar" => "toolbar".to_string(),
        "AXProgressIndicator" => "progressbar".to_string(),
        "AXSplitter" => "separator".to_string(),
        "AXLink" => "link".to_string(),
        "AXTable" => "table".to_string(),
        "AXCell" => "dataitem".to_string(),
        "AXColumn" => "header".to_string(),
        "AXDisclosureTriangle" => "treeitem".to_string(),
        _ => ax_role
            .strip_prefix("AX")
            .unwrap_or(ax_role)
            .to_lowercase(),
    }
}

/// Best-effort display label. AX exposes title, description, and a
/// `LabelValue` attribute; chain through them.
fn safe_name(elem: ax::AXUIElementRef) -> String {
    let t = copy_string_attr(elem, ax::kAXTitleAttribute);
    if !t.is_empty() {
        return t;
    }
    let d = copy_string_attr(elem, ax::kAXDescriptionAttribute);
    if !d.is_empty() {
        return d;
    }
    copy_string_attr(elem, ax::kAXLabelValueAttribute)
}

fn safe_automation_id(elem: ax::AXUIElementRef) -> String {
    copy_string_attr(elem, ax::kAXIdentifierAttribute)
}

fn safe_pid(elem: ax::AXUIElementRef) -> u32 {
    let mut pid: ax::pid_t = 0;
    let err = unsafe { ax::AXUIElementGetPid(elem, &mut pid) };
    if err == ax::kAXErrorSuccess {
        pid as u32
    } else {
        0
    }
}

fn get_value(elem: ax::AXUIElementRef) -> String {
    copy_value_as_string(elem)
}

fn get_bounds(elem: ax::AXUIElementRef) -> Option<(i32, i32, i32, i32)> {
    let pos = copy_axvalue_point(elem)?;
    let size = copy_axvalue_size(elem)?;
    if size.width > 0.0 && size.height > 0.0 {
        Some((
            pos.x as i32,
            pos.y as i32,
            size.width as i32,
            size.height as i32,
        ))
    } else {
        None
    }
}

fn get_actions(elem: ax::AXUIElementRef) -> Vec<String> {
    let ax_actions = copy_action_names(elem);
    let mut out = Vec::new();
    for a in &ax_actions {
        match a.as_str() {
            "AXPress" => {
                out.push("invoke".to_string());
                // AXPress on a checkbox-like is also "toggle" from the
                // LLM's point of view; add a more specific tag below
                // by inspecting the role/attributes.
            }
            "AXShowMenu" => out.push("expand_collapse".to_string()),
            "AXPick" => out.push("select".to_string()),
            "AXIncrement" | "AXDecrement" => out.push("scroll".to_string()),
            _ => {}
        }
    }
    // set_value if value attribute is writable
    let cf_attr = CFString::new(ax::kAXValueAttribute);
    let mut settable: core_foundation::base::Boolean = 0;
    let err = unsafe {
        ax::AXUIElementIsAttributeSettable(elem, cf_attr.as_concrete_TypeRef(), &mut settable)
    };
    if err == ax::kAXErrorSuccess && settable != 0 {
        out.push("set_value".to_string());
    }
    // get_text if element has a string value
    if !copy_value_as_string(elem).is_empty() {
        out.push("get_text".to_string());
    }
    // toggle if checkbox/menuitem with AXPress (already pushed invoke)
    let role = copy_string_attr(elem, ax::kAXRoleAttribute);
    if role == "AXCheckBox" && ax_actions.iter().any(|a| a == "AXPress") {
        out.push("toggle".to_string());
    }
    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.clone()));
    out
}

fn get_states(elem: ax::AXUIElementRef) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(false) = copy_bool_attr(elem, ax::kAXEnabledAttribute) {
        out.push("disabled".to_string());
    }
    let role = copy_string_attr(elem, ax::kAXRoleAttribute);
    if role == "AXCheckBox" {
        let v = copy_value_as_string(elem);
        match v.as_str() {
            "1" | "true" => out.push("checked".to_string()),
            "0" | "false" => out.push("unchecked".to_string()),
            _ => {}
        }
    }
    if let Some(b) = copy_bool_attr(elem, ax::kAXDisclosingAttribute) {
        out.push(if b { "expanded" } else { "collapsed" }.to_string());
    } else if let Some(b) = copy_bool_attr(elem, ax::kAXExpandedAttribute) {
        out.push(if b { "expanded" } else { "collapsed" }.to_string());
    }
    out
}

fn get_process_name(pid: u32) -> String {
    crate::os::process::get_process_name(pid).unwrap_or_default()
}

// ---------------------------------------------------------------------------
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
fn ensure_trusted() -> Result<(), String> {
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
    if !include_invisible {
        if let Some((_, _, w, h)) = get_bounds(elem) {
            if w <= 0 || h <= 0 {
                return None;
            }
        }
    }

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

// ---------------------------------------------------------------------------
// Window discovery — application enumeration via NSWorkspace
// ---------------------------------------------------------------------------

/// Enumerate AXWindowRefs belonging to running apps, optionally filtering
/// by window title substring (case-insensitive).
fn enumerate_windows(title_filter: Option<&str>) -> Vec<(AxElem, String, u32, String)> {
    use objc2::rc::Retained;
    use objc2_app_kit::NSWorkspace;

    let mut out = Vec::new();
    let workspace = NSWorkspace::sharedWorkspace();
    let apps: Retained<objc2_foundation::NSArray<objc2_app_kit::NSRunningApplication>> =
        workspace.runningApplications();

    for app in &apps {
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
            if let Some(filter) = title_filter {
                if !title.to_lowercase().contains(&filter.to_lowercase()) {
                    continue;
                }
            }
            out.push((win, title, pid, process_name.clone()));
        }
    }
    out
}

fn find_window(title: Option<&str>) -> Result<AxElem, String> {
    if let Some(t) = title {
        let wins = enumerate_windows(Some(t));
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

pub(super) fn get_ui_tree_inner(
    window_title: Option<&str>,
    max_depth: usize,
    include_invisible: bool,
) -> Result<UIElement, String> {
    ensure_trusted()?;
    clear_native();
    let win = find_window(window_title)?;
    let mut st = WalkState::new(TREE_WALK_TIMEOUT_SECS);
    let mut elem = build_element(
        win.as_ref(),
        0,
        max_depth,
        include_invisible,
        &mut st,
    )
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

pub(super) fn list_accessible_windows_inner(
    title_filter: Option<&str>,
) -> Result<Vec<AccessibleWindowInfo>, String> {
    ensure_trusted()?;
    let wins = enumerate_windows(title_filter);
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

pub(super) fn get_focused_element_inner() -> Result<Option<UIElement>, String> {
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
    let mut ui = UIElement::new(eid, get_role(focused.as_ref()));
    ui.name = safe_name(focused.as_ref());
    ui.value = get_value(focused.as_ref());
    ui.automation_id = safe_automation_id(focused.as_ref());
    ui.states = get_states(focused.as_ref());
    ui.actions = get_actions(focused.as_ref());
    ui.bounds = get_bounds(focused.as_ref());
    Ok(Some(ui))
}

pub(super) fn get_element_children_inner(
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
        let mut ui = UIElement::new(eid, get_role(elem));
        ui.name = safe_name(elem);
        ui.value = get_value(elem);
        ui.automation_id = safe_automation_id(elem);
        ui.states = get_states(elem);
        ui.actions = get_actions(elem);
        ui.bounds = get_bounds(elem);
        results.push(ui);
    }

    let children = copy_elements_attr(elem, ax::kAXChildrenAttribute);
    for child in children {
        if st.exhausted() {
            break;
        }
        search_recursive(child.as_ref(), params, results, depth + 1, max_depth, st);
    }
}

pub(super) fn find_elements_inner(p: &FindElementsParams) -> Result<Vec<UIElement>, String> {
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

pub(super) fn get_element_text_inner(element_id: &str) -> Result<String, String> {
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

// ---------------------------------------------------------------------------
// F5c — write ops: click, focus, set_value, toggle, select, expand,
// collapse, scroll
// ---------------------------------------------------------------------------

/// Perform a named AX action on an element. Returns a human-readable
/// message on success; maps common AXError codes to actionable errors.
fn perform_action(elem: ax::AXUIElementRef, action: &str) -> Result<String, String> {
    let cf_action = CFString::new(action);
    let err = unsafe { ax::AXUIElementPerformAction(elem, cf_action.as_concrete_TypeRef()) };
    match err {
        ax::kAXErrorSuccess => Ok(format!("Performed {}", action)),
        ax::kAXErrorActionUnsupported => {
            Err(format!("Element does not support action '{}'", action))
        }
        ax::kAXErrorCannotComplete => Err(format!(
            "Action '{}' could not complete (element may be hidden or busy)",
            action
        )),
        ax::kAXErrorInvalidUIElement => {
            Err("Element is no longer valid (may have been destroyed)".into())
        }
        ax::kAXErrorAPIDisabled => Err("Accessibility permission not granted".into()),
        _ => Err(format!("AX error {} performing '{}'", err, action)),
    }
}

/// Set an attribute value on an element. Generic over the CFType — the
/// caller builds the CFString/CFBoolean/CFNumber wrapper. Returns the
/// same human-readable message shape as `perform_action`.
fn set_attribute(
    elem: ax::AXUIElementRef,
    attr: &str,
    value: CFTypeRef,
) -> Result<String, String> {
    let cf_attr = CFString::new(attr);
    let err = unsafe { ax::AXUIElementSetAttributeValue(elem, cf_attr.as_concrete_TypeRef(), value) };
    match err {
        ax::kAXErrorSuccess => Ok(format!("Set {}", attr)),
        ax::kAXErrorAttributeUnsupported => {
            Err(format!("Element does not expose '{}'", attr))
        }
        ax::kAXErrorIllegalArgument => {
            Err(format!("Illegal argument for '{}'", attr))
        }
        ax::kAXErrorCannotComplete => Err(format!(
            "Setting '{}' could not complete (element may be read-only or busy)",
            attr
        )),
        ax::kAXErrorInvalidUIElement => {
            Err("Element is no longer valid (may have been destroyed)".into())
        }
        ax::kAXErrorAPIDisabled => Err("Accessibility permission not granted".into()),
        _ => Err(format!("AX error {} setting '{}'", err, attr)),
    }
}

pub(super) fn click_element_inner(element_id: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    perform_action(elem.as_ref(), ax::kAXPressAction)
}

pub(super) fn focus_element_inner(element_id: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    let value = CFBoolean::true_value();
    set_attribute(
        elem.as_ref(),
        ax::kAXFocusedAttribute,
        value.as_concrete_TypeRef() as CFTypeRef,
    )
}

pub(super) fn set_element_value_inner(element_id: &str, val: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    let cf_val = CFString::new(val);
    set_attribute(
        elem.as_ref(),
        ax::kAXValueAttribute,
        cf_val.as_concrete_TypeRef() as CFTypeRef,
    )
}

pub(super) fn toggle_element_inner(element_id: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    // AX checkboxes expose `AXPress` — a press toggles. For non-checkbox
    // widgets that expose a writable boolean value, flip the value.
    let role = copy_string_attr(elem.as_ref(), ax::kAXRoleAttribute);
    if role == "AXCheckBox" {
        return perform_action(elem.as_ref(), ax::kAXPressAction);
    }
    // Read current boolean value and flip it.
    let current = copy_bool_attr(elem.as_ref(), ax::kAXValueAttribute).ok_or_else(|| {
        format!(
            "Element '{}' (role={}) has no toggleable value — \
             AXPress unsupported and kAXValueAttribute is not a boolean",
            element_id, role
        )
    })?;
    let next = if current {
        CFBoolean::false_value()
    } else {
        CFBoolean::true_value()
    };
    set_attribute(
        elem.as_ref(),
        ax::kAXValueAttribute,
        next.as_concrete_TypeRef() as CFTypeRef,
    )
}

pub(super) fn select_element_inner(element_id: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    let role = copy_string_attr(elem.as_ref(), ax::kAXRoleAttribute);
    // For menu items, table rows, and outline rows, AXPress performs the
    // selection. For popup/combobox items, flip kAXSelectedAttribute.
    if matches!(role.as_str(), "AXMenuItem" | "AXMenuBarItem" | "AXRow") {
        return perform_action(elem.as_ref(), ax::kAXPressAction);
    }
    let value = CFBoolean::true_value();
    set_attribute(
        elem.as_ref(),
        ax::kAXSelectedAttribute,
        value.as_concrete_TypeRef() as CFTypeRef,
    )
}

pub(super) fn expand_element_inner(element_id: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    let value = CFBoolean::true_value();
    // kAXDisclosingAttribute for outlines/rows, kAXExpandedAttribute for
    // disclosure triangles. Try Disclosing first, fall back to Expanded.
    match set_attribute(
        elem.as_ref(),
        ax::kAXDisclosingAttribute,
        value.as_concrete_TypeRef() as CFTypeRef,
    ) {
        Ok(m) => Ok(m),
        Err(_) => set_attribute(
            elem.as_ref(),
            ax::kAXExpandedAttribute,
            value.as_concrete_TypeRef() as CFTypeRef,
        ),
    }
}

pub(super) fn collapse_element_inner(element_id: &str) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    let value = CFBoolean::false_value();
    match set_attribute(
        elem.as_ref(),
        ax::kAXDisclosingAttribute,
        value.as_concrete_TypeRef() as CFTypeRef,
    ) {
        Ok(m) => Ok(m),
        Err(_) => set_attribute(
            elem.as_ref(),
            ax::kAXExpandedAttribute,
            value.as_concrete_TypeRef() as CFTypeRef,
        ),
    }
}

pub(super) fn scroll_element_inner(
    element_id: &str,
    direction: &str,
    _amount: f64,
) -> Result<String, String> {
    ensure_trusted()?;
    let elem = resolve_native(element_id)?;
    // `into_view` / empty direction → scroll-to-visible. Directional
    // scrolls (up/down/left/right) would need synthesised CGEvent scrolls
    // or AXIncrement/AXDecrement actions — the Windows impl supports both
    // shapes but directional scroll on macOS is tracked as a follow-up
    // (see .agent/F5_ACCESSIBILITY_DESIGN.md).
    match direction {
        "" | "into_view" | "visible" => {
            // AX has no public constant for the scroll-to-visible action
            // name via accessibility-sys 0.2; use the literal.
            perform_action(elem.as_ref(), "AXScrollToVisible")
        }
        "up" | "down" | "left" | "right" => {
            let action = if direction == "up" || direction == "left" {
                ax::kAXDecrementAction
            } else {
                ax::kAXIncrementAction
            };
            perform_action(elem.as_ref(), action)
        }
        other => Err(format!(
            "Unknown scroll direction '{}' — use up/down/left/right/into_view",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
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
        |reply| Job::ClickElement { element_id: id, reply },
        || Err("AX worker not running".into()),
    )
}

pub fn focus_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::FocusElement { element_id: id, reply },
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
        |reply| Job::ToggleElement { element_id: id, reply },
        || Err("AX worker not running".into()),
    )
}

pub fn select_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::SelectElement { element_id: id, reply },
        || Err("AX worker not running".into()),
    )
}

pub fn expand_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::ExpandElement { element_id: id, reply },
        || Err("AX worker not running".into()),
    )
}

pub fn collapse_element_impl(id: &str) -> Result<String, String> {
    let id = id.to_string();
    ax_worker::submit(
        |reply| Job::CollapseElement { element_id: id, reply },
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
        |reply| Job::GetElementText { element_id: id, reply },
        || Err("AX worker not running".into()),
    )
}


#[cfg(test)]
mod tests {
    use super::normalize_role;

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
