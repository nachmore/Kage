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
//!   - `acp.agent.user_steering_path` — where this machine keeps the
//!     user's steering doc. The backup's `steering/user.md` is written to
//!     this machine's choice, never to a path the backup names.
//!
//! Other paths (`pocket_tts.custom_sound_path`, agent connection paths
//! under `acp.connections[].mode.spawn_command`, …) are kept verbatim.
//! They may not exist on the new machine — we let the user fix them
//! rather than guess at silent rewrites.
//!
//! ## Where import writes
//!
//! Only to this machine's steering docs and under its `extension-data/`
//! root. Archive paths are validated ([`sanitise_archive_path`]), no
//! symlink (or Windows reparse point) at or under the root is followed,
//! every destination is checked before anything is written, and each file
//! is written to a temp file and renamed into place. A refusal is an
//! [`UnsafeImportPath`] error.

use crate::atomic_write::Perms;
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

/// Import refused because an archive entry would be written somewhere other
/// than under Kage's `extension-data/` root: a bad entry name, or a symlink
/// (or Windows reparse point) on this machine along the way. Returned before
/// anything is written. The import command maps it to
/// `errors.config.import_unsafe_path`.
#[derive(Debug)]
pub struct UnsafeImportPath {
    /// The offending archive entry name, or the local path of the link.
    pub path: String,
    /// Why (English, for logs).
    pub reason: String,
}

impl std::fmt::Display for UnsafeImportPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Refusing to import {}: {}", self.path, self.reason)
    }
}

impl std::error::Error for UnsafeImportPath {}

fn unsafe_path(path: impl Into<String>, reason: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(UnsafeImportPath {
        path: path.into(),
        reason: reason.to_string(),
    })
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
                .map_err(|e| unsafe_path(name.escape_debug().to_string(), e))?;
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

    // Check every destination before writing anything: a refused path
    // leaves the disk untouched. Steering goes where THIS machine keeps it
    // (`local`, never the backup's `user_steering_path`).
    let ext_root = extension_data_root();
    check_no_file_folder_clash(extension_files.iter().map(|(rel, _)| rel.as_path()))?;
    if let Some(root) = &ext_root {
        for (rel, _) in &extension_files {
            check_destination_under(root, rel)?;
        }
    }
    // The auto doc is Kage's and holds what it learned about the user:
    // owner-only, as everywhere else it is written. The user's own doc keeps
    // the permissions they gave it.
    let mut steering_writes: Vec<(PathBuf, Vec<u8>, Perms)> = Vec::new();
    for (body, path, perms) in [
        (
            steering_auto,
            Config::get_auto_steering_path().ok(),
            Perms::Private,
        ),
        (
            steering_user,
            resolve_user_steering_path(local),
            Perms::Inherit,
        ),
    ] {
        if let (Some(body), Some(path)) = (body, path) {
            steering_writes.push((steering_target(&path)?, body, perms));
        }
    }

    // Side-files: written AFTER the config round-trip validated. If the
    // parse failed above we'd have bailed without touching disk.
    let mut steering_bytes = 0usize;
    for (target, body, perms) in &steering_writes {
        write_steering(target, body, *perms)?;
        steering_bytes += body.len();
    }

    let mut extensions_written = 0usize;
    if let Some(root) = &ext_root {
        for (rel, body) in &extension_files {
            write_under(root, rel, body)?;
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
    // Whether Kage runs this machine's shell startup files is a decision
    // about this machine's rc files; a backup (from Windows, where the
    // toggle is hidden, or from before it existed) would silently turn it
    // back on and run them at once.
    imported.acp.agent.import_login_shell_env = local.acp.agent.import_login_shell_env;
    // Where this machine keeps the user's steering doc. A backup naming a
    // path (any path: a hand-made archive could point at a shell rc file)
    // must not decide where import writes, or what Kage later reads and
    // sends to the agent as steering.
    imported.acp.agent.user_steering_path = local.acp.agent.user_steering_path.clone();

    // A backup from an older build can carry an extension's own
    // `enabled: false`, which this build would otherwise ignore. Same fold
    // as `Config::load`.
    imported.fold_legacy_extension_enabled();

    imported
}

// --- Helpers ----------------------------------------------------------------

/// Serialises the unit tests that write the (test-isolated) extension-data
/// root: one tree for the whole test process, which a backup export reads
/// in full and its import writes back.
#[cfg(test)]
pub(crate) static EXTENSION_DATA_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn extension_data_root() -> Option<PathBuf> {
    Some(
        crate::paths::config_dir()?
            .join("kage")
            .join("extension-data"),
    )
}

fn resolve_user_steering_path(config: &Config) -> Option<PathBuf> {
    match config.acp.agent.user_steering_path.as_deref() {
        Some(p) if !p.trim().is_empty() => Some(PathBuf::from(p)),
        _ => crate::steering_io::default_user_steering_path().ok(),
    }
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
                // Leftover of an interrupted atomic write (or of an import
                // by an older build): not user data.
                if entry.file_name().to_str().is_some_and(|n| {
                    crate::atomic_write::is_temp_file_name(n) || is_legacy_import_temp(n)
                }) {
                    continue;
                }
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push((rel, path));
            }
            // Symlinks intentionally skipped — exporting a backup
            // shouldn't follow links out of the user's data dir.
        }
    }
    Ok(out)
}

