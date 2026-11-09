//! A record's fields to cell field names and back (docs/design/SYNC.md
//! §7.2). Scopes are added by the caller.
//!
//! | Cell name | Holds |
//! |---|---|
//! | `f` | Whole field `f` |
//! | `+<first>` | A group that changes together, as an object of its present fields |
//! | `f[]<element>` | One element of set `f`: `true` present, `false` removed |
//! | `f{}<key>` | One entry of map `f`: its value, `null` removed |
//! | `v` | The whole record, when the structure is `whole` |
//!
//! `<element>` is the element's canonical JSON when that is at most
//! [`MAX_ELEMENT_LEN`] characters. A longer element is named
//! `#<32 hex of its SHA-256>` and its cell holds the element itself (never
//! `true`) while present. A map key longer than that, or starting with
//! `#`, is named the same way and its cell holds `[key, value]`.
//!
//! Field names must pass [`valid_field_name`]. `split` skips one that
//! doesn't (with a warning) rather than build a name `join` would misread.
//!
//! Round trip: `join(split(x)) == x` for fields that aren't local, when
//! sets are sorted by canonical JSON without duplicates, no value is
//! `null`, and no set or map is empty. Otherwise `join` gives the same
//! record in normal form: a set comes back sorted and without duplicates,
//! an empty set or map comes back only if some cell for it exists, and
//! `null` reads as "absent". `split(join(split(x))) == split(x)` always.

use super::canonical::canonical;
use super::types::Structure;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The single cell of a `whole` record.
pub const WHOLE: &str = "v";
/// List items: where the item exists (`all` or an OS name).
pub const AVAIL: &str = "avail";
/// List items: the item's fractional position.
pub const ORDER: &str = "order";
/// Longest inline set element or map key, in characters.
pub const MAX_ELEMENT_LEN: usize = 128;

const SET: &str = "[]";
const MAP: &str = "{}";
const HASHED: char = '#';

/// What a cell name addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellName<'a> {
    Whole(&'a str),
    /// `+<first>`: the group whose first field is `first`.
    Group(&'a str),
    /// `field[]element`. `element` is canonical JSON, or `#<hash>`.
    Element {
        field: &'a str,
        element: &'a str,
    },
    /// `field{}key`. `key` is the key itself, or `#<hash>`.
    Entry {
        field: &'a str,
        key: &'a str,
    },
}

/// Read a cell name. Field names hold neither `[]` nor `{}`, so the first
/// of them separates the field from the element or key.
pub fn parse_name(name: &str) -> CellName<'_> {
    if let Some(first) = name.strip_prefix('+') {
        return CellName::Group(first);
    }
    let set = name.find(SET);
    let map = name.find(MAP);
    match (set, map) {
        (Some(s), m) if m.is_none_or(|m| s < m) => CellName::Element {
            field: &name[..s],
            element: &name[s + SET.len()..],
        },
        (_, Some(m)) => CellName::Entry {
            field: &name[..m],
            key: &name[m + MAP.len()..],
        },
        _ => CellName::Whole(name),
    }
}

/// A set element or map entry: these merge as a union and never conflict
/// (§8.5).
pub fn is_union(name: &str) -> bool {
    matches!(
        parse_name(name),
        CellName::Element { .. } | CellName::Entry { .. }
    )
}

/// The value that says a cell's thing is gone: `false` for a set element,
/// `null` for everything else. Capture writes it for a name `split` no
/// longer produces.
pub fn absent_value(name: &str) -> Value {
    match parse_name(name) {
        CellName::Element { .. } => Value::Bool(false),
        _ => Value::Null,
    }
}

/// A field name a cell name can carry: not empty, no `@` (the scope
/// separator), no `[]` or `{}`, and not starting with `+`.
pub fn valid_field_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('+')
        && !name.contains('@')
        && !name.contains(SET)
        && !name.contains(MAP)
}

fn hashed(v: &[u8]) -> String {
    let h = Sha256::digest(v);
    format!("{HASHED}{}", hex::encode(&h[..16]))
}

/// The cell name and present-value of one set element.
pub fn element_cell(field: &str, element: &Value) -> (String, Value) {
    let c = canonical(element);
    let text = String::from_utf8_lossy(&c);
    if text.chars().count() <= MAX_ELEMENT_LEN {
        (format!("{field}{SET}{text}"), Value::Bool(true))
    } else {
        (format!("{field}{SET}{}", hashed(&c)), element.clone())
    }
}

