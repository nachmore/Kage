//! The envelope of every `.kage` file (docs/design/SYNC.md §5.4):
//! AES-256-GCM, with the folder, device, kind and name in the
//! authenticated data, so a planted, edited or moved file fails to open.
//!
//! ```text
//! magic   "KSYNC"            5 bytes
//! ver     0x01               1 byte
//! kind    'H' | 'S' | 'B'    1 byte
//! epoch   u32 LE             4 bytes
//! nonce   random             12 bytes
//! ciphertext = AES-256-GCM(enc key of the epoch, nonce, AAD, deflate(plain))
//! tag                        16 bytes
//! AAD = "kage-sync|1|" folder_id "|" device "|" kind "|" file name
//! ```
//!
//! The deflate stream is raw DEFLATE (RFC 1951). The file name in the AAD
//! is the whole name, `head-000000000042.kage` or `<32 hex>.kage`. Golden
//! files of version 1 are in `tests/fixtures/sync/envelope/`.

use super::crypto::{device_text, Keys};
use super::types::DeviceId;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use std::io::{Read, Write};

/// The first bytes of every sealed file.
pub const MAGIC: &[u8; 5] = b"KSYNC";
/// The envelope version this build writes and reads.
pub const VERSION: u8 = 1;
/// magic, version, kind, epoch, nonce.
pub const HEADER_LEN: usize = 5 + 1 + 1 + 4 + 12;
/// The GCM tag after the ciphertext.
pub const TAG_LEN: usize = 16;

/// The largest plaintext of a head (§5.8).
pub const MAX_HEAD_PLAIN: usize = 64 * 1024;
/// The largest plaintext of a shard (§5.8).
pub const MAX_SHARD_PLAIN: usize = 4 * 1024 * 1024;
/// The largest plaintext of a blob: a chat of 8 MiB (§4.6), escaped as JSON.
pub const MAX_BLOB_PLAIN: usize = 16 * 1024 * 1024;

/// The kind of a data file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Head,
    Shard,
    Blob,
}

impl FileKind {
    /// The kind byte of the header and the letter in the AAD.
    pub fn letter(self) -> u8 {
        match self {
            Self::Head => b'H',
            Self::Shard => b'S',
            Self::Blob => b'B',
        }
    }
}

/// The largest plaintext a file of `kind` may hold. [`open`] refuses more,
/// so a writer must split or move a value to a blob before sealing it.
pub fn max_plain(kind: FileKind) -> usize {
    match kind {
        FileKind::Head => MAX_HEAD_PLAIN,
        FileKind::Shard => MAX_SHARD_PLAIN,
        FileKind::Blob => MAX_BLOB_PLAIN,
    }
}

/// The largest file of `kind` worth reading: its plaintext stored without
/// compression, plus the envelope. Pass it to `SyncFs::read`.
pub fn max_sealed(kind: FileKind) -> u64 {
    let plain = max_plain(kind);
    // Deflate's stored blocks add 5 bytes per 64 KiB, plus a little.
    (plain + plain / 1024 + 1024 + HEADER_LEN + TAG_LEN) as u64
}

/// What a file is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aad<'a> {
    pub folder_id: &'a str,
    pub device: DeviceId,
    pub kind: FileKind,
    /// The whole file name, with `.kage`.
    pub name: &'a str,
}

impl Aad<'_> {
    fn text(&self) -> String {
        format!(
            "kage-sync|1|{}|{}|{}|{}",
            self.folder_id,
            device_text(self.device),
            self.kind.letter() as char,
            self.name
        )
    }
}

