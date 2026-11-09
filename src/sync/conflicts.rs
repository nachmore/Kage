//! The conflicts list as synced records in `sync.conflicts`
//! (docs/design/SYNC.md §10): deterministic ids and cells, the actions of
//! §10.4, and the retention pass.
//!
//! A conflict record has two cells, `body@all` (the [`ConflictBody`] as
//! JSON, never changed) and `state@all` (`open`, `dismissed` or
//! `restored`). One found by merging is stamped with its *loser's* stamp
//! and wall time: the id names the loser, so that is the one thing every
//! detector of the entry agrees on. Computers that found the same pair
//! write identical cells. One that found the loser against a newer winner
//! writes another body at the same stamp, and the merge's tie-break keeps
//! one of them everywhere. A dismiss or restore gets a fresh stamp, newer
//! than the loser's, so a computer that finds the conflict late, even
//! against a winner written after the dismiss, can't reopen it.
//!
//! Writes here go through [`Tx::write`] (own edits) or [`Tx::merge`]
//! (detected conflicts, which every computer finds alike, so they publish
//! as echoes). A record that doesn't exist yet takes its bucket from the
//! caller's `bucket` function, `|ns, id| keys.bucket(ns, id, 1 << 16)`.

use super::canonical::{canonical, same};
use super::hlc::Hlc;
use super::merge::{ancestors, is_visible, merge_record, MAX_ANCESTORS};
use super::replica::Tx;
use super::status::ConflictState;
use super::types::{
    Cell, ConflictBody, ConflictKind, DeviceId, FieldKey, Record, Side, Stamp, Tomb,
};
use super::ConflictAction;
use sha2::{Digest, Sha256};

/// The namespace of the conflicts list.
pub const NS: &str = "sync.conflicts";
/// The cell holding the body.
pub const BODY: &str = "body";
/// The cell holding the state.
pub const STATE: &str = "state";
pub const OPEN: &str = "open";
pub const DISMISSED: &str = "dismissed";
pub const RESTORED: &str = "restored";

/// Open entries are dismissed this long after the winner's wall time.
pub const OPEN_DAYS: i64 = 90;
/// Resolved entries are tombstoned this long after they were resolved.
pub const RESOLVED_DAYS: i64 = 30;
/// Most entries the list keeps.
pub const MAX_ENTRIES: usize = 500;
/// Most bytes of bodies the list keeps.
pub const MAX_BYTES: usize = 64 << 20;

const DAY_MS: i64 = 86_400_000;

/// The bucket hash of a new record: `Keys::bucket(ns, id, 1 << 16)`.
pub type BucketFn<'a> = &'a dyn Fn(&str, &str) -> u32;

/// HMAC-SHA256 (RFC 2104). The same bytes as `hmac::Hmac<Sha256>`;
/// written out so this module needs no crate beyond `sha2`.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// `c` + the first 16 hex of `HMAC(name key, ns | id | field | loser stamp)`.
/// A record-level conflict has an empty field.
pub fn conflict_id(name_key: &[u8; 32], body: &ConflictBody) -> String {
    let field = body
        .field
        .as_ref()
        .map(|f| f.to_string())
        .unwrap_or_default();
    let msg = format!("{}|{}|{}|{}", body.ns, body.rid, field, body.loser.stamp);
    let mac = hmac_sha256(name_key, msg.as_bytes());
    format!("c{}", &hex::encode(mac)[..16])
}

fn plain_cell(value: serde_json::Value, stamp: Stamp, wall_ms: i64) -> Cell {
    Cell {
        value,
        stamp,
        wall_ms,
        anc: Vec::new(),
    }
}

/// The record for a conflict: `body@all` and `state@all` (`open`,
/// `dismissed` or `restored`), both at `stamp`.
pub fn conflict_record(
    id: &str,
    body: &ConflictBody,
    state: &str,
    stamp: Stamp,
    wall_ms: i64,
) -> Record {
    let mut r = Record::new(NS, id);
    let body = serde_json::to_value(body).unwrap_or_default();
    r.cells
        .insert(FieldKey::all(BODY), plain_cell(body, stamp, wall_ms));
    r.cells.insert(
        FieldKey::all(STATE),
        plain_cell(serde_json::Value::from(state), stamp, wall_ms),
    );
    r
}

/// The record a merge's conflict becomes: open, at the loser's stamp and
/// wall time (module docs).
pub fn detected(name_key: &[u8; 32], body: &ConflictBody) -> Record {
    conflict_record(
        &conflict_id(name_key, body),
        body,
        OPEN,
        body.loser.stamp,
        body.loser.wall_ms,
    )
}

/// The body of a conflict record. `None` when it is missing or written in
/// a form this build doesn't know.
pub fn body_of(r: &Record) -> Option<ConflictBody> {
    let c = r.cell(BODY, super::types::Scope::All)?;
    serde_json::from_value(c.value.clone()).ok()
}

