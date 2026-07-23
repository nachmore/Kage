//! Cross-device config export / import.
//!
//! The launcher pitch demands moving to a second machine without
//! retyping everything. This module bundles the user's config plus
//! the side-files that aren't in `config.json` itself (steering docs,
//! extension data) into a single archive — optionally
//! passphrase-encrypted — that the user can drop on another machine
//! and import.
//!
//! ## File layout (zip archive)
//!
//! ```text
//! manifest.json            { format, version, exported_at, kage_version }
//! config.json              the full config, with device-local fields stripped on import
//! steering/auto.md         optional, only if present
//! steering/user.md         optional, only if present
//! extension-data/<ext>/<key>.json   recursive copy of the dir
//! ```
//!
//! Two output shapes:
//!
//!   - **plain**: the zip bytes themselves. File extension `.kage`.
//!   - **encrypted**: AES-256-GCM-wrapped zip with a header carrying
//!     the Argon2id salt and the nonce. File extension `.kage.enc`.
//!     The envelope shape is small enough to be obvious (see
//!     `EncryptedEnvelope::encode`); we keep it custom rather than
//!     depending on `age` so we avoid a heavy dep that few callers will
//!     trigger.
//!
//! ## What's stripped on import
//!
//! Some fields would be actively wrong if copied wholesale:
//!
//!   - `telemetry.install_id` — random per-install GUID. Importing
//!     it would correlate two devices into one in analytics. Local
//!     value is preserved.
//!   - Window geometry (`chat_window_*`, `last_window_*`,
//!     `launcher_*`) — ties to the source machine's monitor layout.
//!     Local values preserved.
//!   - `auto_start` — OS-level autostart hook (scheduled task / Run
//!     key on Windows, LaunchAgent on macOS, XDG autostart on Linux).
//!     The exported value is informational only; we re-apply via the
//!     OS on import.
//!   - `last_extension_update_check`, `updates.last_check_time`,
//!     `updates.last_updated_version` — cache fields that just trigger
//!     fresh checks. Reset to None.
//!
//! Sensitive paths (`user_steering_path`,
//! `pocket_tts.custom_sound_path`, agent connection paths under
//! `acp.connections[].mode.spawn_command`, …) are kept verbatim. They
//! may not exist on the new machine — we let the user fix them rather
//! than guess at silent rewrites.

use crate::config::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// Magic prefix marking the encrypted envelope. Appearing as the
/// first bytes of an imported file means "this is an encrypted Kage
/// backup." Plain archives start with the zip magic (`PK\x03\x04`)
/// and don't carry this prefix.
const ENCRYPTED_MAGIC: &[u8] = b"KAGE-ENC-1\0";

/// Argon2id parameter set. OWASP's 2025 baseline (Memory: 19456 KiB,
/// Iterations: 2, Parallelism: 1) — pricey enough to make brute force
/// painful, fast enough that interactive import doesn't feel slow.
const ARGON2_M_COST: u32 = 19_456;
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Manifest entry written into the archive. Versioned so future
/// breaking changes can refuse to import without giving the user a
/// confusing error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub kage_version: String,
}

impl BundleManifest {
    pub const CURRENT_VERSION: u32 = 1;
    pub const FORMAT: &'static str = "kage-backup";

