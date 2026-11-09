//! Fakes for sync tests (docs/design/SYNC.md §18): an in-memory folder with
//! injected faults, a clock to drive, a memory key store and fake cloud
//! files. Public so integration tests can use them; not part of the app.
//!
//! C0 skeleton: the fakes exist but do nothing yet. WP-S2 writes them.

use super::engine::Clock;
use super::fsio::{Entry, SyncFs};
use super::keystore::{KeyStore, KeyStoreKind, StoredKey};
use super::NOT_BUILT;
use crate::os::cloud_files::{CloudFiles, Provider, Residency, SuggestedFolder};
use crate::os::power::PowerState;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

fn not_built() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, NOT_BUILT)
}

/// A simulated sync folder shared by several computers, with faults to
/// inject (late, partial, reordered and duplicated files).
#[derive(Debug, Default)]
pub struct FaultyFs {
    _files: (),
}

impl SyncFs for FaultyFs {
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

/// A clock that only moves when told to.
#[derive(Debug, Default)]
pub struct FakeClock {
    now_ms: AtomicI64,
}

impl FakeClock {
    pub fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
        }
    }

    pub fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn power(&self) -> PowerState {
        PowerState::AC
    }
}

/// A key store that keeps keys in memory, as every isolated run uses.
#[derive(Debug, Default)]
pub struct MemoryKeyStore {
    _keys: (),
}

impl KeyStore for MemoryKeyStore {
    fn kind(&self) -> KeyStoreKind {
        KeyStoreKind::Memory
    }

    fn get(&self, folder_id: &str) -> anyhow::Result<Option<StoredKey>> {
        let _ = folder_id;
        Ok(None)
    }

    fn set(&self, folder_id: &str, key: &StoredKey) -> anyhow::Result<()> {
        let _ = (folder_id, key);
        anyhow::bail!(NOT_BUILT)
    }

    fn delete(&self, folder_id: &str) -> anyhow::Result<()> {
        let _ = folder_id;
        Ok(())
    }
}

/// Cloud files whose placeholders a test controls.
#[derive(Debug, Default)]
pub struct FakeCloudFiles {
    _placeholders: (),
}

impl CloudFiles for FakeCloudFiles {
    fn residency(&self, path: &Path) -> Residency {
        let _ = path;
        Residency::Local
    }

    fn request_download(&self, path: &Path) {
        let _ = path;
    }

    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()> {
        let _ = dir;
        Ok(())
    }

    fn detect_provider(&self, path: &Path) -> Provider {
        let _ = path;
        Provider::Other
    }

    fn suggest_folders(&self) -> Vec<SuggestedFolder> {
        Vec::new()
    }
}