/// The state of a conflict record.
pub fn state_of(r: &Record) -> Option<ConflictState> {
    let c = r.cell(STATE, super::types::Scope::All)?;
    serde_json::from_value(c.value.clone()).ok()
}

fn store(tx: &mut Tx<'_>, r: &Record, bucket: Option<BucketFn<'_>>) -> anyhow::Result<()> {
    let b = match tx.bucket_of(&r.ns, &r.id)? {
        Some(b) => b,
        None => match bucket {
            Some(f) => f(&r.ns, &r.id),
            None => anyhow::bail!("a new record needs a bucket"),
        },
    };
    tx.write(r, b)
}

fn floor_of(r: &Record) -> u64 {
    r.max_stamp().map_or(0, |s| s.hlc)
}

/// Record the conflicts a merge found, in the caller's transaction.
/// Returns how many entries were new or changed.
pub fn record_detected(
    tx: &mut Tx<'_>,
    name_key: &[u8; 32],
    bodies: &[ConflictBody],
    bucket: BucketFn<'_>,
) -> anyhow::Result<usize> {
    let mut n = 0;
    for body in bodies {
        let rec = detected(name_key, body);
        let b = match tx.bucket_of(NS, &rec.id)? {
            Some(b) => b,
            None => bucket(NS, &rec.id),
        };
        if tx.merge(&rec, b)?.changed {
            n += 1;
        }
    }
    Ok(n)
}

/// Record a conflict made by this computer's own action (a join, a
/// backup import, a restore, items deleted while away), with a fresh
/// stamp. Returns its id.
#[allow(clippy::too_many_arguments)]
pub fn record_action(
    tx: &mut Tx<'_>,
    name_key: &[u8; 32],
    body: &ConflictBody,
    state: &str,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
    bucket: BucketFn<'_>,
) -> anyhow::Result<String> {
    let id = conflict_id(name_key, body);
    let mut local = tx.record(NS, &id)?.unwrap_or_else(|| Record::new(NS, &id));
    let stamp = Stamp::new(hlc.tick(now_ms, floor_of(&local)), me);
    merge_record(
        &mut local,
        &conflict_record(&id, body, state, stamp, now_ms),
    );
    store(tx, &local, Some(bucket))?;
    Ok(id)
}

/// A fresh `state@all` cell.
fn set_state(
    tx: &mut Tx<'_>,
    rec: &mut Record,
    state: &str,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
) -> anyhow::Result<()> {
    let key = FieldKey::all(STATE);
    let stamp = Stamp::new(hlc.tick(now_ms, floor_of(rec)), me);
    let anc = ancestors(rec.cells.get(&key), me);
    rec.cells.insert(
        key,
        Cell {
            value: serde_json::Value::from(state),
            stamp,
            wall_ms: now_ms,
            anc,
        },
    );
    store(tx, rec, None)
}

/// A tombstone that covers the record's newest cells.
fn tombstone(rec: &mut Record, now_ms: i64, me: DeviceId, hlc: &mut Hlc) {
    rec.del = Some(Tomb {
        stamp: Stamp::new(hlc.tick(now_ms, floor_of(rec)), me),
        wall_ms: now_ms,
        anc: rec.newest_cell_stamps(MAX_ANCESTORS),
    });
}

/// Up to [`MAX_ANCESTORS`] stamps, first come first kept, without repeats.
fn anc_of(stamps: impl IntoIterator<Item = Stamp>) -> Vec<Stamp> {
    let mut out: Vec<Stamp> = Vec::new();
    for s in stamps {
        if !out.contains(&s) {
            out.push(s);
        }
        if out.len() == MAX_ANCESTORS {
            break;
        }
    }
    out
}

/// Re-stamp every kept cell of a deleted record, with the tombstone's
/// stamp in `anc`, so the restore isn't a conflict anywhere (§10.4).
fn restamp(rec: &mut Record, now_ms: i64, me: DeviceId, hlc: &mut Hlc) {
    let Some(d) = rec.del.clone() else {
        return;
    };
    for c in rec.cells.values_mut() {
        let stamp = Stamp::new(hlc.tick(now_ms, c.stamp.hlc.max(d.stamp.hlc)), me);
        c.anc = anc_of([d.stamp, c.stamp].into_iter().chain(c.anc.iter().copied()));
        c.stamp = stamp;
        c.wall_ms = now_ms;
    }
}

/// **Restore** in Recently deleted: bring back a tombstoned record with
/// every kept cell. Returns whether anything changed.
pub fn restore_record(
    tx: &mut Tx<'_>,
    ns: &str,
    id: &str,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
) -> anyhow::Result<bool> {
    let Some(mut rec) = tx.record(ns, id)? else {
        return Ok(false);
    };
    if is_visible(&rec) || rec.cells.is_empty() {
        return Ok(false);
    }
    restamp(&mut rec, now_ms, me, hlc);
    store(tx, &rec, None)?;
    Ok(true)
}