    pub fn now() -> Self {
        Self {
            format: Self::FORMAT.to_string(),
            version: Self::CURRENT_VERSION,
            exported_at: chrono::Utc::now().to_rfc3339(),
            kage_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Summary returned to the frontend after a successful import. The
/// counts feed the success toast — "imported N shortcuts, M
/// extensions, K bytes of steering" — so the user can see at a
/// glance whether it picked up what they expected.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportSummary {
    pub shortcuts: usize,
    pub extensions: usize,
    pub steering_bytes: usize,
    pub kage_version: String,
    pub exported_at: String,
}

/// Produce a date-stamped filename suggestion for the export dialog.
/// The encrypted variant adds `.enc` so users can tell at a glance
/// what they downloaded.
pub fn default_filename(encrypted: bool) -> String {
    let stamp = chrono::Local::now().format("%Y-%m-%d");
    if encrypted {
        format!("kage-backup-{}.kage.enc", stamp)
    } else {
        format!("kage-backup-{}.kage", stamp)
    }
}

/// Build the archive bytes for the current Config plus the
/// side-files (steering docs, extension data) on disk. Optionally
/// wraps with AES-GCM if a passphrase is given.
pub fn export(config: &Config, passphrase: Option<&str>) -> Result<Vec<u8>> {
    let archive = build_archive(config)?;
    if let Some(pw) = passphrase {
        if pw.is_empty() {
            anyhow::bail!("Passphrase cannot be empty");
        }
        encrypt(&archive, pw)
    } else {
        Ok(archive)
    }
}

/// Inverse of [`export`]. Reads the bytes (handling the encrypted
/// envelope when present), unpacks the archive, sanitises the
/// imported Config against the `local` snapshot of the current
/// device's privacy fields, writes side-files to disk, and returns
/// both the rebuilt Config and a summary.
pub fn import(
    bytes: &[u8],
    passphrase: Option<&str>,
    local: &Config,
) -> Result<(Config, ImportSummary)> {
    let archive = if bytes.starts_with(ENCRYPTED_MAGIC) {
        let pw = passphrase.context("Encrypted backup: passphrase required")?;
        if pw.is_empty() {
            anyhow::bail!("Encrypted backup: passphrase required");
        }
        decrypt(bytes, pw)?
    } else {
        bytes.to_vec()
    };
    unpack_archive(&archive, local)
}

// --- Archive build / unpack --------------------------------------------------

fn build_archive(config: &Config) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // manifest.json
        zip.start_file("manifest.json", opts)?;
        zip.write_all(serde_json::to_string_pretty(&BundleManifest::now())?.as_bytes())?;

        // config.json — pretty-printed so a curious user can diff exports
        zip.start_file("config.json", opts)?;
        zip.write_all(serde_json::to_string_pretty(config)?.as_bytes())?;

        // Steering docs — only added when present, so a fresh install
        // export doesn't carry empty placeholders.
        if let Ok(auto_path) = Config::get_auto_steering_path() {
            if auto_path.exists() {
                let body = std::fs::read(&auto_path).context("Failed to read auto-steering")?;
                zip.start_file("steering/auto.md", opts)?;
                zip.write_all(&body)?;
            }
        }
        if let Some(user_path) = resolve_user_steering_path(config) {
            if user_path.exists() {
                let body = std::fs::read(&user_path).context("Failed to read user-steering")?;
                zip.start_file("steering/user.md", opts)?;
                zip.write_all(&body)?;
            }
        }

        // Extension data — copy the whole subtree under
        // <config_dir>/kage/extension-data/, preserving relative
        // paths so import can rehydrate it 1:1.
        if let Some(ext_root) = extension_data_root() {
            if ext_root.exists() {
                let entries = walk_files(&ext_root)?;
                for (rel, full) in entries {
                    let body = std::fs::read(&full)
                        .with_context(|| format!("Failed to read {:?}", full))?;
                    let zip_path = format!(
                        "extension-data/{}",
                        rel.to_string_lossy().replace('\\', "/")
                    );
                    zip.start_file(zip_path, opts)?;
                    zip.write_all(&body)?;
                }
            }
        }

        zip.finish()?;
    }
    Ok(buf)
}

