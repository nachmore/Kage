//! The `save_config` three-way merge (docs/design/SYNC.md §9.5 item 2).
//!
//! A window sends a whole `Config` together with the revision it loaded.
//! Whatever the window didn't change keeps its current value, so a stale
//! window can't revert a field that another window or sync changed since:
//!
//! - a setting (each `sync::catalog` row that isn't a map or a list): when
//!   the incoming value equals the value at the base revision, the current
//!   value stays;
//! - `extension_states`: the same rule, per extension;
//! - `extensions`: per extension, then per field;
//! - a list: item by item by its id, then field by field. An item the
//!   window removed goes; an item added elsewhere since the base stays.
//!   The window's order wins only when the window reordered the list.
//!
//! The merge works on the JSON form of the three configs.

use crate::sync::catalog::{list_at, ListSpec, Rule, SYNC_FIELDS};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// `incoming` with every part the caller didn't change since `base` taken
/// from `current`.
pub fn merge(base: &Value, incoming: &Value, current: &Value) -> Value {
    let mut out = incoming.clone();
    for row in SYNC_FIELDS {
        match row.rule {
            Rule::List(_) => {
                let Some(spec) = list_at(row.path) else {
                    continue;
                };
                let merged = merge_list(
                    spec,
                    get(base, row.path),
                    get(incoming, row.path),
                    get(current, row.path),
                );
                put(&mut out, row.path, merged);
            }
            Rule::ExtensionSettings => {
                let merged = merge_map(
                    get(base, row.path),
                    get(incoming, row.path),
                    get(current, row.path),
                    2,
                );
                put(&mut out, row.path, merged);
            }
            _ => {
                if let Some(path) = row.path.strip_suffix(".*") {
                    let merged =
                        merge_map(get(base, path), get(incoming, path), get(current, path), 1);
                    put(&mut out, path, merged);
                } else {
                    let merged = pick(
                        get(base, row.path),
                        get(incoming, row.path),
                        get(current, row.path),
                    );
                    put(&mut out, row.path, merged.cloned());
                }
            }
        }
    }
    out
}

/// The value at a dotted path.
fn get<'a>(tree: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(tree, |cur, seg| cur.get(seg))
}

/// Set the value at a dotted path (creating objects on the way), or remove
/// it for `None`.
fn put(tree: &mut Value, path: &str, value: Option<Value>) {
    let segs: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = segs.split_last() else {
        return;
    };
    let mut cur = tree;
    for seg in parents {
        let Some(obj) = cur.as_object_mut() else {
            return;
        };
        if value.is_none() && !obj.contains_key(*seg) {
            return;
        }
        cur = obj
            .entry(seg.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let Some(obj) = cur.as_object_mut() else {
        return;
    };
    match value {
        Some(v) => {
            obj.insert(last.to_string(), v);
        }
        None => {
            obj.remove(*last);
        }
    }
}

/// The caller's value, unless it is the one it started from: then the
/// current one.
fn pick<'a>(
    base: Option<&'a Value>,
    incoming: Option<&'a Value>,
    current: Option<&'a Value>,
) -> Option<&'a Value> {
    if incoming == base {
        current
    } else {
        incoming
    }
}

fn as_map(v: Option<&Value>) -> Option<&Map<String, Value>> {
    v.and_then(Value::as_object)
}

/// Merge maps key by key, `depth` levels down; below that a value is one
/// whole value.
fn merge_map(
    base: Option<&Value>,
    incoming: Option<&Value>,
    current: Option<&Value>,
    depth: usize,
) -> Option<Value> {
    if incoming == base {
        return current.cloned();
    }
    let objects = [base, incoming, current]
        .iter()
        .all(|v| v.is_none_or(Value::is_object));
    if depth == 0 || !objects || (incoming.is_none() && current.is_none()) {
        return incoming.cloned();
    }
    let empty = Map::new();
    let (b, i, c) = (
        as_map(base).unwrap_or(&empty),
        as_map(incoming).unwrap_or(&empty),
        as_map(current).unwrap_or(&empty),
    );
    let mut out = Map::new();
    let keys: std::collections::BTreeSet<&String> =
        b.keys().chain(i.keys()).chain(c.keys()).collect();
    for k in keys {
        if let Some(v) = merge_map(b.get(k), i.get(k), c.get(k), depth - 1) {
            out.insert(k.clone(), v);
        }
    }
    Some(Value::Object(out))
}

