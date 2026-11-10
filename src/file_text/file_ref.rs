//! `sha256` and `file_ref` for a file an extension read
//! (docs/design/EXTENSION_FILE_TEXT.md §5.11).
//!
//! - `sha256`: of the bytes read. The same content gives the same hash on
//!   every computer.
//! - `file_ref`: `fr1_` + base64url of the first 16 bytes of
//!   HMAC-SHA256(key, `v1\0` + extension id + `\0` + canonical path in
//!   NFC). Stable for the same file at the same place on this computer,
//!   different per extension (two extensions can't link their records),
//!   meaningless elsewhere, and not reversible to the path. Renaming or
//!   moving the file changes it.
//!
//! The key is 32 random bytes in `<data_local_dir>/kage/file-ref.key`,
//! owner-only, created on first use and replaced if malformed. It is
//! never backed up or synced. If it can't be written, a key for this run
//! of Kage is used instead, so refs still match within the session.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Bytes in the key.
pub const KEY_BYTES: usize = 32;
/// Bytes of the HMAC kept in a ref.
const REF_BYTES: usize = 16;
/// The prefix of every ref this version makes.
pub const REF_PREFIX: &str = "fr1_";
const KEY_FILE: &str = "file-ref.key";

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Where the key lives.
pub fn key_path() -> Option<PathBuf> {
    Some(crate::paths::data_local_dir()?.join("kage").join(KEY_FILE))
}

/// The key at `path`: read it, or make and save a new one when the file
/// is missing or isn't exactly `KEY_BYTES` long. A file that is there but
/// can't be read is an error and stays as it is: replacing it would change
/// every ref extensions have stored, for what may be a passing failure (a
/// scanner holding the file).
pub fn load_or_create_key(path: &Path) -> std::io::Result<[u8; KEY_BYTES]> {
    match std::fs::read(path) {
        Ok(bytes) => {
            if let Ok(key) = <[u8; KEY_BYTES]>::try_from(bytes.as_slice()) {
                return Ok(key);
            }
            log::warn!("File text: the file_ref key was malformed; making a new one");
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let key = random_key()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::atomic_write::write(path, key, crate::atomic_write::Perms::Private)?;
    Ok(key)
}

fn random_key() -> std::io::Result<[u8; KEY_BYTES]> {
    use rand::TryRngCore;
    let mut key = [0u8; KEY_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut key)
        .map_err(std::io::Error::other)?;
    Ok(key)
}

/// This computer's key, or this run's if the file can't be used. One
/// caller at a time, so two first uses can't each write a key.
fn key() -> [u8; KEY_BYTES] {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    match key_path().map(|p| load_or_create_key(&p)) {
        Some(Ok(key)) => key,
        failed => {
            if let Some(Err(e)) = failed {
                log::warn!(
                    "File text: couldn't keep the file_ref key ({e}); using one for this run"
                );
            }
            static SESSION: OnceLock<[u8; KEY_BYTES]> = OnceLock::new();
            *SESSION.get_or_init(|| random_key().unwrap_or_else(|_| rand::random()))
        }
    }
}

/// `file_ref` for `path` read by `extension_id`, with this computer's key.
pub fn file_ref(extension_id: &str, path: &Path) -> String {
    file_ref_with(&key(), extension_id, path)
}

/// `file_ref` with a given key. The path is made canonical (links
/// resolved) when it can be, then NFC.
pub fn file_ref_with(key: &[u8; KEY_BYTES], extension_id: &str, path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(&text);
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(b"v1\0");
    mac.update(extension_id.as_bytes());
    mac.update(b"\0");
    mac.update(nfc.as_bytes());
    let digest = mac.finalize().into_bytes();
    format!(
        "{REF_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..REF_BYTES])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_lowercase_hex_of_the_bytes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn refs_are_stable_per_file_and_differ_per_extension_and_place() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "same").unwrap();
        std::fs::write(&b, "same").unwrap();
        let key = [7u8; KEY_BYTES];

        let r = file_ref_with(&key, "mindmap", &a);
        assert_eq!(r, file_ref_with(&key, "mindmap", &a));
        assert_ne!(r, file_ref_with(&key, "notes", &a));
        assert_ne!(r, file_ref_with(&key, "mindmap", &b));
        assert_ne!(r, file_ref_with(&[8u8; KEY_BYTES], "mindmap", &a));
        // A link to the file is the same file.
        #[cfg(unix)]
        {
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&a, &link).unwrap();
            assert_eq!(r, file_ref_with(&key, "mindmap", &link));
        }

        assert!(r.starts_with(REF_PREFIX));
        let body = &r[REF_PREFIX.len()..];
        assert_eq!(body.len(), 22);
        assert!(body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(!r.contains("a.txt"));
    }

    #[test]
    fn the_key_is_created_owner_only_kept_and_replaced_if_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kage").join(KEY_FILE);
        let first = load_or_create_key(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), first);
        assert_eq!(load_or_create_key(&path).unwrap(), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        std::fs::write(&path, b"short").unwrap();
        let replaced = load_or_create_key(&path).unwrap();
        assert_ne!(replaced, first);
        assert_eq!(std::fs::read(&path).unwrap(), replaced);
    }

    #[cfg(unix)]
    #[test]
    fn a_key_that_cant_be_read_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEY_FILE);
        let key = load_or_create_key(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads it anyway; there is nothing to test then.
        if std::fs::read(&path).is_err() {
            assert!(load_or_create_key(&path).is_err());
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), key);
    }

    #[test]
    fn the_default_key_lives_in_the_local_data_folder() {
        let path = key_path().unwrap();
        assert!(crate::paths::is_under_isolation_root(&path));
        assert!(path.ends_with(Path::new("kage").join(KEY_FILE)));
        let r = file_ref("mindmap", Path::new("/nowhere/x.txt"));
        assert_eq!(r, file_ref("mindmap", Path::new("/nowhere/x.txt")));
    }
}