/// Delete the conflict's record, covering its current cells. Returns
/// whether anything changed.
fn delete_target(
    tx: &mut Tx<'_>,
    ns: &str,
    id: &str,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
) -> anyhow::Result<bool> {
    let Some(mut rec) = tx.record(ns, id)? else {
        return Ok(false);
    };
    if !is_visible(&rec) {
        return Ok(false);
    }
    tombstone(&mut rec, now_ms, me, hlc);
    store(tx, &rec, None)?;
    tx.clear_acks(ns, id)?;
    Ok(true)
}

/// What a resolve or restore touched: `(ns, id)` of every record written.
pub type Touched = Vec<(String, String)>;

/// Apply a conflicts-list action (§10.4) and update the entry's state.
///
/// - **Dismiss** (Keep current): `dismissed`.
/// - **Delete anyway**: a tombstone whose `anc` covers the record's newest
///   cells; `dismissed`.
/// - **Restore**: the loser's value as a new cell with a fresh stamp and
///   the current cell (and a tombstone, if any) in `anc`, so it wins
///   everywhere without a conflict. A deleted record comes back with all
///   its cells. The replaced value goes in a `replaced_by_restore` entry
///   that starts out dismissed: the user just chose, so it doesn't count
///   as an open conflict, but it stays under Resolved to undo the restore.
///   A loser that was a delete writes a tombstone. A loser without a field
///   (`deleted_while_away`) holds the whole record in its shard form
///   ([`Record::to_json`]); every cell of it is restored. `restored`.
#[allow(clippy::too_many_arguments)]
pub fn resolve(
    tx: &mut Tx<'_>,
    name_key: &[u8; 32],
    id: &str,
    action: ConflictAction,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
    bucket: BucketFn<'_>,
) -> anyhow::Result<Touched> {
    let mut rec = tx
        .record(NS, id)?
        .filter(is_visible)
        .ok_or_else(|| anyhow::anyhow!("no such conflict"))?;
    let body = body_of(&rec).ok_or_else(|| anyhow::anyhow!("unreadable conflict"))?;
    let target = (body.ns.clone(), body.rid.clone());
    let mut touched = Touched::new();
    let state = match action {
        ConflictAction::Dismiss => DISMISSED,
        ConflictAction::DeleteAnyway => {
            if delete_target(tx, &body.ns, &body.rid, now_ms, me, hlc)? {
                touched.push(target);
            }
            DISMISSED
        }
        ConflictAction::Restore => {
            if body.loser.deleted {
                if delete_target(tx, &body.ns, &body.rid, now_ms, me, hlc)? {
                    touched.push(target);
                }
            } else {
                let value = body
                    .loser
                    .value
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("the replaced version has no value"))?;
                let saved = match &body.field {
                    Some(fk) => vec![(fk.clone(), value, None)],
                    None => Record::from_json(&body.ns, &value)?
                        .cells
                        .into_iter()
                        .map(|(k, c)| (k, c.value, Some(c.stamp)))
                        .collect(),
                };
                touched.extend(restore_cells(
                    tx, name_key, &body, saved, now_ms, me, hlc, bucket,
                )?);
            }
            RESTORED
        }
    };
    set_state(tx, &mut rec, state, now_ms, me, hlc)?;
    touched.push((NS.to_string(), id.to_string()));
    Ok(touched)
}

/// Write `saved` cells back into the conflict's record (§10.4 Restore).
#[allow(clippy::too_many_arguments)]
fn restore_cells(
    tx: &mut Tx<'_>,
    name_key: &[u8; 32],
    body: &ConflictBody,
    saved: Vec<(FieldKey, serde_json::Value, Option<Stamp>)>,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
    bucket: BucketFn<'_>,
) -> anyhow::Result<Touched> {
    let mut target = tx
        .record(&body.ns, &body.rid)?
        .unwrap_or_else(|| Record::new(body.ns.clone(), body.rid.clone()));
    let hidden = !is_visible(&target);
    if hidden {
        restamp(&mut target, now_ms, me, hlc);
    }
    let mut changed = hidden;
    let mut replaced = Vec::new();
    for (fk, value, was) in saved {
        let cur = target.cells.get(&fk).cloned();
        if cur.as_ref().is_some_and(|c| same(&c.value, &value)) {
            continue;
        }
        let del = target.del.as_ref().map(|d| d.stamp);
        let floor = cur
            .as_ref()
            .map(|c| c.stamp)
            .into_iter()
            .chain(del)
            .chain(was)
            .max()
            .map_or(0, |s| s.hlc);
        let stamp = Stamp::new(hlc.tick(now_ms, floor), me);
        let anc = anc_of(
            cur.as_ref()
                .map(|c| c.stamp)
                .into_iter()
                .chain(del)
                .chain(was)
                .chain(cur.iter().flat_map(|c| c.anc.iter().copied())),
        );
        target.cells.insert(
            fk.clone(),
            Cell {
                value,
                stamp,
                wall_ms: now_ms,
                anc,
            },
        );
        changed = true;
        if let Some(c) = cur {
            replaced.push(ConflictBody {
                ns: body.ns.clone(),
                rid: body.rid.clone(),
                field: Some(fk),
                kind: ConflictKind::ReplacedByRestore,
                loser: Side {
                    stamp: c.stamp,
                    wall_ms: c.wall_ms,
                    deleted: false,
                    value: Some(c.value),
                },
                winner: Some(Side {
                    stamp,
                    wall_ms: now_ms,
                    deleted: false,
                    value: None,
                }),
            });
        }
    }
    let mut touched = Touched::new();
    if !changed {
        return Ok(touched);
    }
    store(tx, &target, Some(bucket))?;
    touched.push((body.ns.clone(), body.rid.clone()));
    for r in replaced {
        let id = record_action(tx, name_key, &r, DISMISSED, now_ms, me, hlc, bucket)?;
        touched.push((NS.to_string(), id));
    }
    Ok(touched)
}