/// The cell name and value of one map entry.
pub fn entry_cell(field: &str, key: &str, value: &Value) -> (String, Value) {
    if key.chars().count() <= MAX_ELEMENT_LEN && !key.starts_with(HASHED) {
        (format!("{field}{MAP}{key}"), value.clone())
    } else {
        (
            format!("{field}{MAP}{}", hashed(key.as_bytes())),
            Value::Array(vec![Value::String(key.to_string()), value.clone()]),
        )
    }
}

fn contains(list: &[String], name: &str) -> bool {
    list.iter().any(|n| n == name)
}

/// Split a record's fields into cell field names and values.
pub fn split(fields: &Map<String, Value>, s: &Structure) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let synced = |k: &str| !contains(&s.local, k);
    if s.whole {
        let rest: Map<String, Value> = fields
            .iter()
            .filter(|(k, _)| synced(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.insert(WHOLE.to_string(), Value::Object(rest));
        return out;
    }
    let grouped = |k: &str| s.together.iter().any(|g| contains(g, k));
    for (k, v) in fields {
        if !synced(k) || grouped(k) || v.is_null() {
            continue;
        }
        if !valid_field_name(k) {
            log::warn!("sync: skipped a field whose name a cell can't carry");
            continue;
        }
        if contains(&s.sets, k) {
            let Some(items) = v.as_array() else {
                log::warn!("sync: skipped set field {k}: not an array");
                continue;
            };
            for e in items {
                let (name, val) = element_cell(k, e);
                out.insert(name, val);
            }
        } else if contains(&s.maps, k) {
            let Some(entries) = v.as_object() else {
                log::warn!("sync: skipped map field {k}: not an object");
                continue;
            };
            for (mk, mv) in entries.iter().filter(|(_, mv)| !mv.is_null()) {
                let (name, val) = entry_cell(k, mk, mv);
                out.insert(name, val);
            }
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    for g in &s.together {
        let Some(first) = g.first() else { continue };
        let members: Map<String, Value> = g
            .iter()
            .filter(|k| synced(k))
            .filter_map(|k| {
                fields
                    .get(k)
                    .filter(|v| !v.is_null())
                    .map(|v| (k.clone(), v.clone()))
            })
            .collect();
        if !members.is_empty() {
            out.insert(format!("+{first}"), Value::Object(members));
        }
    }
    out
}

/// Rebuild a record's fields from its cells (effective values, names
/// without scopes). Local fields aren't in the cells; the caller adds them
/// from the local record.
pub fn join<'a>(
    cells: impl Iterator<Item = (&'a str, &'a Value)>,
    s: &Structure,
) -> Map<String, Value> {
    let mut out = Map::new();
    if s.whole {
        for (name, v) in cells {
            if name == WHOLE {
                if let Value::Object(o) = v {
                    out = o.clone();
                }
            }
        }
        out.retain(|k, _| !contains(&s.local, k));
        return out;
    }
    let mut sets: BTreeMap<String, BTreeMap<Vec<u8>, Value>> = BTreeMap::new();
    let mut maps: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (name, v) in cells {
        match parse_name(name) {
            CellName::Whole(f) => {
                if !v.is_null() {
                    out.insert(f.to_string(), v.clone());
                }
            }
            CellName::Group(_) => {
                if let Value::Object(o) = v {
                    for (k, x) in o.iter().filter(|(_, x)| !x.is_null()) {
                        out.insert(k.clone(), x.clone());
                    }
                }
            }
            CellName::Element { field, element } => {
                let set = sets.entry(field.to_string()).or_default();
                let item = if element.starts_with(HASHED) {
                    match v {
                        Value::Bool(_) | Value::Null => None,
                        other => Some(other.clone()),
                    }
                } else if *v == Value::Bool(true) {
                    serde_json::from_str::<Value>(element).ok()
                } else {
                    None
                };
                if let Some(e) = item {
                    set.insert(canonical(&e), e);
                }
            }
            CellName::Entry { field, key } => {
                let map = maps.entry(field.to_string()).or_default();
                if key.starts_with(HASHED) {
                    if let Some([Value::String(k), x]) = v.as_array().map(|a| a.as_slice()) {
                        if !x.is_null() {
                            map.insert(k.clone(), x.clone());
                        }
                    }
                } else if !v.is_null() {
                    map.insert(key.to_string(), v.clone());
                }
            }
        }
    }
    for (f, items) in sets {
        out.insert(f, Value::Array(items.into_values().collect()));
    }
    for (f, m) in maps {
        out.insert(f, Value::Object(m));
    }
    out.retain(|k, _| !contains(&s.local, k));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mind_map_items() -> Structure {
        Structure {
            whole: false,
            together: vec![
                vec![
                    "status".into(),
                    "doneAt".into(),
                    "suggested".into(),
                    "confidence".into(),
                ],
                vec!["title".into(), "titleSource".into()],
            ],
            sets: vec!["entityIds".into(), "terms".into()],
            maps: vec!["mentionOrigin".into()],
            local: vec!["rev".into(), "enrich".into()],
            per_os: vec![],
        }
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn rejoin(cells: &BTreeMap<String, Value>, s: &Structure) -> Map<String, Value> {
        join(cells.iter().map(|(k, v)| (k.as_str(), v)), s)
    }

    #[test]
    fn whole_fields_groups_sets_maps_and_local_fields() {
        let s = mind_map_items();
        let fields = obj(json!({
            "text": "Call Dana",
            "status": "open", "confidence": 0.5,
            "title": "Call", "titleSource": "user",
            "entityIds": ["e1", "e2"],
            "mentionOrigin": { "e1": "capture", "e2": "agent" },
            "rev": 7, "enrich": { "x": 1 }
        }));
        let cells = split(&fields, &s);
        let names: Vec<&str> = cells.keys().map(|k| k.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "+status",
                "+title",
                "entityIds[]\"e1\"",
                "entityIds[]\"e2\"",
                "mentionOrigin{}e1",
                "mentionOrigin{}e2",
                "text",
            ]
        );
        assert_eq!(
            cells["+status"],
            json!({ "status": "open", "confidence": 0.5 })
        );
        assert_eq!(cells["entityIds[]\"e1\""], json!(true));
        assert_eq!(cells["mentionOrigin{}e2"], json!("agent"));

        let mut want = fields.clone();
        want.remove("rev");
        want.remove("enrich");
        assert_eq!(rejoin(&cells, &s), want);
    }

    #[test]
    fn removed_elements_and_entries_drop_out() {
        let s = mind_map_items();
        let cells: BTreeMap<String, Value> = [
            ("entityIds[]\"e1\"".to_string(), json!(true)),
            ("entityIds[]\"e2\"".to_string(), json!(false)),
            ("terms[]\"x\"".to_string(), json!(false)),
            ("mentionOrigin{}e1".to_string(), json!(null)),
            ("text".to_string(), json!(null)),
        ]
        .into_iter()
        .collect();
        let j = rejoin(&cells, &s);
        assert_eq!(
            Value::Object(j),
            json!({ "entityIds": ["e1"], "terms": [], "mentionOrigin": {} })
        );
        assert_eq!(absent_value("entityIds[]\"e1\""), json!(false));
        assert_eq!(absent_value("mentionOrigin{}e1"), json!(null));
        assert_eq!(absent_value("+status"), json!(null));
        assert_eq!(absent_value("text"), json!(null));
    }

    #[test]
    fn sets_come_back_sorted_and_without_duplicates() {
        let s = mind_map_items();
        let fields = obj(json!({ "terms": ["b", 2, "a", "b", { "k": 1 }] }));
        let cells = split(&fields, &s);
        assert_eq!(cells.len(), 4);
        let j = rejoin(&cells, &s);
        // Sorted by canonical bytes: `"` sorts before digits and `{`.
        assert_eq!(j["terms"], json!(["a", "b", 2, { "k": 1 }]));
        assert_eq!(split(&j, &s), cells);
    }

    #[test]
    fn element_key_length_limit() {
        let s = mind_map_items();
        let short = "x".repeat(MAX_ELEMENT_LEN - 2); // plus two quotes
        let long = "y".repeat(MAX_ELEMENT_LEN);
        let fields = obj(json!({ "terms": [short.clone(), long.clone()] }));
        let cells = split(&fields, &s);
        let inline = format!("terms[]\"{short}\"");
        assert_eq!(cells[&inline], json!(true));
        let hashed: Vec<_> = cells.keys().filter(|k| k.starts_with("terms[]#")).collect();
        assert_eq!(hashed.len(), 1);
        assert_eq!(hashed[0].len(), "terms[]#".len() + 32);
        assert_eq!(cells[hashed[0]], json!(long));
        let mut j = rejoin(&cells, &s);
        let mut want = json!([short, long]);
        want.as_array_mut().unwrap().sort_by_key(canonical);
        assert_eq!(j.remove("terms").unwrap(), want);

        // A hashed element that was removed is `false`.
        let gone: BTreeMap<String, Value> = [(hashed[0].clone(), absent_value(hashed[0]))]
            .into_iter()
            .collect();
        assert_eq!(rejoin(&gone, &s)["terms"], json!([]));

        // Long map keys, and keys that look hashed, carry the key.
        let long_key = "k".repeat(MAX_ELEMENT_LEN + 1);
        let fields = obj(json!({ "mentionOrigin": { long_key.clone(): 1, "#x": 2, "a@b": 3 } }));
        let cells = split(&fields, &s);
        assert!(cells.contains_key("mentionOrigin{}a@b"));
        assert_eq!(
            cells
                .keys()
                .filter(|k| k.starts_with("mentionOrigin{}#"))
                .count(),
            2
        );
        assert_eq!(rejoin(&cells, &s), fields);
    }

    #[test]
    fn whole_records_are_one_cell() {
        let s = Structure {
            whole: true,
            local: vec!["fileRefs".into()],
            ..Structure::default()
        };
        let fields = obj(json!({ "body": "long text", "fileRefs": ["r1"], "n": 1 }));
        let cells = split(&fields, &s);
        assert_eq!(
            cells,
            [(WHOLE.to_string(), json!({ "body": "long text", "n": 1 }))]
                .into_iter()
                .collect()
        );
        assert_eq!(
            Value::Object(rejoin(&cells, &s)),
            json!({ "body": "long text", "n": 1 })
        );
    }

    #[test]
    fn nulls_and_bad_shapes_are_skipped() {
        let s = mind_map_items();
        let fields = obj(json!({
            "text": null, "terms": "not-a-list", "mentionOrigin": [1],
            "a@b": 1, "x[]y": 2, "+g": 3, "ok": 4, "status": null
        }));
        let cells = split(&fields, &s);
        assert_eq!(cells, [("ok".to_string(), json!(4))].into_iter().collect());
    }

    #[test]
    fn names_parse_back() {
        assert_eq!(parse_name("text"), CellName::Whole("text"));
        assert_eq!(parse_name("+status"), CellName::Group("status"));
        assert_eq!(
            parse_name("terms[]\"a{}b\""),
            CellName::Element {
                field: "terms",
                element: "\"a{}b\""
            }
        );
        assert_eq!(
            parse_name("m{}k[]x"),
            CellName::Entry {
                field: "m",
                key: "k[]x"
            }
        );
        assert!(is_union("terms[]1"));
        assert!(is_union("m{}k"));
        assert!(!is_union("+status"));
        assert!(!is_union(AVAIL));
        for bad in ["", "+a", "a@b", "a[]b", "a{}b"] {
            assert!(!valid_field_name(bad), "{bad}");
        }
        assert!(valid_field_name("ui.theme"));
    }

    /// `split(join(split(x))) == split(x)` on random records.
    #[test]
    fn split_is_stable_through_join() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        let s = mind_map_items();
        let mut r = StdRng::seed_from_u64(11);
        let atoms = [
            json!("a"),
            json!(1),
            json!(1.0),
            json!(null),
            json!({ "z": [1] }),
            json!(true),
        ];
        for _ in 0..2_000 {
            let mut f = Map::new();
            for name in [
                "text",
                "status",
                "doneAt",
                "title",
                "rev",
                "entityIds",
                "mentionOrigin",
            ] {
                if !r.random_bool(0.6) {
                    continue;
                }
                let v = match name {
                    "entityIds" => (0..r.random_range(0..4))
                        .map(|_| atoms[r.random_range(0..atoms.len())].clone())
                        .collect(),
                    "mentionOrigin" => {
                        let mut m = Map::new();
                        for _ in 0..r.random_range(0..3) {
                            m.insert(
                                ["e1", "e2", "#e"][r.random_range(0..3)].into(),
                                atoms[r.random_range(0..atoms.len())].clone(),
                            );
                        }
                        Value::Object(m)
                    }
                    _ => atoms[r.random_range(0..atoms.len())].clone(),
                };
                f.insert(name.into(), v);
            }
            let once = split(&f, &s);
            let again = split(&rejoin(&once, &s), &s);
            assert_eq!(
                once.keys().collect::<Vec<_>>(),
                again.keys().collect::<Vec<_>>()
            );
            for (k, v) in &once {
                assert!(crate::sync::canonical::same(v, &again[k]), "{k}");
            }
        }
    }
}