/// One item's fields: each the caller's, unless it didn't change it.
fn merge_fields(base: &Value, incoming: &Value, current: &Value) -> Value {
    merge_map(Some(base), Some(incoming), Some(current), 1).unwrap_or_else(|| incoming.clone())
}

/// Where an item of the merged list came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Id(String),
    /// An item the caller added without an id, by its incoming position.
    New(usize),
}

/// Merge a list item by item by its id (§4.4), then field by field.
fn merge_list(
    spec: &ListSpec,
    base: Option<&Value>,
    incoming: Option<&Value>,
    current: Option<&Value>,
) -> Option<Value> {
    if incoming == base {
        return current.cloned();
    }
    let (Some(Value::Array(ba)), Some(Value::Array(ia)), Some(Value::Array(ca))) =
        (base, incoming, current)
    else {
        return incoming.cloned();
    };
    let idf = spec.id_field;
    let id_of = |v: &Value| {
        v.get(idf)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let items = reidentify(idf, ba, ia);
    let keys: Vec<Key> = items
        .iter()
        .enumerate()
        .map(|(n, v)| id_of(v).map_or(Key::New(n), Key::Id))
        .collect();
    let bmap: HashMap<String, &Value> = ba
        .iter()
        .filter_map(|v| id_of(v).map(|id| (id, v)))
        .collect();
    let cmap: HashMap<String, &Value> = ca
        .iter()
        .filter_map(|v| id_of(v).map(|id| (id, v)))
        .collect();
    let in_incoming: HashSet<&Key> = keys.iter().collect();

    // The caller's items, merged. `None`: the item stays gone.
    let mut merged: HashMap<Key, Value> = HashMap::new();
    // Items the current list doesn't hold where the caller has them: new
    // here, or removed elsewhere and edited here.
    let mut placed_by_caller: HashSet<Key> = HashSet::new();
    for (key, item) in keys.iter().zip(&items) {
        let Key::Id(id) = key else {
            merged.insert(key.clone(), item.clone());
            placed_by_caller.insert(key.clone());
            continue;
        };
        match (bmap.get(id), cmap.get(id)) {
            (Some(b), Some(c)) => {
                merged.insert(key.clone(), merge_fields(b, item, c));
            }
            (None, Some(c)) => {
                // Added here and elsewhere under one id: the caller's copy.
                merged.insert(key.clone(), merge_fields(&Value::Null, item, c));
            }
            (None, None) => {
                merged.insert(key.clone(), item.clone());
                placed_by_caller.insert(key.clone());
            }
            (Some(b), None) => {
                // Removed elsewhere: it stays removed unless edited here.
                if item != *b {
                    merged.insert(key.clone(), item.clone());
                    placed_by_caller.insert(key.clone());
                }
            }
        }
    }
    // Items only in the current list: added elsewhere since the base stay;
    // the ones the caller removed go.
    let current_keys: Vec<Key> = ca.iter().filter_map(|v| id_of(v).map(Key::Id)).collect();
    for (key, v) in current_keys
        .iter()
        .zip(ca.iter().filter(|v| id_of(v).is_some()))
    {
        let Key::Id(id) = key else { continue };
        if !in_incoming.contains(key) && !bmap.contains_key(id) {
            merged.insert(key.clone(), v.clone());
        }
    }

    let common = |order: &[Key]| -> Vec<Key> {
        order
            .iter()
            .filter(|k| {
                matches!(k, Key::Id(id) if bmap.contains_key(id)) && in_incoming.contains(k)
            })
            .cloned()
            .collect()
    };
    let base_keys: Vec<Key> = ba.iter().filter_map(|v| id_of(v).map(Key::Id)).collect();
    let reordered = common(&base_keys) != common(&keys);
    let (skeleton, extra_order, extra_set): (&[Key], &[Key], HashSet<Key>) = if reordered {
        // The caller's order; items added elsewhere go after the item they
        // follow there.
        let added_elsewhere = current_keys
            .iter()
            .filter(|k| !in_incoming.contains(k))
            .cloned()
            .collect();
        (&keys, &current_keys, added_elsewhere)
    } else {
        // The current order; the caller's new items go after the item they
        // follow in the caller's list.
        (&current_keys, &keys, placed_by_caller.clone())
    };
    let extras = |k: &Key| extra_set.contains(k);
    let mut order: Vec<Key> = skeleton
        .iter()
        .filter(|k| merged.contains_key(k) && !extras(k))
        .cloned()
        .collect();
    // An extra item goes after the item it follows in its own list, or
    // last when nothing it precedes there is in the merged list.
    let mut prev: Option<Key> = None;
    for (j, k) in extra_order.iter().enumerate() {
        if extras(k) && merged.contains_key(k) && !order.contains(k) {
            let trailing = !extra_order[j + 1..]
                .iter()
                .any(|later| !extras(later) && order.contains(later));
            let at = if trailing {
                order.len()
            } else {
                prev.as_ref()
                    .and_then(|p| order.iter().position(|o| o == p))
                    .map_or(0, |i| i + 1)
            };
            order.insert(at, k.clone());
        }
        if order.contains(k) {
            prev = Some(k.clone());
        }
    }
    Some(Value::Array(
        order
            .into_iter()
            .filter_map(|k| merged.remove(&k))
            .collect(),
    ))
}

/// The caller's items, where an item an editor rebuilt without its id gets
/// the id back: from the base item equal to it in every other field, or
/// else from the base item at the same position of a list of the same
/// length with at most one other field changed (the §4.4 rule for lost
/// ids), as long as no other incoming item names that id.
fn reidentify(idf: &str, base: &[Value], incoming: &[Value]) -> Vec<Value> {
    let id_of = |v: &Value| {
        v.get(idf)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let named: HashSet<String> = incoming.iter().filter_map(id_of).collect();
    let mut free: Vec<(usize, &Value, String)> = base
        .iter()
        .enumerate()
        .filter_map(|(n, v)| {
            id_of(v)
                .filter(|id| !named.contains(id))
                .map(|id| (n, v, id))
        })
        .collect();
    let without_id = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove(idf);
        }
        v
    };
    let mut out = incoming.to_vec();
    for (pos, item) in out.iter_mut().enumerate() {
        if id_of(item).is_some() || !item.is_object() {
            continue;
        }
        let bare = without_id(item);
        let found = free
            .iter()
            .position(|(_, b, _)| without_id(b) == bare)
            .or_else(|| {
                (incoming.len() == base.len())
                    .then(|| {
                        free.iter().position(|(n, b, _)| {
                            *n == pos && fields_changed(&without_id(b), &bare) <= 1
                        })
                    })
                    .flatten()
            });
        if let Some(k) = found {
            let (_, _, id) = free.remove(k);
            if let Some(o) = item.as_object_mut() {
                o.insert(idf.to_string(), Value::String(id));
            }
        }
    }
    out
}

