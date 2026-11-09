//! Per-OS and per-computer overrides (docs/design/SYNC.md §9): which cell
//! a value comes from on this computer, where an edit goes, and what a
//! scope action writes.
//!
//! Pins ("This computer", held values) live in the replica's `pins` table,
//! never in cells. These functions take the pin when they need it and
//! leave adding or removing pins to the caller: `scope_writes` returns
//! only the cell writes of an action.

use super::merge::is_visible;
use super::split::AVAIL;
use super::types::{DefaultScope, FieldKey, OsKind, Record, Scope, ScopeAction, Stamp};
use serde_json::Value;

/// The value of one field on this computer, and the cell it came from.
/// `from` and `stamp` are `None` when the value is a pin or local.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Effective<'a> {
    pub value: Option<&'a Value>,
    pub from: Option<Scope>,
    pub stamp: Option<Stamp>,
}

/// A non-null cell of `field` at `scope`.
fn set_cell<'a>(r: &'a Record, field: &str, scope: Scope) -> Option<&'a super::types::Cell> {
    r.cell(field, scope).filter(|c| !c.value.is_null())
}

/// §7.5: the pin, else the cell for this OS, else the shared cell. A
/// `null` cell means "no value at this scope" and is skipped. `None`
/// means "keep the local value" (absence never overrides, R4).
pub fn effective<'a>(
    r: &'a Record,
    field: &str,
    os: OsKind,
    pin: Option<&'a Value>,
) -> Effective<'a> {
    if let Some(p) = pin {
        return Effective {
            value: Some(p),
            from: None,
            stamp: None,
        };
    }
    for scope in [Scope::Os(os), Scope::All] {
        if let Some(c) = set_cell(r, field, scope) {
            return Effective {
                value: Some(&c.value),
                from: Some(scope),
                stamp: Some(c.stamp),
            };
        }
    }
    Effective::default()
}

/// §9.2: the scope an edit of `field` on this OS writes to: where the
/// effective value comes from, else the row's default. A "This computer"
/// pin comes first; the caller checks it and updates the pin instead.
pub fn edit_target(r: &Record, field: &str, os: OsKind, default: DefaultScope) -> Scope {
    if set_cell(r, field, Scope::Os(os)).is_some() {
        Scope::Os(os)
    } else if set_cell(r, field, Scope::All).is_some() {
        Scope::All
    } else {
        match default {
            DefaultScope::PerOs => Scope::Os(os),
            DefaultScope::Shared => Scope::All,
        }
    }
}

/// §9.3: the cells a scope action writes, given `current`, the effective
/// value on this computer. A `Null` value clears that cell.
///
/// | Action | Cell writes | The caller also |
/// |---|---|---|
/// | `all` | `f@all = x`; `f@<os> = null` for every OS with a value | removes the pin |
/// | `os` | `f@<this os> = x` | removes the pin |
/// | `use_shared` | `f@<this os> = null`, if it has a value | removes the pin |
/// | `this_computer` | none | pins `x` |
/// | `stop_this_computer` | none | removes the pin, projects |
/// | `avail_all` | `avail@all = "all"` | |
/// | `avail_os` | `avail@all = "<this os>"` | |
/// | `avail_this_computer` | none | pins the item (`*`), tombstones it once confirmed |
pub fn scope_writes(
    r: &Record,
    field: &str,
    os: OsKind,
    action: ScopeAction,
    current: &Value,
) -> Vec<(FieldKey, Value)> {
    match action {
        ScopeAction::All => std::iter::once((FieldKey::all(field), current.clone()))
            .chain(
                OsKind::ALL
                    .into_iter()
                    .filter(|o| set_cell(r, field, Scope::Os(*o)).is_some())
                    .map(|o| (FieldKey::os(field, o), Value::Null)),
            )
            .collect(),
        ScopeAction::Os => vec![(FieldKey::os(field, os), current.clone())],
        ScopeAction::UseShared => set_cell(r, field, Scope::Os(os))
            .map(|_| (FieldKey::os(field, os), Value::Null))
            .into_iter()
            .collect(),
        ScopeAction::AvailAll => vec![(FieldKey::all(AVAIL), Value::from("all"))],
        ScopeAction::AvailOs => vec![(FieldKey::all(AVAIL), Value::from(os.as_str()))],
        ScopeAction::ThisComputer
        | ScopeAction::StopThisComputer
        | ScopeAction::AvailThisComputer => Vec::new(),
    }
}

