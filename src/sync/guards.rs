//! Guards: synced items that start programs wait for approval, bulk
//! changes ask first, a reset config isn't captured, and churn freezes a
//! value (docs/design/SYNC.md §8.10). Also the "held back" test of §4.3:
//! values that look like they hold a key stay on this computer.
//!
//! | Item | Runnable part | Held means |
//! |---|---|---|
//! | Agent connection that is the active one | `mode` (a local spawn command) | the previous command stays |
//! | `acp.active_connection_id` naming one whose command isn't approved | the target's `mode` | the previous active connection stays |
//! | Internal site | `proxy` (script and language) | the profile is off here |
//! | Automation with a schedule or signal trigger | `trigger` (without `last_run`) and `steps` | the automation is off here |
//!
//! A held value is a `needs_approval` pin, so the hold never travels to the
//! folder. `held.<ns>/<id>` meta lists it for review; "Keep off" moves the
//! hash to `kept_off.<ns>/<id>` so the same version doesn't ask again.
//! Approval stores `SHA-256(runnable part)` in
//! `device.json.approved_runnables`; whatever this computer itself runs
//! (captured from its own files while switched on) counts as approved.

use super::canonical::{canonical, same};
use super::catalog::{self, CFG_NS};
use super::conflicts;
use super::engine::SyncEngine;
use super::merge::is_visible;
use super::replica::Tx;
use super::sources::config::{EXTENSION_STATE, FIELD};
use super::sources::LocalRecord;
use super::types::{LocalKey, OsKind, PinReason, Record};
use crate::config::Config;
use crate::config_export::sensitive::{looks_like_secret, secret_spans, Rules};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// More deletes than this in one tick ask first (§8.10).
pub const MASS_DELETES: usize = 50;
/// Or more than this share of a namespace…
pub const MASS_SHARE: f64 = 0.2;
/// …once at least this many.
pub const MASS_MIN: usize = 10;

/// Whether `deletes` out of `count` records is a mass change.
pub fn is_mass_change(deletes: usize, count: u64) -> bool {
    deletes > MASS_DELETES
        || (deletes >= MASS_MIN && count > 0 && deletes as f64 > MASS_SHARE * count as f64)
}

/// Whether a value stays on this computer because it looks like it holds
/// a credential (§4.3 "Held back"). Uses `config_export::sensitive`, so
/// sync and backups agree.
pub fn held_back(ns: &str, id: &str, name: &str, v: &Value) -> bool {
    if ns == CFG_NS {
        return catalog::cfg_row(id).is_some_and(|r| r.held_back)
            && v.as_str().is_some_and(looks_like_secret);
    }
    if ns == "cfg.ext" {
        return v.as_str().is_some_and(looks_like_secret);
    }
    let Some(l) = catalog::list_spec(ns) else {
        return false;
    };
    if !l.held_back.contains(&name) {
        return false;
    }
    match (ns, name) {
        ("list.store_sources", "url") => v.as_str().is_some_and(looks_like_secret),
        ("list.connections", "mode") => v
            .get("spawn_command")
            .and_then(Value::as_str)
            .is_some_and(|s| !secret_spans(s).is_empty()),
        ("list.internal_sites", "proxy") => v
            .get("script")
            .and_then(Value::as_str)
            .is_some_and(|s| !secret_spans(s).is_empty()),
        _ => false,
    }
}

/// The guard key of a list item: `<ns>/<id>`.
pub fn guard_key(ns: &str, id: &str) -> String {
    format!("{ns}/{id}")
}

