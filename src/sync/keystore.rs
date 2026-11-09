//! Where this computer keeps the data key (docs/design/SYNC.md §11.4):
//! the OS keychain where it is reliable, else an owner-only file, and only
//! memory whenever user dirs are isolated (tests, the mock-app sweep).
//!
//! | Backend | Used when |
//! |---|---|
//! | macOS Keychain | The build has a team identifier in its signing information |
//! | Windows Credential Manager | Always |
//! | Linux Secret Service | It answers within 2 seconds |
//! | `<data_local>/kage/sync/key.bin` | Otherwise |
//! | Memory | Whenever user dirs are isolated |
//!
//! The value is the DEK and its epoch, as 37 bytes: version `1`, the epoch
//! as u32 LE, the DEK. Service `Kage Sync`, account `<folder_id>`. Only the
//! engine thread calls a store, never on the launch path.

use crate::os::secret_store::{self, SecretBackend};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use zeroize::Zeroizing;

/// Why a store refused (`Denied`, `Unavailable`), carried inside the
/// `anyhow::Error`s the stores return.
pub use crate::os::secret_store::SecretError as KeyStoreError;

/// The service name every OS store files the key under.
pub const SERVICE: &str = "Kage Sync";
/// The file backend's file in `<data_local>/kage/sync/`.
pub const KEY_FILE: &str = "key.bin";
/// The version byte of a stored key.
const STORED_V1: u8 = 1;
const STORED_LEN: usize = 1 + 4 + 32;

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

impl KeyStoreKind {
    /// The name `DevicePrefs::key_store` records.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::CredentialManager => "credential_manager",
            Self::SecretService => "secret_service",
            Self::File => "file",
            Self::Memory => "memory",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        [
            Self::Keychain,
            Self::CredentialManager,
            Self::SecretService,
            Self::File,
            Self::Memory,
        ]
        .into_iter()
        .find(|k| k.wire() == s)
    }
}

/// The data key of one folder, and its epoch.
#[derive(Clone)]
pub struct StoredKey {
    pub epoch: u32,
    pub dek: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for StoredKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredKey")
            .field("epoch", &self.epoch)
            .field("dek", &"<redacted>")
            .finish()
    }
}

impl StoredKey {
    pub fn new(epoch: u32, dek: &[u8; 32]) -> Self {
        Self {
            epoch,
            dek: Zeroizing::new(*dek),
        }
    }

    fn encode(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(STORED_LEN));
        out.push(STORED_V1);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.dek[..]);
        out
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != STORED_LEN || bytes[0] != STORED_V1 {
            return None;
        }
        let epoch = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let mut dek = Zeroizing::new([0u8; 32]);
        dek.copy_from_slice(&bytes[5..]);
        Some(Self { epoch, dek })
    }
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

/// Whether the real stores are off limits (see `os::secret_store`).
fn isolated() -> bool {
    secret_store::is_off_limits()
}

/// The key store for this computer, by the §11.4 rules. `local_dir` is
/// `<data_local>/kage/sync/`, where the file backend keeps `key.bin`.
/// Probes the OS store (up to 2 seconds on Linux): call it on the engine
/// thread.
pub fn default_key_store(local_dir: &Path) -> Arc<dyn KeyStore> {
    if isolated() {
        return memory_store();
    }
    if secret_store::keychain_is_reliable() {
        Arc::new(OsKeyStore)
    } else {
        file_key_store(local_dir)
    }
}

/// The store a computer recorded in `DevicePrefs::key_store`, so a key kept
/// in the file stays found after the keychain becomes reliable (an update
/// to a signed build). A kind this OS doesn't have falls back to
/// [`default_key_store`].
pub fn key_store_for(kind: KeyStoreKind, local_dir: &Path) -> Arc<dyn KeyStore> {
    if isolated() {
        return memory_store();
    }
    match kind {
        KeyStoreKind::File => file_key_store(local_dir),
        k if Some(k) == os_kind() => Arc::new(OsKeyStore),
        _ => default_key_store(local_dir),
    }
}

/// The owner-only file, as Settings → Sync offers when the keychain is
/// denied or gone (§15). Never touches an OS store, so it works under
/// isolation too, inside the isolated `local_dir`.
pub fn file_key_store(local_dir: &Path) -> Arc<dyn KeyStore> {
    Arc::new(FileKeyStore::new(local_dir))
}

fn os_kind() -> Option<KeyStoreKind> {
    Some(match secret_store::backend() {
        SecretBackend::Keychain => KeyStoreKind::Keychain,
        SecretBackend::CredentialManager => KeyStoreKind::CredentialManager,
        SecretBackend::SecretService => KeyStoreKind::SecretService,
    })
}