/// A temp an older build's backup import wrote next to its target,
/// `.<name>.<pid>-<seq>.kage-import.tmp`, which a crash could leave behind.
fn is_legacy_import_temp(file_name: &str) -> bool {
    let Some(rest) = file_name
        .strip_prefix('.')
        .and_then(|r| r.strip_suffix(".kage-import.tmp"))
    else {
        return false;
    };
    let Some((name, ids)) = rest.rsplit_once('.') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    !name.is_empty()
        && ids
            .split_once('-')
            .is_some_and(|(pid, seq)| digits(pid) && digits(seq))
}

/// Validate that an archive entry's relative path is safe to write
/// under our extension-data root, the same on every platform: `/`-separated
/// plain names only. Rejects absolute paths, `..`, backslashes, drive
/// letters and other `:` (alternate data streams), NUL and control
/// characters, names Windows would silently alias (trailing `.` or space)
/// or treat as a device (`CON`, `NUL`, `COM1`, …), and paths that name
/// nothing.
pub fn sanitise_archive_path(rel: &str) -> Result<PathBuf> {
    if rel.is_empty() {
        anyhow::bail!("empty path");
    }
    if rel.starts_with('/') {
        anyhow::bail!("absolute path in archive: {}", rel.escape_debug());
    }
    if rel.chars().any(|c| c == '\\' || c == ':' || c.is_control()) {
        anyhow::bail!(
            "disallowed character in archive path: {}",
            rel.escape_debug()
        );
    }
    let mut clean = PathBuf::new();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.ends_with('.') || seg.ends_with(' ') || is_windows_device_name(seg) {
            anyhow::bail!(
                "disallowed path component in archive: {}",
                rel.escape_debug()
            );
        }
        clean.push(seg);
    }
    if clean.as_os_str().is_empty() {
        anyhow::bail!("archive path names nothing: {}", rel.escape_debug());
    }
    Ok(clean)
}

/// Names Windows opens as a device rather than a file, whatever follows the
/// first `.` (`con.json` is `CON`) and ignoring spaces before it.
fn is_windows_device_name(seg: &str) -> bool {
    let stem = seg
        .split('.')
        .next()
        .unwrap_or(seg)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let Some(n) = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
    else {
        return false;
    };
    let mut chars = n.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('0'..='9' | '¹' | '²' | '³'), None)
    )
}

/// A symlink, or on Windows any reparse point (junction, mount point, …):
/// an entry whose contents live somewhere other than where it sits.
fn is_link(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    md.file_type().is_symlink()
}

