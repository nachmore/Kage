//! Merging records cell by cell (docs/design/SYNC.md §8.5). Every cell
//! merges the same way: the higher stamp wins. A replaced value the winner
//! doesn't name in `anc` becomes a conflict.
//!
//! C0 skeleton: nothing merges yet. WP-S1 writes the bodies and the
//! property tests.

use super::types::{Cell, ConflictBody, DeviceId, Record, Stamp};

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
pub fn merge_record(local: &mut Record, incoming: &Record) -> MergeOutcome {
    let _ = (local, incoming);
    MergeOutcome::default()
}

/// A record is visible when it has no tombstone, or some cell is newer
/// than the tombstone (§7.2).
pub fn is_visible(r: &Record) -> bool {
    let _ = r;
    false
}

/// The `anc` of a new cell that replaces `base`, the cell the user saw
/// (§7.4).
pub fn ancestors(base: Option<&Cell>, me: DeviceId) -> Vec<Stamp> {
    let _ = (base, me);
    Vec::new()
}