/// Whether a list item exists on this OS: visible, and `avail` is `all` or
/// this OS (§7.5). A record without `avail` (a setting) exists everywhere;
/// an `avail` this build doesn't know (an OS it doesn't run on) doesn't
/// exist here. A `*` pin is the caller's to check.
pub fn exists_here(r: &Record, os: OsKind) -> bool {
    if !is_visible(r) {
        return false;
    }
    match r.cell(AVAIL, Scope::All).map(|c| &c.value) {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s == "all" || s == os.as_str(),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{Cell, DeviceId, Tomb};
    use serde_json::json;

    const MAC: OsKind = OsKind::Macos;
    const WIN: OsKind = OsKind::Windows;

    fn st(n: u64) -> Stamp {
        Stamp::new(n, DeviceId([1; 10]))
    }

    fn rec(cells: &[(&str, Value, u64)]) -> Record {
        let mut r = Record::new("cfg", "ui.font_size");
        for (k, v, s) in cells {
            r.cells.insert(
                k.parse().unwrap(),
                Cell {
                    value: v.clone(),
                    stamp: st(*s),
                    wall_ms: 0,
                    anc: vec![],
                },
            );
        }
        r
    }

    fn writes(r: &Record, action: ScopeAction, x: Value) -> Vec<(String, Value)> {
        scope_writes(r, "v", MAC, action, &x)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn effective_prefers_pin_then_this_os_then_shared() {
        let r = rec(&[("v@all", json!(14), 1), ("v@macos", json!(16), 2)]);
        let e = effective(&r, "v", MAC, None);
        assert_eq!(e.value, Some(&json!(16)));
        assert_eq!(e.from, Some(Scope::Os(MAC)));
        assert_eq!(e.stamp, Some(st(2)));
        let e = effective(&r, "v", WIN, None);
        assert_eq!((e.value, e.from), (Some(&json!(14)), Some(Scope::All)));
        let pin = json!(20);
        let e = effective(&r, "v", MAC, Some(&pin));
        assert_eq!((e.value, e.from, e.stamp), (Some(&pin), None, None));
        // Null means no value at that scope.
        let r = rec(&[("v@all", json!(14), 1), ("v@macos", json!(null), 2)]);
        assert_eq!(effective(&r, "v", MAC, None).value, Some(&json!(14)));
        let r = rec(&[("v@all", json!(null), 1)]);
        assert_eq!(effective(&r, "v", MAC, None), Effective::default());
        assert_eq!(effective(&Record::default(), "v", MAC, None).value, None);
    }

    #[test]
    fn edit_target_writes_where_the_value_lives() {
        let shared = rec(&[("v@all", json!(14), 1)]);
        let mac = rec(&[("v@all", json!(14), 1), ("v@macos", json!(16), 2)]);
        let none = rec(&[]);
        let cleared = rec(&[("v@macos", json!(null), 2)]);
        for d in [DefaultScope::Shared, DefaultScope::PerOs] {
            assert_eq!(edit_target(&mac, "v", MAC, d), Scope::Os(MAC));
            assert_eq!(edit_target(&shared, "v", MAC, d), Scope::All);
            assert_eq!(edit_target(&mac, "v", WIN, d), Scope::All);
        }
        assert_eq!(
            edit_target(&none, "v", MAC, DefaultScope::PerOs),
            Scope::Os(MAC)
        );
        assert_eq!(
            edit_target(&none, "v", MAC, DefaultScope::Shared),
            Scope::All
        );
        assert_eq!(
            edit_target(&cleared, "v", MAC, DefaultScope::Shared),
            Scope::All
        );
    }

    #[test]
    fn every_scope_action_writes_exactly_its_cells() {
        let r = rec(&[
            ("v@all", json!(14), 1),
            ("v@macos", json!(16), 2),
            ("v@windows", json!(12), 3),
            ("v@linux", json!(null), 4),
        ]);
        let x = json!(16);
        assert_eq!(
            writes(&r, ScopeAction::All, x.clone()),
            vec![
                ("v@all".into(), json!(16)),
                ("v@macos".into(), json!(null)),
                ("v@windows".into(), json!(null)),
            ]
        );
        assert_eq!(
            writes(&r, ScopeAction::Os, x.clone()),
            vec![("v@macos".into(), json!(16))]
        );
        assert_eq!(
            writes(&r, ScopeAction::UseShared, x.clone()),
            vec![("v@macos".into(), json!(null))]
        );
        let shared_only = rec(&[("v@all", json!(14), 1)]);
        assert!(writes(&shared_only, ScopeAction::UseShared, x.clone()).is_empty());
        for a in [
            ScopeAction::ThisComputer,
            ScopeAction::StopThisComputer,
            ScopeAction::AvailThisComputer,
        ] {
            assert!(writes(&r, a, x.clone()).is_empty(), "{a:?}");
        }
        assert_eq!(
            writes(&r, ScopeAction::AvailAll, x.clone()),
            vec![("avail@all".into(), json!("all"))]
        );
        assert_eq!(
            writes(&r, ScopeAction::AvailOs, x),
            vec![("avail@all".into(), json!("macos"))]
        );
    }

    /// I7 in miniature: a Mac override leaves Windows alone, and "use the
    /// value for all computers" returns Macs to the shared value.
    #[test]
    fn a_mac_override_and_back() {
        let mut r = rec(&[("v@all", json!(14), 1)]);
        for (k, v) in scope_writes(&r, "v", MAC, ScopeAction::Os, &json!(16)) {
            r.cells.insert(
                k,
                Cell {
                    value: v,
                    stamp: st(2),
                    wall_ms: 0,
                    anc: vec![],
                },
            );
        }
        assert_eq!(effective(&r, "v", MAC, None).value, Some(&json!(16)));
        assert_eq!(effective(&r, "v", WIN, None).value, Some(&json!(14)));
        for (k, v) in scope_writes(&r, "v", MAC, ScopeAction::UseShared, &json!(16)) {
            r.cells.insert(
                k,
                Cell {
                    value: v,
                    stamp: st(3),
                    wall_ms: 0,
                    anc: vec![],
                },
            );
        }
        assert_eq!(effective(&r, "v", MAC, None).value, Some(&json!(14)));
        assert_eq!(edit_target(&r, "v", MAC, DefaultScope::Shared), Scope::All);
    }

    #[test]
    fn exists_here_follows_avail_and_visibility() {
        assert!(exists_here(&rec(&[]), MAC));
        assert!(exists_here(&rec(&[("avail@all", json!("all"), 1)]), WIN));
        let mac_only = rec(&[("avail@all", json!("macos"), 1)]);
        assert!(exists_here(&mac_only, MAC));
        assert!(!exists_here(&mac_only, WIN));
        assert!(!exists_here(
            &rec(&[("avail@all", json!("android"), 1)]),
            MAC
        ));
        assert!(!exists_here(&rec(&[("avail@all", json!(3), 1)]), MAC));
        let mut gone = rec(&[("avail@all", json!("all"), 1)]);
        gone.del = Some(Tomb {
            stamp: st(5),
            wall_ms: 0,
            anc: vec![st(1)],
        });
        assert!(!exists_here(&gone, MAC));
    }
}