/// The part of an item that runs a program by itself, if it has one.
pub fn runnable_of(ns: &str, fields: &serde_json::Map<String, Value>) -> Option<Value> {
    match ns {
        "list.macros" => {
            let trigger = fields.get("trigger")?;
            let kind = trigger.get("type").and_then(Value::as_str)?;
            if kind != "schedule" && kind != "signal" {
                return None;
            }
            let mut t = trigger.clone();
            if let Some(o) = t.as_object_mut() {
                o.remove("last_run");
            }
            Some(serde_json::json!({
                "trigger": t,
                "steps": fields.get("steps").cloned().unwrap_or(Value::Null),
            }))
        }
        "list.internal_sites" => {
            let proxy = fields.get("proxy")?;
            let script = proxy.get("script").and_then(Value::as_str).unwrap_or("");
            (!script.trim().is_empty()).then(|| proxy.clone())
        }
        "list.connections" => {
            let mode = fields.get("mode")?;
            (mode.get("type").and_then(Value::as_str) == Some("local")).then(|| mode.clone())
        }
        _ => None,
    }
}

/// `SHA-256(canonical runnable part)` as hex.
pub fn runnable_hash(v: &Value) -> String {
    hex::encode(Sha256::digest(canonical(v)))
}

/// Whether a local item runs here now: a switched-on automation or site,
/// or the active connection.
fn runs_here(
    ns: &str,
    id: &str,
    fields: &serde_json::Map<String, Value>,
    active: Option<&str>,
) -> bool {
    match ns {
        "list.connections" => active == Some(id),
        _ => fields
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    }
}

/// The approval a capture records: the hash of what this computer runs
/// from its own files, unless a hold is in place.
pub(crate) fn approval_of(
    ns: &str,
    id: &str,
    local: &LocalRecord,
    active: Option<&str>,
    approved: &BTreeMap<String, String>,
    tx: &Tx<'_>,
) -> anyhow::Result<Option<String>> {
    let Some(r) = runnable_of(ns, &local.fields) else {
        return Ok(None);
    };
    if !runs_here(ns, id, &local.fields, active) {
        return Ok(None);
    }
    let held = ["enabled", "mode"].iter().any(|f| {
        matches!(
            tx.pin(&LocalKey::new(ns, id, *f)),
            Ok(Some((_, PinReason::NeedsApproval)))
        )
    });
    if held {
        return Ok(None);
    }
    let h = runnable_hash(&r);
    Ok((approved.get(&guard_key(ns, id)) != Some(&h)).then_some(h))
}

/// Whether this computer's grant covers everything the installed
/// manifests for `id` request (`config_export::consent::keep_local`'s
/// rule). A theme runs no code, so its switch is the synced one.
pub(crate) fn grant_covers(
    config: &Config,
    installed: &[crate::extensions::InstalledItem],
    id: &str,
) -> bool {
    let code: Vec<&crate::extensions::InstalledItem> = installed
        .iter()
        .filter(|i| i.manifest.kind == "extension" && i.manifest.id == id)
        .collect();
    if code.is_empty() {
        return true;
    }
    let granted: BTreeSet<&str> = config
        .extension_grants
        .get(id)
        .map(|g| g.granted.iter().map(String::as_str).collect())
        .unwrap_or_default();
    code.iter().all(|item| {
        crate::extensions::normalize_permissions(
            item.manifest.permissions.as_deref().unwrap_or(&[]),
            id,
        )
        .iter()
        .all(|cap| granted.contains(cap.as_str()))
    })
}

