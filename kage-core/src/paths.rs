//! The per-user directories Kage reads and writes — config, logs, caches,
//! and the home-relative files of the agents it integrates with — behind
//! one switch that points them all somewhere else.
//!
//! Production resolves exactly like the `dirs` crate. Tests must not: the
//! integration harness dispatches every Tauri command against the real
//! OS, and on 2026-11-03 a local `cargo test` overwrote a developer's
//! `config.json` with `Config::default()` and emptied their permission
//! audit log. Setting `HOME` isn't enough — on Windows `dirs` asks the
//! shell's known-folder API, not the environment — and mutating the
//! environment from a multi-threaded test binary is itself unsound. So
//! Kage code resolves these directories through here, and a test calls
//! [`isolate`] once before anything touches them.
//!
//! [`USER_DIRS_ENV`] does the same for a whole process tree (e.g. a
//! sidecar a test spawns, or a manual run against a scratch profile).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Environment variable naming a directory to use instead of the real
/// per-user directories (`<it>/config`, `<it>/data-local`, `<it>/data`,
/// `<it>/cache`, `<it>/home`). Empty means unset.
pub const USER_DIRS_ENV: &str = "KAGE_USER_DIRS";

static ISOLATED_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Redirect every directory in this module to subdirectories of `root`
/// for the rest of the process. First call wins; returns whether `root`
/// is now the active root (`false` if a different one was set earlier).
pub fn isolate(root: impl Into<PathBuf>) -> bool {
    let root = root.into();
    ISOLATED_ROOT.get_or_init(|| root.clone()) == &root
}

/// The override root, if any (an [`isolate`] call, else [`USER_DIRS_ENV`]).
pub fn isolation_root() -> Option<PathBuf> {
    ISOLATED_ROOT.get().cloned().or_else(|| {
        std::env::var_os(USER_DIRS_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
}

/// Whether the directories are redirected (tests assert this before
/// doing anything that writes).
pub fn is_isolated() -> bool {
    isolation_root().is_some()
}

fn resolve(sub: &str, real: fn() -> Option<PathBuf>) -> Option<PathBuf> {
    match isolation_root() {
        Some(root) => Some(root.join(sub)),
        None => real(),
    }
}

/// `dirs::config_dir()` (Kage keeps its own files under `<it>/kage`).
pub fn config_dir() -> Option<PathBuf> {
    resolve("config", dirs::config_dir)
}

/// `dirs::data_local_dir()` (logs, crash markers).
pub fn data_local_dir() -> Option<PathBuf> {
    resolve("data-local", dirs::data_local_dir)
}

/// `dirs::data_dir()`.
pub fn data_dir() -> Option<PathBuf> {
    resolve("data", dirs::data_dir)
}

/// `dirs::cache_dir()`.
pub fn cache_dir() -> Option<PathBuf> {
    resolve("cache", dirs::cache_dir)
}

/// `dirs::home_dir()` — for the files Kage reads or writes under the
/// user's home (agent `mcp.json` registrations, session stores, the
/// macOS LaunchAgent). Not for showing paths to the user.
pub fn home_dir() -> Option<PathBuf> {
    resolve("home", dirs::home_dir)
}

/// Whether `path` lies under the isolation root (for test assertions).
pub fn is_under_isolation_root(path: &Path) -> bool {
    isolation_root().is_some_and(|root| path.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_dirs_live_under_the_root_and_first_call_wins() {
        let root = std::env::temp_dir().join(format!("kage-paths-test-{}", std::process::id()));
        assert!(isolate(&root));
        assert!(
            !isolate(root.join("other")),
            "a second root must not replace the first"
        );
        assert!(is_isolated());
        for dir in [
            config_dir(),
            data_local_dir(),
            data_dir(),
            cache_dir(),
            home_dir(),
        ] {
            let dir = dir.expect("isolated dirs always resolve");
            assert!(is_under_isolation_root(&dir), "{}", dir.display());
        }
        assert_ne!(config_dir(), home_dir());
    }
}
