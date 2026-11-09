//! Where this computer keeps the data key (docs/design/SYNC.md §11.4):
//! the OS keychain where it is reliable, else an owner-only file, and only
//! memory whenever user dirs are isolated (tests, the mock-app sweep).
//!
//! C0 skeleton: `default_key_store` returns a store that keeps nothing.
//! WP-S2 writes the selection and the backends.

use super::NOT_BUILT;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Which backend holds the key. Shown in Settings → Sync → Advanced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStoreKind {
    Keychain,
    CredentialManager,
    SecretService,
    File,
    Memory,
}

/// The data key of one folder, and its epoch.
#[derive(Clone)]
pub struct StoredKey {
    pub epoch: u32,
    pub dek: Zeroizing<[u8; 32]>,
}

/// A place to keep one data key per folder (service `Kage Sync`, account
/// `<folder_id>`). Only the engine thread calls it, never on the launch
/// path.
pub trait KeyStore: Send + Sync {
    fn kind(&self) -> KeyStoreKind;
    fn get(&self, folder_id: &str) -> anyhow::Result<Option<StoredKey>>;
    fn set(&self, folder_id: &str, key: &StoredKey) -> anyhow::Result<()>;
    fn delete(&self, folder_id: &str) -> anyhow::Result<()>;
}

/// The key store for this computer, by the §11.4 rules. `local_dir` is
/// `<data_local>/kage/sync/`, where the file backend keeps `key.bin`.
pub fn default_key_store(local_dir: &Path) -> Arc<dyn KeyStore> {
    let _ = local_dir;
    Arc::new(NoKeyStore)
}

/// Keeps nothing: until WP-S2, no key is ever stored.
struct NoKeyStore;

impl KeyStore for NoKeyStore {
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