/// One memory store per process, so every isolated caller sees the same
/// keys.
fn memory_store() -> Arc<dyn KeyStore> {
    static MEMORY: OnceLock<Arc<MemoryStore>> = OnceLock::new();
    MEMORY
        .get_or_init(|| Arc::new(MemoryStore::default()))
        .clone()
}

// --- Memory ------------------------------------------------------------------

/// Keys in memory, gone when the process exits.
#[derive(Default)]
pub struct MemoryStore {
    keys: Mutex<HashMap<String, StoredKey>>,
}

impl MemoryStore {
    fn keys(&self) -> std::sync::MutexGuard<'_, HashMap<String, StoredKey>> {
        self.keys.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl KeyStore for MemoryStore {
    fn kind(&self) -> KeyStoreKind {
        KeyStoreKind::Memory
    }

    fn get(&self, folder_id: &str) -> anyhow::Result<Option<StoredKey>> {
        Ok(self.keys().get(folder_id).cloned())
    }

    fn set(&self, folder_id: &str, key: &StoredKey) -> anyhow::Result<()> {
        self.keys().insert(folder_id.to_string(), key.clone());
        Ok(())
    }

    fn delete(&self, folder_id: &str) -> anyhow::Result<()> {
        self.keys().remove(folder_id);
        Ok(())
    }
}

// --- The OS store --------------------------------------------------------------

/// The OS keychain of this build, through `os::secret_store`.
struct OsKeyStore;

impl KeyStore for OsKeyStore {
    fn kind(&self) -> KeyStoreKind {
        os_kind().unwrap_or(KeyStoreKind::Memory)
    }

    fn get(&self, folder_id: &str) -> anyhow::Result<Option<StoredKey>> {
        match secret_store::get(SERVICE, folder_id)? {
            None => Ok(None),
            Some(bytes) => StoredKey::decode(&bytes)
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("the stored sync key is damaged")),
        }
    }

    fn set(&self, folder_id: &str, key: &StoredKey) -> anyhow::Result<()> {
        secret_store::set(SERVICE, folder_id, &key.encode())
    }

    fn delete(&self, folder_id: &str) -> anyhow::Result<()> {
        secret_store::delete(SERVICE, folder_id)
    }
}

// --- The file ------------------------------------------------------------------

/// `key.bin`: `{ "v": 1, "keys": { "<folder_id>": "<base64 of the 37
/// bytes>" } }`, owner-only.
struct FileKeyStore {
    path: PathBuf,
    /// One read-modify-write at a time.
    lock: Mutex<()>,
}

#[derive(Default, Serialize, Deserialize)]
struct KeyFileBody {
    #[serde(default)]
    v: u32,
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

impl Drop for KeyFileBody {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for v in self.keys.values_mut() {
            v.zeroize();
        }
    }
}

