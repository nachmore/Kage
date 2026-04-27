//! Resilient mutex/rwlock extensions.
//!
//! Mutex poisoning in this app is almost always a nuisance rather than actual
//! data corruption: a panic in one thread leaves the lock "poisoned" and every
//! subsequent `.lock().unwrap()` panics the caller too, cascading the failure.
//!
//! `LockExt` recovers the inner guard instead of panicking, logging a warning
//! on the first recovery so operators still get a signal in `kage.log`. For
//! long-running desktop daemons this is almost always what we want: a transient
//! panic somewhere shouldn't take down every background task that touches the
//! same state.
//!
//! If you genuinely need "panic on poison" semantics (e.g. the poisoned state
//! is actually invalid), keep using `.lock().unwrap()` explicitly.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Extension trait for `std::sync::Mutex` that recovers from lock poisoning.
pub trait LockExt<T> {
    /// Acquire the lock, recovering the inner data if the mutex is poisoned.
    /// Logs a warning on recovery so the event still appears in logs.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        match self.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!(
                    "Mutex was poisoned (recovering inner data) — a prior panic left this lock in a poisoned state. \
                     Check earlier log entries for the root cause."
                );
                poisoned.into_inner()
            }
        }
    }
}

/// Extension trait for `std::sync::RwLock` that recovers from lock poisoning.
#[allow(dead_code)] // Not used yet, but mirrors LockExt for when RwLock usage appears
pub trait RwLockExt<T> {
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T>;
    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> RwLockExt<T> for RwLock<T> {
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T> {
        match self.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!("RwLock was poisoned (recovering read guard)");
                poisoned.into_inner()
            }
        }
    }

    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T> {
        match self.write() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!("RwLock was poisoned (recovering write guard)");
                poisoned.into_inner()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn recovers_from_poisoned_mutex() {
        let m = Arc::new(Mutex::new(42_u32));
        let m2 = Arc::clone(&m);

        // Poison the mutex by panicking while holding the lock
        let _ = thread::spawn(move || {
            let _guard = m2.lock().unwrap();
            panic!("intentional poison");
        })
        .join();

        // Lock should be poisoned now
        assert!(m.lock().is_err(), "expected mutex to be poisoned");

        // But lock_or_recover returns the inner value successfully
        let guard = m.lock_or_recover();
        assert_eq!(*guard, 42);
    }

    #[test]
    fn works_on_healthy_mutex() {
        let m = Mutex::new(String::from("hello"));
        let guard = m.lock_or_recover();
        assert_eq!(&*guard, "hello");
    }

    #[test]
    fn recovers_from_poisoned_rwlock() {
        let rw = Arc::new(RwLock::new(vec![1, 2, 3]));
        let rw2 = Arc::clone(&rw);

        let _ = thread::spawn(move || {
            let _guard = rw2.write().unwrap();
            panic!("intentional poison");
        })
        .join();

        assert!(rw.read().is_err());
        let r = rw.read_or_recover();
        assert_eq!(&*r, &vec![1, 2, 3]);
    }
}