/// One live entry, for the limits.
struct Entry {
    rec: Record,
    resolved: bool,
    since_ms: i64,
    bytes: usize,
}

/// Dismiss open entries older than 90 days, tombstone resolved ones older
/// than 30, and keep the list within its count and size limits (§10.3):
/// past them, the oldest resolved entries go first, then the oldest open
/// ones. Returns how many records it changed.
pub fn retention_pass(
    tx: &mut Tx<'_>,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
) -> anyhow::Result<usize> {
    let mut changed = 0;
    let mut live = Vec::new();
    for mut rec in tx.records(NS)? {
        if !is_visible(&rec) {
            continue;
        }
        let body_cell = rec.cell(BODY, super::types::Scope::All);
        let bytes = body_cell.map_or(0, |c| canonical(&c.value).len());
        let state = state_of(&rec).unwrap_or(ConflictState::Open);
        if state == ConflictState::Open {
            let since = body_of(&rec)
                .and_then(|b| b.winner.map(|w| w.wall_ms))
                .or(body_cell.map(|c| c.wall_ms))
                .unwrap_or(now_ms);
            if now_ms - since >= OPEN_DAYS * DAY_MS {
                set_state(tx, &mut rec, DISMISSED, now_ms, me, hlc)?;
                changed += 1;
                live.push(Entry {
                    rec,
                    resolved: true,
                    since_ms: now_ms,
                    bytes,
                });
            } else {
                live.push(Entry {
                    rec,
                    resolved: false,
                    since_ms: since,
                    bytes,
                });
            }
            continue;
        }
        let since = rec
            .cell(STATE, super::types::Scope::All)
            .map_or(now_ms, |c| c.wall_ms);
        if now_ms - since >= RESOLVED_DAYS * DAY_MS {
            tombstone(&mut rec, now_ms, me, hlc);
            store(tx, &rec, None)?;
            changed += 1;
            continue;
        }
        live.push(Entry {
            rec,
            resolved: true,
            since_ms: since,
            bytes,
        });
    }

    let mut count = live.len();
    let mut bytes: usize = live.iter().map(|e| e.bytes).sum();
    if count <= MAX_ENTRIES && bytes <= MAX_BYTES {
        return Ok(changed);
    }
    // Resolved first, then open; oldest first within each.
    live.sort_by_key(|e| (!e.resolved, e.since_ms));
    for mut e in live {
        if count <= MAX_ENTRIES && bytes <= MAX_BYTES {
            break;
        }
        tombstone(&mut e.rec, now_ms, me, hlc);
        store(tx, &e.rec, None)?;
        log::info!("sync: dropped conflict {} over the list limit", e.rec.id);
        count -= 1;
        bytes -= e.bytes;
        changed += 1;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::replica::Replica;
    use crate::sync::types::Scope;
    use serde_json::json;

    const KEY: [u8; 32] = [42; 32];
    const NOW: i64 = 1_762_686_000_000;

    fn a() -> DeviceId {
        DeviceId([1; 10])
    }

    fn b() -> DeviceId {
        DeviceId([2; 10])
    }

    fn bucket(_: &str, id: &str) -> u32 {
        id.len() as u32
    }

    fn cell(v: serde_json::Value, s: Stamp) -> Cell {
        Cell {
            value: v,
            stamp: s,
            wall_ms: crate::sync::hlc::wall_ms_of(s.hlc),
            anc: vec![],
        }
    }

    fn theme(v: &str, s: Stamp) -> Record {
        let mut r = Record::new("cfg", "ui.theme");
        r.cells.insert(FieldKey::all("v"), cell(json!(v), s));
        r
    }

    fn body(loser: Stamp, winner: Stamp) -> ConflictBody {
        ConflictBody {
            ns: "cfg".into(),
            rid: "ui.theme".into(),
            field: Some(FieldKey::all("v")),
            kind: ConflictKind::ConcurrentEdit,
            loser: Side {
                stamp: loser,
                wall_ms: 1,
                deleted: false,
                value: Some(json!("light")),
            },
            winner: Some(Side {
                stamp: winner,
                wall_ms: 2,
                deleted: false,
                value: None,
            }),
        }
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex::encode(hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn ids_and_stamps_are_deterministic() {
        let b1 = body(Stamp::new(10, b()), Stamp::new(20, a()));
        let id = conflict_id(&KEY, &b1);
        assert!(id.starts_with('c') && id.len() == 17);
        assert!(id[1..].bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id, conflict_id(&KEY, &b1.clone()));
        // The id names ns, id, field and loser; not the winner or kind.
        let mut other_winner = b1.clone();
        other_winner.winner.as_mut().unwrap().stamp = Stamp::new(30, a());
        assert_eq!(conflict_id(&KEY, &other_winner), id);
        let mut other_loser = b1.clone();
        other_loser.loser.stamp = Stamp::new(11, b());
        assert_ne!(conflict_id(&KEY, &other_loser), id);
        assert_ne!(conflict_id(&[1; 32], &b1), id);
        let mut no_field = b1.clone();
        no_field.field = None;
        assert_ne!(conflict_id(&KEY, &no_field), id);

        let rec = detected(&KEY, &b1);
        assert_eq!(rec.id, id);
        for name in [BODY, STATE] {
            let c = rec.cell(name, Scope::All).unwrap();
            assert_eq!(c.stamp, Stamp::new(10, b()), "the loser's stamp");
            assert_eq!(c.wall_ms, 1);
        }
        assert_eq!(body_of(&rec), Some(b1.clone()));
        assert_eq!(state_of(&rec), Some(ConflictState::Open));

        // A detector that saw a newer winner writes the same id at the same
        // stamp; merging the two picks one body the same way everywhere.
        let ro = detected(&KEY, &other_winner);
        assert_eq!(ro.id, id);
        let mut x = rec.clone();
        merge_record(&mut x, &ro);
        let mut y = ro.clone();
        merge_record(&mut y, &rec);
        assert_eq!(x, y);
    }

    /// Two computers detect the same conflict by merging the same cells in
    /// opposite orders and write identical records.
    #[test]
    fn two_detectors_write_identical_cells() {
        let mine = theme("light", Stamp::new(10, b()));
        let theirs = theme("dark", Stamp::new(20, a()));
        let mut on_a = theirs.clone();
        let out_a = merge_record(&mut on_a, &mine);
        let mut on_b = mine.clone();
        let out_b = merge_record(&mut on_b, &theirs);
        assert_eq!(out_a.conflicts, out_b.conflicts);
        let ra = detected(&KEY, &out_a.conflicts[0]);
        let rb = detected(&KEY, &out_b.conflicts[0]);
        assert_eq!(ra, rb);

        let mut r = Replica::open_in_memory().unwrap();
        r.tx(|tx| {
            assert_eq!(record_detected(tx, &KEY, &out_a.conflicts, &bucket)?, 1);
            // The same conflict found again changes nothing.
            assert_eq!(record_detected(tx, &KEY, &out_b.conflicts, &bucket)?, 0);
            assert_eq!(tx.records(NS)?, vec![ra.clone()]);
            assert_eq!(
                tx.take_dirty()?,
                vec![(NS.to_string(), bucket(NS, &ra.id), false)]
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_late_detector_cant_reopen_a_dismissed_conflict() {
        let b1 = body(Stamp::new(10, b()), Stamp::new(20, a()));
        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(0);
        let id = r
            .tx(|tx| {
                record_detected(tx, &KEY, std::slice::from_ref(&b1), &bucket)?;
                let id = conflict_id(&KEY, &b1);
                resolve(
                    tx,
                    &KEY,
                    &id,
                    ConflictAction::Dismiss,
                    NOW,
                    b(),
                    &mut hlc,
                    &bucket,
                )?;
                Ok(id)
            })
            .unwrap();
        let dismissed = r.tx(|tx| tx.record(NS, &id)).unwrap().unwrap();
        assert_eq!(state_of(&dismissed), Some(ConflictState::Dismissed));

        // Here: the late detection arrives after the dismiss.
        r.tx(|tx| record_detected(tx, &KEY, std::slice::from_ref(&b1), &bucket))
            .unwrap();
        let after = r.tx(|tx| tx.record(NS, &id)).unwrap().unwrap();
        assert_eq!(state_of(&after), Some(ConflictState::Dismissed));

        // Elsewhere: the detection came first; the dismiss still wins.
        let mut late = detected(&KEY, &b1);
        merge_record(&mut late, &dismissed);
        assert_eq!(state_of(&late), Some(ConflictState::Dismissed));

        // A computer that was offline still holds the loser and finds it
        // against a winner written after the dismiss. Same id, and the
        // dismiss still wins.
        let mut newer = b1.clone();
        newer.winner.as_mut().unwrap().stamp = Stamp::new(hlc_at(NOW + 60_000), a());
        newer.winner.as_mut().unwrap().wall_ms = NOW + 60_000;
        assert_eq!(conflict_id(&KEY, &newer), id);
        r.tx(|tx| record_detected(tx, &KEY, std::slice::from_ref(&newer), &bucket))
            .unwrap();
        let after = r.tx(|tx| tx.record(NS, &id)).unwrap().unwrap();
        assert_eq!(state_of(&after), Some(ConflictState::Dismissed));
        let mut far = detected(&KEY, &newer);
        merge_record(&mut far, &dismissed);
        assert_eq!(state_of(&far), Some(ConflictState::Dismissed));
    }

    /// I3 in miniature: B's value lost to A's; restoring it on B wins
    /// everywhere without a new conflict, and records what it replaced.
    #[test]
    fn restore_writes_a_non_conflicting_cell_and_replaced_by_restore() {
        let on_a = theme("dark", Stamp::new(hlc_at(NOW - 60_000), a()));
        let mut on_b = theme("light", Stamp::new(hlc_at(NOW - 120_000), b()));
        let out = merge_record(&mut on_b, &on_a);
        assert_eq!(out.conflicts.len(), 1);

        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(hlc_at(NOW - 60_000));
        let (id, touched) = r
            .tx(|tx| {
                tx.put_record(&on_b, 7)?;
                record_detected(tx, &KEY, &out.conflicts, &bucket)?;
                let id = conflict_id(&KEY, &out.conflicts[0]);
                let t = resolve(
                    tx,
                    &KEY,
                    &id,
                    ConflictAction::Restore,
                    NOW,
                    b(),
                    &mut hlc,
                    &bucket,
                )?;
                Ok((id, t))
            })
            .unwrap();
        let (restored, entries) = r
            .tx(|tx| Ok((tx.record("cfg", "ui.theme")?.unwrap(), tx.records(NS)?)))
            .unwrap();
        assert!(touched.contains(&("cfg".to_string(), "ui.theme".to_string())));
        let v = restored.cell("v", Scope::All).unwrap();
        assert_eq!(v.value, json!("light"));
        assert_eq!(v.stamp.dev, b());
        assert!(v.anc.contains(&on_a.cell("v", Scope::All).unwrap().stamp));

        // On A: no conflict, and the restored value wins.
        let mut a_side = on_a.clone();
        let out = merge_record(&mut a_side, &restored);
        assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
        assert_eq!(a_side.cell("v", Scope::All).unwrap().value, json!("light"));

        // The entry is restored; a replaced_by_restore entry keeps "dark".
        assert_eq!(entries.len(), 2);
        let first = entries.iter().find(|e| e.id == id).unwrap();
        assert_eq!(state_of(first), Some(ConflictState::Restored));
        let undo = entries.iter().find(|e| e.id != id).unwrap();
        let ub = body_of(undo).unwrap();
        assert_eq!(ub.kind, ConflictKind::ReplacedByRestore);
        assert_eq!(ub.loser.value, Some(json!("dark")));
        assert_eq!(state_of(undo), Some(ConflictState::Dismissed));

        // Restoring that entry undoes the restore.
        r.tx(|tx| {
            resolve(
                tx,
                &KEY,
                &undo.id,
                ConflictAction::Restore,
                NOW + 1_000,
                b(),
                &mut hlc,
                &bucket,
            )
        })
        .unwrap();
        let back = r.tx(|tx| tx.record("cfg", "ui.theme")).unwrap().unwrap();
        assert_eq!(back.cell("v", Scope::All).unwrap().value, json!("dark"));
    }

    fn hlc_at(ms: i64) -> u64 {
        crate::sync::hlc::physical(ms)
    }

    fn deleted_shortcut() -> (Record, Record) {
        let mut rec = Record::new("list.shortcuts", "s1");
        rec.cells.insert(
            FieldKey::all("name"),
            cell(json!("gh"), Stamp::new(hlc_at(NOW - 5_000), a())),
        );
        let edited_on_b = Stamp::new(hlc_at(NOW - 4_000), b());
        rec.cells
            .insert(FieldKey::all("prompt"), cell(json!("new"), edited_on_b));
        let mut deleted = rec.clone();
        deleted.del = Some(Tomb {
            stamp: Stamp::new(hlc_at(NOW - 3_000), a()),
            wall_ms: NOW - 3_000,
            anc: vec![Stamp::new(hlc_at(NOW - 5_000), a())],
        });
        (rec, deleted)
    }

    #[test]
    fn restoring_an_edit_that_was_deleted_brings_the_record_back() {
        let (_, deleted) = deleted_shortcut();
        let mut probe = deleted.clone();
        probe.del = None;
        let mut hidden = deleted.clone();
        let out = merge_record(&mut hidden, &deleted);
        assert!(out.conflicts.is_empty());
        // Detect it as a fresh pair: the delete arrives where B's edit is.
        let mut on_b = probe;
        let out = merge_record(&mut on_b, &deleted);
        assert_eq!(out.conflicts.len(), 1);
        assert_eq!(out.conflicts[0].kind, ConflictKind::EditVsDelete);

        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(hlc_at(NOW));
        r.tx(|tx| {
            tx.put_record(&on_b, 1)?;
            record_detected(tx, &KEY, &out.conflicts, &bucket)?;
            let id = conflict_id(&KEY, &out.conflicts[0]);
            resolve(
                tx,
                &KEY,
                &id,
                ConflictAction::Restore,
                NOW,
                b(),
                &mut hlc,
                &bucket,
            )?;
            let back = tx.record("list.shortcuts", "s1")?.unwrap();
            assert!(is_visible(&back));
            assert_eq!(back.cells.len(), 2);
            // No conflict where the delete came from.
            let mut on_a = deleted.clone();
            let out = merge_record(&mut on_a, &back);
            assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
            assert!(is_visible(&on_a));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn delete_anyway_and_restore_of_a_delete_write_a_covering_tombstone() {
        let (rec, deleted) = deleted_shortcut();
        // B's edit came after A's delete: delete_vs_edit.
        let mut later = rec.clone();
        later.cells.insert(
            FieldKey::all("prompt"),
            cell(json!("newer"), Stamp::new(hlc_at(NOW - 1_000), b())),
        );
        for action in [ConflictAction::DeleteAnyway, ConflictAction::Restore] {
            let mut here = deleted.clone();
            let out = merge_record(&mut here, &later);
            assert_eq!(out.conflicts[0].kind, ConflictKind::DeleteVsEdit);
            let mut r = Replica::open_in_memory().unwrap();
            let mut hlc = Hlc::new(hlc_at(NOW));
            r.tx(|tx| {
                tx.put_record(&here, 1)?;
                record_detected(tx, &KEY, &out.conflicts, &bucket)?;
                let id = conflict_id(&KEY, &out.conflicts[0]);
                resolve(tx, &KEY, &id, action, NOW, a(), &mut hlc, &bucket)?;
                let gone = tx.record("list.shortcuts", "s1")?.unwrap();
                assert!(!is_visible(&gone), "{action:?}");
                // Elsewhere, the covering delete isn't an edit_vs_delete.
                let mut on_b = later.clone();
                let out = merge_record(&mut on_b, &gone);
                assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
                let entry = tx.record(NS, &id)?.unwrap();
                let want = if action == ConflictAction::Restore {
                    ConflictState::Restored
                } else {
                    ConflictState::Dismissed
                };
                assert_eq!(state_of(&entry), Some(want));
                Ok(())
            })
            .unwrap();
        }
    }

    #[test]
    fn recently_deleted_restore_and_whole_record_losers() {
        let (_, deleted) = deleted_shortcut();
        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(hlc_at(NOW));
        r.tx(|tx| {
            tx.put_record(&deleted, 1)?;
            assert!(restore_record(
                tx,
                "list.shortcuts",
                "s1",
                NOW,
                b(),
                &mut hlc
            )?);
            let back = tx.record("list.shortcuts", "s1")?.unwrap();
            assert!(is_visible(&back));
            let d = deleted.del.as_ref().unwrap().stamp;
            assert!(back.cells.values().all(|c| c.anc.contains(&d)));
            assert!(!restore_record(
                tx,
                "list.shortcuts",
                "s1",
                NOW,
                b(),
                &mut hlc
            )?);
            assert!(!restore_record(
                tx,
                "list.shortcuts",
                "nope",
                NOW,
                b(),
                &mut hlc
            )?);

            // deleted_while_away: the loser is the whole record.
            let mut gone = deleted.clone();
            gone.id = "s2".into();
            let away = ConflictBody {
                ns: "list.shortcuts".into(),
                rid: "s2".into(),
                field: None,
                kind: ConflictKind::DeletedWhileAway,
                loser: Side {
                    stamp: gone.max_stamp().unwrap(),
                    wall_ms: NOW - 3_000,
                    deleted: false,
                    value: Some(gone.to_json()),
                },
                winner: None,
            };
            let id = record_action(tx, &KEY, &away, OPEN, NOW, b(), &mut hlc, &bucket)?;
            resolve(
                tx,
                &KEY,
                &id,
                ConflictAction::Restore,
                NOW,
                b(),
                &mut hlc,
                &bucket,
            )?;
            let s2 = tx.record("list.shortcuts", "s2")?.unwrap();
            assert!(is_visible(&s2));
            assert_eq!(s2.cell("name", Scope::All).unwrap().value, json!("gh"));
            assert_eq!(s2.cell("prompt", Scope::All).unwrap().value, json!("new"));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn resolving_an_unknown_conflict_fails() {
        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(0);
        assert!(r
            .tx(|tx| resolve(
                tx,
                &KEY,
                "c0000",
                ConflictAction::Dismiss,
                NOW,
                a(),
                &mut hlc,
                &bucket
            ))
            .is_err());
    }

    fn entry(n: u64, state: &str, winner_wall: i64, state_wall: i64, size: usize) -> Record {
        let mut bd = body(Stamp::new(n, b()), Stamp::new(n + 1, a()));
        bd.rid = format!("r{n}");
        bd.winner.as_mut().unwrap().wall_ms = winner_wall;
        bd.loser.value = Some(json!("x".repeat(size)));
        let mut rec = conflict_record(
            &conflict_id(&KEY, &bd),
            &bd,
            OPEN,
            Stamp::new(n + 1, a()),
            winner_wall,
        );
        if state != OPEN {
            rec.cells.insert(
                FieldKey::all(STATE),
                Cell {
                    value: json!(state),
                    stamp: Stamp::new(n + 2, b()),
                    wall_ms: state_wall,
                    anc: vec![],
                },
            );
        }
        rec
    }

    #[test]
    fn retention_by_age() {
        let day = DAY_MS;
        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(hlc_at(NOW));
        let fresh = entry(1, OPEN, NOW - 89 * day, 0, 1);
        let stale = entry(2, OPEN, NOW - 91 * day, 0, 1);
        let resolved_recent = entry(3, DISMISSED, NOW - 200 * day, NOW - 29 * day, 1);
        let resolved_old = entry(4, RESTORED, NOW - 200 * day, NOW - 31 * day, 1);
        r.tx(|tx| {
            for (i, e) in [&fresh, &stale, &resolved_recent, &resolved_old]
                .iter()
                .enumerate()
            {
                tx.put_record(e, i as u32)?;
            }
            assert_eq!(retention_pass(tx, NOW, a(), &mut hlc)?, 2);
            let get = |tx: &Tx<'_>, id: &str| tx.record(NS, id).map(|r| r.unwrap());
            assert_eq!(state_of(&get(tx, &fresh.id)?), Some(ConflictState::Open));
            assert_eq!(
                state_of(&get(tx, &stale.id)?),
                Some(ConflictState::Dismissed)
            );
            assert!(is_visible(&get(tx, &resolved_recent.id)?));
            assert!(!is_visible(&get(tx, &resolved_old.id)?));
            // Running again changes nothing.
            assert_eq!(retention_pass(tx, NOW, a(), &mut hlc)?, 0);
            // 30 days later: the fresh entry ages out, and the two resolved
            // ones (one by the age-out above) are tombstoned.
            assert_eq!(retention_pass(tx, NOW + 30 * day, a(), &mut hlc)?, 3);
            assert_eq!(
                state_of(&get(tx, &fresh.id)?),
                Some(ConflictState::Dismissed)
            );
            assert!(!is_visible(&get(tx, &stale.id)?));
            assert!(!is_visible(&get(tx, &resolved_recent.id)?));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn retention_by_count_drops_resolved_then_oldest_open() {
        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(hlc_at(NOW));
        r.tx(|tx| {
            for n in 0..(MAX_ENTRIES as u64 + 5) {
                let state = if n % 101 == 0 { DISMISSED } else { OPEN };
                let e = entry(n * 10, state, NOW - 1_000_000 + n as i64, NOW - 1_000, 1);
                tx.put_record(&e, n as u32)?;
            }
            assert_eq!(retention_pass(tx, NOW, a(), &mut hlc)?, 5);
            let left: Vec<Record> = tx.records(NS)?.into_iter().filter(is_visible).collect();
            assert_eq!(left.len(), MAX_ENTRIES);
            // The 5 resolved ones (n = 0, 101, …, 404) went first.
            assert!(left
                .iter()
                .all(|e| state_of(e) == Some(ConflictState::Open)));
            Ok(())
        })
        .unwrap();
        r.tx(|tx| {
            for n in 0..3u64 {
                let e = entry(100_000 + n, OPEN, NOW - 500 + n as i64, 0, 1);
                tx.put_record(&e, 1)?;
            }
            assert_eq!(retention_pass(tx, NOW, a(), &mut hlc)?, 3);
            // Then the oldest open ones: the new entries are the newest.
            for n in 0..3u64 {
                let mut bd = body(Stamp::new(100_000 + n, b()), Stamp::new(100_001 + n, a()));
                bd.rid = format!("r{}", 100_000 + n);
                let id = conflict_id(&KEY, &bd);
                assert!(is_visible(&tx.record(NS, &id)?.unwrap()));
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn retention_by_size() {
        let mut r = Replica::open_in_memory().unwrap();
        let mut hlc = Hlc::new(hlc_at(NOW));
        let big = MAX_BYTES / 3;
        r.tx(|tx| {
            for n in 0..4u64 {
                tx.put_record(
                    &entry(n * 10, OPEN, NOW - 1_000 + n as i64, 0, big),
                    n as u32,
                )?;
            }
            assert_eq!(retention_pass(tx, NOW, a(), &mut hlc)?, 2);
            let left: Vec<Record> = tx.records(NS)?.into_iter().filter(is_visible).collect();
            assert_eq!(left.len(), 2);
            let mut ids: Vec<String> = left.iter().map(|e| body_of(e).unwrap().rid).collect();
            ids.sort();
            assert_eq!(ids, vec!["r20", "r30"]);
            Ok(())
        })
        .unwrap();
    }
}