impl FileKeyStore {
    fn new(local_dir: &Path) -> Self {
        Self {
            path: local_dir.join(KEY_FILE),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> anyhow::Result<KeyFileBody> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("the sync key file is damaged"))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(KeyFileBody::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, body: &KeyFileBody) -> anyhow::Result<()> {
        if body.keys.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            };
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = Zeroizing::new(serde_json::to_vec(body)?);
        crate::atomic_write::write(&self.path, &json[..], crate::atomic_write::Perms::Private)?;
        Ok(())
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

impl KeyStore for FileKeyStore {
    fn kind(&self) -> KeyStoreKind {
        KeyStoreKind::File
    }

    fn get(&self, folder_id: &str) -> anyhow::Result<Option<StoredKey>> {
        let _g = self.guard();
        let body = self.load()?;
        let Some(text) = body.keys.get(folder_id) else {
            return Ok(None);
        };
        let bytes = Zeroizing::new(
            b64()
                .decode(text)
                .map_err(|_| anyhow::anyhow!("the sync key file is damaged"))?,
        );
        StoredKey::decode(&bytes)
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("the sync key file is damaged"))
    }

    fn set(&self, folder_id: &str, key: &StoredKey) -> anyhow::Result<()> {
        let _g = self.guard();
        // A damaged file holds nothing usable: start a new one.
        let mut body = self.load().unwrap_or_else(|e| {
            log::warn!("Replacing the sync key file: {}", e);
            KeyFileBody::default()
        });
        body.v = 1;
        body.keys
            .insert(folder_id.to_string(), b64().encode(&key.encode()[..]));
        self.save(&body)
    }

    fn delete(&self, folder_id: &str) -> anyhow::Result<()> {
        let _g = self.guard();
        let mut body = match self.load() {
            Ok(b) => b,
            Err(e) => {
                log::warn!("Removing a damaged sync key file: {}", e);
                KeyFileBody::default()
            }
        };
        body.keys.remove(folder_id);
        self.save(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "5qg2vbn8h1xk0d3m";
    const B: &str = "a9x3h6t1qq8m0ve2";

    fn key(epoch: u32, fill: u8) -> StoredKey {
        StoredKey::new(epoch, &[fill; 32])
    }

    fn round_trip(store: &dyn KeyStore) {
        assert!(store.get(A).unwrap().is_none());
        store.set(A, &key(1, 7)).unwrap();
        store.set(B, &key(2, 8)).unwrap();
        let got = store.get(A).unwrap().unwrap();
        assert_eq!((got.epoch, &got.dek[..]), (1, &[7u8; 32][..]));
        // Replacing keeps one key per folder.
        store.set(A, &key(3, 9)).unwrap();
        let got = store.get(A).unwrap().unwrap();
        assert_eq!((got.epoch, &got.dek[..]), (3, &[9u8; 32][..]));
        store.delete(A).unwrap();
        assert!(store.get(A).unwrap().is_none());
        store.delete(A).unwrap();
        assert_eq!(store.get(B).unwrap().unwrap().epoch, 2);
        store.delete(B).unwrap();
    }

    /// §11.4: while user dirs are isolated, the store is memory and no OS
    /// key store is ever called.
    #[test]
    fn isolated_runs_never_call_an_os_key_store() {
        let _ = crate::paths::data_local_dir();
        assert!(crate::paths::is_isolated());
        let before = secret_store::os_calls();
        let dir = tempfile::tempdir().unwrap();
        let store = default_key_store(dir.path());
        assert_eq!(store.kind(), KeyStoreKind::Memory);
        for kind in [
            KeyStoreKind::Keychain,
            KeyStoreKind::CredentialManager,
            KeyStoreKind::SecretService,
            KeyStoreKind::Memory,
        ] {
            assert_eq!(key_store_for(kind, dir.path()).kind(), KeyStoreKind::Memory);
        }
        let folder = "zzzzzzzzzzzzzzzz";
        store.set(folder, &key(1, 1)).unwrap();
        // The same process-wide store.
        assert!(default_key_store(dir.path()).get(folder).unwrap().is_some());
        store.delete(folder).unwrap();
        assert_eq!(secret_store::os_calls(), before);
        assert_eq!(secret_store::os_calls(), 0);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn memory_store_round_trips() {
        round_trip(&MemoryStore::default());
    }

    #[test]
    fn file_store_round_trips_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("kage/sync");
        let store = file_key_store(&local);
        assert_eq!(store.kind(), KeyStoreKind::File);
        store.set(A, &key(1, 7)).unwrap();
        let path = local.join(KEY_FILE);
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A second instance reads what the first wrote.
        assert_eq!(file_key_store(&local).get(A).unwrap().unwrap().epoch, 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(A) && text.contains("\"v\":1"));
        store.delete(A).unwrap();
        assert!(!path.exists(), "an empty key file is removed");
        round_trip(store.as_ref());
        assert_eq!(
            key_store_for(KeyStoreKind::File, &local).kind(),
            KeyStoreKind::Memory
        );
    }

    #[test]
    fn a_damaged_key_file_is_an_error_then_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), b"{ not json").unwrap();
        let store = file_key_store(dir.path());
        assert!(store.get(A).is_err());
        store.set(A, &key(4, 4)).unwrap();
        assert_eq!(store.get(A).unwrap().unwrap().epoch, 4);
        std::fs::write(
            dir.path().join(KEY_FILE),
            br#"{"v":1,"keys":{"5qg2vbn8h1xk0d3m":"AAAA"}}"#,
        )
        .unwrap();
        assert!(store.get(A).is_err());
    }

    #[test]
    fn stored_keys_encode_as_documented() {
        let k = key(0x0102_0304, 0xab);
        let bytes = k.encode();
        assert_eq!(bytes.len(), 37);
        assert_eq!(&bytes[..5], &[1, 4, 3, 2, 1]);
        assert_eq!(&bytes[5..], &[0xab; 32]);
        let back = StoredKey::decode(&bytes).unwrap();
        assert_eq!((back.epoch, &back.dek[..]), (k.epoch, &k.dek[..]));
        assert!(StoredKey::decode(&bytes[..36]).is_none());
        let mut v2 = bytes.to_vec();
        v2[0] = 2;
        assert!(StoredKey::decode(&v2).is_none());
        assert!(
            !format!("{:?}", k).contains("171"),
            "Debug never prints the key"
        );
    }

    #[test]
    fn kinds_have_wire_names() {
        for k in [
            KeyStoreKind::Keychain,
            KeyStoreKind::CredentialManager,
            KeyStoreKind::SecretService,
            KeyStoreKind::File,
            KeyStoreKind::Memory,
        ] {
            assert_eq!(KeyStoreKind::from_wire(k.wire()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.wire());
        }
        assert_eq!(KeyStoreKind::from_wire("enterprise"), None);
    }
}