/// How many top-level fields differ between two items.
fn fields_changed(a: &Value, b: &Value) -> usize {
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else {
        return usize::MAX;
    };
    let keys: HashSet<&String> = a.keys().chain(b.keys()).collect();
    keys.into_iter().filter(|k| a.get(*k) != b.get(*k)).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    fn tree(c: &Config) -> Value {
        serde_json::to_value(c).unwrap()
    }

    fn config(v: Value) -> Config {
        serde_json::from_value(v).unwrap()
    }

    fn shortcut(id: &str, name: &str) -> crate::config::ShortcutConfig {
        serde_json::from_value(json!({ "name": name, "url": "https://example.com", "sync_id": id }))
            .unwrap()
    }

    fn two_shortcuts() -> Config {
        Config {
            shortcuts: vec![shortcut("s1", "one"), shortcut("s2", "two")],
            ..Config::default()
        }
    }

    fn names(c: &Config) -> Vec<String> {
        c.shortcuts.iter().map(|s| s.name.clone()).collect()
    }

    /// I5 (backend half): a stale save doesn't revert a setting changed
    /// since the window loaded, and the window's own change still lands.
    #[test]
    fn a_stale_save_keeps_what_changed_elsewhere() {
        let base = Config::default();
        let mut current = base.clone();
        current.ui.theme = "dark".into(); // synced from another computer
        let mut incoming = base.clone();
        incoming.ui.font_size = 18; // the user's edit in a stale window
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(out.ui.theme, "dark");
        assert_eq!(out.ui.font_size, 18);
    }

    #[test]
    fn the_callers_change_wins_where_both_changed() {
        let base = Config::default();
        let mut current = base.clone();
        current.ui.font_size = 16;
        let mut incoming = base.clone();
        incoming.ui.font_size = 20;
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(out.ui.font_size, 20);
    }

    /// Fields the window never shows, such as the tool permissions a
    /// command changed meanwhile, keep their current value.
    #[test]
    fn backend_changes_survive_a_stale_save() {
        let base = Config::default();
        let mut current = base.clone();
        current.tool_permissions.terminator_mode = !base.tool_permissions.terminator_mode;
        current.telemetry.install_id = Some("abc".into());
        let mut incoming = base.clone();
        incoming.ui.show_time = !base.ui.show_time;
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(
            out.tool_permissions.terminator_mode,
            current.tool_permissions.terminator_mode
        );
        assert_eq!(out.telemetry.install_id.as_deref(), Some("abc"));
        assert_eq!(out.ui.show_time, incoming.ui.show_time);
    }

    #[test]
    fn extension_switches_and_settings_merge_per_key() {
        let mut base = Config::default();
        base.extension_states.insert("a".into(), true);
        base.extension_states.insert("b".into(), true);
        base.extensions
            .insert("a".into(), json!({ "city": "Paris", "units": "c" }));
        let mut current = base.clone();
        current.extension_states.insert("b".into(), false);
        current.extension_states.insert("c".into(), true);
        current
            .extensions
            .insert("a".into(), json!({ "city": "Rome", "units": "c" }));
        let mut incoming = base.clone();
        incoming.extension_states.insert("a".into(), false);
        incoming
            .extensions
            .insert("a".into(), json!({ "city": "Paris", "units": "f" }));
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(out.extension_states.get("a"), Some(&false), "the caller's");
        assert_eq!(
            out.extension_states.get("b"),
            Some(&false),
            "changed elsewhere"
        );
        assert_eq!(
            out.extension_states.get("c"),
            Some(&true),
            "added elsewhere"
        );
        assert_eq!(out.extensions["a"], json!({ "city": "Rome", "units": "f" }));
    }

    #[test]
    fn list_items_merge_by_id_and_field() {
        let base = two_shortcuts();
        let mut current = base.clone();
        current.shortcuts[1].url = Some("https://elsewhere".into());
        current.shortcuts.push(shortcut("s3", "three"));
        let mut incoming = base.clone();
        incoming.shortcuts[1].name = "TWO".into();
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(names(&out), ["one", "TWO", "three"]);
        assert_eq!(out.shortcuts[1].url.as_deref(), Some("https://elsewhere"));
    }

    #[test]
    fn a_removed_item_goes_and_a_new_one_lands_where_the_caller_put_it() {
        let base = two_shortcuts();
        let mut current = base.clone();
        current.shortcuts.push(shortcut("s3", "three"));
        let mut incoming = base.clone();
        incoming.shortcuts.remove(0);
        incoming.shortcuts.insert(0, shortcut("", "new"));
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(names(&out), ["new", "two", "three"]);
    }

    #[test]
    fn the_callers_reorder_wins_and_keeps_items_added_elsewhere() {
        let base = two_shortcuts();
        let mut current = base.clone();
        current.shortcuts.push(shortcut("s3", "three"));
        let mut incoming = base.clone();
        incoming.shortcuts.reverse();
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(names(&out), ["two", "one", "three"]);
    }

    #[test]
    fn a_new_item_keeps_its_place_among_the_others() {
        let base = two_shortcuts();
        let mut current = base.clone();
        current.shortcuts.push(shortcut("s3", "three"));
        let mut incoming = base.clone();
        incoming.shortcuts.insert(1, shortcut("", "new"));
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(names(&out), ["one", "new", "two", "three"]);
    }

    #[test]
    fn an_item_removed_elsewhere_stays_removed_unless_edited_here() {
        let base = two_shortcuts();
        let mut current = base.clone();
        current.shortcuts.clear();
        let mut incoming = base.clone();
        incoming.shortcuts[1].name = "TWO".into();
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        assert_eq!(names(&out), ["TWO"]);
    }

    /// An editor that rebuilt its items without `sync_id` still edits the
    /// items it showed, not new copies of them.
    #[test]
    fn items_rebuilt_without_ids_get_them_back() {
        let base = two_shortcuts();
        let current = base.clone();
        let mut incoming = base.clone();
        for s in &mut incoming.shortcuts {
            s.sync_id.clear();
        }
        incoming.shortcuts[1].name = "TWO".into();
        let out = config(merge(&tree(&base), &tree(&incoming), &tree(&current)));
        let ids: Vec<&str> = out.shortcuts.iter().map(|s| s.sync_id.as_str()).collect();
        assert_eq!(ids, ["s1", "s2"]);
        assert_eq!(names(&out), ["one", "TWO"]);
    }

    #[test]
    fn nothing_changed_means_the_current_config() {
        let base = Config::default();
        let mut current = base.clone();
        current.ui.theme = "dark".into();
        current.shortcuts = vec![shortcut("s9", "nine")];
        let out = merge(&tree(&base), &tree(&base), &tree(&current));
        assert_eq!(out, tree(&current));
    }
}
