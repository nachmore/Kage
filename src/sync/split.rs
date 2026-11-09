//! A record's fields to cell field names and back (docs/design/SYNC.md
//! §7.2): whole fields, `+group`, `set[]element` and `map{}key`. Scopes are
//! added by the caller.
//!
//! C0 skeleton: both directions return nothing. WP-S1 writes the bodies.

use super::types::Structure;
use std::collections::BTreeMap;

/// Split a record's fields into cell field names and values.
pub fn split(
    fields: &serde_json::Map<String, serde_json::Value>,
    s: &Structure,
) -> BTreeMap<String, serde_json::Value> {
    let _ = (fields, s);
    BTreeMap::new()
}

/// Rebuild a record's fields from its cells. `split` then `join` round-trips.
pub fn join<'a>(
    cells: impl Iterator<Item = (&'a str, &'a serde_json::Value)>,
    s: &Structure,
) -> serde_json::Map<String, serde_json::Value> {
    let _ = (cells, s);
    serde_json::Map::new()
}
