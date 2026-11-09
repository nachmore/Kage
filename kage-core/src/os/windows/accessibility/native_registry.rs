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

/// [`resolve`] for a tool that acts on the element (clicks, types,
/// toggles …): refused when the element belongs to Kage itself, whose
/// approval buttons only the user may press (`os::kage_target`). WebView2
/// content reports msedgewebview2.exe as its process, so for such an
/// element the process that started that WebView2 decides (UI Automation
/// can invoke a button in a window another one covers), and failing that
/// the window under its centre (and that window's root).
pub(super) fn resolve_for_input(element_id: &str) -> Result<UiaElement, String> {
    use crate::os::platform::kage_target;
    let elem = resolve(element_id)?;
    let pid = super::element::process_id(&elem);
    let mut kage = crate::os::kage_target::is_kage_process(pid);
    if !kage
        && crate::os::process::get_process_name(pid).as_deref()
            == Some(kage_target::WEBVIEW2_PROCESS)
    {
        kage = kage_target::is_kage_webview_impl(pid);
        if !kage {
            if let Some((x, y, w, h)) = super::element::bounds(&elem) {
                kage = crate::os::kage_target::point_is_on_kage(x + w / 2, y + h / 2);
            }
        }
    }
    if kage {
        log::warn!("Refused an input action on Kage's own window ({element_id})");
        return Err(crate::os::kage_target::REFUSAL.to_string());
    }
    Ok(elem)
}

pub(super) fn clear() {
    NATIVE_REGISTRY.with(|registry| registry.borrow_mut().clear());
    tree::clear_registry();
}
