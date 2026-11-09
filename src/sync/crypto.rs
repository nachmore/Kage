//! Keys and the passphrase (docs/design/SYNC.md §11.3): a random data key
//! (DEK) per epoch, wrapped by Argon2id of the sync passphrase in a key
//! file, and HKDF subkeys for encryption, names and extension device tags.
//!
//! Fixed labels, part of the v1 format (golden vectors in the tests):
//! - `enc`, `name` and `extdev` are HKDF-SHA256 of the DEK with no salt and
//!   the infos `kage-sync/v1/enc`, `kage-sync/v1/name` and
//!   `kage-sync/v1/extdev`;
//! - a content name is `hex(HMAC-SHA256(name, plaintext))[..32]`;
//! - a bucket is the first two bytes of `HMAC-SHA256(name, ns "|" id)`,
//!   big-endian, modulo the bucket count;
//! - a device tag is `HKDF-Expand(extdev, "kage-sync/v1/ext-device|" ext "|"
//!   device)`, 12 bytes as 24 lowercase hex characters.
//!
//! The passphrase is normalised before Argon2 ([`normalize_passphrase`]):
//! NFC, and a generated passphrase is accepted in any case, with or without
//! its dashes and spaces.

use super::types::DeviceId;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

const INFO_ENC: &[u8] = b"kage-sync/v1/enc";
const INFO_NAME: &[u8] = b"kage-sync/v1/name";
const INFO_EXTDEV: &[u8] = b"kage-sync/v1/extdev";
const INFO_EXT_DEVICE: &str = "kage-sync/v1/ext-device";

/// The Argon2id cost new key files use (§11.3). It runs once per computer
/// at setup, join, unlock and passphrase change, never per sync.
pub const KDF_M_KIB: u32 = 64 * 1024;
pub const KDF_T: u32 = 3;
pub const KDF_P: u32 = 1;
/// Bytes of Argon2 salt in new key files.
const SALT_LEN: usize = 16;
/// The most a key file may ask for. A key file is plain JSON anyone with
/// write access to the folder could plant, so its cost is bounded.
const KDF_MAX_M_KIB: u32 = 256 * 1024;
const KDF_MAX_T: u32 = 8;
const KDF_MAX_P: u32 = 4;
/// nonce | DEK | tag.
const WRAPPED_LEN: usize = 12 + 32 + 16;

/// The 32-symbol Crockford base32 alphabet, lowercase as ids use it.
const CROCKFORD_LOWER: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
/// Characters in a generated passphrase, and per group.
const PASSPHRASE_CHARS: usize = 25;
const PASSPHRASE_GROUP: usize = 5;

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
        let hk = hkdf::Hkdf::<Sha256>::new(None, dek);
        let expand = |info: &[u8]| {
            let mut out = Zeroizing::new([0u8; 32]);
            // 32 bytes is far below HKDF-SHA256's 8160-byte limit.
            hk.expand(info, &mut out[..])
                .expect("32 bytes is a valid HKDF-SHA256 length");
            out
        };
        Self {
            enc: expand(INFO_ENC),
            name: expand(INFO_NAME),
            extdev: expand(INFO_EXTDEV),
        }
    }

    /// The key that seals files (§5.4).
    pub(crate) fn enc_key(&self) -> &[u8; 32] {
        &self.enc
    }

    /// A content name: 32 hex characters of a keyed hash of `plain`, the
    /// canonical plaintext of a shard or blob.
    pub fn content_name(&self, plain: &[u8]) -> String {
        let mac = self.mac(&[plain]);
        hex::encode(&mac[..16])
    }

    /// The shard bucket of a record, out of `of` (a power of two, at
    /// least 1).
    pub fn bucket(&self, ns: &str, id: &str, of: u32) -> u32 {
        if of <= 1 {
            return 0;
        }
        let mac = self.mac(&[ns.as_bytes(), b"|", id.as_bytes()]);
        u32::from(u16::from_be_bytes([mac[0], mac[1]])) % of
    }

    /// The key for content names, buckets and conflict ids.
    pub fn name_key(&self) -> &[u8; 32] {
        &self.name
    }

    /// `HKDF(extdev, extension id ‖ device id)[..12]` as 24 lowercase hex
    /// characters (§12.6): stable per extension and computer, not linkable
    /// across extensions.
    pub fn ext_device_tag(&self, ext: &str, dev: DeviceId) -> String {
        let hk = hkdf::Hkdf::<Sha256>::from_prk(&self.extdev[..])
            .expect("a 32-byte key is a valid HKDF-SHA256 PRK");
        let info = format!("{}|{}|{}", INFO_EXT_DEVICE, ext, device_text(dev));
        let mut out = [0u8; 12];
        hk.expand(info.as_bytes(), &mut out)
            .expect("12 bytes is a valid HKDF-SHA256 length");
        hex::encode(out)
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.name[..])
            .expect("HMAC takes a key of any length");
        for p in parts {
            mac.update(p);
        }
        mac.finalize().into_bytes().into()
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

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WrongPassphrase => "the passphrase doesn't unlock this key file",
            Self::UnsupportedKdf => "the key file uses a key derivation this build doesn't take",
            Self::Malformed => "the key file is damaged or isn't for this folder",
        })
    }
}

