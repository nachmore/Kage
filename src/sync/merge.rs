//! Merging records cell by cell (docs/design/SYNC.md §8.5). Every cell
//! merges the same way: the higher stamp wins. A replaced value the winner
//! doesn't name in `anc` becomes a conflict.
//!
//! The result of a merge depends only on the set of cells and tombstones
//! merged, never on their order: each cell keeps the greatest version
//! under one total order (stamp, then, only after a clone, the value's
//! canonical hash, wall time and ancestors), and so does the tombstone.
//! Conflicts are reported for pairs that meet for the first time in this
//! merge: one side was here, the other arrived.

use super::canonical::{canonical, hash, same};
use super::split::is_union;
use super::types::{
    Cell, ConflictBody, ConflictKind, DeviceId, FieldKey, Record, Side, Stamp, Tomb,
};
use std::cmp::Ordering;
use std::collections::BTreeSet;

/// Most ancestor stamps a cell carries (§7.4).
pub const MAX_ANCESTORS: usize = 3;

/// What merging one incoming record changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergeOutcome {
    /// The local record changed (a newer cell or tombstone arrived).
    pub changed: bool,
    /// Values that stopped being current without their winner having seen
    /// them.
    pub conflicts: Vec<ConflictBody>,
}

/// Merge `incoming` into `local`. Commutative, associative and idempotent.
///
/// An empty `local` (no namespace, id, cells or tombstone) takes the
/// incoming record's namespace and id. Namespaces under `sync.` never
/// produce conflicts (§10.1).
pub fn merge_record(local: &mut Record, incoming: &Record) -> MergeOutcome {
    if local.ns.is_empty() && local.id.is_empty() && local.cells.is_empty() && local.del.is_none() {
        local.ns.clone_from(&incoming.ns);
        local.id.clone_from(&incoming.id);
    }
    debug_assert!(local.ns == incoming.ns && local.id == incoming.id);
    let detect = !local.ns.starts_with("sync.");
    let was_visible = is_visible(local);
    let mut out = MergeOutcome::default();
    let mut found = Vec::new();

    // Cells whose current version came in with `incoming`.
    let mut arrived: BTreeSet<FieldKey> = BTreeSet::new();
    for (fk, inc) in &incoming.cells {
        match local.cells.get_mut(fk) {
            None => {
                local.cells.insert(fk.clone(), inc.clone());
                arrived.insert(fk.clone());
                out.changed = true;
            }
            Some(cur) => match cmp_cells(inc, cur) {
                Ordering::Greater => {
                    found.extend(cell_conflict(&incoming.ns, &incoming.id, fk, cur, inc));
                    *cur = inc.clone();
                    arrived.insert(fk.clone());
                    out.changed = true;
                }
                Ordering::Less => {
                    found.extend(cell_conflict(&incoming.ns, &incoming.id, fk, inc, cur));
                }
                Ordering::Equal => {}
            },
        }
    }

    let mut del_arrived = false;
    if let Some(d) = &incoming.del {
        let newer = local
            .del
            .as_ref()
            .is_none_or(|cur| cmp_tombs(d, cur) == Ordering::Greater);
        if newer {
            local.del = Some(d.clone());
            del_arrived = true;
            out.changed = true;
        }
    }
    if let Some(d) = &local.del {
        // A cell and the tombstone meet for the first time when exactly one
        // of them arrived now.
        let fresh = |fk: &FieldKey| arrived.contains(fk) != del_arrived;
        if is_visible(local) {
            // Only when this merge is what brought the edit and the delete
            // together: the delete is new here, or the record was hidden.
            // A further edit of a record already shown again isn't news.
            if del_arrived || !was_visible {
                found.extend(delete_vs_edit(local, d, &fresh));
            }
        } else {
            found.extend(edit_vs_delete(local, d, &fresh));
        }
    }
    if detect {
        out.conflicts = found;
    }
    out
}

/// A record is visible when it has no tombstone, or some cell is newer
/// than the tombstone (§7.2).
pub fn is_visible(r: &Record) -> bool {
    match &r.del {
        None => true,
        Some(d) => r.cells.values().any(|c| c.stamp > d.stamp),
    }
}

