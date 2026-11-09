//! The `list.*` namespaces: `config.json` lists merged item by item and
//! field by field, with `order` and `avail` cells (docs/design/SYNC.md
//! §4.4).
//!
//! An item's record id is its stable id (`sync_id`, or `id` for agent
//! connections and internal sites). Its fields are the item's fields,
//! without the id and without the fields that stay on this computer
//! (`trigger.last_run` of an automation), plus its position as `order`
//! (the [`super::ORDER`] convention). A write rebuilds the whole list:
//! synced items in `(order, id)` order, this computer's own items where
//! they were, and each item's local fields from its local copy.

use super::{LocalRecord, LocalState, LocalWrite, NsSpec, ProjectResult, Source, ORDER};
use crate::config::Config;
use crate::sync::catalog::{Avail, ListSpec, SYNC_LISTS};
use crate::sync::engine::{CommitError, ConfigPort};
use crate::sync::sources::config::{get_path, set_path};
use crate::sync::types::{DefaultScope, Origin, OsKind, Structure};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Every list of `config.json`.
pub struct ListsSource {
    port: Arc<dyn ConfigPort>,
    os: OsKind,
}

impl ListsSource {
    pub fn new(port: Arc<dyn ConfigPort>, os: OsKind) -> Self {
        Self { port, os }
    }
}

/// The namespace's spec.
pub fn spec_of(l: &ListSpec) -> NsSpec {
    NsSpec {
        ns: l.ns.to_string(),
        category: l.category,
        schema: 1,
        echo: true,
        structure: Structure {
            per_os: l.per_os.iter().map(|s| s.to_string()).collect(),
            ..Structure::default()
        },
        default_scope: DefaultScope::Shared,
        large: false,
        label_field: crate::sync::catalog::label_field(l.ns).map(str::to_string),
    }
}

/// Remove a dotted `path` (`trigger.last_run`) from an item, returning
/// the value that was there.
fn take_path(item: &mut Map<String, Value>, path: &str) -> Option<Value> {
    match path.split_once('.') {
        None => item.remove(path),
        Some((head, rest)) => item
            .get_mut(head)
            .and_then(Value::as_object_mut)
            .and_then(|o| take_path(o, rest)),
    }
}

/// Put `value` back at a dotted `path` of an item, if its parent object is
/// there.
fn put_path(item: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            item.insert(path.to_string(), value);
        }
        Some((head, rest)) => {
            if let Some(o) = item.get_mut(head).and_then(Value::as_object_mut) {
                put_path(o, rest, value);
            }
        }
    }
}

/// Where a new item exists by default (§4.4).
pub fn default_avail(l: &ListSpec, item: &Map<String, Value>, os: OsKind) -> String {
    match l.avail {
        Avail::All => "all".into(),
        Avail::MadeOn => os.as_str().into(),
        Avail::ProgramsMadeOn => {
            if item.get("action_type").and_then(Value::as_str) == Some("run_program")
                || item.get("action_type").is_none()
            {
                // `run_program` is the default action of a shortcut.
                os.as_str().into()
            } else {
                "all".into()
            }
        }
    }
}

/// The items of one list in a config tree.
fn items<'a>(tree: &'a Value, l: &ListSpec) -> Vec<&'a Map<String, Value>> {
    get_path(tree, l.path)
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