impl std::error::Error for CryptoError {}

/// A new random data key.
pub fn new_dek() -> Zeroizing<[u8; 32]> {
    let mut dek = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(&mut dek[..]);
    dek
}

/// The Argon2id cost of a key file, without its salt.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfCost {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfCost {
    /// What new key files use (§11.3).
    pub const DEFAULT: Self = Self {
        m_kib: KDF_M_KIB,
        t: KDF_T,
        p: KDF_P,
    };
}

/// The AAD of a wrapped DEK: `kage-sync-dek|<folder_id>|<epoch>`.
fn wrap_aad(folder_id: &str, epoch: u32) -> String {
    format!("kage-sync-dek|{}|{}", folder_id, epoch)
}

/// Wrap `dek` with the passphrase, for `folder_id` and `epoch`.
pub fn wrap_dek(
    passphrase: &str,
    dek: &[u8; 32],
    folder_id: &str,
    epoch: u32,
) -> anyhow::Result<KeyFile> {
    wrap_dek_with(passphrase, dek, folder_id, epoch, KdfCost::DEFAULT)
}

/// [`wrap_dek`] at another Argon2 cost. Tests use a cheap one; the app
/// always uses [`KdfCost::DEFAULT`].
#[doc(hidden)]
pub fn wrap_dek_with(
    passphrase: &str,
    dek: &[u8; 32],
    folder_id: &str,
    epoch: u32,
    cost: KdfCost,
) -> anyhow::Result<KeyFile> {
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let kdf = Kdf {
        alg: "argon2id".to_string(),
        m_kib: cost.m_kib,
        t: cost.t,
        p: cost.p,
        salt: b64().encode(salt),
    };
    let written_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    seal_key_file(passphrase, dek, folder_id, epoch, kdf, nonce, written_at)
}

/// Build a key file from fixed inputs (the golden fixtures use this).
#[doc(hidden)]
pub fn seal_key_file(
    passphrase: &str,
    dek: &[u8; 32],
    folder_id: &str,
    epoch: u32,
    kdf: Kdf,
    nonce: [u8; 12],
    written_at: String,
) -> anyhow::Result<KeyFile> {
    let salt = b64()
        .decode(&kdf.salt)
        .map_err(|e| anyhow::anyhow!("bad salt: {}", e))?;
    let kek = derive_kek(passphrase, &salt, &kdf).map_err(anyhow::Error::new)?;
    let aad = wrap_aad(folder_id, epoch);
    let cipher = Aes256Gcm::new_from_slice(&kek[..]).expect("a 32-byte AES-256 key");
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &dek[..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("AES-GCM refused to seal the data key"))?;
    let mut wrapped = Vec::with_capacity(WRAPPED_LEN);
    wrapped.extend_from_slice(&nonce);
    wrapped.extend_from_slice(&sealed);
    Ok(KeyFile {
        epoch,
        kdf,
        wrapped: b64().encode(&wrapped),
        aad,
        written_at,
    })
}