/// The `anc` of a new cell that replaces `base`, the cell the user saw
/// (§7.4).
pub fn ancestors(base: Option<&Cell>, me: DeviceId) -> Vec<Stamp> {
    match base {
        None => Vec::new(),
        // A run of my own edits counts as one edit.
        Some(b) if b.stamp.dev == me => b.anc.clone(),
        Some(b) => std::iter::once(b.stamp)
            .chain(b.anc.iter().copied())
            .take(MAX_ANCESTORS)
            .collect(),
    }
}

/// The total order cells merge by. Stamps decide; equal stamps only come
/// from a clone (§8.9), where the higher canonical hash of the value wins,
/// then wall time, ancestors and the value's bytes, so every computer
/// keeps the same version.
pub fn cmp_cells(a: &Cell, b: &Cell) -> Ordering {
    a.stamp.cmp(&b.stamp).then_with(|| {
        let by_value = if same(&a.value, &b.value) {
            Ordering::Equal
        } else {
            hash(&a.value).cmp(&hash(&b.value))
        };
        by_value
            .then(a.wall_ms.cmp(&b.wall_ms))
            .then_with(|| a.anc.cmp(&b.anc))
            .then_with(|| raw(&a.value).cmp(&raw(&b.value)))
    })
}

/// The total order tombstones merge by.
pub fn cmp_tombs(a: &Tomb, b: &Tomb) -> Ordering {
    a.stamp
        .cmp(&b.stamp)
        .then(a.wall_ms.cmp(&b.wall_ms))
        .then_with(|| a.anc.cmp(&b.anc))
}

/// The value as serde_json writes it: tells `1` from `1.0` for the last
/// tie-break, where canonical bytes can't.
fn raw(v: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_else(|_| canonical(v))
}

fn side(c: &Cell, with_value: bool) -> Side {
    Side {
        stamp: c.stamp,
        wall_ms: c.wall_ms,
        deleted: false,
        value: with_value.then(|| c.value.clone()),
    }
}

fn tomb_side(d: &Tomb) -> Side {
    Side {
        stamp: d.stamp,
        wall_ms: d.wall_ms,
        deleted: true,
        value: None,
    }
}

/// §8.5 `merge_cell`: `lo` stopped being current because `w` replaced it.
fn cell_conflict(ns: &str, id: &str, fk: &FieldKey, lo: &Cell, w: &Cell) -> Option<ConflictBody> {
    if is_union(&fk.name) || same(&lo.value, &w.value) {
        return None;
    }
    if lo.stamp != w.stamp && (w.anc.contains(&lo.stamp) || lo.stamp.dev == w.stamp.dev) {
        return None;
    }
    Some(ConflictBody {
        ns: ns.to_string(),
        rid: id.to_string(),
        field: Some(fk.clone()),
        kind: ConflictKind::ConcurrentEdit,
        loser: side(lo, true),
        winner: Some(side(w, false)),
    })
}

/// The deleter hadn't seen `c`: another computer's cell, not named in the
/// tombstone's `anc` and newer than every stamp there.
fn unseen_by_delete(c: &Cell, d: &Tomb) -> bool {
    c.stamp.dev != d.stamp.dev && !d.anc.contains(&c.stamp) && d.anc.iter().all(|a| c.stamp > *a)
}

/// "Delete beats an edit it hadn't seen": a hidden record's cells that the
/// deleter hadn't seen. One entry per field; set elements and map entries
/// don't count.
fn edit_vs_delete(r: &Record, d: &Tomb, fresh: &dyn Fn(&FieldKey) -> bool) -> Vec<ConflictBody> {
    r.cells
        .iter()
        .filter(|(fk, c)| {
            fresh(fk) && c.stamp < d.stamp && !is_union(&fk.name) && unseen_by_delete(c, d)
        })
        .map(|(fk, c)| ConflictBody {
            ns: r.ns.clone(),
            rid: r.id.clone(),
            field: Some(fk.clone()),
            kind: ConflictKind::EditVsDelete,
            loser: side(c, true),
            winner: Some(tomb_side(d)),
        })
        .collect()
}

