//! Worker-thread-local mapping between public element IDs and UIA handles.

use std::cell::RefCell;
use std::collections::HashMap;

use uiautomation::core::UIElement as UiaElement;

use crate::computer_control::tree;

thread_local! {
    static NATIVE_REGISTRY: RefCell<HashMap<String, UiaElement>> = RefCell::new(HashMap::new());
}

pub(super) fn register(elem: &UiaElement) -> String {
    let handle = elem
        .get_runtime_id()
        .unwrap_or_default()
        .iter()
        .fold(0u64, |acc, &value| {
            acc.wrapping_mul(31).wrapping_add(value as u64)
        });
    let id = tree::register_element(handle);
    NATIVE_REGISTRY.with(|registry| registry.borrow_mut().insert(id.clone(), elem.clone()));
    id
}

pub(super) fn resolve(element_id: &str) -> Result<UiaElement, String> {
    NATIVE_REGISTRY.with(|registry| {
        registry.borrow().get(element_id).cloned().ok_or_else(|| {
            format!(
                "Element '{}' not found. IDs are ephemeral — call get_ui_tree() to get fresh IDs.",
                element_id
            )
        })
    })
}

pub(super) fn clear() {
    NATIVE_REGISTRY.with(|registry| registry.borrow_mut().clear());
    tree::clear_registry();
}