fn unpack_archive(bytes: &[u8], local: &Config) -> Result<(Config, ImportSummary)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .context("Backup file isn't a valid Kage archive")?;

    // First pass: pull manifest + config + steering blobs into memory.
    let mut manifest: Option<BundleManifest> = None;
    let mut config_json: Option<Vec<u8>> = None;
    let mut steering_auto: Option<Vec<u8>> = None;
    let mut steering_user: Option<Vec<u8>> = None;
    // Extension data we'll write after we've validated everything else.
    let mut extension_files: Vec<(PathBuf, Vec<u8>)> = Vec::new();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut body = Vec::new();
        entry
            .read_to_end(&mut body)
            .with_context(|| format!("Failed to read {} from archive", name))?;

        if name == "manifest.json" {
            manifest = Some(serde_json::from_slice(&body).context("Bad manifest.json")?);
        } else if name == "config.json" {
            config_json = Some(body);
        } else if name == "steering/auto.md" {
            steering_auto = Some(body);
        } else if name == "steering/user.md" {
            steering_user = Some(body);
        } else if let Some(rel) = name.strip_prefix("extension-data/") {
            // Reject any traversal — the zip spec lets entries name
            // `..` segments and we want a hard refusal rather than
            // potentially writing outside the extension-data root.
            let safe = sanitise_archive_path(rel)
                .with_context(|| format!("Refusing unsafe archive path {}", name))?;
            extension_files.push((safe, body));
        }
        // Unknown entries are ignored. Forward-compat: a future
        // version might add `themes/`, `prompts/`, etc.; old code
        // shouldn't fail on them.
    }

    let manifest = manifest.context("Backup is missing manifest.json")?;
    if manifest.format != BundleManifest::FORMAT {
        anyhow::bail!("Not a Kage backup (format = {})", manifest.format);
    }
    if manifest.version > BundleManifest::CURRENT_VERSION {
        anyhow::bail!(
            "Backup is from a newer Kage (format v{}). Update Kage and retry.",
            manifest.version
        );
    }
    let config_bytes = config_json.context("Backup is missing config.json")?;
    let imported: Config =
        serde_json::from_slice(&config_bytes).context("Backup config.json was unparseable")?;

    let merged = sanitise_imported_config(imported, local);

    // Side-files: write atomically AFTER we've validated the config
    // round-trip. If the parse failed above we'd have bailed without
    // touching disk.
    let mut steering_bytes = 0usize;
    if let Ok(auto_path) = Config::get_auto_steering_path() {
        if let Some(body) = steering_auto {
            ensure_parent(&auto_path)?;
            std::fs::write(&auto_path, &body).context("Failed to write auto-steering")?;
            steering_bytes += body.len();
        }
    }
    if let Some(body) = steering_user {
        if let Some(user_path) = resolve_user_steering_path(&merged) {
            ensure_parent(&user_path)?;
            std::fs::write(&user_path, &body).context("Failed to write user-steering")?;
            steering_bytes += body.len();
        }
    }

    let mut extensions_written = 0usize;
    if let Some(ext_root) = extension_data_root() {
        for (rel, body) in &extension_files {
            let full = ext_root.join(rel);
            ensure_parent(&full)?;
            std::fs::write(&full, body).with_context(|| format!("Failed to write {:?}", full))?;
            extensions_written += 1;
        }
    }

    let summary = ImportSummary {
        shortcuts: merged.shortcuts.len(),
        extensions: extensions_written,
        steering_bytes,
        kage_version: manifest.kage_version.clone(),
        exported_at: manifest.exported_at.clone(),
    };

    Ok((merged, summary))
}

// --- Sanitisation -----------------------------------------------------------

/// Copy device-local privacy fields from `local` into `imported`,
/// drop ephemeral cache markers. Pure on the structs — separated so
/// tests can drive every clause without spinning up Tauri.
pub fn sanitise_imported_config(mut imported: Config, local: &Config) -> Config {
    // Telemetry — install_id stays unique to this device, consent is
    // imported but the install id never. Without this, two of the
    // user's machines would aggregate into a single anonymous user
    // in Aptabase.
    imported.telemetry.install_id = local.telemetry.install_id.clone();

    // Update cache fields — let the post-import boot trigger a fresh
    // check rather than carrying a stamp from a different clock /
    // network.
    imported.updates.last_check_time = None;
    imported.updates.last_updated_version = None;
    imported.last_extension_update_check = None;

    // Window geometry — every field that captures pixel positions on
    // the source machine. The local layout almost never matches.
    imported.ui.chat_window_width = local.ui.chat_window_width;
    imported.ui.chat_window_height = local.ui.chat_window_height;
    imported.ui.chat_window_x = local.ui.chat_window_x;
    imported.ui.chat_window_y = local.ui.chat_window_y;
    imported.ui.last_window_x = local.ui.last_window_x;
    imported.ui.last_window_y = local.ui.last_window_y;
    imported.ui.launcher_width = local.ui.launcher_width;
    imported.ui.launcher_height = local.ui.launcher_height;

    // OS-level startup flag — the actual registry/launchd entry is
    // owned by the platform integration, not the JSON. Re-apply via
    // the local value (the import flow calls set_startup_enabled if
    // the saved preference disagrees).
    imported.system.auto_start = local.system.auto_start;

    imported
}

// --- Helpers ----------------------------------------------------------------