/// Refuse `path` (already `lstat`ed into `md`) if it is a link, or isn't a
/// directory where one is needed (or is one where a file goes).
fn refuse_link_or_kind(path: &Path, md: &std::fs::Metadata, want_dir: bool) -> Result<()> {
    let reason = if is_link(md) {
        "it is a symlink or other link"
    } else if want_dir && !md.is_dir() {
        "it is not a folder"
    } else if !want_dir && md.is_dir() {
        "it is a folder"
    } else {
        return Ok(());
    };
    Err(unsafe_path(path.display().to_string(), reason))
}

/// Refuse a destination under `root` that would follow a symlink (or
/// reparse point) at the root or any component below it, or replace a
/// directory. Components that don't exist yet are fine.
fn check_destination_under(root: &Path, rel: &Path) -> Result<()> {
    let parts: Vec<_> = rel.components().collect();
    let mut cur = root.to_path_buf();
    for i in 0..=parts.len() {
        if i > 0 {
            cur.push(parts[i - 1]);
        }
        let md = match std::fs::symlink_metadata(&cur) {
            Ok(md) => md,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("Failed to inspect {:?}", cur)),
        };
        refuse_link_or_kind(&cur, &md, i < parts.len())?;
    }
    Ok(())
}

/// Refuse a backup whose entries need one name to be both a file and a
/// folder (`ext/a` and `ext/a/b.json`), compared case-insensitively as on
/// Windows and macOS. Caught here, the import fails before anything is
/// written rather than midway.
fn check_no_file_folder_clash<'a>(rels: impl Iterator<Item = &'a Path>) -> Result<()> {
    let key = |p: &Path| {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut files = std::collections::HashSet::new();
    let mut folders = std::collections::HashSet::new();
    for rel in rels {
        files.insert(key(rel));
        let mut parent = rel.parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            folders.insert(key(p));
            parent = p.parent();
        }
    }
    if let Some(clash) = files.intersection(&folders).next() {
        anyhow::bail!(
            "Backup is inconsistent: extension-data/{} is both a file and a folder",
            clash
        );
    }
    Ok(())
}

/// Make sure `dir` is a real directory (not a link), creating it if absent.
fn ensure_real_dir(dir: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(md) => return refuse_link_or_kind(dir, &md, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("Failed to inspect {:?}", dir)),
    }
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("Failed to create {:?}", dir)),
    }
    let md =
        std::fs::symlink_metadata(dir).with_context(|| format!("Failed to inspect {:?}", dir))?;
    refuse_link_or_kind(dir, &md, true)
}

/// Write `body` to `rel` under `root` without following a link at or
/// below the root: each directory is created (or checked) one level at a
/// time with `lstat`, and the file goes through
/// [`crate::atomic_write::write_no_follow`] (its rename replaces a link
/// instead of writing through it). Owner-only, like every other write of
/// extension data: it can hold an extension's API keys.
fn write_under(root: &Path, rel: &Path, body: &[u8]) -> Result<()> {
    let parts: Vec<_> = rel.components().collect();
    let Some((last, dirs)) = parts.split_last() else {
        anyhow::bail!("Empty destination under {:?}", root);
    };
    // Above the root is this machine's own config folder.
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {:?}", parent))?;
    }
    let mut cur = root.to_path_buf();
    ensure_real_dir(&cur)?;
    for part in dirs {
        cur.push(part);
        ensure_real_dir(&cur)?;
    }
    let target = cur.join(last);
    if let Ok(md) = std::fs::symlink_metadata(&target) {
        refuse_link_or_kind(&target, &md, false)?;
    }
    crate::atomic_write::write_no_follow(&target, body, Perms::Private)
        .with_context(|| format!("Failed to write {:?}", target))
}

/// Where to write a steering doc this machine keeps at `path`. A symlink
/// there is the user's own setup (a dotfiles link), so its target is used.
fn steering_target(path: &Path) -> Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => {
            std::fs::canonicalize(path).with_context(|| format!("Failed to resolve {:?}", path))
        }
        _ => Ok(path.to_path_buf()),
    }
}

