//! UI element data structures, ID management, and tree serialization.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::lock_ext::LockExt;

// ---------------------------------------------------------------------------
// Element ID registry
// ---------------------------------------------------------------------------

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static REGISTRY: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Register a native element handle (stored as an opaque u64) and return an ephemeral ID.
pub fn register_element(native_handle: u64) -> String {
    let id = format!("e{}", ID_COUNTER.fetch_add(1, Ordering::Relaxed));
    REGISTRY.lock_or_recover().insert(id.clone(), native_handle);
    id
}

/// Resolve an ephemeral ID back to its native handle.
pub fn resolve_element(eid: &str) -> Result<u64, String> {
    REGISTRY.lock_or_recover().get(eid).copied().ok_or_else(|| {
        format!(
            "Element '{}' not found. IDs are ephemeral — \
                 call get_ui_tree() or find_elements() to get fresh IDs.",
            eid
        )
    })
}

/// Clear all registered IDs. Call before building a new tree snapshot.
pub fn clear_registry() {
    REGISTRY.lock_or_recover().clear();
}

// ---------------------------------------------------------------------------
// Noise roles — structural clutter with no useful info
// ---------------------------------------------------------------------------

const NOISE_ROLES: &[&str] = &[
    "separator",
    "thumb",
    "scrollbar",
    "image",
    "pane",
    "group",
    "header",
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate `s` so its byte length is `<= max_bytes`, splitting only at a UTF-8
/// codepoint boundary. Strings ≤ `max_bytes` long are returned untouched; longer
/// ones are sliced at the largest char boundary `<= keep_bytes`. Used for the
/// element name/value preview where naive byte slicing can panic on emoji/CJK
/// at the truncation offset.
fn truncate_at_char_boundary(s: &str, max_bytes: usize, keep_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = keep_bytes.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ---------------------------------------------------------------------------
// UIElement
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct UIElement {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub automation_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<(i32, i32, i32, i32)>, // (x, y, w, h)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<UIElement>,
    /// Metadata injected by the provider (e.g. truncation warnings)
    #[serde(skip)]
    pub meta: String,
}

impl UIElement {
    /// Create a new UIElement with required fields.
    pub fn new(id: String, role: String) -> Self {
        Self {
            id,
            role,
            name: String::new(),
            value: String::new(),
            automation_id: String::new(),
            states: Vec::new(),
            actions: Vec::new(),
            bounds: None,
            children: Vec::new(),
            meta: String::new(),
        }
    }

    /// Check if this element is structural noise with no useful info.
    pub fn is_noise(&self) -> bool {
        if !NOISE_ROLES.contains(&self.role.as_str()) {
            return false;
        }
        if !self.name.trim().is_empty() || !self.value.trim().is_empty() || !self.actions.is_empty()
        {
            return false;
        }
        true
    }

    /// Count total elements in this subtree.
    pub fn count_elements(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(|c| c.count_elements())
            .sum::<usize>()
    }

    /// Serialize to compact text tree format for LLM consumption.
    ///
    /// Noise elements (nameless separators, scrollbars, etc.) that have
    /// children are replaced by their children (flattened). Noise leaves
    /// are omitted entirely.
    pub fn to_text(&self, indent: usize, max_depth: usize) -> String {
        // Noise leaf → skip
        if self.is_noise() && self.children.is_empty() {
            return String::new();
        }

        // Noise container → flatten children
        if self.is_noise() && !self.children.is_empty() {
            let mut lines = Vec::new();
            for child in &self.children {
                let text = child.to_text(indent, max_depth);
                if !text.is_empty() {
                    lines.push(text);
                }
            }
            return lines.join("\n");
        }

        let pad = "  ".repeat(indent);
        let mut parts = vec![format!("{}[{}]", pad, self.role)];

        if !self.name.is_empty() {
            parts.push(format!(
                "\"{}\"",
                truncate_at_char_boundary(&self.name, 80, 77)
            ));
        }

        parts.push(format!("{{{}}}", self.id));

        if !self.value.is_empty() {
            parts.push(format!(
                "value=\"{}\"",
                truncate_at_char_boundary(&self.value, 80, 77)
            ));
        }

        if !self.states.is_empty() {
            parts.push(format!("state=[{}]", self.states.join(",")));
        }

        if !self.actions.is_empty() {
            parts.push(format!("actions=[{}]", self.actions.join(",")));
        }

        if let Some((x, y, w, h)) = self.bounds {
            parts.push(format!("({}x{}@{},{})", w, h, x, y));
        }

        let mut lines = vec![parts.join(" ")];

        if indent < max_depth {
            for child in &self.children {
                let text = child.to_text(indent + 1, max_depth);
                if !text.is_empty() {
                    lines.push(text);
                }
            }
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem_with_name(name: &str) -> UIElement {
        let mut e = UIElement::new("e1".into(), "button".into());
        e.name = name.into();
        e
    }

    fn elem_with_value(value: &str) -> UIElement {
        let mut e = UIElement::new("e1".into(), "edit".into());
        e.value = value.into();
        e
    }

    #[test]
    fn to_text_handles_short_ascii_name() {
        let e = elem_with_name("Save");
        let text = e.to_text(0, 5);
        assert!(text.contains("\"Save\""));
    }

    #[test]
    fn to_text_truncates_long_name_at_utf8_boundary() {
        // Place a 4-byte emoji straddling the 77-byte truncation point.
        // 75 ASCII + 🦀(4 bytes) → byte 75 starts the emoji; byte 77 is mid-codepoint.
        // Naive `&s[..77]` panics. Truncation must back up to byte 75.
        let mut name = "a".repeat(75);
        name.push_str("🦀🦀");
        let e = elem_with_name(&name);
        let text = e.to_text(0, 5);
        assert!(text.contains("\"aaa")); // contains the prefix
        assert!(text.is_ascii() || text.chars().all(|c| c.len_utf8() <= 4));
        // No partial emoji bytes:
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }

    #[test]
    fn to_text_truncates_long_value_at_utf8_boundary() {
        // Mixed CJK/emoji that places multibyte codepoints near boundary.
        let mut value = "x".repeat(76);
        value.push_str("中文测试🎉");
        let e = elem_with_value(&value);
        let text = e.to_text(0, 5);
        assert!(text.contains("value=\"xx"));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }

    #[test]
    fn truncate_at_char_boundary_does_not_panic_on_multibyte_at_offset() {
        // Direct test of the helper for clarity.
        let s = "a".repeat(78) + "🎉"; // emoji's 4 bytes straddle 78..82, so byte 77 is ASCII
        let out = truncate_at_char_boundary(&s, 80, 77);
        assert_eq!(out.len(), 77);

        // Now place the emoji starting at 75 → bytes 75..79 are the emoji.
        // Byte 77 is mid-emoji. Truncation must back up to byte 75.
        let s = "a".repeat(75) + "🎉🎉";
        let out = truncate_at_char_boundary(&s, 80, 77);
        assert_eq!(out.len(), 75);
        assert!(out.chars().all(|c| c == 'a'));
    }

    #[test]
    fn truncate_short_string_returned_unchanged() {
        let s = "hello";
        assert_eq!(truncate_at_char_boundary(s, 80, 77), "hello");
    }

    #[test]
    fn is_noise_keeps_named_separator() {
        let mut e = UIElement::new("e1".into(), "separator".into());
        e.name = "Visible label".into();
        assert!(!e.is_noise(), "named separator should not be noise");
    }

    #[test]
    fn is_noise_drops_unnamed_separator() {
        let e = UIElement::new("e1".into(), "separator".into());
        assert!(e.is_noise());
    }

    #[test]
    fn register_resolve_clear_lifecycle() {
        clear_registry();
        let id_a = register_element(0xAAAA);
        let id_b = register_element(0xBBBB);
        assert_ne!(id_a, id_b);
        assert_eq!(resolve_element(&id_a).unwrap(), 0xAAAA);
        assert_eq!(resolve_element(&id_b).unwrap(), 0xBBBB);

        clear_registry();
        assert!(resolve_element(&id_a).is_err());
    }

    #[test]
    fn to_text_flattens_noise_container_preserving_children() {
        // group with no name → noise container; its named children should
        // be promoted to the parent indent rather than nested under it.
        let mut group = UIElement::new("g".into(), "group".into());
        let mut btn1 = UIElement::new("b1".into(), "button".into());
        btn1.name = "OK".into();
        let mut btn2 = UIElement::new("b2".into(), "button".into());
        btn2.name = "Cancel".into();
        group.children = vec![btn1, btn2];

        let text = group.to_text(0, 5);
        // No "[group]" line at indent 0 — group was flattened.
        assert!(!text.contains("[group]"));
        assert!(text.contains("\"OK\""));
        assert!(text.contains("\"Cancel\""));
    }
}