fn extension_data_root() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("kage").join("extension-data"))
}

fn resolve_user_steering_path(config: &Config) -> Option<PathBuf> {
    match config.acp.agent.user_steering_path.as_deref() {
        Some(p) if !p.trim().is_empty() => Some(PathBuf::from(p)),
        _ => crate::steering_io::default_user_steering_path().ok(),
    }
}

fn ensure_parent(p: &Path) -> Result<()> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent of {:?}", p))?;
    }
    Ok(())
}

/// Walk a directory recursively, returning `(relative_path, full_path)`
/// pairs. We need the relative path for the zip entry name and the
/// full path for the read.
fn walk_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push((rel, path));
            }
            // Symlinks intentionally skipped — exporting a backup
            // shouldn't follow links out of the user's data dir.
        }
    }
    Ok(out)
}

/// Validate that an archive entry's relative path is safe to write
/// under our extension-data root. Rejects absolute paths, parent-dir
/// traversals (`..`), and any invalid Windows drive prefix on
/// non-Windows systems too (paranoid: the source machine's archive
/// could carry such entries even if we aren't currently building
/// them).
pub fn sanitise_archive_path(rel: &str) -> Result<PathBuf> {
    if rel.is_empty() {
        anyhow::bail!("empty path");
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        anyhow::bail!("absolute path in archive: {}", rel);
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::Normal(n) => clean.push(n),
            Component::CurDir => {
                // `./foo/bar` — ignore the dot, keep walking.
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("disallowed path component in archive: {}", rel);
            }
        }
    }
    Ok(clean)
}

// --- Encryption --------------------------------------------------------------