/// Write a steering doc at the [`steering_target`] this machine chose.
fn write_steering(target: &Path, body: &[u8], perms: Perms) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent of {:?}", target))?;
    }
    crate::atomic_write::write_no_follow(target, body, perms)
        .with_context(|| format!("Failed to write {:?}", target))
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
    fn sanitise_keeps_local_login_shell_env_preference() {
        // Device-local: a backup must not turn it back on (or off).
        let mut local = fake_config();
        local.acp.agent.import_login_shell_env = false;
        let mut imported = fake_config();
        imported.acp.agent.import_login_shell_env = true;
        let out = sanitise_imported_config(imported, &local);
        assert!(!out.acp.agent.import_login_shell_env);

        local.acp.agent.import_login_shell_env = true;
        imported = fake_config();
        imported.acp.agent.import_login_shell_env = false;
        assert!(
            sanitise_imported_config(imported, &local)
                .acp
                .agent
                .import_login_shell_env
        );
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
    fn sanitise_folds_a_legacy_extension_enabled_false() {
        let mut imported = fake_config();
        imported.extensions.insert(
            "link-preview".to_string(),
            serde_json::json!({ "enabled": false, "max_previews": 3 }),
        );
        imported
            .extension_states
            .insert("link-preview".to_string(), true);
        let merged = sanitise_imported_config(imported, &fake_config());
        assert_eq!(merged.extension_states.get("link-preview"), Some(&false));
        assert_eq!(
            merged.extensions.get("link-preview"),
            Some(&serde_json::json!({ "max_previews": 3 }))
        );
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
    fn sanitise_imported_config_keeps_local_steering_path() {
        let mut local = fake_config();
        local.acp.agent.user_steering_path = Some("/home/me/steering.md".into());
        let mut imported = fake_config();
        imported.acp.agent.user_steering_path = Some("/home/me/.zshrc".into());
        let merged = sanitise_imported_config(imported, &local);
        assert_eq!(
            merged.acp.agent.user_steering_path.as_deref(),
            Some("/home/me/steering.md")
        );

        local.acp.agent.user_steering_path = None;
        let mut imported = fake_config();
        imported.acp.agent.user_steering_path = Some("/home/me/.zshrc".into());
        assert_eq!(
            sanitise_imported_config(imported, &local)
                .acp
                .agent
                .user_steering_path,
            None
        );
    }

    #[test]
    fn archive_paths_are_validated_the_same_on_every_platform() {
        for bad in [
            "../escape",
            "/tmp/escape",
            "C:/escape",
            "c:escape",
            "C:\\escape",
            "..\\escape",
            "ext\\file.json",
            "\\\\server\\share\\x",
            "a\0b",
            "ext/a\nb.json",
            ".",
            "./",
            "//",
            "x/./../y",
            "x/..",
            "ext/file.json:stream",
            "ext/trailing.",
            "ext/trailing ",
            "ext/CON",
            "CON/a.json",
            "ext/con.json",
            "ext/nul.json",
            "ext/NUL",
            "ext/Prn.txt",
            "ext/aux",
            "ext/com1.txt",
            "ext/COM9",
            "ext/lpt1",
            "ext/LPT0.json",
            "ext/COM\u{b9}",
            "ext/CON .txt",
            "ext/CONIN$",
            "ext/conout$.json",
        ] {
            assert!(sanitise_archive_path(bad).is_err(), "{:?}", bad);
        }
        assert_eq!(
            sanitise_archive_path("./ext//a.json").unwrap(),
            PathBuf::from("ext").join("a.json")
        );
        for good in [
            "ext/console.json",
            "ext/com10.json",
            "ext/lpt.json",
            "ext/nul-state.json",
            "my-ext_2/nested/key.v2.json",
        ] {
            assert!(sanitise_archive_path(good).is_ok(), "{:?}", good);
        }
    }

    // --- Import writes only where this machine chooses ---------------------
    //
    // These run against the isolated per-process config dir (`crate::paths`
    // isolates unit tests). They share its `extension-data/` root, so each
    // uses its own extension ids and takes `EXTENSION_DATA_TEST_LOCK`.

    fn disk() -> std::sync::MutexGuard<'static, ()> {
        super::EXTENSION_DATA_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn isolated_ext_root() -> PathBuf {
        let root = extension_data_root().unwrap();
        assert!(crate::paths::is_under_isolation_root(&root), "{:?}", root);
        root
    }

    /// A hand-made v1 bundle: manifest + `config` + `entries`.
    fn handmade_bundle(config: &Config, entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("manifest.json", opts).unwrap();
            zip.write_all(&serde_json::to_vec(&BundleManifest::now()).unwrap())
                .unwrap();
            zip.start_file("config.json", opts).unwrap();
            zip.write_all(&serde_json::to_vec(config).unwrap()).unwrap();
            for (name, body) in entries {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(body).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    fn expect_refused(bytes: &[u8], local: &Config) -> UnsafeImportPath {
        let err = import(bytes, None, local).expect_err("import must be refused");
        let refused = err
            .downcast_ref::<UnsafeImportPath>()
            .unwrap_or_else(|| panic!("not an UnsafeImportPath: {:#}", err));
        UnsafeImportPath {
            path: refused.path.clone(),
            reason: refused.reason.clone(),
        }
    }

    fn local_with_steering(path: &Path) -> Config {
        let mut local = Config::default();
        local.acp.agent.user_steering_path = Some(path.to_string_lossy().into_owned());
        local
    }

    fn no_temp_files_in(dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !crate::atomic_write::is_temp_file_name(&name) && !name.ends_with(".tmp"),
                "left behind {:?}",
                name
            );
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn import_writes_extension_data_and_auto_steering_owner_only() {
        // The regression: import wrote them with the umask's default (0644
        // under umask 022), replacing 0600 files.
        use std::os::unix::fs::PermissionsExt;
        let _disk = disk();
        let _auto_doc = crate::steering_io::AUTO_DOC_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = isolated_ext_root();
        let ext_file = root.join("perm-ext").join("secrets.json");
        std::fs::create_dir_all(ext_file.parent().unwrap()).unwrap();
        std::fs::write(&ext_file, b"{}").unwrap();
        std::fs::set_permissions(&ext_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let user_doc = scratch.path().join("user-steering.md");
        std::fs::write(&user_doc, b"old").unwrap();
        std::fs::set_permissions(&user_doc, std::fs::Permissions::from_mode(0o640)).unwrap();
        let auto_doc = Config::get_auto_steering_path().unwrap();
        assert!(crate::paths::is_under_isolation_root(&auto_doc));

        let bundle = handmade_bundle(
            &fake_config(),
            &[
                ("extension-data/perm-ext/secrets.json", b"{\"key\":\"k\"}"),
                ("extension-data/perm-ext/new.json", b"[]"),
                ("steering/auto.md", b"- likes tea"),
                ("steering/user.md", b"be brief"),
            ],
        );
        import(&bundle, None, &local_with_steering(&user_doc)).unwrap();

        assert_eq!(mode(&ext_file), 0o600);
        assert_eq!(mode(&root.join("perm-ext").join("new.json")), 0o600);
        assert_eq!(mode(&auto_doc), 0o600);
        assert_eq!(std::fs::read(&auto_doc).unwrap(), b"- likes tea");
        assert_eq!(mode(&user_doc), 0o640, "the user's own doc keeps its mode");
        assert_eq!(std::fs::read(&user_doc).unwrap(), b"be brief");
        no_temp_files_in(&root.join("perm-ext"));
        no_temp_files_in(scratch.path());
        std::fs::remove_dir_all(root.join("perm-ext")).unwrap();
        std::fs::remove_file(&auto_doc).unwrap();
    }

    #[test]
    fn export_skips_leftover_temps() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        std::fs::create_dir_all(root.join("ext")).unwrap();
        std::fs::write(root.join("ext").join("data.json"), b"{}").unwrap();
        for leftover in [
            ".data.json.kage-tmp-0123456789abcdef",
            ".data.json.4242-7.kage-import.tmp",
        ] {
            std::fs::write(root.join("ext").join(leftover), b"{").unwrap();
        }
        // Look-alikes a user (or an extension) could have named.
        for kept in [
            "notes.kage-import.tmp",
            ".data.json.x-7.kage-import.tmp",
            ".data.json.kage-import.tmp",
        ] {
            std::fs::write(root.join("ext").join(kept), b"mine").unwrap();
        }
        let mut names: Vec<String> = walk_files(root)
            .unwrap()
            .into_iter()
            .map(|(rel, _)| rel.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ".data.json.kage-import.tmp",
                ".data.json.x-7.kage-import.tmp",
                "data.json",
                "notes.kage-import.tmp",
            ]
        );
    }

    #[test]
    fn import_writes_steering_only_where_this_machine_keeps_it() {
        let _disk = disk();
        let scratch = tempfile::tempdir().unwrap();
        let chosen_by_archive = scratch.path().join("outside").join("chosen-by-archive.txt");
        let local_path = scratch.path().join("mine").join("user-steering.md");
        let local = local_with_steering(&local_path);
        let mut imported = fake_config();
        imported.acp.agent.user_steering_path =
            Some(chosen_by_archive.to_string_lossy().into_owned());
        let bundle = handmade_bundle(
            &imported,
            &[("steering/user.md", b"archive-controlled content")],
        );

        let (merged, summary) = import(&bundle, None, &local).unwrap();
        assert!(!chosen_by_archive.exists());
        assert!(!chosen_by_archive.parent().unwrap().exists());
        assert_eq!(
            std::fs::read_to_string(&local_path).unwrap(),
            "archive-controlled content"
        );
        assert_eq!(
            merged.acp.agent.user_steering_path,
            local.acp.agent.user_steering_path
        );
        assert_eq!(summary.steering_bytes, 26);
        no_temp_files_in(local_path.parent().unwrap());

        // No local choice: the default path under Kage's config dir, still
        // never the archive's.
        let default_path = crate::steering_io::default_user_steering_path().unwrap();
        assert!(crate::paths::is_under_isolation_root(&default_path));
        let (merged, _) = import(&bundle, None, &Config::default()).unwrap();
        assert!(!chosen_by_archive.exists());
        assert_eq!(merged.acp.agent.user_steering_path, None);
        assert_eq!(
            std::fs::read_to_string(&default_path).unwrap(),
            "archive-controlled content"
        );
        std::fs::remove_file(&default_path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn import_refuses_a_symlinked_extension_dir_and_writes_nothing() {
        let _disk = disk();
        let root = isolated_ext_root();
        std::fs::create_dir_all(&root).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let outside = scratch.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let steering = scratch.path().join("user-steering.md");
        let local = local_with_steering(&steering);
        std::os::unix::fs::symlink(&outside, root.join("symlinked-ext")).unwrap();

        let bundle = handmade_bundle(
            &fake_config(),
            &[
                ("steering/user.md", b"steering"),
                ("extension-data/symlinked-ok/a.json", b"[1]"),
                ("extension-data/symlinked-ext/escaped.json", b"overwritten"),
            ],
        );
        let refused = expect_refused(&bundle, &local);
        assert!(refused.reason.contains("symlink"), "{}", refused);
        assert!(refused.path.contains("symlinked-ext"), "{}", refused);

        assert!(!outside.join("escaped.json").exists());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(!root.join("symlinked-ok").exists(), "nothing is written");
        assert!(!steering.exists(), "nothing is written");
        std::fs::remove_file(root.join("symlinked-ext")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn import_refuses_a_symlinked_nested_dir_or_file_and_writes_nothing() {
        let _disk = disk();
        let root = isolated_ext_root();
        let scratch = tempfile::tempdir().unwrap();
        let outside = scratch.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let ext = root.join("nested-link-ext");
        std::fs::create_dir_all(ext.join("real")).unwrap();
        std::os::unix::fs::symlink(&outside, ext.join("real").join("sub")).unwrap();
        std::os::unix::fs::symlink(outside.join("target.json"), ext.join("link.json")).unwrap();
        // A dangling link names a file that doesn't exist yet.
        std::os::unix::fs::symlink(outside.join("dangling.json"), ext.join("dangling.json"))
            .unwrap();

        for entry in [
            "extension-data/nested-link-ext/real/sub/deep/x.json",
            "extension-data/nested-link-ext/real/sub/x.json",
            "extension-data/nested-link-ext/link.json",
            "extension-data/nested-link-ext/dangling.json",
        ] {
            let bundle = handmade_bundle(
                &fake_config(),
                &[
                    ("extension-data/nested-link-ok/a.json", b"[1]"),
                    (entry, b"overwritten"),
                ],
            );
            let refused = expect_refused(&bundle, &Config::default());
            assert!(refused.reason.contains("symlink"), "{}: {}", entry, refused);
            assert!(!root.join("nested-link-ok").exists(), "{}", entry);
        }
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        std::fs::remove_dir_all(&ext).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_extension_data_root_is_refused() {
        // The shared isolated root can't be swapped for a link while other
        // tests use it, so drive the checks against a private root.
        let scratch = tempfile::tempdir().unwrap();
        let outside = scratch.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let root = scratch.path().join("extension-data");
        std::os::unix::fs::symlink(&outside, &root).unwrap();
        let rel = Path::new("ext").join("a.json");
        let err = check_destination_under(&root, &rel).unwrap_err();
        assert!(
            err.downcast_ref::<UnsafeImportPath>().is_some(),
            "{:#}",
            err
        );
        let err = write_under(&root, &rel, b"x").unwrap_err();
        assert!(
            err.downcast_ref::<UnsafeImportPath>().is_some(),
            "{:#}",
            err
        );
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn write_under_creates_real_dirs_and_replaces_files() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("extension-data");
        let rel = Path::new("ext").join("deep").join("a.json");
        check_destination_under(&root, &rel).unwrap();
        write_under(&root, &rel, b"one").unwrap();
        write_under(&root, &rel, b"two").unwrap();
        assert_eq!(std::fs::read(root.join(&rel)).unwrap(), b"two");
        no_temp_files_in(&root.join("ext").join("deep"));
        // A folder where the file goes is refused, not replaced.
        let dir_rel = Path::new("ext").join("deep");
        assert!(check_destination_under(&root, &dir_rel).is_err());
        assert!(write_under(&root, &dir_rel, b"x").is_err());
        assert!(root.join(&rel).is_file());
    }

    #[test]
    fn import_refuses_a_name_that_is_both_file_and_folder_and_writes_nothing() {
        let _disk = disk();
        let root = isolated_ext_root();
        for (file, nested) in [
            (
                "extension-data/clash-ext/a",
                "extension-data/clash-ext/a/b.json",
            ),
            (
                "extension-data/clash-ext/A",
                "extension-data/clash-ext/a/b.json",
            ),
        ] {
            for order in [[file, nested], [nested, file]] {
                let bundle = handmade_bundle(
                    &fake_config(),
                    &[(order[0], b"1".as_slice()), (order[1], b"2".as_slice())],
                );
                let err = import(&bundle, None, &Config::default()).unwrap_err();
                assert!(format!("{:#}", err).contains("both a file and a folder"));
                assert!(!root.join("clash-ext").exists(), "{:?}", order);
            }
        }
        let fine = handmade_bundle(
            &fake_config(),
            &[
                ("extension-data/clash-ext/a.json", b"1"),
                ("extension-data/clash-ext/a/b.json", b"2"),
            ],
        );
        assert_eq!(
            import(&fine, None, &Config::default())
                .unwrap()
                .1
                .extensions,
            2
        );
        std::fs::remove_dir_all(root.join("clash-ext")).unwrap();
    }

    #[test]
    fn import_refuses_bad_entry_names_and_writes_nothing() {
        let _disk = disk();
        let root = isolated_ext_root();
        let scratch = tempfile::tempdir().unwrap();
        let steering = scratch.path().join("user-steering.md");
        let local = local_with_steering(&steering);
        for bad in [
            "extension-data/bad-names/../../escape.json",
            "extension-data/bad-names/CON",
            "extension-data/bad-names/nul.json",
            "extension-data/C:/escape.json",
            "extension-data/bad-names\\escape.json",
            "extension-data/bad-names/a.json:stream",
            "extension-data//abs/escape.json",
        ] {
            let bundle = handmade_bundle(
                &fake_config(),
                &[
                    ("steering/user.md", b"steering"),
                    ("extension-data/bad-names-ok/a.json", b"[1]"),
                    (bad, b"x"),
                ],
            );
            let refused = expect_refused(&bundle, &local);
            assert!(refused.path.starts_with("extension-data/"), "{}", refused);
            assert!(!root.join("bad-names-ok").exists(), "{}", bad);
            assert!(!root.join("bad-names").exists(), "{}", bad);
            assert!(!steering.exists(), "{}", bad);
        }
    }

    #[test]
    fn a_v1_bundle_round_trips_plain_and_encrypted() {
        let _disk = disk();
        // Export picks up the auto doc if there is one; import writes it back.
        let _auto_doc = crate::steering_io::AUTO_DOC_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = isolated_ext_root();
        let ext = root.join("round-trip-ext");
        std::fs::create_dir_all(ext.join("nested")).unwrap();
        std::fs::write(ext.join("notes.json"), b"[\"a\",\"b\"]").unwrap();
        std::fs::write(ext.join("nested").join("deep.json"), b"{\"k\":1}").unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let steering = scratch.path().join("user-steering.md");
        std::fs::write(&steering, b"be brief").unwrap();
        let mut config = local_with_steering(&steering);
        config.ui.language = Some("fr".into());

        let plain = export(&config, None).unwrap();
        let encrypted = export(&config, Some("pw")).unwrap();
        assert!(encrypted.starts_with(ENCRYPTED_MAGIC));

        for (bytes, pw) in [(&plain, None), (&encrypted, Some("pw"))] {
            std::fs::remove_dir_all(&ext).unwrap();
            std::fs::remove_file(&steering).unwrap();
            let (merged, summary) = import(bytes, pw, &config).unwrap();
            assert_eq!(merged.ui.language.as_deref(), Some("fr"));
            assert_eq!(
                merged.acp.agent.user_steering_path,
                config.acp.agent.user_steering_path
            );
            assert_eq!(std::fs::read(&steering).unwrap(), b"be brief");
            assert_eq!(
                std::fs::read(ext.join("notes.json")).unwrap(),
                b"[\"a\",\"b\"]"
            );
            assert_eq!(
                std::fs::read(ext.join("nested").join("deep.json")).unwrap(),
                b"{\"k\":1}"
            );
            assert!(summary.extensions >= 2);
            assert!(summary.steering_bytes >= 8);
            assert_eq!(summary.kage_version, env!("CARGO_PKG_VERSION"));
            no_temp_files_in(&ext);
        }
        // Importing over existing files replaces them in place.
        std::fs::write(ext.join("notes.json"), b"stale").unwrap();
        import(&plain, None, &config).unwrap();
        assert_eq!(
            std::fs::read(ext.join("notes.json")).unwrap(),
            b"[\"a\",\"b\"]"
        );
        assert!(
            import(&encrypted, None, &config).is_err(),
            "needs passphrase"
        );
        std::fs::remove_dir_all(&ext).unwrap();
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