/// Unwrap a key file's DEK with the passphrase.
pub fn unwrap_dek(
    passphrase: &str,
    kf: &KeyFile,
    folder_id: &str,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    // A key file of another folder, or one moved between epochs, fails
    // here, before the expensive derivation.
    if kf.aad != wrap_aad(folder_id, kf.epoch) {
        return Err(CryptoError::Malformed);
    }
    let wrapped = b64()
        .decode(&kf.wrapped)
        .map_err(|_| CryptoError::Malformed)?;
    if wrapped.len() != WRAPPED_LEN {
        return Err(CryptoError::Malformed);
    }
    let salt = b64()
        .decode(&kf.kdf.salt)
        .map_err(|_| CryptoError::Malformed)?;
    let kek = derive_kek(passphrase, &salt, &kf.kdf)?;
    let cipher = Aes256Gcm::new_from_slice(&kek[..]).expect("a 32-byte AES-256 key");
    let (nonce, sealed) = wrapped.split_at(12);
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: kf.aad.as_bytes(),
                },
            )
            .map_err(|_| CryptoError::WrongPassphrase)?,
    );
    let mut dek = Zeroizing::new([0u8; 32]);
    if plain.len() != 32 {
        return Err(CryptoError::Malformed);
    }
    dek.copy_from_slice(&plain);
    Ok(dek)
}

/// Argon2id of the normalised passphrase, after checking the key file's
/// parameters are ones this build takes.
fn derive_kek(
    passphrase: &str,
    salt: &[u8],
    kdf: &Kdf,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    if kdf.alg != "argon2id"
        || kdf.p == 0
        || kdf.p > KDF_MAX_P
        || kdf.t == 0
        || kdf.t > KDF_MAX_T
        || kdf.m_kib < 8 * kdf.p
        || kdf.m_kib > KDF_MAX_M_KIB
    {
        return Err(CryptoError::UnsupportedKdf);
    }
    if salt.len() < argon2::MIN_SALT_LEN {
        return Err(CryptoError::Malformed);
    }
    let params = argon2::Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32))
        .map_err(|_| CryptoError::UnsupportedKdf)?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let normalized = Zeroizing::new(normalize_passphrase(passphrase));
    let mut kek = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(normalized.as_bytes(), salt, &mut kek[..])
        .map_err(|_| CryptoError::UnsupportedKdf)?;
    Ok(kek)
}

/// What Argon2 is given for a passphrase: its NFC form, so the same
/// passphrase typed on macOS and Windows gives the same key. A passphrase
/// that reads as a generated one (25 Crockford base32 characters, in any
/// case, with or without dashes and spaces, with `I`/`L` for `1` and `O`
/// for `0`) becomes its canonical `XXXXX-XXXXX-XXXXX-XXXXX-XXXXX` form.
pub fn normalize_passphrase(passphrase: &str) -> String {
    let mut nfc = Zeroizing::new(
        icu_normalizer::ComposingNormalizerBorrowed::new_nfc()
            .normalize(passphrase)
            .into_owned(),
    );
    let mut symbols = Zeroizing::new(String::with_capacity(PASSPHRASE_CHARS));
    let mut generated = true;
    for c in nfc.chars() {
        if c == '-' || c.is_ascii_whitespace() {
            continue;
        }
        match crockford_symbol(c) {
            Some(sym) if symbols.len() < PASSPHRASE_CHARS => symbols.push(sym),
            _ => {
                generated = false;
                break;
            }
        }
    }
    if generated && symbols.len() == PASSPHRASE_CHARS {
        return group_passphrase(&symbols);
    }
    // Moved out, not copied: the caller wipes it.
    std::mem::take(&mut *nfc)
}