/// The item's stable id, if it has one.
fn item_id<'a>(item: &'a Map<String, Value>, l: &ListSpec) -> Option<&'a str> {
    item.get(l.id_field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// The record of one item: its synced fields and its position.
pub fn record_of(item: &Map<String, Value>, l: &ListSpec, pos: usize, os: OsKind) -> LocalRecord {
    let mut fields = item.clone();
    fields.remove(l.id_field);
    for p in l.this_computer {
        take_path(&mut fields, p);
    }
    let avail = default_avail(l, &fields, os);
    fields.insert(ORDER.to_string(), Value::from(pos as u64));
    LocalRecord {
        fields,
        avail: Some(avail),
    }
}

/// The local state of every list in a config.
pub fn state_of(config: &Config, os: OsKind) -> LocalState {
    let tree = serde_json::to_value(config).unwrap_or(Value::Null);
    let mut st = LocalState::default();
    for l in SYNC_LISTS {
        let mut seen = BTreeSet::new();
        for (pos, item) in items(&tree, l).into_iter().enumerate() {
            // `Config::ensure_item_ids` gives every item an id; one without
            // (or a repeat) isn't synced until it has one.
            let Some(id) = item_id(item, l) else { continue };
            if !seen.insert(id.to_string()) {
                continue;
            }
            st.put(l.ns, id, record_of(item, l, pos, os));
        }
    }
    st
}

/// Whether `item` reads as an item of list `l` in this build.
fn item_fits(l: &ListSpec, item: &Value) -> bool {
    let mut tree = serde_json::json!({});
    set_path(&mut tree, l.path, Value::Array(vec![item.clone()]));
    Config::deserialize(&tree).is_ok()
}

/// Rebuild list `l` from the writes for it. Every write for a list carries
/// every synced item that exists here; items with no write are this
/// computer's own (a `*` pin, or a new item not captured yet) and keep
/// their places.
fn rebuild(l: &ListSpec, old: &[&Map<String, Value>], writes: &[&LocalWrite]) -> Vec<Value> {
    let by_id: BTreeMap<&str, &Map<String, Value>> = old
        .iter()
        .filter_map(|i| item_id(i, l).map(|id| (id, *i)))
        .collect();
    let written: BTreeSet<&str> = writes.iter().map(|w| w.id.as_str()).collect();
    let mut synced: Vec<(String, &str, Value)> = Vec::new();
    for w in writes {
        let Some(rec) = &w.value else { continue };
        let mut fields = rec.fields.clone();
        let key = match fields.remove(ORDER) {
            Some(Value::String(s)) => s,
            _ => String::new(),
        };
        fields.insert(l.id_field.to_string(), Value::String(w.id.clone()));
        let local = by_id.get(w.id.as_str());
        for p in l.this_computer {
            if let Some(v) = local.and_then(|o| {
                let mut copy = (*o).clone();
                take_path(&mut copy, p)
            }) {
                put_path(&mut fields, p, v);
            }
        }
        let mut value = Value::Object(fields);
        if !item_fits(l, &value) {
            match local {
                Some(o) => {
                    log::warn!(
                        "sync: a synced item of {} doesn't fit this version; kept the local one",
                        l.ns
                    );
                    value = Value::Object((*o).clone());
                }
                None => {
                    log::warn!(
                        "sync: a synced item of {} doesn't fit this version; left it out",
                        l.ns
                    );
                    continue;
                }
            }
        }
        synced.push((key, w.id.as_str(), value));
    }
    synced.sort_by(|a, b| (a.0.as_str(), a.1).cmp(&(b.0.as_str(), b.1)));
    let mut out: Vec<Value> = synced.into_iter().map(|(_, _, v)| v).collect();
    // This computer's own items, after the synced item that came before
    // each of them locally.
    let mut prev_at: Option<usize> = None;
    for item in old {
        let id = item_id(item, l);
        let own = id.is_none_or(|id| !written.contains(id));
        if own {
            let at = prev_at.map_or(0, |i| i + 1).min(out.len());
            out.insert(at, Value::Object((*item).clone()));
            prev_at = Some(at);
        } else if let Some(i) = id.and_then(|id| {
            out.iter()
                .position(|v| v.get(l.id_field).and_then(Value::as_str) == Some(id))
        }) {
            prev_at = Some(i);
        }
    }
    out
}

/// Apply the writes to a copy of `local`.
pub fn apply_writes(local: &Config, writes: &[LocalWrite]) -> Config {
    let mut tree = serde_json::to_value(local).unwrap_or(Value::Null);
    for l in SYNC_LISTS {
        let mine: Vec<&LocalWrite> = writes.iter().filter(|w| w.ns == l.ns).collect();
        if mine.is_empty() {
            continue;
        }
        let old_tree = tree.clone();
        let old = items(&old_tree, l);
        let next = rebuild(l, &old, &mine);
        set_path(&mut tree, l.path, Value::Array(next));
    }
    Config::deserialize(&tree).unwrap_or_else(|e| {
        log::warn!("sync: the merged lists don't read back ({e}); kept the local lists");
        local.clone()
    })
}

impl Source for ListsSource {
    fn id(&self) -> &'static str {
        "lists"
    }

    fn specs(&self) -> Vec<NsSpec> {
        SYNC_LISTS.iter().map(spec_of).collect()
    }

    fn read_local(&mut self) -> anyhow::Result<LocalState> {
        let (_, config) = self.port.snapshot();
        Ok(state_of(&config, self.os))
    }

    fn project(
        &mut self,
        writes: Vec<LocalWrite>,
        origin: Origin,
    ) -> anyhow::Result<ProjectResult> {
        let (rev, local) = self.port.snapshot();
        let mut next = apply_writes(&local, &writes);
        next.ensure_item_ids();
        let read_back = state_of(&next, self.os);
        match self.port.commit(rev, next, origin) {
            Ok(_) => Ok(ProjectResult::Applied { read_back }),
            Err(CommitError::Stale) => Ok(ProjectResult::Stale),
            Err(CommitError::Suspended) => {
                anyhow::bail!("config.json is from a newer build; saves are suspended")
            }
            Err(CommitError::Io(e)) => Err(e),
        }
    }
}

