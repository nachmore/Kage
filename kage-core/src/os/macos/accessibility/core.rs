//! Retained AX references, registry ownership, and AX attribute readers.

#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

use accessibility_sys as ax;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGSize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;

use crate::computer_control::tree;
// Constants — match the Windows provider's tuning
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Retained AXUIElementRef wrapper — CFRetain in `new`, CFRelease in `Drop`
// ---------------------------------------------------------------------------
/// Owned `AXUIElementRef` — adds one retain, drops one release.
pub(super) struct AxElem(ax::AXUIElementRef);

impl AxElem {
    /// Retain and wrap. Caller must have a valid, non-null ref.
    pub(super) fn from_ref(r: ax::AXUIElementRef) -> Self {
        unsafe {
            CFRetain(r as CFTypeRef);
        }
        Self(r)
    }

    /// Take ownership of a ref from a `Copy*` function (which already did
    /// the retain on our behalf). Use this only for AX functions documented
    /// as returning an owned reference.
    pub(super) fn take(r: ax::AXUIElementRef) -> Self {
        Self(r)
    }

    pub(super) fn as_ref(&self) -> ax::AXUIElementRef {
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

pub(super) fn register_native(elem: ax::AXUIElementRef) -> String {
    // Opaque u64 identity: pointer value itself is fine for disambiguation.
    let handle = elem as usize as u64;
    let id = tree::register_element(handle);
    NATIVE_REGISTRY.with(|r| {
        r.borrow_mut().insert(id.clone(), AxElem::from_ref(elem));
    });
    id
}

pub(super) fn resolve_native(eid: &str) -> Result<AxElem, String> {
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

pub(super) fn clear_native() {
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
pub(super) fn copy_string_attr(elem: ax::AXUIElementRef, attr: &str) -> String {
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
pub(super) fn copy_bool_attr(elem: ax::AXUIElementRef, attr: &str) -> Option<bool> {
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
pub(super) fn copy_element_attr(elem: ax::AXUIElementRef, attr: &str) -> Option<AxElem> {
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
pub(super) fn copy_elements_attr(elem: ax::AXUIElementRef, attr: &str) -> Vec<AxElem> {
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
pub(super) fn copy_axvalue_point(elem: ax::AXUIElementRef) -> Option<CGPoint> {
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

pub(super) fn copy_axvalue_size(elem: ax::AXUIElementRef) -> Option<CGSize> {
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
pub(super) fn copy_value_as_string(elem: ax::AXUIElementRef) -> String {
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

pub(super) fn copy_action_names(elem: ax::AXUIElementRef) -> Vec<String> {
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
pub(super) fn get_role(elem: ax::AXUIElementRef) -> String {
    let ax_role = copy_string_attr(elem, ax::kAXRoleAttribute);
    let subrole = copy_string_attr(elem, ax::kAXSubroleAttribute);
    normalize_role(&ax_role, &subrole)
}

/// Pure role-normalisation logic, extracted so it's unit-testable
/// without needing a live AXUIElementRef.
pub(super) fn normalize_role(ax_role: &str, _subrole: &str) -> String {
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
        _ => ax_role.strip_prefix("AX").unwrap_or(ax_role).to_lowercase(),
    }
}

/// Best-effort display label. AX exposes title, description, and a
/// `LabelValue` attribute; chain through them.
pub(super) fn safe_name(elem: ax::AXUIElementRef) -> String {
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

pub(super) fn safe_automation_id(elem: ax::AXUIElementRef) -> String {
    copy_string_attr(elem, ax::kAXIdentifierAttribute)
}

pub(super) fn safe_pid(elem: ax::AXUIElementRef) -> u32 {
    let mut pid: ax::pid_t = 0;
    let err = unsafe { ax::AXUIElementGetPid(elem, &mut pid) };
    if err == ax::kAXErrorSuccess {
        pid as u32
    } else {
        0
    }
}

pub(super) fn get_value(elem: ax::AXUIElementRef) -> String {
    copy_value_as_string(elem)
}

pub(super) fn get_bounds(elem: ax::AXUIElementRef) -> Option<(i32, i32, i32, i32)> {
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

pub(super) fn get_actions(elem: ax::AXUIElementRef) -> Vec<String> {
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

pub(super) fn get_states(elem: ax::AXUIElementRef) -> Vec<String> {
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

pub(super) fn get_process_name(pid: u32) -> String {
    crate::os::process::get_process_name(pid).unwrap_or_default()
}
