//! The conflicts list as synced records in `sync.conflicts`
//! (docs/design/SYNC.md §10): deterministic ids and cells, and the
//! retention pass.
//!
//! C0 skeleton: WP-S1 writes the bodies.

use super::hlc::Hlc;
use super::replica::Tx;
use super::types::{ConflictBody, DeviceId, Record, Stamp};

/// `c` + the first 16 hex of `HMAC(name key, ns | id | field | loser stamp)`.
pub fn conflict_id(name_key: &[u8; 32], body: &ConflictBody) -> String {
    let _ = (name_key, body);
    String::new()
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
    let _ = (body, state, stamp, wall_ms);
    Record {
        ns: "sync.conflicts".to_string(),
        id: id.to_string(),
        ..Record::default()
    }
}

/// Dismiss open entries older than 90 days, tombstone resolved ones older
/// than 30, and keep the list within its count and size limits (§10.3).
/// Returns how many records it changed.
pub fn retention_pass(
    tx: &mut Tx<'_>,
    now_ms: i64,
    me: DeviceId,
    hlc: &mut Hlc,
) -> anyhow::Result<usize> {
    let _ = (tx, now_ms, me, hlc);
    Ok(0)
}
