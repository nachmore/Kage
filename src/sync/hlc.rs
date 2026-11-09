//! Hybrid logical clock (docs/design/SYNC.md §7.3). Physical part: wall
//! milliseconds shifted left 16 bits; logical part: the low 16 bits.
//!
//! C0 skeleton: `tick` and `observe` don't advance yet. WP-S1 writes them.

/// The clock of one computer. `last` is saved in the replica's `meta` in
/// the same transaction as the cells it stamps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hlc {
    last: u64,
}

impl Hlc {
    /// A clock that continues after `last`, the value saved before.
    pub fn new(last: u64) -> Self {
        Self { last }
    }

    /// A local event. `floor` is the highest stamp of the cell being
    /// replaced (both the one the user saw and the one the replica holds),
    /// so the new stamp is newer than both.
    pub fn tick(&mut self, wall_ms: i64, floor: u64) -> u64 {
        let _ = (wall_ms, floor);
        self.last
    }

    /// A remote stamp was merged. Advances, but never past wall + 1 hour.
    pub fn observe(&mut self, remote: u64, wall_ms: i64) {
        let _ = (remote, wall_ms);
    }

    /// The newest stamp issued or observed.
    pub fn last(&self) -> u64 {
        self.last
    }
}
