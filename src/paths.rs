//! Kage's per-user directories. Resolve them through here, never via
//! `dirs::*_dir()` directly — see `kage_core::paths` for why.
//!
//! Unit tests of this crate are isolated automatically: the first lookup
//! points every directory at a per-process scratch dir. Integration tests
//! link the non-test library and must call [`isolate`] themselves (the
//! mock-app harness does, and its command sweep asserts it).

use std::path::PathBuf;

#[allow(unused_imports)] // the full surface, for tests and future callers
pub use kage_core::paths::{
    is_isolated, is_under_isolation_root, isolate, isolation_root, USER_DIRS_ENV,
};

#[inline]
fn guard() {
    #[cfg(test)]
    test_isolation();
}

#[cfg(test)]
fn test_isolation() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root = std::env::temp_dir().join(format!("kage-unit-test-dirs-{}", std::process::id()));
        kage_core::paths::isolate(root);
    });
}

pub fn config_dir() -> Option<PathBuf> {
    guard();
    kage_core::paths::config_dir()
}

pub fn data_local_dir() -> Option<PathBuf> {
    guard();
    kage_core::paths::data_local_dir()
}

pub fn data_dir() -> Option<PathBuf> {
    guard();
    kage_core::paths::data_dir()
}

pub fn cache_dir() -> Option<PathBuf> {
    guard();
    kage_core::paths::cache_dir()
}

pub fn home_dir() -> Option<PathBuf> {
    guard();
    kage_core::paths::home_dir()
}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_tests_never_resolve_real_user_dirs() {
        let dir = super::config_dir().expect("resolves");
        assert!(super::is_under_isolation_root(&dir), "{}", dir.display());
        assert_ne!(Some(dir), dirs::config_dir());
    }
}
