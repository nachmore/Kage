//! How an import combines a full backup with this machine's settings and
//! files.
//!
//! Every import keeps this machine's value wherever the backup's is
//! missing, empty, or a placeholder an earlier build wrote
//! ([`PLACEHOLDER`]) — but only where the item is provably the same one, so
//! a local secret is never moved to a different command, site or server:
//!
//!   - **Kage's singleton settings** (`store_url`, `ui.*`, …): the same
//!     setting on the same machine. A string holding the placeholder
//!     keeps this machine's value only if the rest of it matches
//!     ([`template_matches`]: `npx __REMOVED…` is this machine's `npx …`;
//!     `https://evil/…?t=__REMOVED…` is not this machine's
//!     `https://store/…?t=…`).
//!   - **Agent connections and internal sites** (stable ids): the same `id`
//!     (unique on both sides), and the same connection type; for a site,
//!     everything but its name and script must be equal, since those decide
//!     where the script runs.
//!   - **Other lists** (shortcuts, automations, app modes, …): only an item
//!     that still holds a placeholder, and only when this machine has a
//!     twin of it (equal wherever the backup's copy has no placeholder). The
//!     local item then replaces the copy whole.
//!   - **An extension whose credential stays here**: when this machine
//!     holds a credential for an extension — a sensitive setting (declared,
//!     or named like one) or a data file holding one, or a data file Kage
//!     can't read (which may) — that the backup doesn't replace with its own
//!     (judged per setting and per file: an unrelated file in the backup
//!     replaces nothing; a file Kage can't read, never), the extension keeps
//!     this machine's settings and data files whole and takes none of the
//!     backup's ([`settings_credential_stays`], [`file_credential_stays`]),
//!     and the user is told. Nothing in the backup can then point that
//!     credential somewhere else, whatever its fields are called or hold.
//!     The same rule holds for a settings-only backup, which never carries
//!     a credential.
//!   - **Other extension settings**: merged by name, recursively; a missing
//!     or empty value is this machine's. None of them holds a credential of
//!     this machine's (that extension would be kept whole), so nothing put
//!     back can go somewhere the backup chose.
//!   - **Other extension data**: the backup's copy, unless it holds nothing
//!     and this machine's does.
//!
//! Anything else in the backup wins: it's a real value from the machine
//! that made it.

use super::allowlist::is_empty;
use super::sensitive::{Rules, PLACEHOLDER};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};

/// Whether `v` holds anything worth keeping, at any depth: a number, or a
/// string that isn't blank or the placeholder. Booleans and nulls are
/// settings, not content, so `{"token": ""}` (what an extension writes on
/// sign-out) holds nothing.
pub fn has_content(v: &Value) -> bool {
    match v {
        Value::Number(_) => true,
        Value::String(s) => !s.trim().is_empty() && !s.contains(PLACEHOLDER),
        Value::Array(a) => a.iter().any(has_content),
        Value::Object(o) => o.values().any(has_content),
        _ => false,
    }
}

/// Whether a file holds anything: not blank, and (if JSON) with content.
/// Anything that isn't JSON and isn't blank counts.
pub fn body_has_content(body: &[u8]) -> bool {
    if body.iter().all(u8::is_ascii_whitespace) {
        return false;
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v) => has_content(&v),
        Err(_) => true,
    }
}

/// Whether `v` holds the placeholder anywhere.
pub fn contains_placeholder(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains(PLACEHOLDER),
        Value::Array(a) => a.iter().any(contains_placeholder),
        Value::Object(o) => o
            .iter()
            .any(|(k, c)| k.contains(PLACEHOLDER) || contains_placeholder(c)),
        _ => false,
    }
}

/// Whether `local` is what `template` was before an earlier build replaced
/// parts of it with the placeholder: equal outside the placeholders. A
/// template that is only the placeholder matches any value.
pub fn template_matches(template: &str, local: &str) -> bool {
    if local.contains(PLACEHOLDER) {
        return false;
    }
    let parts: Vec<&str> = template.split(PLACEHOLDER).collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return template == local;
    };
    if local.len() < first.len() + last.len() || !local.starts_with(first) || !local.ends_with(last)
    {
        return false;
    }
    let mut rest = &local[first.len()..local.len() - last.len()];
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

// --- Kage's settings ------------------------------------------------------------

/// The hotkeys, each merged as one unit.
const HOTKEYS: &[&str] = &[
    "hotkey",
    "clipboard_hotkey",
    "inline_assist_hotkey",
    "voice_hotkey",
];

