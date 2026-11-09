//! Folder I/O for sync (docs/design/SYNC.md §5, §14.2). The engine reaches
//! the sync folder only through [`SyncFs`], so tests can run several
//! computers against one simulated cloud and inject faults.
//!
//! C0 skeleton: `RealFs` refuses every call. WP-S2 writes it on a pool of
//! I/O threads with timeouts.

use super::NOT_BUILT;
use std::io;
use std::path::Path;
use std::time::Duration;

/// One directory entry. File times are hints, never facts (§3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub len: u64,
    pub modified_ms: Option<i64>,
    pub is_dir: bool,
}

/// What the engine may do in the sync folder.
pub trait SyncFs: Send + Sync {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>>;
    /// Read at most `max_len` bytes, giving up after `timeout`. Runs on the
    /// I/O pool.
    fn read(&self, path: &Path, max_len: u64, timeout: Duration) -> io::Result<Vec<u8>>;
    /// Write a new file: `~kage-*.tmp`, fsync, then rename to `name`.
    fn write_once(&self, dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()>;
    /// Create a file that must not exist yet (the root files).
    fn create_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove(&self, path: &Path) -> io::Result<()>;
    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;
}

/// The real file system, behind a small pool of I/O threads.
#[derive(Debug, Default)]
pub struct RealFs {
    _pool: (),
}

impl RealFs {
    pub fn new() -> Self {
        Self::default()
    }
}

fn not_built() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, NOT_BUILT)
}

impl SyncFs for RealFs {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let _ = dir;
        Err(not_built())
    }

    fn read(&self, path: &Path, max_len: u64, timeout: Duration) -> io::Result<Vec<u8>> {
        let _ = (path, max_len, timeout);
        Err(not_built())
    }

    fn write_once(&self, dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
        let _ = (dir, name, bytes);
        Err(not_built())
    }

    fn create_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let _ = (path, bytes);
        Err(not_built())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let _ = (from, to);
        Err(not_built())
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        let _ = path;
        Err(not_built())
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        let _ = dir;
        Err(not_built())
    }
}
