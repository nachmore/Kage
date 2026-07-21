//! UIA actions resolved from the worker thread's native handle registry.

use uiautomation::patterns::*;
use uiautomation::types::{ExpandCollapseState, ScrollAmount};

use super::element;
use super::native_registry;

pub(crate) fn click_element_inner(element_id: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if let Ok(pattern) = elem.get_pattern::<UIInvokePattern>() {
        if pattern.invoke().is_ok() {
            return Ok(format!("Invoked [{}] '{}'", role, name));
        }
    }
    if let Ok(pattern) = elem.get_pattern::<UITogglePattern>() {
        if pattern.toggle().is_ok() {
            let state = pattern
                .get_toggle_state()
                .map(|state| format!("{:?}", state))
                .unwrap_or_default();
            return Ok(format!("Toggled [{}] '{}' → {}", role, name, state));
        }
    }
    if let Ok(pattern) = elem.get_pattern::<UISelectionItemPattern>() {
        if pattern.select().is_ok() {
            return Ok(format!("Selected [{}] '{}'", role, name));
        }
    }
    if let Ok(pattern) = elem.get_pattern::<UIExpandCollapsePattern>() {
        if let Ok(state) = pattern.get_state() {
            match state {
                ExpandCollapseState::Collapsed => {
                    if pattern.expand().is_ok() {
                        return Ok(format!("Expanded [{}] '{}'", role, name));
                    }
                }
                _ => {
                    if pattern.collapse().is_ok() {
                        return Ok(format!("Collapsed [{}] '{}'", role, name));
                    }
                }
            }
        }
    }
    if elem.click().is_ok() {
        return Ok(format!(
            "Clicked [{}] '{}' (coordinate fallback)",
            role, name
        ));
    }
    Err(format!("Failed to click [{}] '{}'", role, name))
}

pub(crate) fn focus_element_inner(element_id: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if elem.set_focus().is_ok() {
        Ok(format!("Focused [{}] '{}'", role, name))
    } else {
        Err(format!("Failed to focus [{}] '{}'", role, name))
    }
}

pub(crate) fn set_element_value_inner(element_id: &str, value: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if let Ok(pattern) = elem.get_pattern::<UIValuePattern>() {
        if pattern.set_value(value).is_ok() {
            return Ok(format!("Set value on [{}] '{}'", role, name));
        }
    }
    if elem.set_focus().is_ok() {
        let _ = elem.send_keys("{Ctrl}a", 20);
        let _ = elem.send_keys(value, 10);
        return Ok(format!(
            "Typed value into [{}] '{}' (keyboard fallback)",
            role, name
        ));
    }
    Err(format!("Failed to set value on [{}] '{}'", role, name))
}

pub(crate) fn toggle_element_inner(element_id: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if let Ok(pattern) = elem.get_pattern::<UITogglePattern>() {
        if pattern.toggle().is_ok() {
            let state = pattern
                .get_toggle_state()
                .map(|state| format!("{:?}", state))
                .unwrap_or_default();
            return Ok(format!("Toggled [{}] '{}' → {}", role, name, state));
        }
    }
    Err(format!("[{}] '{}' does not support toggle", role, name))
}

pub(crate) fn select_element_inner(element_id: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if let Ok(pattern) = elem.get_pattern::<UISelectionItemPattern>() {
        if pattern.select().is_ok() {
            return Ok(format!("Selected [{}] '{}'", role, name));
        }
    }
    if let Ok(pattern) = elem.get_pattern::<UIInvokePattern>() {
        if pattern.invoke().is_ok() {
            return Ok(format!("Invoked [{}] '{}' (select fallback)", role, name));
        }
    }
    Err(format!("[{}] '{}' does not support selection", role, name))
}

pub(crate) fn expand_element_inner(element_id: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if let Ok(pattern) = elem.get_pattern::<UIExpandCollapsePattern>() {
        if pattern.expand().is_ok() {
            return Ok(format!("Expanded [{}] '{}'", role, name));
        }
    }
    Err(format!("[{}] '{}' does not support expand", role, name))
}

pub(crate) fn collapse_element_inner(element_id: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if let Ok(pattern) = elem.get_pattern::<UIExpandCollapsePattern>() {
        if pattern.collapse().is_ok() {
            return Ok(format!("Collapsed [{}] '{}'", role, name));
        }
    }
    Err(format!("[{}] '{}' does not support collapse", role, name))
}

pub(crate) fn scroll_element_inner(
    element_id: &str,
    direction: &str,
    _amount: f64,
) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    let (role, name) = (element::role(&elem), element::name(&elem));
    if let Ok(pattern) = elem.get_pattern::<UIScrollPattern>() {
        let result = match direction {
            "up" => pattern.scroll(ScrollAmount::NoAmount, ScrollAmount::SmallDecrement),
            "down" => pattern.scroll(ScrollAmount::NoAmount, ScrollAmount::SmallIncrement),
            "left" => pattern.scroll(ScrollAmount::SmallDecrement, ScrollAmount::NoAmount),
            "right" => pattern.scroll(ScrollAmount::SmallIncrement, ScrollAmount::NoAmount),
            _ => return Err(format!("Invalid scroll direction: {}", direction)),
        };
        if result.is_ok() {
            return Ok(format!("Scrolled {} on [{}] '{}'", direction, role, name));
        }
    }
    Err(format!("[{}] '{}' does not support scrolling", role, name))
}

pub(crate) fn get_element_text_inner(element_id: &str) -> Result<String, String> {
    let elem = native_registry::resolve(element_id)?;
    if let Ok(pattern) = elem.get_pattern::<UITextPattern>() {
        if let Ok(range) = pattern.get_document_range() {
            if let Ok(text) = range.get_text(-1) {
                if !text.is_empty() {
                    return Ok(text);
                }
            }
        }
    }
    if let Ok(pattern) = elem.get_pattern::<UIValuePattern>() {
        if let Ok(value) = pattern.get_value() {
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    Ok(element::name(&elem))
}