/// The matching rules for an item that lost its id (§4.4 "Lost ids"), in
/// order: all known fields equal; the same name, unique on both sides; the
/// same position with at most one field changed. `orphans` are the items
/// without a known id, `missing` the known records no item holds any more
/// (both as their fields, `order` holding the position). Returns pairs
/// `(orphan id, record id)`.
pub fn rematch(
    orphans: &BTreeMap<String, Map<String, Value>>,
    missing: &BTreeMap<String, Map<String, Value>>,
    name_field: Option<&str>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut free_o: BTreeSet<&String> = orphans.keys().collect();
    let mut free_m: BTreeSet<&String> = missing.keys().collect();
    // Positions aside, and an empty field the same as a missing one.
    let without_order = |m: &Map<String, Value>| {
        let mut c = m.clone();
        c.remove(ORDER);
        c.retain(|_, v| !v.is_null());
        c
    };
    // 1. All known fields equal.
    for o in orphans.keys() {
        let ow = without_order(&orphans[o]);
        if let Some(m) = free_m
            .iter()
            .find(|m| {
                crate::sync::canonical::same(
                    &Value::Object(without_order(&missing[**m])),
                    &Value::Object(ow.clone()),
                )
            })
            .copied()
        {
            out.push((o.clone(), m.clone()));
            free_o.remove(o);
            free_m.remove(m);
        }
    }
    // 2. The same name, unique on both sides.
    if let Some(f) = name_field {
        let name = |m: &Map<String, Value>| m.get(f).and_then(Value::as_str).map(str::to_string);
        for o in free_o.clone() {
            let Some(n) = name(&orphans[o]).filter(|n| !n.is_empty()) else {
                continue;
            };
            let same_o = free_o
                .iter()
                .filter(|x| name(&orphans[**x]).as_ref() == Some(&n))
                .count();
            let same_m: Vec<&String> = free_m
                .iter()
                .filter(|x| name(&missing[**x]).as_ref() == Some(&n))
                .copied()
                .collect();
            if same_o == 1 && same_m.len() == 1 {
                let m = same_m[0];
                out.push((o.clone(), m.clone()));
                free_o.remove(o);
                free_m.remove(m);
            }
        }
    }
    // 3. The same position with at most one field changed.
    for o in free_o.clone() {
        let pos = orphans[o].get(ORDER);
        let Some(m) = free_m
            .iter()
            .find(|m| {
                let mm = &missing[**m];
                mm.get(ORDER) == pos && pos.is_some() && {
                    let a = without_order(&orphans[o]);
                    let b = without_order(mm);
                    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
                    keys.iter().filter(|k| a.get(**k) != b.get(**k)).count() <= 1
                }
            })
            .copied()
        else {
            continue;
        };
        out.push((o.clone(), m.clone()));
        free_o.remove(o);
        free_m.remove(m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::catalog::list_spec;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn items_become_records_without_their_id_or_local_fields() {
        let c = Config {
            macros: vec![serde_json::from_value(json!({
                "name": "Daily", "sync_id": "sabc",
                "trigger": { "type": "schedule", "interval": "every_1h", "last_run": "2026-01-01T00:00:00Z" }
            }))
            .unwrap()],
            shortcuts: vec![
                serde_json::from_value(json!({ "name": "gh", "sync_id": "s1", "action_type": "open_url", "url": "https://x" })).unwrap(),
                serde_json::from_value(json!({ "name": "vim", "sync_id": "s2", "action_type": "run_program", "path": "/usr/bin/vim" })).unwrap(),
                serde_json::from_value(json!({ "name": "no id" })).unwrap(),
            ],
            ..Config::default()
        };
        let st = state_of(&c, OsKind::Macos);
        let m = st.get("list.macros", "sabc").unwrap();
        assert!(m.fields.get("sync_id").is_none());
        assert!(m.fields["trigger"].get("last_run").is_none());
        assert_eq!(m.fields["trigger"]["interval"], json!("every_1h"));
        assert_eq!(m.avail.as_deref(), Some("all"));
        assert_eq!(
            st.get("list.shortcuts", "s1").unwrap().avail.as_deref(),
            Some("all")
        );
        let vim = st.get("list.shortcuts", "s2").unwrap();
        assert_eq!(vim.avail.as_deref(), Some("macos"));
        assert_eq!(vim.fields[ORDER], json!(1));
        assert_eq!(
            st.records
                .keys()
                .filter(|(ns, _)| ns == "list.shortcuts")
                .count(),
            2
        );
        let conn = st.get("list.connections", "default").unwrap();
        assert_eq!(conn.avail.as_deref(), Some("macos"));
    }

    #[test]
    fn writes_rebuild_the_list_in_order_and_keep_local_fields() {
        let c = Config {
            macros: vec![
                serde_json::from_value(json!({
                    "name": "A", "sync_id": "sa",
                    "trigger": { "type": "schedule", "interval": "every_1h", "last_run": "2026-01-01T00:00:00Z" }
                }))
                .unwrap(),
                serde_json::from_value(json!({ "name": "mine", "sync_id": "sl" })).unwrap(),
            ],
            ..Config::default()
        };
        let rec = |name: &str, order: &str, trigger: Value| LocalRecord {
            fields: obj(json!({ "name": name, "order": order, "trigger": trigger })),
            avail: None,
        };
        let writes = vec![
            LocalWrite {
                ns: "list.macros".into(),
                id: "sb".into(),
                value: Some(rec("B", "a0", json!({ "type": "manual" }))),
            },
            LocalWrite {
                ns: "list.macros".into(),
                id: "sa".into(),
                value: Some(rec(
                    "A2",
                    "a1",
                    json!({ "type": "schedule", "interval": "every_5m" }),
                )),
            },
        ];
        let next = apply_writes(&c, &writes);
        let names: Vec<&str> = next.macros.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["B", "A2", "mine"]);
        match &next.macros[1].trigger {
            crate::config::AutomationTrigger::Schedule { interval, last_run } => {
                assert_eq!(interval, "every_5m");
                assert_eq!(last_run.as_deref(), Some("2026-01-01T00:00:00Z"));
            }
            other => panic!("{other:?}"),
        }
        // A synced item that doesn't fit is left out; the rest apply.
        let bad = vec![LocalWrite {
            ns: "list.macros".into(),
            id: "sx".into(),
            value: Some(LocalRecord {
                fields: obj(json!({ "name": 5, "order": "a0" })),
                avail: None,
            }),
        }];
        let next = apply_writes(&c, &bad);
        assert!(next.macros.iter().all(|m| m.sync_id != "sx"));
        assert_eq!(next.macros.len(), 2);
        let _ = list_spec("list.macros").unwrap();
    }

    #[test]
    fn lost_ids_are_matched_by_content_then_name_then_position() {
        let orphans: BTreeMap<String, Map<String, Value>> = [
            (
                "n1".to_string(),
                obj(json!({ "name": "same", "url": "u", "order": 0 })),
            ),
            (
                "n2".to_string(),
                obj(json!({ "name": "renamed?", "url": "v", "order": 1 })),
            ),
            (
                "n3".to_string(),
                obj(json!({ "name": "by name", "url": "changed", "order": 5 })),
            ),
        ]
        .into();
        let missing: BTreeMap<String, Map<String, Value>> = [
            (
                "old1".to_string(),
                obj(json!({ "name": "same", "url": "u", "order": 3 })),
            ),
            (
                "old2".to_string(),
                obj(json!({ "name": "renamed", "url": "v", "order": 1 })),
            ),
            (
                "old3".to_string(),
                obj(json!({ "name": "by name", "url": "x", "order": 9 })),
            ),
            (
                "gone".to_string(),
                obj(json!({ "name": "gone", "order": 7 })),
            ),
        ]
        .into();
        let mut m = rematch(&orphans, &missing, Some("name"));
        m.sort();
        assert_eq!(
            m,
            vec![
                ("n1".to_string(), "old1".to_string()),
                ("n2".to_string(), "old2".to_string()),
                ("n3".to_string(), "old3".to_string()),
            ]
        );
    }
}