/// The canonical uppercase symbol for a Crockford base32 character.
fn crockford_symbol(c: char) -> Option<char> {
    let c = c.to_ascii_uppercase();
    match c {
        'O' => Some('0'),
        'I' | 'L' => Some('1'),
        '0'..='9' => Some(c),
        'A'..='Z' if !matches!(c, 'U') => Some(c),
        _ => None,
    }
}

fn group_passphrase(symbols: &str) -> String {
    let mut out = String::with_capacity(PASSPHRASE_CHARS + PASSPHRASE_CHARS / PASSPHRASE_GROUP);
    for (i, c) in symbols.chars().enumerate() {
        if i > 0 && i % PASSPHRASE_GROUP == 0 {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// "Make one for me": 25 Crockford base32 characters (125 bits) in five
/// groups of five.
pub fn generate_passphrase() -> String {
    let mut rng = rand::rng();
    let symbols: Zeroizing<String> = Zeroizing::new(
        (0..PASSPHRASE_CHARS)
            .map(|_| CROCKFORD_LOWER[rng.random_range(0..32usize)].to_ascii_uppercase() as char)
            .collect(),
    );
    group_passphrase(&symbols)
}

/// 80 random bits as 16 lowercase Crockford base32 characters: the form of
/// folder and device ids (§5.1).
pub fn random_id() -> String {
    let mut bytes = [0u8; 10];
    rand::rng().fill_bytes(&mut bytes);
    base32_lower(&bytes)
}

/// The text form of a device id (§5.1): its 80 bits, most significant
/// first, as 16 lowercase Crockford base32 characters. Text sorts like the
/// bytes.
pub fn device_text(dev: DeviceId) -> String {
    base32_lower(&dev.0)
}

/// The device id named by 16 lowercase Crockford base32 characters.
pub fn parse_device_text(text: &str) -> Option<DeviceId> {
    if !is_id_text(text) {
        return None;
    }
    let mut acc: u128 = 0;
    for b in text.bytes() {
        let v = CROCKFORD_LOWER.iter().position(|&a| a == b)? as u128;
        acc = (acc << 5) | v;
    }
    let mut out = [0u8; 10];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (acc >> (8 * (9 - i))) as u8;
    }
    Some(DeviceId(out))
}

/// Whether `text` has the form of a folder or device id.
pub fn is_id_text(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| CROCKFORD_LOWER.contains(&b))
}

fn base32_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for &b in bytes {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(CROCKFORD_LOWER[((buf >> bits) & 31) as usize] as char);
        }
        buf &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(CROCKFORD_LOWER[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap Argon2 for tests; the app always uses [`KdfCost::DEFAULT`].
    const CHEAP: KdfCost = KdfCost {
        m_kib: 64,
        t: 1,
        p: 1,
    };
    const FOLDER: &str = "5qg2vbn8h1xk0d3m";

    fn dek(fill: u8) -> [u8; 32] {
        [fill; 32]
    }

    /// The HKDF labels are part of the v1 format: these values must never
    /// change. They were computed independently (Python's `hmac` and
    /// `hashlib`), not by this code. `sync::envelope`'s golden files depend
    /// on them too.
    #[test]
    fn hkdf_labels_are_fixed() {
        let mut dek = [0u8; 32];
        for (i, b) in dek.iter_mut().enumerate() {
            *b = i as u8;
        }
        let keys = Keys::derive(&dek);
        assert_eq!(
            hex::encode(&keys.enc[..]),
            "e7f54e8219b2737f7a306bc85d6ad5ba151b1d1ee5b73fd0efd1fa9b0f37737a"
        );
        assert_eq!(
            hex::encode(&keys.name[..]),
            "7f781e1f5c06c38de898b9a4703ea76bad6e3c4998c7728bb1329eb8e83d8120"
        );
        assert_eq!(
            hex::encode(&keys.extdev[..]),
            "54c0f74daa40fa6a76cb3b5e64711663fd02aba9ee3bcd0b35e8bb2369e1257f"
        );
        assert_eq!(
            keys.content_name(br#"{"ns":"cfg"}"#),
            "58f03860723ff1d0402268a4c67a0337"
        );
        let dev = parse_device_text("k7m2p9xqa3rt5vwe").unwrap();
        assert_eq!(
            keys.ext_device_tag("mindmap", dev),
            "dafb1f8a0c52086b2a539f00"
        );
        assert_eq!(keys.bucket("list.shortcuts", "s7hq2m0c5d1z9w3pa", 16), 6);
    }

    #[test]
    fn subkeys_differ_and_depend_on_the_dek() {
        let a = Keys::derive(&dek(1));
        let b = Keys::derive(&dek(2));
        assert_ne!(&a.enc[..], &a.name[..]);
        assert_ne!(&a.name[..], &a.extdev[..]);
        assert_ne!(&a.enc[..], &b.enc[..]);
        assert_ne!(a.content_name(b"x"), b.content_name(b"x"));
    }

    #[test]
    fn content_names_are_32_hex_and_keyed() {
        let keys = Keys::derive(&dek(7));
        let n = keys.content_name(b"same plaintext");
        assert_eq!(n.len(), 32);
        assert!(n
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(n, keys.content_name(b"same plaintext"));
        assert_ne!(n, keys.content_name(b"other plaintext"));
    }

    #[test]
    fn buckets_stay_in_range_and_spread() {
        let keys = Keys::derive(&dek(3));
        assert_eq!(keys.bucket("ns", "id", 1), 0);
        assert_eq!(keys.bucket("ns", "id", 0), 0);
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..400 {
            let b = keys.bucket("ext.mindmap.c.items", &format!("id{}", i), 16);
            assert!(b < 16);
            seen.insert(b);
        }
        assert_eq!(seen.len(), 16, "400 ids should reach every bucket");
    }

    #[test]
    fn device_tags_are_stable_and_not_linkable() {
        let keys = Keys::derive(&dek(9));
        let dev = DeviceId([1; 10]);
        let other = DeviceId([2; 10]);
        let t = keys.ext_device_tag("mindmap", dev);
        assert_eq!(t.len(), 24);
        assert_eq!(t, keys.ext_device_tag("mindmap", dev));
        assert_ne!(t, keys.ext_device_tag("todos", dev));
        assert_ne!(t, keys.ext_device_tag("mindmap", other));
    }

    #[test]
    fn wrap_unwrap_and_rewrap() {
        let d = new_dek();
        let kf = wrap_dek_with("correct horse battery", &d, FOLDER, 1, CHEAP).unwrap();
        assert_eq!(kf.epoch, 1);
        assert_eq!(kf.aad, "kage-sync-dek|5qg2vbn8h1xk0d3m|1");
        assert_eq!(kf.kdf.alg, "argon2id");
        assert_eq!(b64().decode(&kf.kdf.salt).unwrap().len(), 16);
        assert_eq!(b64().decode(&kf.wrapped).unwrap().len(), WRAPPED_LEN);
        assert!(chrono::DateTime::parse_from_rfc3339(&kf.written_at).is_ok());
        let back = unwrap_dek("correct horse battery", &kf, FOLDER).unwrap();
        assert_eq!(&back[..], &d[..]);

        // Change passphrase: the same DEK under a new key file.
        let kf2 = wrap_dek_with("a new passphrase!", &back, FOLDER, 1, CHEAP).unwrap();
        assert_ne!(kf2.wrapped, kf.wrapped);
        assert_ne!(kf2.kdf.salt, kf.kdf.salt);
        assert_eq!(
            &unwrap_dek("a new passphrase!", &kf2, FOLDER).unwrap()[..],
            &d[..]
        );
        assert_eq!(
            unwrap_dek("correct horse battery", &kf2, FOLDER).unwrap_err(),
            CryptoError::WrongPassphrase
        );
    }

    #[test]
    fn unwrap_refuses_the_wrong_folder_epoch_or_damage() {
        let kf = wrap_dek_with("passphrase 123", &dek(5), FOLDER, 2, CHEAP).unwrap();
        assert_eq!(
            unwrap_dek("passphrase 123", &kf, "aaaaaaaaaaaaaaaa").unwrap_err(),
            CryptoError::Malformed
        );
        let mut moved = kf.clone();
        moved.epoch = 3;
        assert_eq!(
            unwrap_dek("passphrase 123", &moved, FOLDER).unwrap_err(),
            CryptoError::Malformed
        );
        // An AAD rewritten to match the moved epoch still fails the tag.
        moved.aad = wrap_aad(FOLDER, 3);
        assert_eq!(
            unwrap_dek("passphrase 123", &moved, FOLDER).unwrap_err(),
            CryptoError::WrongPassphrase
        );
        let mut flipped = kf.clone();
        let mut raw = b64().decode(&flipped.wrapped).unwrap();
        raw[20] ^= 1;
        flipped.wrapped = b64().encode(&raw);
        assert_eq!(
            unwrap_dek("passphrase 123", &flipped, FOLDER).unwrap_err(),
            CryptoError::WrongPassphrase
        );
        let mut short = kf.clone();
        short.wrapped = b64().encode([0u8; 40]);
        assert_eq!(
            unwrap_dek("passphrase 123", &short, FOLDER).unwrap_err(),
            CryptoError::Malformed
        );
        let mut garbage = kf.clone();
        garbage.wrapped = "not base64!".into();
        assert_eq!(
            unwrap_dek("passphrase 123", &garbage, FOLDER).unwrap_err(),
            CryptoError::Malformed
        );
    }

    #[test]
    fn unwrap_refuses_kdfs_it_does_not_take() {
        let kf = wrap_dek_with("passphrase 123", &dek(5), FOLDER, 1, CHEAP).unwrap();
        for change in [
            |k: &mut Kdf| k.alg = "scrypt".into(),
            |k: &mut Kdf| k.m_kib = KDF_MAX_M_KIB + 1,
            |k: &mut Kdf| k.t = 0,
            |k: &mut Kdf| k.t = KDF_MAX_T + 1,
            |k: &mut Kdf| k.p = 0,
            |k: &mut Kdf| k.p = KDF_MAX_P + 1,
            |k: &mut Kdf| k.m_kib = 4,
        ] {
            let mut bad = kf.clone();
            change(&mut bad.kdf);
            assert_eq!(
                unwrap_dek("passphrase 123", &bad, FOLDER).unwrap_err(),
                CryptoError::UnsupportedKdf
            );
        }
        let mut bad_salt = kf.clone();
        bad_salt.kdf.salt = b64().encode([1u8; 4]);
        assert_eq!(
            unwrap_dek("passphrase 123", &bad_salt, FOLDER).unwrap_err(),
            CryptoError::Malformed
        );
    }

    /// The real cost, once: what a key file the app writes says.
    #[test]
    fn new_key_files_use_the_documented_cost() {
        let kf = wrap_dek("passphrase 123", &dek(4), FOLDER, 1).unwrap();
        assert_eq!((kf.kdf.m_kib, kf.kdf.t, kf.kdf.p), (65536, 3, 1));
        assert_eq!(
            &unwrap_dek("passphrase 123", &kf, FOLDER).unwrap()[..],
            &dek(4)[..]
        );
    }

    #[test]
    fn key_files_round_trip_as_the_documented_json() {
        let kf = wrap_dek_with("passphrase 123", &dek(5), FOLDER, 1, CHEAP).unwrap();
        let json = serde_json::to_value(&kf).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["aad", "epoch", "kdf", "wrapped", "written_at"]);
        let mut kdf: Vec<_> = json["kdf"].as_object().unwrap().keys().cloned().collect();
        kdf.sort();
        assert_eq!(kdf, ["alg", "m_kib", "p", "salt", "t"]);
        let back: KeyFile = serde_json::from_value(json).unwrap();
        assert_eq!(back, kf);
    }

    #[test]
    fn generated_passphrases_have_the_documented_shape() {
        let p = generate_passphrase();
        assert_eq!(p.len(), 29);
        let groups: Vec<&str> = p.split('-').collect();
        assert_eq!(groups.len(), 5);
        for g in groups {
            assert_eq!(g.len(), 5);
            assert!(g
                .bytes()
                .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b)));
        }
        assert_ne!(p, generate_passphrase());
        assert_eq!(normalize_passphrase(&p), p);
    }

    #[test]
    fn generated_passphrases_are_typed_forgivingly() {
        let canonical = "ABCDE-FGHJK-MNPQR-STVWX-YZ012";
        for typed in [
            "abcde-fghjk-mnpqr-stvwx-yz012",
            "ABCDEFGHJKMNPQRSTVWXYZ012",
            "abcde fghjk mnpqr stvwx yz012\n",
            "ABCDE-FGHJK-MNPQR-STVWX-YZO12",
            "ABCDE-FGHJK-MNPQR-STVWX-YZ0I2",
            "ABCDE-FGHJK-MNPQR-STVWX-YZ0l2",
        ] {
            assert_eq!(normalize_passphrase(typed), canonical, "{:?}", typed);
        }
        let kf = wrap_dek_with(canonical, &dek(6), FOLDER, 1, CHEAP).unwrap();
        assert!(unwrap_dek("abcdefghjkmnpqrstvwxyz012", &kf, FOLDER).is_ok());
    }

    #[test]
    fn other_passphrases_are_only_nfc_normalised() {
        // `U` isn't Crockford, and a 24- or 26-symbol one isn't generated.
        for p in [
            "ABCDE-FGHJK-MNPQR-STVWX-YZ01U",
            "ABCDE-FGHJK-MNPQR-STVWX-YZ01",
            "ABCDE-FGHJK-MNPQR-STVWX-YZ0123",
            "  my passphrase with spaces  ",
            "Correct Horse",
        ] {
            assert_eq!(normalize_passphrase(p), p);
        }
        let decomposed = "cafe\u{301} au lait!";
        let composed = "caf\u{e9} au lait!";
        assert_eq!(normalize_passphrase(decomposed), composed);
        let kf = wrap_dek_with(composed, &dek(6), FOLDER, 1, CHEAP).unwrap();
        assert!(unwrap_dek(decomposed, &kf, FOLDER).is_ok());
    }

    #[test]
    fn ids_are_crockford_and_round_trip() {
        let id = random_id();
        assert!(is_id_text(&id), "{}", id);
        assert_ne!(id, random_id());
        assert_eq!(device_text(DeviceId([0; 10])), "0000000000000000");
        assert_eq!(device_text(DeviceId([0xff; 10])), "zzzzzzzzzzzzzzzz");
        let dev = DeviceId([0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23]);
        let text = device_text(dev);
        assert_eq!(text, "28t5cy4tqkff0093");
        assert_eq!(parse_device_text(&text), Some(dev));
        assert_eq!(parse_device_text("28T5CY4TQKFF0093"), None);
        assert_eq!(parse_device_text("28t5cy4tqkff009"), None);
        assert_eq!(parse_device_text("28t5cy4tqkff009u"), None);
        // Text sorts like the bytes.
        let a = DeviceId([0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let b = DeviceId([0, 0, 0, 0, 0, 0, 0, 0, 1, 0]);
        assert!(a < b && device_text(a) < device_text(b));
    }

    #[test]
    fn new_deks_are_random() {
        assert_ne!(&new_dek()[..], &new_dek()[..]);
        assert_ne!(&new_dek()[..], &[0u8; 32][..]);
    }
}
