//! Keys and the passphrase (docs/design/SYNC.md §11.3): a random data key
//! (DEK) per epoch, wrapped by Argon2id of the sync passphrase in a key
//! file, and HKDF subkeys for encryption, names and extension device tags.
//!
//! C0 skeleton: only `new_dek` works. WP-S2 writes the rest and its golden
//! vectors.

use super::NOT_BUILT;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The subkeys derived from one epoch's DEK.
#[derive(Clone)]
pub struct Keys {
    enc: Zeroizing<[u8; 32]>,
    name: Zeroizing<[u8; 32]>,
    extdev: Zeroizing<[u8; 32]>,
}

impl Keys {
    /// HKDF-SHA256 of the DEK into `enc`, `name` and `extdev`.
    pub fn derive(dek: &[u8; 32]) -> Self {
        let _ = dek;
        Self {
            enc: Zeroizing::new([0; 32]),
            name: Zeroizing::new([0; 32]),
            extdev: Zeroizing::new([0; 32]),
        }
    }

    /// A content name: 32 hex characters of a keyed hash of `plain`.
    pub fn content_name(&self, plain: &[u8]) -> String {
        let _ = (plain, &self.enc);
        String::new()
    }

    /// The shard bucket of a record, out of `of`.
    pub fn bucket(&self, ns: &str, id: &str, of: u32) -> u32 {
        let _ = (ns, id, of);
        0
    }

    /// The key for content names, buckets and conflict ids.
    pub fn name_key(&self) -> &[u8; 32] {
        &self.name
    }

    /// `HKDF(extdev, extension id ‖ device id)[..12]` as text (§12.6).
    pub fn ext_device_tag(&self, ext: &str, dev: super::types::DeviceId) -> String {
        let _ = (ext, dev, &self.extdev);
        String::new()
    }
}

/// The key derivation stored in a key file, so its cost can change later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    /// `argon2id`.
    pub alg: String,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
    /// Base64 of 16 bytes.
    pub salt: String,
}

/// `keys/eNNNN-<dev>-NNNN.json`: one wrapped DEK, written once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFile {
    pub epoch: u32,
    pub kdf: Kdf,
    /// Base64 of nonce (12) | AES-256-GCM(KEK, DEK) | tag (16).
    pub wrapped: String,
    /// `kage-sync-dek|<folder_id>|<epoch>`.
    pub aad: String,
    /// RFC 3339.
    pub written_at: String,
}

/// Why a key file didn't unwrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// The passphrase doesn't unlock this key file.
    WrongPassphrase,
    /// The key file names a KDF or parameters this build doesn't take.
    UnsupportedKdf,
    /// The key file is damaged or isn't for this folder.
    Malformed,
}

/// A new random data key.
pub fn new_dek() -> Zeroizing<[u8; 32]> {
    use rand::RngCore;
    let mut dek = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(&mut dek[..]);
    dek
}

/// Wrap `dek` with the passphrase, for `folder_id` and `epoch`.
pub fn wrap_dek(
    passphrase: &str,
    dek: &[u8; 32],
    folder_id: &str,
    epoch: u32,
) -> anyhow::Result<KeyFile> {
    let _ = (passphrase, dek, folder_id, epoch);
    anyhow::bail!(NOT_BUILT)
}

/// Unwrap a key file's DEK with the passphrase.
pub fn unwrap_dek(
    passphrase: &str,
    kf: &KeyFile,
    folder_id: &str,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let _ = (passphrase, kf, folder_id);
    Err(CryptoError::WrongPassphrase)
}

/// "Make one for me": 25 Crockford base32 characters (125 bits) in five
/// groups of five.
pub fn generate_passphrase() -> String {
    String::new()
}