/// Why a file didn't open. `Truncated` (short or still downloading) is
/// retried; `BadTag` means changed, planted, moved, or cut short past the
/// header. The engine treats every one as "not ready" at first (§8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    Truncated,
    BadTag,
    UnknownEpoch(u32),
    TooLarge,
    NameMismatch,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => f.write_str("the file is shorter than an envelope"),
            Self::BadTag => f.write_str("the file doesn't authenticate"),
            Self::UnknownEpoch(e) => write!(f, "no key for epoch {}", e),
            Self::TooLarge => f.write_str("the file is over its size limit"),
            Self::NameMismatch => f.write_str("the file's content doesn't match its name"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Seal `plain` under the keys of `epoch`.
pub fn seal(keys: &Keys, epoch: u32, aad: &Aad<'_>, plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    seal_with_nonce(keys, epoch, aad, plain, nonce)
}

/// [`seal`] with a given nonce. Only for golden files: a nonce must never
/// repeat under one key.
#[doc(hidden)]
pub fn seal_with_nonce(
    keys: &Keys,
    epoch: u32,
    aad: &Aad<'_>,
    plain: &[u8],
    nonce: [u8; 12],
) -> Vec<u8> {
    let mut deflate =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    deflate
        .write_all(plain)
        .expect("compressing into memory can't fail");
    let compressed = deflate
        .finish()
        .expect("compressing into memory can't fail");
    let cipher = Aes256Gcm::new_from_slice(keys.enc_key()).expect("a 32-byte AES-256 key");
    let aad_text = aad.text();
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &compressed,
                aad: aad_text.as_bytes(),
            },
        )
        .expect("AES-GCM seals any length a file can have");
    let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(aad.kind.letter());
    out.extend_from_slice(&epoch.to_le_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// The epoch a sealed file names, if it has a whole header.
pub fn peek_epoch(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < HEADER_LEN || &bytes[..5] != MAGIC {
        return None;
    }
    Some(u32::from_le_bytes([
        bytes[7], bytes[8], bytes[9], bytes[10],
    ]))
}

/// Open a sealed file. `keys` looks up the keys of the file's epoch. For a
/// shard or blob the plaintext must also match its content name.
pub fn open(
    keys: &dyn Fn(u32) -> Option<Keys>,
    aad: &Aad<'_>,
    bytes: &[u8],
) -> Result<Vec<u8>, OpenError> {
    if bytes.len() as u64 > max_sealed(aad.kind) {
        return Err(OpenError::TooLarge);
    }
    if bytes.len() < HEADER_LEN + TAG_LEN {
        return Err(OpenError::Truncated);
    }
    if &bytes[..5] != MAGIC {
        // A sync app can make room for a download before filling it in.
        return Err(if bytes.iter().all(|&b| b == 0) {
            OpenError::Truncated
        } else {
            OpenError::BadTag
        });
    }
    if bytes[5] != VERSION || bytes[6] != aad.kind.letter() {
        return Err(OpenError::BadTag);
    }
    let epoch = u32::from_le_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]);
    let k = keys(epoch).ok_or(OpenError::UnknownEpoch(epoch))?;
    let cipher = Aes256Gcm::new_from_slice(k.enc_key()).expect("a 32-byte AES-256 key");
    let aad_text = aad.text();
    let compressed = cipher
        .decrypt(
            Nonce::from_slice(&bytes[11..HEADER_LEN]),
            Payload {
                msg: &bytes[HEADER_LEN..],
                aad: aad_text.as_bytes(),
            },
        )
        .map_err(|_| OpenError::BadTag)?;
    let limit = max_plain(aad.kind);
    let mut plain = Vec::new();
    flate2::read::DeflateDecoder::new(&compressed[..])
        .take(limit as u64 + 1)
        .read_to_end(&mut plain)
        // Authentic but not deflate: a writer's bug. The file can't be used.
        .map_err(|_| OpenError::BadTag)?;
    if plain.len() > limit {
        return Err(OpenError::TooLarge);
    }
    if matches!(aad.kind, FileKind::Shard | FileKind::Blob) {
        let stem = aad.name.strip_suffix(".kage").unwrap_or(aad.name);
        if stem != k.content_name(&plain) {
            return Err(OpenError::NameMismatch);
        }
    }
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::crypto::{parse_device_text, unwrap_dek, KeyFile};
    use std::path::{Path, PathBuf};

    const FOLDER: &str = "5qg2vbn8h1xk0d3m";

    fn keys(fill: u8) -> Keys {
        Keys::derive(&[fill; 32])
    }

    fn dev() -> DeviceId {
        parse_device_text("k7m2p9xqa3rt5vwe").unwrap()
    }

    fn one(k: &Keys, epoch: u32) -> impl Fn(u32) -> Option<Keys> + '_ {
        move |e| (e == epoch).then(|| k.clone())
    }

    fn shard(k: &Keys, plain: &[u8]) -> (String, Vec<u8>) {
        let name = format!("{}.kage", k.content_name(plain));
        let aad = Aad {
            folder_id: FOLDER,
            device: dev(),
            kind: FileKind::Shard,
            name: &name,
        };
        let sealed = seal(k, 1, &aad, plain);
        (name, sealed)
    }

    fn shard_aad(name: &str) -> Aad<'_> {
        Aad {
            folder_id: FOLDER,
            device: dev(),
            kind: FileKind::Shard,
            name,
        }
    }

    #[test]
    fn seal_open_round_trip_for_every_kind() {
        let k = keys(1);
        let plain = br#"{"ns":"cfg","records":[]}"#;
        for kind in [FileKind::Head, FileKind::Shard, FileKind::Blob] {
            let name = match kind {
                FileKind::Head => "head-000000000042.kage".to_string(),
                _ => format!("{}.kage", k.content_name(plain)),
            };
            let aad = Aad {
                folder_id: FOLDER,
                device: dev(),
                kind,
                name: &name,
            };
            let sealed = seal(&k, 7, &aad, plain);
            assert_eq!(&sealed[..5], MAGIC);
            assert_eq!(sealed[5], 1);
            assert_eq!(sealed[6], kind.letter());
            assert_eq!(peek_epoch(&sealed), Some(7));
            assert_eq!(open(&one(&k, 7), &aad, &sealed).unwrap(), plain);
        }
    }

    #[test]
    fn nonces_differ_between_seals() {
        let k = keys(1);
        let (name, a) = shard(&k, b"{}");
        let (_, b) = shard(&k, b"{}");
        assert_ne!(a, b);
        assert_eq!(open(&one(&k, 1), &shard_aad(&name), &a).unwrap(), b"{}");
    }

    #[test]
    fn any_changed_aad_component_fails() {
        let k = keys(2);
        let plain = b"{\"a\":1}";
        let (name, sealed) = shard(&k, plain);
        let good = shard_aad(&name);
        assert!(open(&one(&k, 1), &good, &sealed).is_ok());
        let other_dev = parse_device_text("a9x3h6t1qq8m0ve2").unwrap();
        let renamed = format!("{}.kage", "0".repeat(32));
        for aad in [
            Aad {
                folder_id: "aaaaaaaaaaaaaaaa",
                ..good
            },
            Aad {
                device: other_dev,
                ..good
            },
            Aad {
                kind: FileKind::Blob,
                ..good
            },
            Aad {
                name: &renamed,
                ..good
            },
        ] {
            assert_eq!(
                open(&one(&k, 1), &aad, &sealed).unwrap_err(),
                OpenError::BadTag,
                "{:?}",
                aad
            );
        }
        // Another computer's key can't open it either.
        assert_eq!(
            open(&one(&keys(3), 1), &good, &sealed).unwrap_err(),
            OpenError::BadTag
        );
    }

    #[test]
    fn truncation_at_every_byte_fails() {
        let k = keys(4);
        let (name, sealed) = shard(&k, br#"{"records":[1,2,3,4,5,6,7,8,9]}"#);
        let aad = shard_aad(&name);
        for len in 0..sealed.len() {
            let err = open(&one(&k, 1), &aad, &sealed[..len]).unwrap_err();
            if len < HEADER_LEN + TAG_LEN {
                assert_eq!(err, OpenError::Truncated, "cut at {}", len);
            } else {
                assert_eq!(err, OpenError::BadTag, "cut at {}", len);
            }
        }
        // Zeros where a download hasn't landed yet read as truncated.
        let zeros = vec![0u8; sealed.len()];
        assert_eq!(
            open(&one(&k, 1), &aad, &zeros).unwrap_err(),
            OpenError::Truncated
        );
    }

    #[test]
    fn a_bit_flip_anywhere_fails() {
        let k = keys(5);
        let (name, sealed) = shard(&k, br#"{"x":"y"}"#);
        let aad = shard_aad(&name);
        for i in 0..sealed.len() {
            for bit in [0, 3, 7] {
                let mut bad = sealed.clone();
                bad[i] ^= 1 << bit;
                let err = open(&one(&k, 1), &aad, &bad).unwrap_err();
                if (7..11).contains(&i) {
                    // The epoch: no key for the epoch it now names.
                    assert!(matches!(err, OpenError::UnknownEpoch(_)), "byte {}", i);
                } else {
                    assert_eq!(err, OpenError::BadTag, "byte {} bit {}", i, bit);
                }
            }
        }
    }

    #[test]
    fn wrong_epoch_is_reported() {
        let k = keys(6);
        let plain = b"{}";
        let name = format!("{}.kage", k.content_name(plain));
        let aad = shard_aad(&name);
        let sealed = seal(&k, 3, &aad, plain);
        assert_eq!(
            open(&one(&k, 2), &aad, &sealed).unwrap_err(),
            OpenError::UnknownEpoch(3)
        );
        // The right epoch number with another epoch's key.
        let wrong = keys(7);
        assert_eq!(
            open(&one(&wrong, 3), &aad, &sealed).unwrap_err(),
            OpenError::BadTag
        );
    }

    #[test]
    fn content_name_mismatch_is_detected() {
        let k = keys(8);
        // Sealed under a name that isn't its content's.
        let name = format!("{}.kage", k.content_name(b"other"));
        let aad = shard_aad(&name);
        let sealed = seal(&k, 1, &aad, b"this");
        assert_eq!(
            open(&one(&k, 1), &aad, &sealed).unwrap_err(),
            OpenError::NameMismatch
        );
        // Heads aren't content-named.
        let head = Aad {
            kind: FileKind::Head,
            name: "head-000000000001.kage",
            ..aad
        };
        let sealed = seal(&k, 1, &head, b"this");
        assert_eq!(open(&one(&k, 1), &head, &sealed).unwrap(), b"this");
    }

    #[test]
    fn oversized_files_and_plaintexts_are_refused() {
        let k = keys(9);
        let head = Aad {
            folder_id: FOLDER,
            device: dev(),
            kind: FileKind::Head,
            name: "head-000000000001.kage",
        };
        // Compresses to almost nothing, inflates past the head limit.
        let bomb = vec![b' '; MAX_HEAD_PLAIN + 1];
        let sealed = seal(&k, 1, &head, &bomb);
        assert!(sealed.len() < 2048);
        assert_eq!(
            open(&one(&k, 1), &head, &sealed).unwrap_err(),
            OpenError::TooLarge
        );
        let exact = vec![b' '; MAX_HEAD_PLAIN];
        let sealed = seal(&k, 1, &head, &exact);
        assert_eq!(
            open(&one(&k, 1), &head, &sealed).unwrap().len(),
            MAX_HEAD_PLAIN
        );
        let huge = vec![0u8; max_sealed(FileKind::Head) as usize + 1];
        assert_eq!(
            open(&one(&k, 1), &head, &huge).unwrap_err(),
            OpenError::TooLarge
        );
    }

    #[test]
    fn foreign_bytes_are_not_ready_or_bad() {
        let k = keys(10);
        let aad = shard_aad("x.kage");
        let mut other = b"{\"not\": \"a kage file\", \"padding\": \"................\"}".to_vec();
        assert_eq!(
            open(&one(&k, 1), &aad, &other).unwrap_err(),
            OpenError::BadTag
        );
        other.truncate(10);
        assert_eq!(
            open(&one(&k, 1), &aad, &other).unwrap_err(),
            OpenError::Truncated
        );
        // A newer envelope version.
        let (name, mut sealed) = shard(&k, b"{}");
        sealed[5] = 2;
        assert_eq!(
            open(&one(&k, 1), &shard_aad(&name), &sealed).unwrap_err(),
            OpenError::BadTag
        );
    }

    // --- Golden files ---------------------------------------------------------

    /// The fixed inputs of the golden files. Never change them: files of
    /// version 1 must keep opening.
    mod golden {
        use super::*;

        pub const PASSPHRASE: &str = "ABCDE-FGHJK-MNPQR-STVWX-YZ012";
        pub const SALT_B64: &str = "AAECAwQFBgcICQoLDA0ODw==";
        pub const WRITTEN_AT: &str = "2026-11-09T10:00:00Z";
        pub const HEAD: &str = r#"{"device":"k7m2p9xqa3rt5vwe","format":1,"min_reader":1,"namespaces":{"cfg":{"buckets":1,"schema":1,"shards":["SHARD"]}},"seq":42}"#;
        pub const SHARD: &str = r#"{"bucket":0,"ns":"cfg","of":1,"records":[{"c":{"ui.font_size@all":[14,"0001930b6f1c0004k7m2p9xqa3rt5vwe",1762686000000]},"id":"main"}],"schema":1}"#;

        pub fn dek() -> [u8; 32] {
            let mut d = [0u8; 32];
            for (i, b) in d.iter_mut().enumerate() {
                *b = 0xa0 ^ i as u8;
            }
            d
        }

        pub fn blob() -> String {
            let text = "Golden blob text. ".repeat(4096);
            serde_json::json!({ "len": text.len(), "data": text }).to_string()
        }

        pub fn dir() -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sync/envelope")
        }

        /// (file name, kind, plaintext) of every sealed golden file.
        pub fn files(k: &Keys) -> Vec<(String, FileKind, Vec<u8>)> {
            let shard = SHARD.as_bytes().to_vec();
            let shard_name = k.content_name(&shard);
            let head = HEAD.replace("SHARD", &shard_name).into_bytes();
            let blob = blob().into_bytes();
            vec![
                ("head-000000000042.kage".to_string(), FileKind::Head, head),
                (format!("{}.kage", shard_name), FileKind::Shard, shard),
                (
                    format!("{}.kage", k.content_name(&blob)),
                    FileKind::Blob,
                    blob,
                ),
            ]
        }

        pub const KEY_FILE: &str = "e0001-k7m2p9xqa3rt5vwe-0001.json";
    }

    /// Writes the golden files. Run it only when adding a new version's
    /// files: `cargo test --lib sync::envelope -- --ignored`.
    #[test]
    #[ignore]
    fn regenerate_golden_files() {
        use crate::sync::crypto::{seal_key_file, Kdf, KdfCost};
        let k = Keys::derive(&golden::dek());
        let dir = golden::dir();
        std::fs::create_dir_all(&dir).unwrap();
        for (i, (name, kind, plain)) in golden::files(&k).into_iter().enumerate() {
            let aad = Aad {
                folder_id: FOLDER,
                device: dev(),
                kind,
                name: &name,
            };
            let sealed = seal_with_nonce(&k, 1, &aad, &plain, [i as u8 + 1; 12]);
            std::fs::write(dir.join(&name), sealed).unwrap();
        }
        let cost = KdfCost::DEFAULT;
        let kdf = Kdf {
            alg: "argon2id".into(),
            m_kib: cost.m_kib,
            t: cost.t,
            p: cost.p,
            salt: golden::SALT_B64.into(),
        };
        let kf = seal_key_file(
            golden::PASSPHRASE,
            &golden::dek(),
            FOLDER,
            1,
            kdf,
            [9; 12],
            golden::WRITTEN_AT.into(),
        )
        .unwrap();
        let mut json = serde_json::to_string_pretty(&kf).unwrap();
        json.push('\n');
        std::fs::write(dir.join(golden::KEY_FILE), json).unwrap();
    }

    /// Version 1 files written by an earlier build still open, and the
    /// key file still unwraps (at the real Argon2 cost).
    #[test]
    fn golden_files_of_version_1_open() {
        let dek = golden::dek();
        let k = Keys::derive(&dek);
        let dir = golden::dir();
        let files = golden::files(&k);
        assert_eq!(files.len(), 3);
        for (name, kind, plain) in files {
            let bytes = std::fs::read(dir.join(&name))
                .unwrap_or_else(|e| panic!("golden file {} is missing: {}", name, e));
            let aad = Aad {
                folder_id: FOLDER,
                device: dev(),
                kind,
                name: &name,
            };
            assert_eq!(open(&one(&k, 1), &aad, &bytes).unwrap(), plain, "{}", name);
        }
        let kf: KeyFile =
            serde_json::from_slice(&std::fs::read(dir.join(golden::KEY_FILE)).unwrap()).unwrap();
        assert_eq!((kf.kdf.m_kib, kf.kdf.t, kf.kdf.p), (65536, 3, 1));
        let back = unwrap_dek(golden::PASSPHRASE, &kf, FOLDER).unwrap();
        assert_eq!(&back[..], &dek[..]);
    }
}
