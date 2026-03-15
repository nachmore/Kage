//! UI element data structures, ID management, and tree serialization.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use once_cell::sync::Lazy;

// ---------------------------------------------------------------------------
// Element ID registry
// ---------------------------------------------------------------------------

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static REGISTRY: Lazy<Mutex<HashMap<String, u64>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Register a native element handle (stored as an opaque u64) and return an ephemeral ID.
pub fn register_element(native_handle: u64) -> String {
    let id = format!("e{}", ID_COUNTER.fetch_add(1, Ordering::Relaxed));
    REGISTRY.lock().unwrap().insert(id.clone(), native_handle);
    id
}

/// Resolve an ephemeral ID back to its native handle.
pub fn resolve_element(eid: &str) -> Result<u64, String> {
    REGISTRY
        .lock()
        .unwrap()
        .get(eid)
        .copied()
        .ok_or_else(|| {
            format!(
                "Element '{}' not found. IDs are ephemeral — \
                 call get_ui_tree() or find_elements() to get fresh IDs.",
                eid
            )
        })
}

/// Clear all registered IDs. Call before building a new tree snapshot.
pub fn clear_registry() {
    REGISTRY.lock().unwrap().clear();
}

// ---------------------------------------------------------------------------
// Noise roles — structural clutter with no useful info
// ---------------------------------------------------------------------------

const NOISE_ROLES: &[&str] = &[
    "separator", "thumb", "scrollbar", "image", "pane", "group", "header",
];

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
        1 + self.children.iter().map(|c| c.count_elements()).sum::<usize>()
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
            let n = if self.name.len() <= 80 {
                &self.name
            } else {
                &self.name[..77]
            };
            parts.push(format!("\"{}\"", n));
        }

        parts.push(format!("{{{}}}", self.id));

        if !self.value.is_empty() {
            let v = if self.value.len() <= 80 {
                &self.value
            } else {
                &self.value[..77]
            };
            parts.push(format!("value=\"{}\"", v));
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
