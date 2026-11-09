//! Per-OS and per-computer overrides (docs/design/SYNC.md §9): which cell
//! a value comes from on this computer, where an edit goes, and what a
//! scope action writes.
//!
//! C0 skeleton: every function answers "nothing". WP-S1 writes the bodies.

use super::types::{DefaultScope, FieldKey, OsKind, Record, Scope, ScopeAction, Stamp};

/// The value of one field on this computer, and the cell it came from.
/// `from` and `stamp` are `None` when the value is a pin or local.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Effective<'a> {
    pub value: Option<&'a serde_json::Value>,
    pub from: Option<Scope>,
    pub stamp: Option<Stamp>,
}

/// §7.5: the pin, else the cell for this OS, else the shared cell. `None`
/// means "keep the local value".
pub fn effective<'a>(
    r: &'a Record,
    field: &str,
    os: OsKind,
    pin: Option<&'a serde_json::Value>,
) -> Effective<'a> {
    let _ = (r, field, os, pin);
    Effective::default()
}

/// §9.2: the scope an edit of `field` on this OS writes to.
pub fn edit_target(r: &Record, field: &str, os: OsKind, default: DefaultScope) -> Scope {
    let _ = (r, field, os, default);
    Scope::All
}

/// §9.3: the cells a scope action writes, given the current effective
/// value. A `Null` value clears that cell.
pub fn scope_writes(
    r: &Record,
    field: &str,
    os: OsKind,
    action: ScopeAction,
    current: &serde_json::Value,
) -> Vec<(FieldKey, serde_json::Value)> {
    let _ = (r, field, os, action, current);
    Vec::new()
}

/// Whether a list item exists on this OS: visible, and `avail` is `all` or
/// this OS (§7.5).
pub fn exists_here(r: &Record, os: OsKind) -> bool {
    let _ = (r, os);
    false
}