/// Top-level keys the caller merges itself.
const CALLER_MERGES: &[&str] = &["extensions", "extension_grants", "version"];

/// Keep this machine's values in `merged` (the backup's config as JSON)
/// wherever the backup's are missing, empty or placeholders (see the module
/// docs). `extensions`, grants and tool-permission rules are the caller's.
pub fn host(merged: &mut Value, local: &Value) {
    node(merged, local, &mut Vec::new());
}

fn node(m: &mut Value, l: &Value, path: &mut Vec<String>) {
    if is_empty(l) || is_path(path, &["tool_permissions", "tools"]) {
        return;
    }
    if is_empty(m) {
        *m = l.clone();
        return;
    }
    if path.len() == 1 && HOTKEYS.contains(&path[0].as_str()) {
        // A hotkey is its key and modifiers together: a backup's key comes
        // with its own modifiers (none included); without a key, this
        // machine's hotkey stays whole.
        if m.get("key").is_none_or(is_empty) {
            *m = l.clone();
        }
        return;
    }
    match (m, l) {
        (Value::String(ms), Value::String(ls))
            if ms.contains(PLACEHOLDER) && template_matches(ms, ls) =>
        {
            *ms = ls.clone();
        }
        (Value::Object(mo), Value::Object(lo)) => {
            for (k, lv) in lo {
                if path.is_empty() && CALLER_MERGES.contains(&k.as_str()) {
                    continue;
                }
                path.push(k.clone());
                match mo.get_mut(k) {
                    Some(mv) => node(mv, lv, path),
                    None => {
                        if !is_empty(lv) && !is_path(path, &["tool_permissions", "tools"]) {
                            mo.insert(k.clone(), lv.clone());
                        }
                    }
                }
                path.pop();
            }
        }
        (Value::Array(ma), Value::Array(la)) => array(ma, la, path),
        _ => {}
    }
}

fn is_path(path: &[String], want: &[&str]) -> bool {
    path.len() == want.len() && path.iter().zip(want).all(|(a, b)| a == b)
}

fn array(ma: &mut [Value], la: &[Value], path: &mut Vec<String>) {
    if is_path(path, &["acp", "connections"]) {
        by_id(ma, la, path, same_connection);
    } else if is_path(path, &["internal_sites"]) {
        by_id(ma, la, path, same_site);
    } else {
        twins(ma, la, path);
    }
}

fn id_of(v: &Value) -> Option<&str> {
    v.get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn str_at<'v>(v: &'v Value, path: &[&str]) -> Option<&'v str> {
    let mut cur = v;
    for seg in path {
        cur = cur.get(seg)?;
    }
    cur.as_str()
}

/// The same kind of connection (a command can only be kept for a local
/// one; each placeholder must still match on its own).
fn same_connection(m: &Value, l: &Value) -> bool {
    let (mt, lt) = (str_at(m, &["mode", "type"]), str_at(l, &["mode", "type"]));
    mt.is_some() && mt == lt
}

/// The same pages, handled the same way: everything but the name and the
/// script is equal (placeholders matching this machine's values).
fn same_site(m: &Value, l: &Value) -> bool {
    let ignore = |p: &[String]| is_path(p, &["name"]) || is_path(p, &["proxy", "script"]);
    equal_modulo(m, l, &mut Vec::new(), &ignore)
}

fn by_id(
    ma: &mut [Value],
    la: &[Value],
    path: &mut Vec<String>,
    verify: fn(&Value, &Value) -> bool,
) {
    let mut local_ids: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, le) in la.iter().enumerate() {
        if let Some(id) = id_of(le) {
            local_ids.entry(id).or_default().push(j);
        }
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for me in ma.iter() {
        if let Some(id) = id_of(me) {
            *counts.entry(id.to_string()).or_default() += 1;
        }
    }
    for (i, me) in ma.iter_mut().enumerate() {
        let Some(id) = id_of(me).map(str::to_string) else {
            continue;
        };
        // An id that isn't unique on either side isn't an identity.
        if counts.get(&id) != Some(&1) {
            continue;
        }
        let Some([j]) = local_ids.get(id.as_str()).map(Vec::as_slice) else {
            continue;
        };
        let le = &la[*j];
        if !verify(me, le) {
            continue;
        }
        path.push(i.to_string());
        node(me, le, path);
        path.pop();
    }
}

