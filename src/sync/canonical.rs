//! Canonical JSON bytes and their hash, so equal values compare equal on
//! every computer and in every Kage version (docs/design/SYNC.md §7.2).
//!
//! C0 skeleton: WP-S1 writes the bodies.

/// `v` as canonical JSON: sorted keys, no spaces, shortest numbers.
pub fn canonical(v: &serde_json::Value) -> Vec<u8> {
    let _ = v;
    Vec::new()
}

/// SHA-256 of [`canonical`].
pub fn hash(v: &serde_json::Value) -> [u8; 32] {
    let _ = v;
    [0; 32]
}
