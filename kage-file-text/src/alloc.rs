//! A counting allocator with a hard cap (EXTENSION_FILE_TEXT §5.5).
//!
//! The worker installs one as its global allocator with
//! `WORKER_ALLOC_CAP_BYTES`. It counts the bytes currently allocated. An
//! allocation that would take the count past the cap fails, and Rust
//! aborts the process ("memory allocation of N bytes failed"). The app
//! sees an abnormal exit and reports `too_complex`.
//!
//! This is the worker's memory limit on every OS. macOS doesn't enforce
//! `RLIMIT_AS`, so there it is the only one.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Wraps the system allocator and refuses to go past `cap` live bytes.
pub struct CappedAlloc {
    cap: usize,
    live: AtomicUsize,
}

impl CappedAlloc {
    /// An allocator that allows at most `cap` bytes at once.
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            live: AtomicUsize::new(0),
        }
    }

    /// The cap in bytes.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Bytes allocated right now.
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    /// Counts `size` more bytes, or returns false if that passes the cap.
    fn reserve(&self, size: usize) -> bool {
        let mut current = self.live.load(Ordering::Relaxed);
        loop {
            let next = match current.checked_add(size) {
                Some(next) if next <= self.cap => next,
                _ => return false,
            };
            match self.live.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(seen) => current = seen,
            }
        }
    }

    fn release(&self, size: usize) {
        self.live.fetch_sub(size, Ordering::Relaxed);
    }
}

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged, and only adds bookkeeping. A refused allocation returns
// null, which `GlobalAlloc` allows.
unsafe impl GlobalAlloc for CappedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let ptr = unsafe { System.alloc(layout) };
        if ptr.is_null() {
            self.release(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !self.reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if ptr.is_null() {
            self.release(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        self.release(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_size = layout.size();
        if new_size > old_size {
            let extra = new_size - old_size;
            if !self.reserve(extra) {
                return std::ptr::null_mut();
            }
            let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
            if new_ptr.is_null() {
                self.release(extra);
            }
            new_ptr
        } else {
            let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
            if !new_ptr.is_null() {
                self.release(old_size - new_size);
            }
            new_ptr
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size, 8).unwrap()
    }

    #[test]
    fn counts_and_releases() {
        let a = CappedAlloc::new(1024);
        unsafe {
            let p = a.alloc(layout(100));
            assert!(!p.is_null());
            assert_eq!(a.live(), 100);
            let q = a.alloc_zeroed(layout(200));
            assert!(!q.is_null());
            assert_eq!(*q, 0);
            assert_eq!(a.live(), 300);
            a.dealloc(p, layout(100));
            a.dealloc(q, layout(200));
        }
        assert_eq!(a.live(), 0);
    }

    #[test]
    fn refuses_past_the_cap() {
        let a = CappedAlloc::new(1024);
        unsafe {
            let p = a.alloc(layout(1000));
            assert!(!p.is_null());
            assert!(a.alloc(layout(25)).is_null());
            assert!(a.alloc_zeroed(layout(25)).is_null());
            assert_eq!(a.live(), 1000, "a refused allocation isn't counted");
            let q = a.alloc(layout(24));
            assert!(!q.is_null(), "exactly up to the cap is allowed");
            a.dealloc(q, layout(24));
            a.dealloc(p, layout(1000));
        }
        assert_eq!(a.live(), 0);
    }

    #[test]
    fn realloc_counts_the_difference() {
        let a = CappedAlloc::new(1024);
        unsafe {
            let p = a.alloc(layout(100));
            let p = a.realloc(p, layout(100), 600);
            assert!(!p.is_null());
            assert_eq!(a.live(), 600);
            assert!(
                a.realloc(p, layout(600), 2000).is_null(),
                "growing past the cap fails"
            );
            assert_eq!(a.live(), 600, "the old block is still counted");
            let p = a.realloc(p, layout(600), 50);
            assert!(!p.is_null());
            assert_eq!(a.live(), 50);
            a.dealloc(p, layout(50));
        }
        assert_eq!(a.live(), 0);
    }

    #[test]
    fn huge_requests_dont_overflow_the_count() {
        let a = CappedAlloc::new(usize::MAX - 1);
        unsafe {
            let p = a.alloc(layout(16));
            assert!(!p.is_null());
            assert!(!a.reserve(usize::MAX));
            a.dealloc(p, layout(16));
        }
    }
}