fn twins(ma: &mut [Value], la: &[Value], path: &[String]) {
    let mut used = vec![false; la.len()];
    for me in ma.iter_mut() {
        if !contains_placeholder(me) {
            continue;
        }
        let ignore = ignore_for(path);
        for (j, le) in la.iter().enumerate() {
            if !used[j] && has_content(le) && twin(me, le, path, ignore) {
                *me = le.clone();
                used[j] = true;
                break;
            }
        }
    }
}

/// Fields two machines' copies of the same item may differ in.
fn ignore_for(path: &[String]) -> fn(&[String]) -> bool {
    if is_path(path, &["macros"]) {
        |p: &[String]| is_path(p, &["trigger", "last_run"])
    } else {
        |_: &[String]| false
    }
}

fn twin(m: &Value, l: &Value, path: &[String], ignore: fn(&[String]) -> bool) -> bool {
    if is_path(path, &["context_rules"]) {
        // App modes match their program by meaning, not spelling.
        let canon = |v: &Value| {
            let mut v = v.clone();
            if let Some(exe) = v.get_mut("executable") {
                if let Some(s) = exe.as_str() {
                    let lower = s.trim().to_lowercase();
                    *exe = Value::String(lower.strip_suffix(".exe").unwrap_or(&lower).to_string());
                }
            }
            v
        };
        return equal_modulo(&canon(m), &canon(l), &mut Vec::new(), &ignore);
    }
    equal_modulo(m, l, &mut Vec::new(), &ignore)
}

/// Whether `m` (a backup's copy) is `l` (this machine's) wherever `m` has
/// no placeholder, outside the paths `ignore` names.
fn equal_modulo(
    m: &Value,
    l: &Value,
    path: &mut Vec<String>,
    ignore: &dyn Fn(&[String]) -> bool,
) -> bool {
    if ignore(path) {
        return true;
    }
    match (m, l) {
        (Value::String(ms), Value::String(ls)) => {
            ms == ls || (ms.contains(PLACEHOLDER) && template_matches(ms, ls))
        }
        (Value::Object(mo), Value::Object(lo)) => {
            let null = Value::Null;
            mo.keys().chain(lo.keys()).all(|k| {
                path.push(k.clone());
                let same = equal_modulo(
                    mo.get(k).unwrap_or(&null),
                    lo.get(k).unwrap_or(&null),
                    path,
                    ignore,
                );
                path.pop();
                same
            })
        }
        (Value::Array(ma), Value::Array(la)) => {
            ma.len() == la.len()
                && ma.iter().zip(la).enumerate().all(|(i, (a, b))| {
                    path.push(i.to_string());
                    let same = equal_modulo(a, b, path, ignore);
                    path.pop();
                    same
                })
        }
        _ => m == l,
    }
}

