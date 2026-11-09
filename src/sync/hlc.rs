//! Hybrid logical clock (docs/design/SYNC.md §7.3). Physical part: wall
//! milliseconds shifted left 16 bits; logical part: the low 16 bits.
//!
//! Two rules keep the clock honest:
//! - A local event is newer than everything this computer issued, the
//!   wall clock, and the `floor` (the newest stamp of the cell it
//!   replaces), so the user's edit wins over what they were looking at.
//!   Every stamp a computer issues is larger than the one before, so two
//!   cells never share a stamp unless one is a copy of the other.
//! - Merging remote stamps moves the clock at most 1 hour past this
//!   computer's wall clock. A computer whose clock is years ahead can't
//!   drag every other computer with it just by being read. Only an edit
//!   of a value it stamped follows it, to beat that value.

/// How far past its own wall clock a computer's clock may be pulled by
/// what it reads.
pub const MAX_AHEAD_MS: i64 = 3_600_000;

/// Bits of the logical counter.
pub const LOGICAL_BITS: u32 = 16;

/// The clock of one computer. `last` is saved in the replica's `meta` in
/// the same transaction as the cells it stamps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hlc {
    last: u64,
}

/// The HLC value of wall time `wall_ms` with a zero counter. Times before
/// 1970 count as 1970; the shift saturates instead of wrapping.
pub fn physical(wall_ms: i64) -> u64 {
    (wall_ms.max(0) as u64).saturating_mul(1 << LOGICAL_BITS)
}

/// The wall time an HLC value carries, for display and ages only (R10).
pub fn wall_ms_of(hlc: u64) -> i64 {
    (hlc >> LOGICAL_BITS).min(i64::MAX as u64) as i64
}

impl Hlc {
    /// A clock that continues after `last`, the value saved before.
    pub fn new(last: u64) -> Self {
        Self { last }
    }

    /// A local event. `floor` is the highest stamp of the cell being
    /// replaced (both the one the user saw and the one the replica holds),
    /// so the new stamp is newer than both. Pass 0 when nothing is
    /// replaced.
    ///
    /// The floor isn't capped: the user's edit must beat the value they
    /// replaced, even one from a computer whose clock is far ahead. The
    /// clock follows it, so every stamp stays unique.
    pub fn tick(&mut self, wall_ms: i64, floor: u64) -> u64 {
        self.last = self
            .last
            .saturating_add(1)
            .max(physical(wall_ms))
            .max(floor.saturating_add(1));
        self.last
    }

    /// A remote stamp was merged. Advances, but never past wall + 1 hour.
    pub fn observe(&mut self, remote: u64, wall_ms: i64) {
        let cap = physical(wall_ms.saturating_add(MAX_AHEAD_MS));
        self.last = self.last.max(remote.min(cap));
    }

    /// The newest stamp issued or observed (within the cap).
    pub fn last(&self) -> u64 {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{DeviceId, Stamp};

    const T: i64 = 1_762_686_000_000;

    #[test]
    fn monotonic_when_the_wall_clock_steps_back() {
        let mut h = Hlc::new(0);
        let a = h.tick(T, 0);
        assert_eq!(a, physical(T));
        let b = h.tick(T - 60_000, 0);
        let c = h.tick(T - 3_600_000, 0);
        assert!(a < b && b < c);
        assert_eq!(c, a + 2, "stepping back only counts on");
        let d = h.tick(T + 1, 0);
        assert_eq!(d, physical(T + 1));
    }

    #[test]
    fn the_floor_is_respected() {
        let mut h = Hlc::new(0);
        let remote = physical(T + 10_000) + 5;
        let s = h.tick(T, remote);
        assert_eq!(s, remote + 1);
        assert_eq!(h.last(), s);
        let next = h.tick(T, 0);
        assert!(next > s);
    }

    #[test]
    fn a_far_floor_wins_the_edit_and_stamps_stay_unique() {
        let mut h = Hlc::new(0);
        let ahead = physical(T + 2 * MAX_AHEAD_MS);
        // Several cells replaced over one far-ahead value (a restore over a
        // tombstone from a fast clock) each get their own stamp.
        let stamps: Vec<u64> = (0..4).map(|_| h.tick(T, ahead)).collect();
        assert_eq!(stamps[0], ahead + 1);
        assert!(stamps.windows(2).all(|w| w[0] < w[1]), "{stamps:?}");
        assert!(h.tick(T, 0) > stamps[3]);
    }

    #[test]
    fn observe_is_capped_at_one_hour() {
        let mut h = Hlc::new(physical(T));
        h.observe(physical(T + 60_000), T);
        assert_eq!(h.last(), physical(T + 60_000));
        h.observe(physical(T + 86_400_000), T);
        assert_eq!(h.last(), physical(T + MAX_AHEAD_MS));
        h.observe(physical(T - 60_000), T);
        assert_eq!(h.last(), physical(T + MAX_AHEAD_MS), "never moves back");
        // Ticks after a capped observe stay within the cap.
        let s = h.tick(T, 0);
        assert_eq!(s, physical(T + MAX_AHEAD_MS) + 1);
    }

    #[test]
    fn the_logical_counter_carries() {
        let full = physical(T) | 0xffff;
        let mut h = Hlc::new(full);
        let s = h.tick(T, 0);
        assert_eq!(s, physical(T + 1));
        assert_eq!(wall_ms_of(s), T + 1);
        assert_eq!(s & 0xffff, 0);
    }

    #[test]
    fn persistence_round_trip() {
        let mut a = Hlc::new(0);
        for i in 0..10 {
            a.tick(T + i, 0);
        }
        let saved = Stamp::new(a.last(), DeviceId([3; 10])).to_string();
        let mut b = Hlc::new(saved.parse::<Stamp>().unwrap().hlc);
        assert_eq!(b, a);
        // A restart with the wall clock behind never issues an older stamp.
        assert!(b.tick(T - 86_400_000, 0) > a.last());
    }

    #[test]
    fn the_text_form_sorts_like_the_number() {
        let dev = DeviceId([7; 10]);
        let mut h = Hlc::new(0);
        let mut prev = Stamp::new(h.tick(T, 0), dev).to_string();
        for i in 0..2_000 {
            let wall = T + (i % 7) * 13 - 40;
            let s = Stamp::new(h.tick(wall, 0), dev).to_string();
            assert!(s > prev, "{s} after {prev}");
            prev = s;
        }
        let small = Stamp::new(physical(1), dev).to_string();
        let big = Stamp::new(physical(T), dev).to_string();
        assert!(small < big);
    }

    #[test]
    fn extremes_saturate() {
        let mut h = Hlc::new(u64::MAX - 1);
        assert_eq!(h.tick(T, 0), u64::MAX);
        assert_eq!(h.tick(T, u64::MAX), u64::MAX);
        assert_eq!(physical(-5), 0);
        assert_eq!(physical(i64::MAX), u64::MAX);
        let mut h = Hlc::new(0);
        h.observe(u64::MAX, i64::MAX);
        assert_eq!(h.last(), u64::MAX);
    }
}
