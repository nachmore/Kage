//! Mutating AX operations for registered accessibility elements.

use accessibility_sys as ax;
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;

use super::core::*;
use super::tree::ensure_trusted;
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
fn set_attribute(elem: ax::AXUIElementRef, attr: &str, value: CFTypeRef) -> Result<String, String> {
    let cf_attr = CFString::new(attr);
    let err =
        unsafe { ax::AXUIElementSetAttributeValue(elem, cf_attr.as_concrete_TypeRef(), value) };
    match err {
        ax::kAXErrorSuccess => Ok(format!("Set {}", attr)),
        ax::kAXErrorAttributeUnsupported => Err(format!("Element does not expose '{}'", attr)),
        ax::kAXErrorIllegalArgument => Err(format!("Illegal argument for '{}'", attr)),
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