/// Apply the guards to a record about to be projected: consent for an
/// extension's on/off, review for runnables, validation of settings.
/// Changes `target` and sets the pins that hold values here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply(
    tx: &mut Tx<'_>,
    ns: &str,
    id: &str,
    target: &mut LocalRecord,
    local: Option<&LocalRecord>,
    rec: &Record,
    local_cfg: &Config,
    _installed: &BTreeSet<String>,
    approved: &BTreeMap<String, String>,
    rules: &Rules,
    os: OsKind,
) -> anyhow::Result<()> {
    if ns == CFG_NS {
        let key = LocalKey::new(ns, id, FIELD);
        let pinned = tx.pin(&key)?;
        let Some(v) = target.fields.get(FIELD).cloned() else {
            return Ok(());
        };
        let local_v = local
            .and_then(|l| l.fields.get(FIELD))
            .cloned()
            .unwrap_or(Value::Null);
        // Validation (§8.11): a value this build can't use stays out.
        if pinned.is_none() && !valid_setting(id, &v, rules) {
            tx.set_pin(&key, Some((local_v.clone(), PinReason::InvalidHere)))?;
            target.fields.insert(FIELD.to_string(), local_v);
            return Ok(());
        }
        if let Some(ext) = id.strip_prefix(EXTENSION_STATE) {
            // Consent (§8.10): switched on here only if the grant covers it.
            let on = v == Value::Bool(true);
            let on_here = local_v == Value::Bool(true);
            if on && !on_here && pinned.is_none() && !consent_ok(local_cfg, ext) {
                tx.set_pin(&key, Some((Value::Bool(false), PinReason::Consent)))?;
                target.fields.insert(FIELD.to_string(), Value::Bool(false));
            }
        }
        let waiting = matches!(pinned, Some((_, PinReason::NeedsApproval)));
        if id == "acp.active_connection_id" && (pinned.is_none() || waiting) {
            // The previous active connection stays while the new one's
            // command isn't approved here. A pointer that waits follows
            // the synced value again once that is approved or points back.
            let synced = super::engine::eff_cell(rec, FIELD, os)
                .map(|c| c.value.clone())
                .unwrap_or_else(|| v.clone());
            match pointer_hold(tx, &synced, &local_v, approved, os)? {
                Some((gk, h)) => {
                    tx.set_pin(&key, Some((local_v.clone(), PinReason::NeedsApproval)))?;
                    target.fields.insert(FIELD.to_string(), local_v);
                    // Listed for review under the connection.
                    ask_review(tx, &gk, &h)?;
                }
                None if waiting => {
                    tx.set_pin(&key, None)?;
                    target.fields.insert(FIELD.to_string(), synced);
                }
                None => {}
            }
        }
        return Ok(());
    }
    let Some(r) = runnable_of(ns, &target.fields) else {
        return Ok(());
    };
    let key = guard_key(ns, id);
    let h = runnable_hash(&r);
    if approved.get(&key) == Some(&h) {
        // Approved: no hold left.
        for f in ["enabled", "mode"] {
            let k = LocalKey::new(ns, id, f);
            if matches!(tx.pin(&k)?, Some((_, PinReason::NeedsApproval))) {
                tx.set_pin(&k, None)?;
            }
        }
        return Ok(());
    }
    match ns {
        "list.macros" | "list.internal_sites" => {
            let k = LocalKey::new(ns, id, "enabled");
            hold_for_review(tx, &k, Value::Bool(false), &key, &h)?;
            target.fields.insert("enabled".into(), Value::Bool(false));
        }
        // Only the active connection runs by itself.
        "list.connections" if local_cfg.acp.active_connection_id == id => {
            if let Some(prev) = local.and_then(|l| l.fields.get("mode")).cloned() {
                if !same(&prev, &r) {
                    let k = LocalKey::new(ns, id, "mode");
                    hold_for_review(tx, &k, prev.clone(), &key, &h)?;
                    target.fields.insert("mode".into(), prev);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether the active pointer moving from `local` to `synced` waits for a
/// review: the connection it points to runs a command that isn't approved
/// here. Gives the guard key and the command's hash.
fn pointer_hold(
    tx: &mut Tx<'_>,
    synced: &Value,
    local: &Value,
    approved: &BTreeMap<String, String>,
    os: OsKind,
) -> anyhow::Result<Option<(String, String)>> {
    let Some(target_id) = synced.as_str() else {
        return Ok(None);
    };
    if Some(target_id) == local.as_str() {
        return Ok(None);
    }
    let Some(conn) = tx.record("list.connections", target_id)? else {
        return Ok(None);
    };
    let pins = super::engine::pins_of(tx, "list.connections", target_id)?;
    let spec =
        super::sources::lists::spec_of(catalog::list_spec("list.connections").expect("a list"));
    let eff = super::engine::effective_record(&spec, &conn, None, &pins, os);
    let Some(r) = runnable_of("list.connections", &eff.fields) else {
        return Ok(None);
    };
    let h = runnable_hash(&r);
    let gk = guard_key("list.connections", target_id);
    Ok((approved.get(&gk) != Some(&h)).then_some((gk, h)))
}

/// Hold a field with `value` (a `needs_approval` pin) and list the item
/// for review.
fn hold_for_review(
    tx: &mut Tx<'_>,
    k: &LocalKey,
    value: Value,
    guard: &str,
    hash: &str,
) -> anyhow::Result<()> {
    tx.set_pin(k, Some((value, PinReason::NeedsApproval)))?;
    ask_review(tx, guard, hash)
}

/// List `guard` for review, unless the user already chose to keep this
/// very version off here.
fn ask_review(tx: &mut Tx<'_>, guard: &str, hash: &str) -> anyhow::Result<()> {
    let kept_off = tx.meta(&format!("kept_off.{guard}"))?;
    if kept_off.as_deref() != Some(hash) {
        tx.set_meta(&format!("held.{guard}"), hash)?;
    }
    Ok(())
}

/// Whether switching `ext` on here is covered by this computer's grant,
/// against the installed manifests [`SyncEngine::refresh_guards`] noted
/// for this projection.
fn consent_ok(config: &Config, ext: &str) -> bool {
    let installed = CONSENT_INSTALLED.with(|c| c.borrow().clone());
    grant_covers(config, &installed, ext)
}

thread_local! {
    /// The installed items for the projection running on this thread.
    pub(crate) static CONSENT_INSTALLED: std::cell::RefCell<Vec<crate::extensions::InstalledItem>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Whether a synced setting value is one this build can use (§8.11):
/// the backup allowlist's shape check where it has one.
pub fn valid_setting(path: &str, v: &Value, rules: &Rules) -> bool {
    if v.is_null() {
        return true;
    }
    match crate::config_export::allowlist::shape_of(path) {
        Some(shape) => crate::config_export::allowlist::accepts(shape, v, rules),
        None => true,
    }
}

/// A mass-change question waiting for the user (meta `hold.<ns>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Hold {
    pub count: usize,
    /// This computer deleted them (the publish waits), or another did (the
    /// local delete waits).
    pub outgoing: bool,
    /// Answered "delete": the next pass goes through.
    pub accepted: bool,
    pub at: i64,
    #[serde(default)]
    pub from: Option<String>,
}

/// Every hold, by namespace.
pub(crate) fn holds(tx: &Tx<'_>) -> anyhow::Result<BTreeMap<String, Hold>> {
    Ok(tx
        .meta("holds")?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

fn set_holds(tx: &mut Tx<'_>, h: &BTreeMap<String, Hold>) -> anyhow::Result<()> {
    tx.set_meta("holds", &serde_json::to_string(h)?)
}

impl SyncEngine {
    /// The hold on `ns`, if any.
    pub(crate) fn mass_hold(&mut self, ns: &str) -> anyhow::Result<Option<Hold>> {
        let Some(core) = self.core.as_mut() else {
            return Ok(None);
        };
        Ok(core.replica.tx(|tx| holds(tx))?.remove(ns))
    }

    /// Another computer deleted many items: hold the local deletes.
    pub(crate) fn hold_incoming(&mut self, ns: &str, count: usize) -> anyhow::Result<()> {
        let now = self.now();
        let Some(core) = self.core.as_mut() else {
            return Ok(());
        };
        core.replica.tx(|tx| {
            let mut h = holds(tx)?;
            let e = h.entry(ns.to_string()).or_insert(Hold {
                count,
                outgoing: false,
                accepted: false,
                at: now,
                from: None,
            });
            e.count = count;
            set_holds(tx, &h)
        })?;
        log::info!("sync: {count} deletes of {ns} from other computers wait for the user's answer");
        Ok(())
    }

    /// This computer deleted many items in one go: hold the publish.
    pub(crate) fn check_outgoing_mass(&mut self, ns: &str, deletes: usize) -> anyhow::Result<()> {
        let now = self.now();
        let Some(core) = self.core.as_mut() else {
            return Ok(());
        };
        core.replica.tx(|tx| {
            let count = tx.count_records(ns)?;
            let mut h = holds(tx)?;
            let prior = h
                .get(ns)
                .filter(|x| x.outgoing && !x.accepted)
                .map_or(0, |x| x.count);
            if !is_mass_change(deletes + prior, count) {
                return Ok(());
            }
            h.insert(
                ns.to_string(),
                Hold {
                    count: deletes + prior,
                    outgoing: true,
                    accepted: false,
                    at: now,
                    from: None,
                },
            );
            log::info!(
                "sync: {} deletes of {ns} on this computer wait for the user's answer",
                deletes + prior
            );
            set_holds(tx, &h)
        })
    }

    /// The user's answer to a mass-change question.
    pub(crate) fn answer_mass_change(&mut self, ns: &str, accept: bool) -> Result<(), AppError> {
        self.require_on()?;
        let now = self.now();
        let hold = self.mass_hold(ns).map_err(super::engine::internal)?;
        let Some(hold) = hold else {
            return Ok(());
        };
        if accept {
            let core = self.core.as_mut().expect("on");
            core.replica
                .tx(|tx| {
                    let mut h = holds(tx)?;
                    if hold.outgoing {
                        h.remove(ns);
                    } else if let Some(x) = h.get_mut(ns) {
                        x.accepted = true;
                    }
                    set_holds(tx, &h)
                })
                .map_err(super::engine::internal)?;
            if !hold.outgoing {
                let all: BTreeSet<(String, String)> = [(ns.to_string(), String::new())].into();
                self.project(&all, false).map_err(super::engine::internal)?;
                let core = self.core.as_mut().expect("on");
                core.replica
                    .tx(|tx| {
                        let mut h = holds(tx)?;
                        h.remove(ns);
                        set_holds(tx, &h)
                    })
                    .map_err(super::engine::internal)?;
            }
            self.rt.first_own_ms.get_or_insert(now);
            self.rt.last_own_ms = Some(0);
            return Ok(());
        }
        // Keep them: bring every held record back as a new edit.
        let core = self.core.as_mut().expect("on");
        let me = core.me;
        let mut touched = BTreeSet::new();
        let super::engine::Core { replica, hlc, .. } = core;
        replica
            .tx(|tx| {
                let mut h = holds(tx)?;
                h.remove(ns);
                set_holds(tx, &h)?;
                for r in tx.records(ns)? {
                    if !is_visible(&r) && conflicts::restore_record(tx, ns, &r.id, now, me, hlc)? {
                        touched.insert((ns.to_string(), r.id.clone()));
                    } else if !is_visible(&r) {
                        // Cells already dropped: capture the local copy anew.
                        super::engine::drop_projected(tx, ns, &r.id)?;
                    }
                }
                tx.set_meta("hlc_last", &hlc.last().to_string())
            })
            .map_err(super::engine::internal)?;
        self.rt.first_own_ms.get_or_insert(now);
        self.rt.last_own_ms = Some(0);
        self.project(&touched, false)
            .map_err(super::engine::internal)?;
        Ok(())
    }

    /// Copy the replica's last values of records about to be deleted here
    /// to `trash/`, kept 30 days (§8.10).
    pub(crate) fn trash_copy(&mut self, ns: &str, ids: Vec<String>) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = self.now();
        let dir = self.env.local_dir.join(super::engine::TRASH_DIR);
        let Some(core) = self.core.as_mut() else {
            return Ok(());
        };
        let recs: Vec<Value> = core.replica.tx(|tx| {
            let mut out = Vec::new();
            for id in &ids {
                if let Some(r) = tx.record(ns, id)? {
                    out.push(r.to_json());
                }
            }
            Ok(out)
        })?;
        std::fs::create_dir_all(&dir)?;
        let name = format!("delete-{}-{}.json", sanitize(ns), now);
        crate::atomic_write::write(
            &dir.join(name),
            serde_json::to_vec_pretty(&serde_json::json!({ "ns": ns, "records": recs }))?,
            crate::atomic_write::Perms::Private,
        )?;
        Ok(())
    }

    /// Re-check pins whose reason may have gone: a grant that now covers an
    /// extension, an extension installed again. Returns the records to
    /// project again.
    pub(crate) fn refresh_guards(&mut self) -> anyhow::Result<BTreeSet<(String, String)>> {
        let installed_items = self.env.config.installed();
        let installed: BTreeSet<String> = installed_items
            .iter()
            .map(|i| i.manifest.id.clone())
            .collect();
        CONSENT_INSTALLED.with(|c| *c.borrow_mut() = installed_items.clone());
        let (_, cfg) = self.env.config.snapshot();
        let approved = self.prefs.approved_runnables.clone();
        let os = self.env.os;
        let Some(core) = self.core.as_mut() else {
            return Ok(BTreeSet::new());
        };
        core.replica.tx(|tx| {
            let mut out = BTreeSet::new();
            for (k, _, reason) in tx.pins(None)? {
                let gone = match reason {
                    PinReason::Consent => {
                        k.id.strip_prefix(EXTENSION_STATE)
                            .is_some_and(|ext| grant_covers(&cfg, &installed_items, ext))
                    }
                    PinReason::NotInstalled => {
                        k.id.strip_prefix(EXTENSION_STATE)
                            .is_some_and(|ext| installed.contains(ext))
                    }
                    PinReason::NeedsApproval if k.ns != CFG_NS => {
                        let rec = tx.record(&k.ns, &k.id)?;
                        rec.is_some_and(|r| {
                            let spec =
                                catalog::list_spec(&k.ns).map(super::sources::lists::spec_of);
                            spec.is_some_and(|spec| {
                                let eff = super::engine::effective_record(
                                    &spec,
                                    &r,
                                    None,
                                    &BTreeMap::new(),
                                    os,
                                );
                                runnable_of(&k.ns, &eff.fields)
                                    .map(|v| runnable_hash(&v))
                                    .as_ref()
                                    == approved.get(&guard_key(&k.ns, &k.id))
                            })
                        })
                    }
                    _ => false,
                };
                if gone {
                    tx.set_pin(&k, None)?;
                    out.insert((k.ns.clone(), k.id.clone()));
                }
            }
            Ok(out)
        })
    }

    /// Approve (or keep off) a held runnable on this computer.
    pub(crate) fn approve_runnable(&mut self, key: &str, approve: bool) -> Result<(), AppError> {
        self.require_on()?;
        let Some((ns, id)) = key.split_once('/') else {
            return Err(AppError::keyed(
                crate::error::ErrorKind::InvalidInput,
                "errors.sync.no_such_item",
                &[],
            ));
        };
        let (ns, id) = (ns.to_string(), id.to_string());
        let core = self.core.as_mut().expect("on");
        let held: Option<String> = core
            .replica
            .tx(|tx| tx.meta(&format!("held.{key}")))
            .map_err(super::engine::internal)?;
        let Some(h) = held else {
            return Ok(());
        };
        if approve {
            self.prefs.approved_runnables.insert(key.to_string(), h);
            self.save_prefs().map_err(super::engine::internal)?;
            let core = self.core.as_mut().expect("on");
            core.replica
                .tx(|tx| {
                    for f in ["enabled", "mode"] {
                        let k = LocalKey::new(&ns, &id, f);
                        if matches!(tx.pin(&k)?, Some((_, PinReason::NeedsApproval))) {
                            tx.set_pin(&k, None)?;
                        }
                    }
                    // The active pointer waited for this connection.
                    let ak = LocalKey::new(CFG_NS, "acp.active_connection_id", FIELD);
                    if matches!(tx.pin(&ak)?, Some((_, PinReason::NeedsApproval))) {
                        tx.set_pin(&ak, None)?;
                    }
                    tx.delete_meta(&format!("held.{key}"))
                })
                .map_err(super::engine::internal)?;
            let set: BTreeSet<(String, String)> = [
                (ns.clone(), id.clone()),
                (CFG_NS.to_string(), "acp.active_connection_id".to_string()),
            ]
            .into();
            self.project(&set, false).map_err(super::engine::internal)?;
        } else {
            // Keep off: stays held, and no longer asks.
            let core = self.core.as_mut().expect("on");
            core.replica
                .tx(|tx| {
                    tx.set_meta(&format!("kept_off.{key}"), &h)?;
                    tx.delete_meta(&format!("held.{key}"))
                })
                .map_err(super::engine::internal)?;
        }
        Ok(())
    }
}

/// A namespace as part of a file name.
fn sanitize(ns: &str) -> String {
    ns.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mass_change_thresholds() {
        assert!(!is_mass_change(50, 10_000));
        assert!(is_mass_change(51, 10_000));
        assert!(!is_mass_change(9, 20));
        assert!(is_mass_change(10, 40));
        assert!(!is_mass_change(10, 60));
        assert!(!is_mass_change(0, 0));
    }

    #[test]
    fn held_back_uses_the_backup_rules() {
        assert!(held_back(
            "cfg",
            "store_url",
            "v",
            &json!("https://x.example/?token=abc123def456ghi789jkl012")
        ));
        assert!(!held_back(
            "cfg",
            "store_url",
            "v",
            &json!("https://store.example/")
        ));
        assert!(!held_back(
            "cfg",
            "ui.theme",
            "v",
            &json!("sk-abcdefghijklmnopqrstuvwxyz0123456789")
        ));
        let mode = json!({ "type": "local", "spawn_command": "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz01234567 codex-acp" });
        assert!(held_back("list.connections", "c1", "mode", &mode));
        let plain = json!({ "type": "local", "spawn_command": "kiro-cli acp" });
        assert!(!held_back("list.connections", "c1", "mode", &plain));
        assert!(!held_back("list.shortcuts", "s1", "path", &json!("x")));
    }

    #[test]
    fn runnables_hash_without_last_run() {
        let a = json!({ "trigger": { "type": "schedule", "interval": "every_1h", "last_run": "x" }, "steps": [{ "prompt": "p" }] });
        let b = json!({ "trigger": { "type": "schedule", "interval": "every_1h", "last_run": "y" }, "steps": [{ "prompt": "p" }] });
        let ra = runnable_of("list.macros", a.as_object().unwrap()).unwrap();
        let rb = runnable_of("list.macros", b.as_object().unwrap()).unwrap();
        assert_eq!(runnable_hash(&ra), runnable_hash(&rb));
        let manual = json!({ "trigger": { "type": "manual" }, "steps": [] });
        assert!(runnable_of("list.macros", manual.as_object().unwrap()).is_none());
        let site = json!({ "proxy": { "script": "fetch()", "language": "js" } });
        assert!(runnable_of("list.internal_sites", site.as_object().unwrap()).is_some());
        let remote = json!({ "mode": { "type": "remote", "host": "h" } });
        assert!(runnable_of("list.connections", remote.as_object().unwrap()).is_none());
        assert_eq!(guard_key("list.macros", "s1"), "list.macros/s1");
    }

    #[test]
    fn settings_validate_against_the_allowlist_shapes() {
        let rules = Rules::new(&[]);
        assert!(valid_setting("ui.font_size", &json!(14), &rules));
        assert!(!valid_setting("ui.font_size", &json!(400), &rules));
        assert!(!valid_setting(
            "ui.window_start_position",
            &json!("sideways"),
            &rules
        ));
        assert!(valid_setting(
            "hotkey",
            &json!({ "modifiers": ["Ctrl"], "key": "K" }),
            &rules
        ));
        assert!(!valid_setting(
            "hotkey",
            &json!({ "modifiers": ["Ctrl"], "key": "NotAKey!" }),
            &rules
        ));
        assert!(valid_setting(
            "acp.agent.default_model",
            &json!("anything"),
            &rules
        ));
    }
}