/// Every placeholder still in `v`, by path (Kage's own settings; the
/// caller reports them for the user to re-enter).
pub fn placeholder_paths(v: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match v {
        Value::String(s) if s.contains(PLACEHOLDER) => out.push(path.clone()),
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                path.push(i.to_string());
                placeholder_paths(c, path, out);
                path.pop();
            }
        }
        Value::Object(o) => {
            for (k, c) in o {
                if path.is_empty() && k == "extensions" {
                    continue;
                }
                path.push(k.clone());
                placeholder_paths(c, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

// --- Extension settings and data --------------------------------------------------

/// Whether the backup's copy of a value counts as absent for keeping this
/// machine's: empty, or holding the placeholder.
fn absent(v: Option<&Value>) -> bool {
    v.is_none_or(|v| is_empty(v) || v.as_str().is_some_and(|s| s.contains(PLACEHOLDER)))
}

/// Whether `v` holds a credential-named value with content anywhere below.
pub fn has_secret_below(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.iter().any(|(k, c)| {
            (crate::extensions::is_sensitive_name(k) && has_content(c)) || has_secret_below(c)
        }),
        Value::Array(a) => a.iter().any(has_secret_below),
        _ => false,
    }
}

/// The values [`keep_local`] puts back: where, and what.
fn plan(
    mo: &Map<String, Value>,
    lo: &Map<String, Value>,
    path: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, Value)>,
) {
    for (k, lv) in lo {
        if path.is_empty() && super::sensitive::is_host_managed(k) {
            continue;
        }
        let mv = mo.get(k);
        if !is_empty(lv) && absent(mv) {
            path.push(k.clone());
            out.push((path.clone(), lv.clone()));
            path.pop();
            continue;
        }
        if let (Some(Value::Object(mc)), Value::Object(lc)) = (mv, lv) {
            path.push(k.clone());
            plan(mc, lc, path, out);
            path.pop();
        }
    }
}

/// Put `value` at `path` under `obj`, through objects that are there.
fn insert_at(obj: &mut Map<String, Value>, path: &[String], value: Value) {
    match path {
        [] => {}
        [last] => {
            obj.insert(last.clone(), value);
        }
        [first, rest @ ..] => {
            if let Some(Value::Object(next)) = obj.get_mut(first) {
                insert_at(next, rest, value);
            }
        }
    }
}

/// Put `local`'s values back in `merged` (one extension's settings from
/// the backup, at any depth) wherever `merged`'s are absent. Only for an
/// extension that keeps no credential here ([`extensions`] keeps the others
/// whole), so nothing put back is a credential of this machine's.
fn keep_local(mo: &mut Map<String, Value>, lo: &Map<String, Value>) {
    let mut restores = Vec::new();
    plan(mo, lo, &mut Vec::new(), &mut restores);
    for (path, value) in restores {
        insert_at(mo, &path, value);
    }
}

/// Whether this machine's credential in an extension's settings (`lo`)
/// stays here through an import because the backup's settings (`mo`;
/// `None` for a settings-only backup, which never carries one) don't carry
/// their own in its place: a sensitive setting (declared, or named like
/// one) with content, at any depth, that the backup lacks (in an earlier
/// build's "leave out" backup, which never carried one, any this machine
/// has).
pub fn settings_credential_stays(
    id: &str,
    mo: Option<&Value>,
    lo: &Value,
    rules: &Rules,
    leave_out: bool,
) -> bool {
    let Value::Object(lo) = lo else {
        return false;
    };
    let mo = match mo {
        Some(Value::Object(mo)) => mo.clone(),
        // A backup without this extension's settings (or with a value
        // that isn't settings) keeps this machine's whole anyway.
        _ => Map::new(),
    };
    let top = |k: &str| rules.is_sensitive_setting(id, k);
    credential_missing(
        &mo,
        lo,
        &top,
        &crate::extensions::is_sensitive_name,
        leave_out,
        true,
    )
}

/// Whether `lo` holds a credential `mo` doesn't carry itself: walked the
/// way [`plan`] walks, a secret (`top` names them at the top level,
/// `nested` below) with content that `mo` lacks, or a value `mo` lacks
/// with a secret below it.
fn credential_missing(
    mo: &Map<String, Value>,
    lo: &Map<String, Value>,
    top: &dyn Fn(&str) -> bool,
    nested: &dyn Fn(&str) -> bool,
    leave_out: bool,
    at_top: bool,
) -> bool {
    lo.iter().any(|(k, lv)| {
        if at_top && super::sensitive::is_host_managed(k) {
            return false;
        }
        let secret = if at_top { top(k) } else { nested(k) };
        let mv = mo.get(k);
        let local_only = absent(mv) || (leave_out && secret && mv != Some(lv));
        let credential = if secret {
            has_content(lv)
        } else {
            has_secret_below(lv)
        };
        if local_only && credential {
            return true;
        }
        match (mv, lv) {
            (Some(Value::Object(mc)), Value::Object(lc)) => {
                credential_missing(mc, lc, top, nested, leave_out, false)
            }
            _ => false,
        }
    })
}

/// Whether the backup's copy of a value (`m`) carries anything this
/// machine's (`l`) doesn't already have: a value that isn't empty or a
/// placeholder and differs from this machine's.
pub fn carries_other_values(m: &Value, l: Option<&Value>) -> bool {
    if absent(Some(m)) {
        return false;
    }
    match (m, l) {
        (Value::Object(mo), Some(Value::Object(lo))) => {
            mo.iter().any(|(k, c)| carries_other_values(c, lo.get(k)))
        }
        (Value::Object(mo), _) => mo.values().any(|c| carries_other_values(c, None)),
        (_, Some(l)) => m != l,
        (_, None) => true,
    }
}

/// Merge the backup's extension settings (`merged`) with this machine's
/// (`local`; see the module docs). An extension in `keep_whole` (folded
/// ids: its credential stays on this machine) keeps this machine's settings
/// whole, under whichever spelling of its id the backup used: none of the
/// backup's are taken. Returns the ids of those whose backup settings
/// carried something else, so the user is told.
pub fn extensions(
    merged: &mut Map<String, Value>,
    local: &Map<String, Value>,
    keep_whole: &BTreeSet<String>,
) -> Vec<String> {
    use super::sensitive::fold;
    let whole = |id: &str| keep_whole.contains(&fold(id));
    let local_of = |id: &str| {
        local
            .iter()
            .find(|(k, _)| fold(k) == fold(id))
            .map(|(k, v)| (k.clone(), v))
    };
    let mut kept: Vec<String> = Vec::new();
    let backup_ids: Vec<String> = merged.keys().filter(|id| whole(id)).cloned().collect();
    for id in backup_ids {
        let Some(mv) = merged.remove(&id) else {
            continue;
        };
        let (name, lv) = match local_of(&id) {
            Some((k, v)) => (k, Some(v)),
            None => (id.clone(), None),
        };
        if carries_other_values(&mv, lv) && !kept.iter().any(|k| fold(k) == fold(&name)) {
            kept.push(name);
        }
    }
    for (id, lv) in local {
        if whole(id) {
            merged.insert(id.clone(), lv.clone());
            continue;
        }
        if is_empty(lv) {
            continue;
        }
        let Some(mv) = merged.get_mut(id) else {
            // A backup without this extension's settings: keep them all.
            merged.insert(id.clone(), lv.clone());
            continue;
        };
        // Settings are an object; anything else in the backup's place
        // (hand-made) holds none of them.
        if absent(Some(mv)) || (lv.is_object() && !mv.is_object()) {
            *mv = lv.clone();
            continue;
        }
        if let (Value::Object(mo), Value::Object(lo)) = (mv, lv) {
            keep_local(mo, lo);
        }
    }
    kept
}

/// What import does with one extension-data file from a backup.
#[derive(Debug, PartialEq, Eq)]
pub enum FileAction {
    /// Write the backup's copy, as it is.
    Replace(Vec<u8>),
    /// Leave this machine's file as it is.
    KeepLocal,
}

/// Whether this machine's data file holds a credential: one its extension
/// declares sensitive (`declared`) with content, or JSON with a
/// credential-named value.
pub fn holds_credential(body: &[u8], declared: bool) -> bool {
    (declared && body_has_content(body))
        || serde_json::from_slice::<Value>(body).is_ok_and(|v| has_secret_below(&v))
}

/// Whether the backup's copy of a data file leaves this machine's
/// credential in it behind: the copy holds nothing, or (JSON objects both)
/// lacks a credential this machine's has — every key, for a declared file.
/// A copy of another shape with content is the backup's own.
pub fn file_credential_stays(incoming: &[u8], local: &[u8], declared: bool) -> bool {
    if !holds_credential(local, declared) {
        return false;
    }
    if !body_has_content(incoming) {
        return true;
    }
    let (Ok(Value::Object(mo)), Ok(Value::Object(lo))) = (
        serde_json::from_slice::<Value>(incoming),
        serde_json::from_slice::<Value>(local),
    ) else {
        return false;
    };
    let secret = |k: &str| declared || crate::extensions::is_sensitive_name(k);
    credential_missing(&mo, &lo, &secret, &secret, false, true)
}

/// What to write for a backup's data file, given this machine's copy: a
/// copy holding nothing never replaces one that holds something, and one
/// that would leave this machine's credential behind is not written (the
/// caller then keeps the whole extension: [`file_credential_stays`]).
/// Otherwise the backup's copy, as it is.
pub fn data_file(incoming: Vec<u8>, local: Option<&[u8]>, declared: bool) -> FileAction {
    let Some(local) = local.filter(|l| body_has_content(l)) else {
        return FileAction::Replace(incoming);
    };
    if !body_has_content(&incoming) || file_credential_stays(&incoming, local, declared) {
        return FileAction::KeepLocal;
    }
    FileAction::Replace(incoming)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const P: &str = PLACEHOLDER;

    #[test]
    fn templates_match_only_the_same_destination() {
        assert!(template_matches(P, "anything"));
        assert!(template_matches(
            &format!("npx {P}"),
            "npx -y @zed/claude-code-acp --key k1"
        ));
        assert!(!template_matches(&format!("evil {P}"), "npx -y x"));
        let url = format!("https://me:{P}@store.example.com/x.json");
        assert!(template_matches(
            &url,
            "https://me:pw@store.example.com/x.json"
        ));
        assert!(!template_matches(
            &url,
            "https://me:pw@evil.example.com/x.json"
        ));
        assert!(!template_matches(
            &format!("https://evil.example/?t={P}"),
            "https://store.example/?t=abc"
        ));
        assert!(
            !template_matches(P, &format!("x {P}")),
            "a placeholder isn't a value"
        );
        assert!(template_matches(&format!("a {P} b {P} c"), "a 1 b 2 c"));
        assert!(!template_matches(&format!("a {P} b {P} c"), "a 1 c"));
    }

    #[test]
    fn missing_empty_and_placeholder_values_keep_this_machines() {
        let local = json!({
            "store_url": "https://me:pw@store.example.com/x.json",
            "ui": { "language": "ja", "theme": "dark" },
            "internal_sites": [{ "id": "s1" }],
            "shortcuts": [{ "name": "a" }]
        });
        let mut merged = json!({
            "store_url": format!("https://me:{P}@store.example.com/x.json"),
            "ui": { "language": null },
            "shortcuts": []
        });
        host(&mut merged, &local);
        assert_eq!(merged["store_url"], local["store_url"]);
        assert_eq!(merged["ui"]["language"], "ja");
        assert_eq!(merged["ui"]["theme"], "dark", "missing");
        assert_eq!(
            merged["internal_sites"], local["internal_sites"],
            "missing list"
        );
        assert_eq!(merged["shortcuts"], local["shortcuts"], "empty list");
        // A real value wins.
        let mut merged = json!({ "ui": { "language": "fr" } });
        host(&mut merged, &local);
        assert_eq!(merged["ui"]["language"], "fr");
        // A hotkey goes whole: a bare F13 stays bare.
        let local = json!({ "hotkey": { "modifiers": ["Alt"], "key": "Space" } });
        let mut merged = json!({ "hotkey": { "modifiers": [], "key": "F13" } });
        host(&mut merged, &local);
        assert_eq!(merged["hotkey"], json!({ "modifiers": [], "key": "F13" }));
        let mut merged = json!({ "hotkey": { "modifiers": ["Ctrl"], "key": "" } });
        host(&mut merged, &local);
        assert_eq!(merged["hotkey"], local["hotkey"]);
    }

    #[test]
    fn a_connection_keeps_its_command_only_for_the_same_id_type_and_program() {
        let local = json!({ "acp": { "connections": [{ "id": "c", "mode": { "type": "local",
            "spawn_command": "npx -y @zed-industries/claude-code-acp --api-key k1" } }] } });
        for (cmd, kept) in [
            (format!("npx {P}"), true),
            (P.to_string(), true),
            (String::new(), true),
            (format!("evil-agent {P}"), false),
        ] {
            let mut merged = json!({ "acp": { "connections": [{ "id": "c", "mode": { "type": "local",
                "spawn_command": cmd } }] } });
            host(&mut merged, &local);
            assert_eq!(
                merged == local,
                kept,
                "{}: {}",
                cmd,
                merged["acp"]["connections"][0]["mode"]["spawn_command"]
            );
        }
        // Another id, or a duplicated one: nothing is kept.
        let mut merged = json!({ "acp": { "connections": [
            { "id": "d", "mode": { "type": "local", "spawn_command": P } }] } });
        host(&mut merged, &local);
        assert!(!merged.to_string().contains("k1"));
        let mut merged = json!({ "acp": { "connections": [
            { "id": "c", "mode": { "type": "local", "spawn_command": P } },
            { "id": "c", "mode": { "type": "local", "spawn_command": P } }] } });
        host(&mut merged, &local);
        assert!(!merged.to_string().contains("k1"));
    }

    fn site(host_name: &str, script: &str) -> Value {
        json!({ "id": "s1", "name": "Wiki", "url_rules": [{ "kind": "domain_suffix", "pattern": host_name }],
                "content_rules": [], "sample_url": "", "enabled": true,
                "proxy": { "lang": "bash", "script": script, "timeout_ms": 0 } })
    }

    #[test]
    fn a_site_keeps_its_script_only_while_it_handles_the_same_pages() {
        let local = json!({ "internal_sites": [site("wiki.example.com", "curl -H 'Authorization: Bearer local123'")] });
        for script in [P, "", "  "] {
            let mut merged = json!({ "internal_sites": [site("wiki.example.com", script)] });
            merged["internal_sites"][0]["name"] = json!("Docs");
            host(&mut merged, &local);
            assert!(merged.to_string().contains("local123"), "{:?}", script);
        }
        for (field, value) in [
            (
                "content_rules",
                json!([{ "target": "url", "pattern": "." }]),
            ),
            ("sample_url", json!("https://attacker.example/collect")),
            ("enabled", json!(false)),
        ] {
            let mut merged = json!({ "internal_sites": [site("wiki.example.com", P)] });
            merged["internal_sites"][0][field] = value;
            host(&mut merged, &local);
            assert!(!merged.to_string().contains("local123"), "{}", field);
        }
        let mut merged = json!({ "internal_sites": [site("evil.example.com", P)] });
        host(&mut merged, &local);
        assert!(!merged.to_string().contains("local123"));
    }

    #[test]
    fn list_items_come_back_only_as_exact_twins() {
        let local = json!({ "shortcuts": [
            { "name": "deploy", "action_type": "run_program", "path": "deploy", "arguments": "--token abc" },
            { "name": "other", "action_type": "run_program", "path": "x", "arguments": "--token zzz" }
        ]});
        let mut merged = json!({ "shortcuts": [
            { "name": "deploy", "action_type": "run_program", "path": "deploy", "arguments": P },
            { "name": "renamed", "action_type": "run_program", "path": "x", "arguments": P }
        ]});
        host(&mut merged, &local);
        assert_eq!(merged["shortcuts"][0], local["shortcuts"][0]);
        assert_eq!(merged["shortcuts"][1]["arguments"], P, "not a twin");
        let mut out = Vec::new();
        placeholder_paths(&merged, &mut Vec::new(), &mut out);
        assert_eq!(
            out,
            vec![vec![
                "shortcuts".to_string(),
                "1".into(),
                "arguments".into()
            ]]
        );
    }

    fn rules() -> Rules {
        Rules::new(&[crate::extensions::InstalledItem {
            manifest: serde_json::from_value(json!({
                "id": "github", "name": "GitHub", "version": "1", "type": "extension",
                "config": { "pat": { "type": "string", "sensitive": true } }
            }))
            .unwrap(),
            path: String::new(),
            enabled: true,
        }])
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    /// `extensions` the way a full import calls it: the extensions whose
    /// settings credential stays here are kept whole.
    fn merge_settings(merged: &mut Map<String, Value>, local: &Map<String, Value>) -> Vec<String> {
        let keep: BTreeSet<String> = local
            .iter()
            .filter(|(id, lv)| settings_credential_stays(id, merged.get(*id), lv, &rules(), false))
            .map(|(id, _)| super::super::sensitive::fold(id))
            .collect();
        extensions(merged, local, &keep)
    }

    #[test]
    fn extension_settings_without_a_credential_here_merge_by_name() {
        // No credential here: missing values are this machine's, wherever
        // the backup's object points.
        let local = obj(json!({ "github": { "trigger": "gh", "units": "m" },
                                "vault": { "endpoint": "https://vault-b.corp.example", "region": "eu" },
                                "gone": { "x": 1 } }));
        let mut merged = obj(json!({ "github": { "trigger": "gh2" } }));
        let kept = merge_settings(&mut merged, &local);
        assert!(kept.is_empty());
        assert_eq!(merged["github"], json!({ "trigger": "gh2", "units": "m" }));
        assert_eq!(merged["vault"], local["vault"], "missing extension");
        assert_eq!(merged["gone"], local["gone"]);
        let mut merged = obj(json!({ "vault": { "endpoint": "https://evil.example" } }));
        assert!(merge_settings(&mut merged, &local).is_empty());
        assert_eq!(
            merged["vault"],
            json!({ "endpoint": "https://evil.example", "region": "eu" })
        );
        // Settings are an object: anything else keeps this machine's.
        let mut merged = obj(json!({ "github": 0 }));
        merge_settings(&mut merged, &local);
        assert_eq!(merged["github"], local["github"]);
    }

    #[test]
    fn an_extension_kept_whole_takes_nothing_under_any_spelling_of_its_id() {
        let local = obj(json!({ "github": { "pat": "ghp_local", "server": "https://a.corp" } }));
        let mut merged = obj(json!({
            "github": { "server": "https://evil.example" },
            "GitHub": { "server": "https://evil.example" }
        }));
        let kept = merge_settings(&mut merged, &local);
        assert_eq!(merged, local);
        assert_eq!(kept, vec!["github".to_string()]);
    }

    #[test]
    fn a_credential_that_stays_here_keeps_its_extensions_settings_whole() {
        let local = obj(json!({
            "github": { "trigger": "gh", "pat": "ghp_local", "units": "m" },
            "vault": { "endpoint": "https://vault-b.corp.example", "api_key": "vaultLocal" },
            "a": { "endpoint": "https://a.corp", "auth": { "refresh_token": "R" } },
            "b": { "authority": "https://login.example.com/contoso", "refresh_token": "R" },
            "spelled": { "server": { "url": "https://vault.corp" }, "api_key": "K" }
        }));
        // However the backup points it — by a field no heuristic names,
        // a URL, a nested object, a key spelled like a nested path — the
        // token stays with this machine's settings, and the user is told.
        let mut merged = obj(json!({
            "github": { "trigger": "gh2", "target": "evil.example" },
            "vault": { "endpoint": "https://evil.example" },
            "a": { "endpoint": "https://evil.example", "auth": { "scope": "x" } },
            "b": { "authority": "https://evil.example" },
            "spelled": { "server": { "url": "https://evil.example" },
                         "server\u{1f}url": "https://vault.corp" }
        }));
        let mut kept = merge_settings(&mut merged, &local);
        assert_eq!(merged, local);
        kept.sort();
        assert_eq!(kept, vec!["a", "b", "github", "spelled", "vault"]);
        // Empty or missing values never replace a sign-in, and a backup
        // with nothing else to say isn't reported.
        for incoming in [
            json!({ "pat": "", "trigger": "gh" }),
            json!({ "trigger": "gh" }),
        ] {
            let mut merged = obj(json!({ "github": incoming }));
            let kept = merge_settings(&mut merged, &local);
            assert_eq!(merged["github"], local["github"]);
            assert!(kept.is_empty(), "{:?}", kept);
        }
        // A backup carrying its own token replaces this machine's settings.
        let mut merged = obj(json!({ "github": { "pat": "ghp_backup", "trigger": "x" } }));
        merge_settings(&mut merged, &local);
        assert_eq!(
            merged["github"],
            json!({ "pat": "ghp_backup", "trigger": "x", "units": "m" })
        );
        // An earlier "leave out" backup never carried one: this machine's wins.
        assert!(settings_credential_stays(
            "github",
            Some(&json!({ "pat": "ghp_handmade" })),
            &local["github"],
            &rules(),
            true
        ));
    }

    #[test]
    fn a_data_file_credential_stays_unless_the_backups_copy_carries_its_own() {
        let local = br#"{"token":"local-secret","items":[1]}"#;
        for empty in [&b""[..], b"  ", b"{}", b"null", br#"{"token":""}"#] {
            assert_eq!(
                data_file(empty.to_vec(), Some(local), false),
                FileAction::KeepLocal,
                "{:?}",
                String::from_utf8_lossy(empty)
            );
        }
        // A copy without the token leaves this machine's file whole.
        assert!(file_credential_stays(br#"{"items":[2]}"#, local, false));
        assert_eq!(
            data_file(br#"{"items":[2]}"#.to_vec(), Some(local), false),
            FileAction::KeepLocal
        );
        // Pointing somewhere else changes nothing: the token isn't moved.
        let moved = br#"{"server":"https://evil.example","items":[2]}"#;
        assert_eq!(
            data_file(moved.to_vec(), Some(local), false),
            FileAction::KeepLocal
        );
        // A copy with its own token is written as it is.
        let own = br#"{"token":"new"}"#;
        assert!(!file_credential_stays(own, local, false));
        assert_eq!(
            data_file(own.to_vec(), Some(local), false),
            FileAction::Replace(own.to_vec())
        );
        // Nothing here, or nothing secret here: written.
        assert_eq!(
            data_file(b"{}".to_vec(), None, false),
            FileAction::Replace(b"{}".to_vec())
        );
        let cache = br#"{"items":[1]}"#;
        assert_eq!(
            data_file(br#"{"items":[3]}"#.to_vec(), Some(cache), false),
            FileAction::Replace(br#"{"items":[3]}"#.to_vec())
        );
        // A token nested under a parent the copy lacks (or left null) stays.
        let local = br#"{"accounts":{"work":{"refresh_token":"R"}},"items":[1]}"#;
        for incoming in [
            &br#"{"items":[2]}"#[..],
            br#"{"accounts":null,"items":[2]}"#,
        ] {
            assert!(file_credential_stays(incoming, local, false));
        }
        // A declared file: every value this machine has is its credential.
        let local = br#"{"k":"opaque-local","expires":1}"#;
        assert!(file_credential_stays(br#"{"expires":2}"#, local, true));
        assert!(!file_credential_stays(
            br#"{"k":"other","expires":2}"#,
            local,
            true
        ));
        assert!(holds_credential(b"opaque bytes", true));
        assert!(!holds_credential(b"opaque bytes", false));
        // Not JSON: a copy with content is the backup's own.
        assert!(!file_credential_stays(
            b"other bytes",
            b"opaque bytes",
            true
        ));
    }
}