/// Encrypted envelope layout — fixed-width prefix the import path
/// can recognise without parsing.
///
/// ```text
/// 11 bytes  KAGE-ENC-1\0          magic + version
/// 16 bytes  Argon2id salt
/// 12 bytes  AES-GCM nonce
///  N bytes  ciphertext (AES-256-GCM with the 16-byte tag appended)
/// ```
fn encrypt(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };
    use rand::TryRngCore;

    let mut salt = [0u8; SALT_LEN];
    rand::rngs::OsRng
        .try_fill_bytes(&mut salt)
        .map_err(|e| anyhow::anyhow!("OS RNG (salt): {}", e))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|e| anyhow::anyhow!("OS RNG (nonce): {}", e))?;

    let key = derive_key(passphrase.as_bytes(), &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow::anyhow!("AES init: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("AES encrypt: {}", e))?;

    let mut out =
        Vec::with_capacity(ENCRYPTED_MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(ENCRYPTED_MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(envelope: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };

    let header = ENCRYPTED_MAGIC.len() + SALT_LEN + NONCE_LEN;
    if envelope.len() < header {
        anyhow::bail!("Backup file is too small to be a valid encrypted bundle");
    }
    if !envelope.starts_with(ENCRYPTED_MAGIC) {
        anyhow::bail!("Backup file is not an encrypted Kage bundle");
    }
    let salt = &envelope[ENCRYPTED_MAGIC.len()..ENCRYPTED_MAGIC.len() + SALT_LEN];
    let nonce_bytes =
        &envelope[ENCRYPTED_MAGIC.len() + SALT_LEN..ENCRYPTED_MAGIC.len() + SALT_LEN + NONCE_LEN];
    let ciphertext = &envelope[header..];

    let key = derive_key(passphrase.as_bytes(), salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow::anyhow!("AES init: {}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ciphertext).map_err(|_| {
        // GCM auth failure is the same surface as wrong passphrase
        // from the user's perspective; we surface a friendly message.
        anyhow::anyhow!("Wrong passphrase or corrupted file")
    })
}

fn derive_key(passphrase: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    use argon2::Argon2;
    let params = argon2::Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(KEY_LEN))
        .map_err(|e| anyhow::anyhow!("Argon2 params: {}", e))?;
    let argon = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase, salt, &mut key)
        .map_err(|e| anyhow::anyhow!("Argon2 derive: {}", e))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_config() -> Config {
        let mut c = Config::default();
        c.telemetry.install_id = Some("device-A".to_string());
        c.ui.chat_window_width = 1234;
        c.ui.last_window_x = Some(99);
        c.updates.last_check_time = Some("2026-01-01T00:00:00Z".to_string());
        c.last_extension_update_check = Some("2026-01-01T00:00:00Z".to_string());
        c.system.auto_start = false;
        c
    }

    #[test]
    fn sanitise_replaces_install_id_with_local() {
        let mut imported = fake_config();
        imported.telemetry.install_id = Some("device-source".to_string());
        let mut local = fake_config();
        local.telemetry.install_id = Some("device-local".to_string());
        let merged = sanitise_imported_config(imported, &local);
        assert_eq!(merged.telemetry.install_id.as_deref(), Some("device-local"));
    }

    #[test]
    fn sanitise_resets_update_cache_fields() {
        let imported = fake_config();
        let local = fake_config();
        let merged = sanitise_imported_config(imported, &local);
        assert!(merged.updates.last_check_time.is_none());
        assert!(merged.updates.last_updated_version.is_none());
        assert!(merged.last_extension_update_check.is_none());
    }

    #[test]
    fn sanitise_keeps_local_window_geometry() {
        let mut imported = fake_config();
        imported.ui.chat_window_width = 9999;
        imported.ui.last_window_x = Some(-100);
        let mut local = fake_config();
        local.ui.chat_window_width = 1024;
        local.ui.last_window_x = Some(50);
        let merged = sanitise_imported_config(imported, &local);
        assert_eq!(merged.ui.chat_window_width, 1024);
        assert_eq!(merged.ui.last_window_x, Some(50));
    }

    #[test]
    fn sanitise_keeps_local_auto_start_flag() {
        let mut imported = fake_config();
        imported.system.auto_start = true;
        let mut local = fake_config();
        local.system.auto_start = false;
        let merged = sanitise_imported_config(imported, &local);
        assert!(!merged.system.auto_start);
    }

    #[test]
    fn manifest_round_trip() {
        let m = BundleManifest::now();
        let json = serde_json::to_string(&m).unwrap();
        let m2: BundleManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m2.format, BundleManifest::FORMAT);
        assert_eq!(m2.version, BundleManifest::CURRENT_VERSION);
    }

    #[test]
    fn default_filename_includes_today() {
        let plain = default_filename(false);
        assert!(plain.starts_with("kage-backup-"));
        assert!(plain.ends_with(".kage"));
        let enc = default_filename(true);
        assert!(enc.ends_with(".kage.enc"));
    }

    #[test]
    fn sanitise_archive_path_rejects_traversal() {
        assert!(sanitise_archive_path("ext1/../../etc/passwd").is_err());
        assert!(sanitise_archive_path("/etc/passwd").is_err());
        assert!(sanitise_archive_path("").is_err());
        let good = sanitise_archive_path("ext1/keys.json").unwrap();
        assert_eq!(good, PathBuf::from("ext1").join("keys.json"));
    }

    #[test]
    fn encrypt_round_trip_with_correct_passphrase() {
        let plaintext = b"hello backup";
        let envelope = encrypt(plaintext, "correct horse battery staple").unwrap();
        // Header check.
        assert!(envelope.starts_with(ENCRYPTED_MAGIC));
        let recovered = decrypt(&envelope, "correct horse battery staple").unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn encrypt_fails_with_wrong_passphrase() {
        let envelope = encrypt(b"secret", "right").unwrap();
        let err = decrypt(&envelope, "wrong").err().unwrap();
        let msg = format!("{}", err);
        assert!(
            msg.to_lowercase().contains("passphrase") || msg.to_lowercase().contains("corrupt")
        );
    }

    #[test]
    fn encrypt_two_calls_produce_different_ciphertexts() {
        // Different salt + nonce per call — ciphertexts should never
        // match even with the same input. Catches a regression where
        // someone hardcodes the salt/nonce while debugging.
        let a = encrypt(b"x", "pw").unwrap();
        let b = encrypt(b"x", "pw").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn decrypt_rejects_short_envelope() {
        let res = decrypt(b"too short", "pw");
        assert!(res.is_err());
    }

    #[test]
    fn decrypt_rejects_missing_magic() {
        let mut not_encrypted = vec![0u8; 64];
        not_encrypted[0] = b'P'; // looks like a zip
        let res = decrypt(&not_encrypted, "pw");
        assert!(res.is_err());
    }
}