/// "An edit beats a delete it hadn't seen": the record is visible again
/// through cells newer than the tombstone, and none of them names the
/// tombstone (a deliberate restore does). One entry per record, against
/// the newest such cell.
fn delete_vs_edit(r: &Record, d: &Tomb, fresh: &dyn Fn(&FieldKey) -> bool) -> Option<ConflictBody> {
    let newer: Vec<(&FieldKey, &Cell)> =
        r.cells.iter().filter(|(_, c)| c.stamp > d.stamp).collect();
    if newer.iter().any(|(_, c)| c.anc.contains(&d.stamp)) {
        return None;
    }
    let met_now = newer
        .iter()
        .any(|(fk, c)| fresh(fk) && c.stamp.dev != d.stamp.dev);
    if !met_now {
        return None;
    }
    let (_, w) = newer.iter().max_by_key(|(_, c)| c.stamp)?;
    Some(ConflictBody {
        ns: r.ns.clone(),
        rid: r.id.clone(),
        field: None,
        kind: ConflictKind::DeleteVsEdit,
        loser: tomb_side(d),
        winner: Some(side(w, false)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{OsKind, Scope};
    use serde_json::{json, Value};

    const NS: &str = "list.shortcuts";

    fn dev(n: u8) -> DeviceId {
        DeviceId([n; 10])
    }

    fn st(hlc: u64, d: u8) -> Stamp {
        Stamp::new(hlc, dev(d))
    }

    fn cell(v: Value, s: Stamp, anc: &[Stamp]) -> Cell {
        Cell {
            value: v,
            stamp: s,
            wall_ms: s.hlc as i64,
            anc: anc.to_vec(),
        }
    }

    fn rec(cells: &[(&str, Cell)]) -> Record {
        let mut r = Record::new(NS, "r1");
        for (k, c) in cells {
            r.cells.insert(k.parse().unwrap(), c.clone());
        }
        r
    }

    fn tomb(s: Stamp, anc: &[Stamp]) -> Tomb {
        Tomb {
            stamp: s,
            wall_ms: s.hlc as i64,
            anc: anc.to_vec(),
        }
    }

    fn merged(a: &Record, b: &Record) -> (Record, MergeOutcome) {
        let mut l = a.clone();
        let out = merge_record(&mut l, b);
        (l, out)
    }

    #[test]
    fn the_higher_stamp_wins_in_both_orders() {
        let a = rec(&[("name@all", cell(json!("A"), st(10, 1), &[]))]);
        let b = rec(&[("name@all", cell(json!("B"), st(20, 2), &[]))]);
        for (x, y) in [(&a, &b), (&b, &a)] {
            let (r, out) = merged(x, y);
            assert_eq!(r.cells[&FieldKey::all("name")].value, json!("B"));
            assert_eq!(out.conflicts.len(), 1);
            let c = &out.conflicts[0];
            assert_eq!(c.kind, ConflictKind::ConcurrentEdit);
            assert_eq!(c.loser.stamp, st(10, 1));
            assert_eq!(c.loser.value, Some(json!("A")));
            assert_eq!(c.winner.as_ref().unwrap().stamp, st(20, 2));
            assert_eq!(c.field, Some(FieldKey::all("name")));
        }
        let (_, out) = merged(&b, &a);
        assert!(!out.changed, "an older cell changes nothing");
        let (_, out) = merged(&a, &b);
        assert!(out.changed);
    }

    #[test]
    fn the_same_value_is_not_a_conflict() {
        let a = rec(&[("v@all", cell(json!(1.0), st(10, 1), &[]))]);
        let b = rec(&[("v@all", cell(json!(1), st(20, 2), &[]))]);
        let (r, out) = merged(&a, &b);
        assert!(out.conflicts.is_empty());
        assert_eq!(r.cells[&FieldKey::all("v")].stamp, st(20, 2));
    }

    #[test]
    fn a_winner_that_saw_the_loser_is_not_a_conflict() {
        let a = rec(&[("name@all", cell(json!("A"), st(10, 1), &[]))]);
        let b = rec(&[("name@all", cell(json!("B"), st(20, 2), &[st(10, 1)]))]);
        let (_, out) = merged(&a, &b);
        assert!(out.conflicts.is_empty());
        let (_, out) = merged(&b, &a);
        assert!(out.conflicts.is_empty());
    }

    #[test]
    fn one_computers_own_edits_are_ordered() {
        let first = cell(json!("A1"), st(10, 1), &[]);
        let second = cell(json!("A2"), st(30, 1), &ancestors(Some(&first), dev(1)));
        assert!(second.anc.is_empty(), "a run of my own edits counts as one");
        let (_, out) = merged(&rec(&[("name@all", first)]), &rec(&[("name@all", second)]));
        assert!(out.conflicts.is_empty());
    }

    #[test]
    fn ancestors_follow_the_base_rule() {
        assert!(ancestors(None, dev(1)).is_empty());
        let base = cell(json!(1), st(5, 2), &[st(4, 3), st(3, 1), st(2, 2)]);
        assert_eq!(
            ancestors(Some(&base), dev(1)),
            vec![st(5, 2), st(4, 3), st(3, 1)]
        );
        assert_eq!(
            ancestors(Some(&base), dev(2)),
            vec![st(4, 3), st(3, 1), st(2, 2)]
        );
    }

    #[test]
    fn equal_stamps_after_a_clone_break_by_hash_and_conflict() {
        let s = st(10, 1);
        let a = rec(&[("name@all", cell(json!("from A"), s, &[]))]);
        let b = rec(&[("name@all", cell(json!("from clone"), s, &[]))]);
        let (ab, out_ab) = merged(&a, &b);
        let (ba, out_ba) = merged(&b, &a);
        assert_eq!(ab, ba);
        let want = if hash(&json!("from A")) > hash(&json!("from clone")) {
            "from A"
        } else {
            "from clone"
        };
        assert_eq!(ab.cells[&FieldKey::all("name")].value, json!(want));
        assert_eq!(out_ab.conflicts.len(), 1);
        assert_eq!(out_ba.conflicts.len(), 1);
        assert_eq!(out_ab.conflicts[0], out_ba.conflicts[0]);
        // Same stamp, same value: nothing to do.
        let (_, out) = merged(&a, &a);
        assert!(!out.changed && out.conflicts.is_empty());
    }

    #[test]
    fn sets_and_maps_never_conflict() {
        let el = "tags[]\"x\"@all";
        let a = rec(&[
            (el, cell(json!(true), st(10, 1), &[])),
            ("m{}k@all", cell(json!("one"), st(10, 1), &[])),
        ]);
        let b = rec(&[
            (el, cell(json!(false), st(20, 2), &[])),
            ("m{}k@all", cell(json!("two"), st(20, 2), &[])),
        ]);
        let (r, out) = merged(&a, &b);
        assert!(out.conflicts.is_empty());
        assert_eq!(
            r.cells[&el.parse::<FieldKey>().unwrap()].value,
            json!(false)
        );
    }

    #[test]
    fn sync_namespaces_never_conflict() {
        let mut a = rec(&[("state@all", cell(json!("open"), st(10, 1), &[]))]);
        let mut b = rec(&[("state@all", cell(json!("dismissed"), st(20, 2), &[]))]);
        a.ns = "sync.conflicts".into();
        b.ns = "sync.conflicts".into();
        let (r, out) = merged(&a, &b);
        assert!(out.conflicts.is_empty());
        assert_eq!(r.cells[&FieldKey::all("state")].value, json!("dismissed"));
    }

    #[test]
    fn scopes_are_separate_cells() {
        let a = rec(&[("hotkey@macos", cell(json!("Cmd+K"), st(10, 1), &[]))]);
        let b = rec(&[("hotkey@windows", cell(json!("Ctrl+K"), st(20, 2), &[]))]);
        let (r, out) = merged(&a, &b);
        assert!(out.conflicts.is_empty());
        assert_eq!(r.cells.len(), 2);
        assert!(r.cell("hotkey", Scope::Os(OsKind::Macos)).is_some());
    }

    #[test]
    fn tombstones_hide_and_newer_cells_show_again() {
        let mut r = rec(&[("name@all", cell(json!("A"), st(10, 1), &[]))]);
        assert!(is_visible(&r));
        r.del = Some(tomb(st(20, 1), &[st(10, 1)]));
        assert!(!is_visible(&r));
        // An edit after the delete brings back the whole record.
        r.cells.insert(
            FieldKey::all("other"),
            cell(json!(1), st(30, 1), &[st(20, 1)]),
        );
        assert!(is_visible(&r));
        assert_eq!(r.cells.len(), 2);
        // A newer tombstone wins over an older one, in both orders.
        let older = Record {
            del: Some(tomb(st(25, 2), &[])),
            ..Record::new(NS, "r1")
        };
        let newer = Record {
            del: Some(tomb(st(40, 2), &[st(30, 1)])),
            ..Record::new(NS, "r1")
        };
        let (x, _) = merged(&older, &newer);
        let (y, _) = merged(&newer, &older);
        assert_eq!(x, y);
        assert_eq!(x.del.unwrap().stamp, st(40, 2));
    }

    #[test]
    fn edit_vs_delete_in_both_orders() {
        // A had seen name@10 and deletes at 30. B edited prompt at 20
        // without A seeing it.
        let seen = cell(json!("gh"), st(10, 1), &[]);
        let a = Record {
            del: Some(tomb(st(30, 1), &[st(10, 1)])),
            ..rec(&[("name@all", seen.clone())])
        };
        let b = rec(&[
            ("name@all", seen),
            ("prompt@all", cell(json!("new"), st(20, 2), &[])),
        ]);
        for (x, y) in [(&a, &b), (&b, &a)] {
            let (r, out) = merged(x, y);
            assert!(!is_visible(&r));
            assert_eq!(out.conflicts.len(), 1, "{:?}", out.conflicts);
            let c = &out.conflicts[0];
            assert_eq!(c.kind, ConflictKind::EditVsDelete);
            assert_eq!(c.field, Some(FieldKey::all("prompt")));
            assert_eq!(c.loser.value, Some(json!("new")));
            assert!(c.winner.as_ref().unwrap().deleted);
        }
        // The deleter saw the edit: no conflict.
        let saw = Record {
            del: Some(tomb(st(30, 1), &[st(20, 2), st(10, 1)])),
            ..Record::new(NS, "r1")
        };
        let (_, out) = merged(&b, &saw);
        assert!(out.conflicts.is_empty());
        // An edit older than what the deleter saw counts as seen.
        let old = rec(&[("prompt@all", cell(json!("old"), st(5, 2), &[]))]);
        let (_, out) = merged(&a, &old);
        assert!(out.conflicts.is_empty());
    }

    #[test]
    fn delete_vs_edit_in_both_orders() {
        let base = cell(json!("gh"), st(10, 1), &[]);
        let a = Record {
            del: Some(tomb(st(20, 1), &[st(10, 1)])),
            ..rec(&[("name@all", base)])
        };
        // B changed it later, without seeing the delete.
        let b = rec(&[("name@all", cell(json!("gh2"), st(30, 2), &[st(10, 1)]))]);
        for (x, y) in [(&a, &b), (&b, &a)] {
            let (r, out) = merged(x, y);
            assert!(is_visible(&r));
            assert_eq!(out.conflicts.len(), 1, "{:?}", out.conflicts);
            let c = &out.conflicts[0];
            assert_eq!(c.kind, ConflictKind::DeleteVsEdit);
            assert_eq!(c.field, None);
            assert!(c.loser.deleted);
            assert_eq!(c.loser.stamp, st(20, 1));
            assert_eq!(c.winner.as_ref().unwrap().stamp, st(30, 2));
        }
        // A deliberate restore names the tombstone: no conflict.
        let restored = rec(&[(
            "name@all",
            cell(json!("gh"), st(30, 2), &[st(20, 1), st(10, 1)]),
        )]);
        for (x, y) in [(&a, &restored), (&restored, &a)] {
            let (r, out) = merged(x, y);
            assert!(is_visible(&r));
            assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
        }
    }

    #[test]
    fn delete_vs_edit_is_reported_once_per_delete() {
        let a = Record {
            del: Some(tomb(st(20, 1), &[st(10, 1)])),
            ..rec(&[("name@all", cell(json!("gh"), st(10, 1), &[]))])
        };
        let b = rec(&[("name@all", cell(json!("gh2"), st(30, 2), &[st(10, 1)]))]);
        let (shown, out) = merged(&a, &b);
        assert_eq!(out.conflicts.len(), 1);
        // A later edit of the record that is already back is no news.
        let c = rec(&[("prompt@all", cell(json!("p"), st(40, 3), &[]))]);
        let (_, out) = merged(&shown, &c);
        assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
    }

    #[test]
    fn a_merge_into_an_empty_record_takes_its_identity() {
        let b = rec(&[("name@all", cell(json!("B"), st(20, 2), &[]))]);
        let (r, out) = merged(&Record::default(), &b);
        assert_eq!(r, b);
        assert!(out.changed && out.conflicts.is_empty());
    }

    // ---- Property tests ----------------------------------------------------

    use crate::sync::hlc::Hlc;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use std::collections::{BTreeMap, HashMap, HashSet};

    /// Seeded cases. CI runs all of them (§18.1).
    const CASES: u64 = 10_000;

    const FIELDS: [&str; 5] = ["a@all", "b@all", "a@macos", "tags[]1@all", "m{}k@all"];

    /// What the merges of a run accounted for.
    #[derive(Default)]
    struct Ledger {
        /// Conflict losers anyone recorded.
        losers: HashSet<Stamp>,
        /// `(lo, w)`: `lo` was replaced by `w` with the same value.
        absorbed: Vec<(Stamp, Stamp)>,
    }

    impl Ledger {
        fn merge(&mut self, local: &mut Record, inc: &Record) {
            for (fk, c) in &inc.cells {
                if let Some(cur) = local.cells.get(fk) {
                    if cur.stamp != c.stamp && same(&cur.value, &c.value) {
                        let (lo, w) = if cur.stamp < c.stamp {
                            (cur, c)
                        } else {
                            (c, cur)
                        };
                        self.absorbed.push((lo.stamp, w.stamp));
                    }
                }
            }
            for c in merge_record(local, inc).conflicts {
                if !c.loser.deleted {
                    self.losers.insert(c.loser.stamp);
                }
            }
        }
    }

    /// One simulated run: 2–5 computers edit, delete, restore and sync one
    /// record, each following the base rule.
    struct Sim {
        rng: StdRng,
        devs: Vec<DeviceId>,
        hlcs: Vec<Hlc>,
        walls: Vec<i64>,
        reps: Vec<Record>,
        /// Every state some computer had: the files others may read.
        sent: Vec<Record>,
        /// Causal past of each cell: its base, and the base's past.
        past: HashMap<Stamp, HashSet<Stamp>>,
        /// Field of each written cell.
        field_of: HashMap<Stamp, FieldKey>,
        ledger: Ledger,
        /// What happened, printed when a check fails.
        trace: Vec<String>,
    }

    impl Sim {
        fn new(seed: u64) -> Self {
            let mut rng = StdRng::seed_from_u64(seed);
            let n = rng.random_range(2..=5);
            let devs = (0..n).map(|i| DeviceId([i as u8 + 1; 10])).collect();
            // Clock skew of up to ±2 hours.
            let walls = (0..n)
                .map(|_| 1_000_000_000 + rng.random_range(-7_200_000..7_200_000))
                .collect();
            Self {
                rng,
                devs,
                hlcs: vec![Hlc::new(0); n],
                walls,
                reps: vec![Record::new(NS, "r1"); n],
                sent: Vec::new(),
                past: HashMap::new(),
                field_of: HashMap::new(),
                ledger: Ledger::default(),
                trace: Vec::new(),
            }
        }

        fn stamp(&mut self, i: usize, floor: u64) -> Stamp {
            Stamp::new(self.hlcs[i].tick(self.walls[i], floor), self.devs[i])
        }

        fn note(&mut self, s: Stamp, fk: &FieldKey, base: Option<Stamp>) {
            let mut past = HashSet::new();
            if let Some(b) = base {
                past.insert(b);
                past.extend(self.past.get(&b).into_iter().flatten().copied());
            }
            self.past.insert(s, past);
            let again = self.field_of.insert(s, fk.clone());
            assert!(again.is_none(), "stamp {s} issued twice");
        }

        fn edit(&mut self, i: usize) {
            let fk: FieldKey = FIELDS[self.rng.random_range(0..FIELDS.len())]
                .parse()
                .unwrap();
            let value = if fk.name.contains("[]") {
                json!(self.rng.random_bool(0.5))
            } else {
                json!(self.rng.random_range(0..4))
            };
            let base = self.reps[i].cells.get(&fk).cloned();
            let s = self.stamp(i, base.as_ref().map_or(0, |b| b.stamp.hlc));
            self.note(s, &fk, base.as_ref().map(|b| b.stamp));
            let c = Cell {
                value,
                stamp: s,
                wall_ms: self.walls[i],
                anc: ancestors(base.as_ref(), self.devs[i]),
            };
            self.trace.push(format!(
                "{i} edit {fk} = {} at {s} anc {:?} (base {:?})",
                c.value,
                c.anc.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                base.map(|b| b.stamp.to_string())
            ));
            self.reps[i].cells.insert(fk, c);
        }

        fn delete(&mut self, i: usize) {
            let floor = self.reps[i].max_stamp().map_or(0, |s| s.hlc);
            let s = self.stamp(i, floor);
            let anc = self.reps[i].newest_cell_stamps(MAX_ANCESTORS);
            self.trace.push(format!("{i} delete at {s}"));
            self.reps[i].del = Some(Tomb {
                stamp: s,
                wall_ms: self.walls[i],
                anc,
            });
        }

        /// "Recently deleted" → Restore: re-stamp every kept cell with the
        /// tombstone in `anc`.
        fn restore(&mut self, i: usize) {
            let Some(d) = self.reps[i].del.clone() else {
                return;
            };
            let cells: Vec<(FieldKey, Cell)> = self.reps[i]
                .cells
                .iter()
                .map(|(k, c)| (k.clone(), c.clone()))
                .collect();
            for (fk, c) in cells {
                let s = self.stamp(i, c.stamp.hlc.max(d.stamp.hlc));
                self.note(s, &fk, Some(c.stamp));
                self.trace
                    .push(format!("{i} restore {fk} at {s} from {}", c.stamp));
                let anc = [d.stamp, c.stamp]
                    .into_iter()
                    .chain(c.anc.iter().copied())
                    .take(MAX_ANCESTORS)
                    .collect();
                let restored = Cell {
                    value: c.value,
                    stamp: s,
                    wall_ms: self.walls[i],
                    anc,
                };
                self.reps[i].cells.insert(fk, restored);
            }
        }

        fn run(&mut self) {
            let n = self.devs.len();
            let ops = self.rng.random_range(10..30);
            for _ in 0..ops {
                let i = self.rng.random_range(0..n);
                self.walls[i] += self.rng.random_range(0..5_000);
                let roll = self.rng.random_range(0..100);
                if roll < 45 {
                    if is_visible(&self.reps[i]) {
                        self.edit(i);
                    } else if self.rng.random_bool(0.5) {
                        self.restore(i);
                    }
                } else if roll < 52 {
                    if is_visible(&self.reps[i]) && !self.reps[i].cells.is_empty() {
                        self.delete(i);
                    }
                } else if !self.sent.is_empty() {
                    // Read some earlier state: late, reordered, duplicated.
                    let k = if self.rng.random_bool(0.5) {
                        self.sent.len() - 1
                    } else {
                        self.rng.random_range(0..self.sent.len())
                    };
                    let inc = self.sent[k].clone();
                    self.trace.push(format!("{i} reads state #{k}"));
                    self.ledger.merge(&mut self.reps[i], &inc);
                    if let Some(m) = inc.max_stamp() {
                        self.hlcs[i].observe(m.hlc, self.walls[i]);
                    }
                }
                self.sent.push(self.reps[i].clone());
            }
        }
    }

    /// `KAGE_SYNC_PROP_SEED=<n>` runs one case and prints its trace.
    #[test]
    fn property_every_order_converges_and_nothing_is_lost_silently() {
        let only: Option<u64> = std::env::var("KAGE_SYNC_PROP_SEED")
            .ok()
            .and_then(|v| v.parse().ok());
        let seeds = match only {
            Some(s) => s..s + 1,
            None => 0..CASES,
        };
        // Not vacuous: enough runs lose values, delete and conflict.
        let (mut with_losers, mut deleted_at_end, mut runs) = (0, 0, 0);
        for seed in seeds {
            let mut sim = Sim::new(seed);
            sim.run();
            let sent = std::mem::take(&mut sim.sent);
            let mut ledger = std::mem::take(&mut sim.ledger);
            runs += 1;

            // Every delivery order, with duplicates, gives the same record.
            let mut fin: Option<Record> = None;
            for _ in 0..3 {
                let mut order: Vec<usize> = (0..sent.len()).collect();
                for _ in 0..sim.rng.random_range(0..4) {
                    order.push(sim.rng.random_range(0..sent.len()));
                }
                for j in (1..order.len()).rev() {
                    order.swap(j, sim.rng.random_range(0..=j));
                }
                let mut r = Record::default();
                for k in order {
                    ledger.merge(&mut r, &sent[k]);
                }
                match &fin {
                    None => fin = Some(r),
                    Some(f) => assert_eq!(&r, f, "seed {seed}: the order changed the result"),
                }
            }
            let fin = fin.unwrap();

            // Commutative, associative, idempotent.
            let mut pick = || sent[sim.rng.random_range(0..sent.len())].clone();
            let (a, b, c) = (pick(), pick(), pick());
            let m = |x: &Record, y: &Record| merged(x, y).0;
            assert_eq!(m(&a, &b), m(&b, &a), "seed {seed}: not commutative");
            assert_eq!(
                m(&m(&a, &b), &c),
                m(&a, &m(&b, &c)),
                "seed {seed}: not associative"
            );
            assert_eq!(m(&a, &a), a, "seed {seed}: not idempotent");

            // No silent loss: every value written is the final one, a
            // recorded loser, or in the causal past of one of those.
            let mut accounted: HashSet<Stamp> = fin.cells.values().map(|c| c.stamp).collect();
            accounted.extend(ledger.losers.iter().copied());
            let mut by_winner: BTreeMap<Stamp, Vec<Stamp>> = BTreeMap::new();
            for (lo, w) in &ledger.absorbed {
                by_winner.entry(*w).or_default().push(*lo);
            }
            let mut todo: Vec<Stamp> = accounted.iter().copied().collect();
            while let Some(s) = todo.pop() {
                let next = sim
                    .past
                    .get(&s)
                    .into_iter()
                    .flatten()
                    .chain(by_winner.get(&s).into_iter().flatten());
                for p in next {
                    if accounted.insert(*p) {
                        todo.push(*p);
                    }
                }
            }
            for (s, fk) in &sim.field_of {
                if is_union(&fk.name) {
                    continue;
                }
                if !accounted.contains(s) {
                    eprintln!("{}", sim.trace.join("\n"));
                    eprintln!("final: {:?}", fin);
                }
                assert!(
                    accounted.contains(s),
                    "seed {seed}: the value at {s} ({fk}) was lost without a conflict"
                );
            }
            with_losers += usize::from(!ledger.losers.is_empty());
            deleted_at_end += usize::from(!is_visible(&fin));
        }
        if runs == CASES as usize {
            assert!(
                with_losers > runs / 4,
                "only {with_losers} runs had conflicts"
            );
            assert!(
                deleted_at_end > runs / 50,
                "only {deleted_at_end} runs ended deleted"
            );
        }
    }
}
