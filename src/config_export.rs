//! Cross-device config export / import.
//!
//! The launcher pitch demands moving to a second machine without
//! retyping everything. A backup comes in exactly two kinds:
//!
//!   - **Encrypted full backup** ([`ExportMode::Encrypted`], the default):
//!     everything — config, steering docs, extension settings and extension
//!     data — AES-256-GCM-encrypted with a key derived (Argon2id) from a
//!     passphrase that can't be blank.
//!   - **Unencrypted settings-only backup** ([`ExportMode::SettingsOnly`]):
//!     only the settings [`allowlist`] names (theme, language, hotkeys,
//!     window and update preferences, which extensions are on and what they
//!     were granted, …) and the extension settings their manifests mark safe
//!     to copy. No steering docs, prompts, shortcuts, automations, agent
//!     setups, internal sites, tool permissions, store addresses, extension
//!     data or free-form extension settings. It's an allowlist, not a scan:
//!     nothing is in the file unless a rule says it may be.
//!
//! There is no unencrypted full backup.
//!
//! ## File layout (zip archive)
//!
//! ```text
//! manifest.json            { format, version, exported_at, kage_version, contents? }
//! config.json              full: the config; settings-only: allowlisted fields only
//! steering/auto.md         full only, if present
//! steering/user.md         full only, if present
//! extension-data/<ext>/<key>.json   full only: recursive copy of the dir
//! ```
//!
//! Manifest versions: v1 is a full backup (older builds read it); v2 is
//! what earlier builds wrote when a backup left items out (`contents:
//! "leave_out"`, `excluded`) or still carried their placeholders — this
//! build writes it for a full backup holding such placeholders, so a build
//! that can't keep local values for them refuses it; v3 is a settings-only
//! backup, which older builds refuse rather than import as a full one.
//!
//! Encrypted output: `KAGE-ENC-1\0` + salt + nonce + ciphertext. Import
//! recognises it by its first bytes, whatever the file is called.
//!
//! ## Import
//!
//! Every import keeps this machine's value wherever the backup's is
//! missing, empty, or a placeholder an earlier build wrote, bound to the
//! item's identity so a local secret never moves to another command, site
//! or server ([`merge`]). A setting or file an extension declares sensitive
//! is never replaced by an empty or missing one, whatever the bundle. A
//! settings-only backup merges only allowlisted fields and leaves everything
//! else here untouched.
//!
//! Config schemas are checked first: `config.json` goes through
//! [`crate::config_migrations`] like `Config::load`; one from a newer Kage is
//! refused ([`UnsupportedConfigSchema`]) before anything changes.
//!
//! Device-local fields are never taken from a backup
//! ([`sanitise_imported_config`]): the install id, window geometry, the OS
//! autostart flag, the login-shell toggle, where the steering doc lives,
//! and update-check caches.
//!
//! ## Where and how import writes
//!
//! Only to this machine's steering docs and under its `extension-data/`
//! root. Archive paths are validated ([`sanitise_archive_path`]) and
//! matched against this machine's file names the way APFS and NTFS compare
//! them ([`sensitive::fold`]); no symlink (or Windows reparse point) at or
//! under the root is followed; a refusal is an [`UnsafeImportPath`] error.
//! The file, the encrypted envelope and every entry are capped in size
//! before they're read, copied or decrypted ([`BackupTooLarge`]).
//!
//! Import is all or nothing: every write is planned and checked, staged to
//! temp files next to its target, recorded in a durable journal
//! ([`journal`]), then committed by renames that move each old file aside.
//! The caller saves `config.json` and calls [`CommittedFiles::finish`],
//! which records the commit before deleting the old files. A failure puts
//! everything back; an import interrupted by a crash is finished or undone
//! the next time Kage starts ([`recover_interrupted_import`]). Extension
//! data and the auto steering doc are written owner-only (they can hold an
//! extension's API keys, and what Kage learned about the user); the user's
//! own steering doc keeps its permissions, and is owner-only when new.

mod allowlist;
mod consent;
mod journal;
mod merge;
pub mod sensitive;

use crate::atomic_write::Perms;
use crate::config::Config;
use crate::extensions::InstalledItem;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// Magic prefix marking the encrypted envelope. Plain archives start with
/// the zip magic (`PK\x03\x04`).
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
const TAG_LEN: usize = 16;

/// Most entries a backup may hold.
pub const MAX_ENTRIES: usize = 10_000;
/// Largest single entry, decompressed.
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// Most bytes all entries together may decompress to.
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// Largest archive: every entry stored uncompressed, plus headers.
pub const MAX_ARCHIVE_BYTES: u64 = MAX_TOTAL_BYTES + 16 * 1024 * 1024;
/// Largest backup file: an archive, or one in its encrypted envelope.
pub const MAX_FILE_BYTES: u64 =
    MAX_ARCHIVE_BYTES + (ENCRYPTED_MAGIC.len() + SALT_LEN + NONCE_LEN + TAG_LEN) as u64;

/// The two kinds of backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportMode {
    /// Everything, encrypted with a passphrase.
    #[default]
    Encrypted,
    /// Allowlisted settings only, unencrypted.
    SettingsOnly,
}

/// What a bundle carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleContents {
    /// Everything the source had.
    Full,
    /// An earlier build's backup that left sensitive items out (`excluded`).
    LeaveOut,
    /// Allowlisted settings only (v3).
    SettingsOnly,
    /// A kind this build doesn't know.
    #[serde(other)]
    Unknown,
}

/// Manifest entry written into the archive. Versioned so future breaking
/// changes can refuse to import without giving the user a confusing error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub kage_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contents: Option<BundleContents>,
    /// What an earlier build's "leave out" backup didn't include. Read
    /// only; entries this build can't parse are skipped.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient_refs"
    )]
    pub excluded: Vec<sensitive::SensitiveRef>,
    /// Display names of the extensions in `excluded`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extension_names: BTreeMap<String, String>,
}

/// Read `excluded`, dropping (with a warning) entries this build can't parse.
fn lenient_refs<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<sensitive::SensitiveRef>, D::Error> {
    let raw: Option<Vec<serde_json::Value>> = Option::deserialize(d)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| match serde_json::from_value(v) {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!(
                    "Backup manifest: ignoring an unrecognised excluded item ({})",
                    e
                );
                None
            }
        })
        .collect())
}

/// What an import does with a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BundleKind {
    /// Merge everything it carries.
    #[default]
    Full,
    /// Merge allowlisted settings only; touch nothing else.
    SettingsOnly,
}

impl BundleManifest {
    /// Highest format this build reads.
    pub const CURRENT_VERSION: u32 = 3;
    pub const FORMAT: &'static str = "kage-backup";

    fn new(version: u32, contents: BundleContents) -> Self {
        Self {
            format: Self::FORMAT.to_string(),
            version,
            exported_at: chrono::Utc::now().to_rfc3339(),
            kage_version: env!("CARGO_PKG_VERSION").to_string(),
            contents: Some(contents),
            excluded: Vec::new(),
            extension_names: BTreeMap::new(),
        }
    }

    /// Whether the bundle is an earlier build's "leave out" backup.
    fn is_leave_out(&self) -> bool {
        self.version == 2
            && match self.contents {
                Some(BundleContents::Full) => false,
                Some(_) => true,
                None => true,
            }
    }

    /// How to import it; refuses kinds this build doesn't know.
    fn kind(&self) -> Result<BundleKind> {
        match (self.version, self.contents) {
            (3, Some(BundleContents::SettingsOnly)) => Ok(BundleKind::SettingsOnly),
            (3, Some(BundleContents::Full)) => Ok(BundleKind::Full),
            (3, _) => anyhow::bail!(
                "Backup is from a newer Kage (an unknown kind of backup). Update Kage and retry."
            ),
            (2, Some(BundleContents::SettingsOnly)) => {
                anyhow::bail!("Backup is inconsistent: a settings-only backup must be format v3")
            }
            _ => Ok(BundleKind::Full),
        }
    }
}

/// An encrypted export without a passphrase.
#[derive(Debug)]
pub struct PassphraseNeeded;

impl std::fmt::Display for PassphraseNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("An encrypted backup needs a passphrase")
    }
}

impl std::error::Error for PassphraseNeeded {}

/// A passphrase that is empty or only whitespace: refused for encrypting.
#[derive(Debug)]
pub struct BlankPassphrase;

impl std::fmt::Display for BlankPassphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("A passphrase can't be empty or only spaces")
    }
}

impl std::error::Error for BlankPassphrase {}

/// The file is an encrypted backup (by its first bytes) and no passphrase
/// was given: the UI asks for one and tries again.
#[derive(Debug)]
pub struct PassphraseRequired;

impl std::fmt::Display for PassphraseRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Encrypted backup: passphrase required")
    }
}

impl std::error::Error for PassphraseRequired {}

/// Decryption failed: a wrong passphrase, or a damaged file (AES-GCM can't
/// tell them apart).
#[derive(Debug)]
pub struct WrongPassphrase;

impl std::fmt::Display for WrongPassphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Wrong passphrase or corrupted file")
    }
}

impl std::error::Error for WrongPassphrase {}

/// A backup over the size limits ([`MAX_FILE_BYTES`], [`MAX_ENTRIES`],
/// [`MAX_ENTRY_BYTES`], [`MAX_TOTAL_BYTES`]). Refused before anything is
/// read past the limit or written (or, on export, before a file nothing
/// could import is made).
#[derive(Debug)]
pub struct BackupTooLarge {
    /// Which limit (English, for logs).
    pub what: String,
}

impl std::fmt::Display for BackupTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Backup is too large: {}", self.what)
    }
}

impl std::error::Error for BackupTooLarge {}

fn too_large(what: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(BackupTooLarge { what: what.into() })
}

/// The backup's `config.json` has a schema this build can't read: from a
/// newer Kage (update and retry), or one too old to migrate. Refused before
/// anything changes.
#[derive(Debug)]
pub struct UnsupportedConfigSchema {
    pub stored: u32,
    pub newer: bool,
}

impl std::fmt::Display for UnsupportedConfigSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.newer {
            write!(
                f,
                "Backup settings are from a newer Kage (config schema v{}, this build reads up to v{}). Update Kage and retry.",
                self.stored,
                crate::config_migrations::CURRENT_VERSION
            )
        } else {
            write!(
                f,
                "Backup settings use config schema v{}, older than this build can read",
                self.stored
            )
        }
    }
}

impl std::error::Error for UnsupportedConfigSchema {}

/// The merged settings would be larger than `Config::load` reads, so the
/// next start would reset them: refused before anything changes.
#[derive(Debug)]
pub struct ConfigTooLarge;

impl std::fmt::Display for ConfigTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The imported settings would be over {} KB",
            Config::MAX_CONFIG_SIZE / 1024
        )
    }
}

impl std::error::Error for ConfigTooLarge {}

/// Refuse a merged config `Config::load` wouldn't read back.
pub fn check_config_size(config: &Config) -> Result<()> {
    check_config_size_within(config, Config::MAX_CONFIG_SIZE)
}

fn check_config_size_within(config: &Config, limit: u64) -> Result<()> {
    let len = serde_json::to_string_pretty(config)?.len() as u64;
    if len > limit {
        return Err(ConfigTooLarge.into());
    }
    Ok(())
}

/// The extensions whose data files here hold a credential: a file their
/// manifest declares sensitive (or named like one) with content, or a JSON
/// file with a credential-named value.
fn extensions_with_secret_data(
    root: Option<&Path>,
    rules: &sensitive::Rules,
) -> Result<HashSet<String>> {
    let mut ids = HashSet::new();
    let Some(root) = root.filter(|r| r.exists()) else {
        return Ok(ids);
    };
    for (rel, full) in list_data_files(root)? {
        let Some((id, _)) = rel.split_once('/') else {
            continue;
        };
        if ids.contains(id) {
            continue;
        }
        let Some(body) = read_regular_file(&full) else {
            continue;
        };
        if merge::holds_credential(&body, declared_data(rules, &rel)) {
            ids.insert(id.to_string());
        }
    }
    Ok(ids)
}

/// Whether the data file at `rel` (`<id>/<file>`) is one its extension
/// declares sensitive, or is named like a credential.
fn declared_data(rules: &sensitive::Rules, rel: &str) -> bool {
    let (id, file) = rel.split_once('/').unwrap_or(("", rel));
    let key = file.strip_suffix(".json").unwrap_or(file);
    rules.is_sensitive_data(id, key)
}

/// An earlier import was interrupted and couldn't be undone (or another
/// Kage process is importing): nothing was changed.
#[derive(Debug)]
pub struct ImportUnfinished;

impl std::fmt::Display for ImportUnfinished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("An earlier import hasn't finished; restart Kage and retry")
    }
}

impl std::error::Error for ImportUnfinished {}

/// Summary returned to the frontend after a successful import.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportSummary {
    /// Full or settings-only.
    #[serde(default)]
    pub kind: BundleKind,
    pub shortcuts: usize,
    /// Extension data files written.
    pub extensions: usize,
    pub steering_bytes: usize,
    pub kage_version: String,
    pub exported_at: String,
    /// Settings-only: how many settings were taken from the backup.
    #[serde(default)]
    pub settings: usize,
    /// How many items an earlier build's backup said it left out.
    #[serde(default)]
    pub left_out: usize,
    /// Extensions (display names) whose sign-in this machine couldn't keep:
    /// the user signs in to them again.
    #[serde(default)]
    pub sign_in_again: Vec<String>,
    /// Kage settings (labels) an earlier build left out, with no local
    /// value to keep.
    #[serde(default)]
    pub reenter: Vec<String>,
    /// Installed extensions (display names) switched off until the user
    /// approves them again: the backup had them on or granted, and this
    /// machine's grant doesn't cover what they request (grants never come
    /// from a backup).
    #[serde(default)]
    pub needs_approval: Vec<String>,
    /// Extensions (display names) that kept this machine's settings and
    /// data whole, though the backup had others: their sign-in here stays,
    /// and the backup didn't carry one to replace it.
    #[serde(default)]
    pub kept_local: Vec<String>,
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

/// A date-stamped filename suggestion for the export dialog.
pub fn default_filename(mode: ExportMode) -> String {
    let stamp = chrono::Local::now().format("%Y-%m-%d");
    match mode {
        ExportMode::Encrypted => format!("kage-backup-{}.kage.enc", stamp),
        ExportMode::SettingsOnly => format!("kage-settings-{}.kage", stamp),
    }
}

// --- Preview ---------------------------------------------------------------------

/// One kind of thing a settings-only backup leaves out. The UI words it
/// (`settings.about.backup.settings_only.left_out.<kind>`, with `count`).
#[derive(Debug, Clone, Serialize)]
pub struct LeftOutItem {
    /// `steering`, `shortcuts`, `automations`, `quick_actions`,
    /// `app_modes`, `agents`, `internal_sites`, `tool_permissions`,
    /// `stores`, `extension_data`, `extension_settings`.
    pub kind: &'static str,
    pub count: usize,
}

/// What `preview_config_export` returns: what an unencrypted settings-only
/// backup of this machine leaves out. Never carries a value.
#[derive(Debug, Clone, Serialize)]
pub struct ExportPreview {
    /// Kinds of things on this machine the file won't have.
    pub left_out: Vec<LeftOutItem>,
    /// Allowlisted settings left out too, because their values didn't look
    /// like settings (labels).
    pub flagged: Vec<String>,
    /// How many settings the file would hold.
    pub settings: usize,
}

/// What a settings-only backup of `config` would leave out.
pub fn preview(config: &Config, installed: &[InstalledItem]) -> Result<ExportPreview> {
    preview_from(config, installed, extension_data_root().as_deref())
}

fn preview_from(
    config: &Config,
    installed: &[InstalledItem],
    ext_root: Option<&Path>,
) -> Result<ExportPreview> {
    let rules = sensitive::Rules::new(installed);
    let names = allowlist::ExtensionNames(sensitive::display_names(installed));
    let only = allowlist::extract(config, &rules, &names);
    let steering = read_steering(config)?;
    let data_files = ext_root
        .filter(|r| r.exists())
        .map(|r| list_data_files(r).map(|f| f.len()))
        .transpose()?
        .unwrap_or(0);
    let docs = [steering.auto.as_deref(), steering.user.as_deref()]
        .into_iter()
        .flatten()
        .filter(|b| steering_has_text(b))
        .count();
    let tp = &config.tool_permissions;
    let rules_count = tp
        .tools
        .iter()
        .filter(|t| t.policy != crate::config::PolicyKind::Ask)
        .count();
    let trust = tp.trust_all
        || tp.terminator_mode
        || !tp.trusted_kinds.is_empty()
        || !tp.trusted_mcp_servers.is_empty();
    let stores = usize::from(
        config
            .store_url
            .as_deref()
            .is_some_and(|u| !u.trim().is_empty()),
    ) + config.store_sources.len();
    let left_out = [
        ("steering", docs),
        ("shortcuts", config.shortcuts.len()),
        ("automations", config.macros.len()),
        ("quick_actions", config.quick_actions.custom_actions.len()),
        ("app_modes", config.context_rules.len()),
        ("agents", config.acp.connections.len()),
        ("internal_sites", config.internal_sites.len()),
        ("tool_permissions", rules_count + usize::from(trust)),
        ("stores", stores),
        ("extension_data", data_files),
        ("extension_settings", only.extension_settings_left_out),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(kind, count)| LeftOutItem { kind, count })
    .collect();
    Ok(ExportPreview {
        left_out,
        flagged: only.flagged,
        settings: only.settings,
    })
}

// --- Export ----------------------------------------------------------------------

/// `config` as every backup starts from: no scratch (`__`) extension
/// settings (an older host saved some holding a pasted token), and none of
/// the tool-permission history the agent records on its own ("ask" rules).
pub fn exportable(config: &Config) -> Config {
    let mut config = config.clone();
    for saved in config.extensions.values_mut() {
        if let Some(obj) = saved.as_object_mut() {
            obj.retain(|k, _| !k.starts_with("__"));
        }
    }
    strip_history(&mut config);
    config
}

/// Drop the tool-permission rules the agent recorded on its own (policy
/// "ask"): a history of every command it asked about, not a setting.
fn strip_history(config: &mut Config) {
    config
        .tool_permissions
        .tools
        .retain(|t| t.policy != crate::config::PolicyKind::Ask);
}

/// Build a backup of the current config (and, for an encrypted one, the
/// steering docs and extension data on disk). An encrypted backup needs a
/// passphrase that isn't blank ([`PassphraseNeeded`], [`BlankPassphrase`]);
/// a settings-only one is never encrypted.
pub fn export(
    config: &Config,
    mode: ExportMode,
    passphrase: Option<&str>,
    installed: &[InstalledItem],
) -> Result<Vec<u8>> {
    export_from(
        config,
        mode,
        passphrase,
        installed,
        extension_data_root().as_deref(),
    )
}

fn export_from(
    config: &Config,
    mode: ExportMode,
    passphrase: Option<&str>,
    installed: &[InstalledItem],
    ext_root: Option<&Path>,
) -> Result<Vec<u8>> {
    match mode {
        ExportMode::Encrypted => {
            let pw = passphrase.ok_or(PassphraseNeeded)?;
            if pw.trim().is_empty() {
                return Err(BlankPassphrase.into());
            }
            let archive = build_full_archive(config, ext_root)?;
            encrypt(&archive, pw)
        }
        ExportMode::SettingsOnly => {
            if passphrase.is_some() {
                anyhow::bail!(
                    "A settings-only backup isn't encrypted; choose a full backup to encrypt"
                );
            }
            let rules = sensitive::Rules::new(installed);
            let names = allowlist::ExtensionNames(sensitive::display_names(installed));
            let only = allowlist::extract(config, &rules, &names);
            let manifest = BundleManifest::new(3, BundleContents::SettingsOnly);
            write_archive(&manifest, &only.tree, &[], &[])
        }
    }
}

/// One file read for a full backup.
struct DataFile {
    /// Path under `extension-data/`, `/`-separated.
    rel: String,
    body: Vec<u8>,
}

/// Every extension-data file under `root` (OS clutter and an import's own
/// temp files left out), as (`/`-separated rel, full path).
fn list_data_files(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    Ok(walk_files(root)?
        .into_iter()
        .map(|(rel, full)| (rel.to_string_lossy().replace('\\', "/"), full))
        .filter(|(rel, _)| !sensitive::is_os_junk(rel))
        .collect())
}

fn build_full_archive(config: &Config, ext_root: Option<&Path>) -> Result<Vec<u8>> {
    let mut files = Vec::new();
    if let Some(root) = ext_root.filter(|r| r.exists()) {
        for (rel, full) in list_data_files(root)? {
            let len = std::fs::symlink_metadata(&full)
                .map(|m| m.len())
                .unwrap_or(0);
            if len > MAX_ENTRY_BYTES {
                return Err(too_large(format!(
                    "an extension-data file is over {} MB",
                    MAX_ENTRY_BYTES / (1024 * 1024)
                )));
            }
            let body = std::fs::read(&full).map_err(|e| {
                // The path can carry a credential (`cache_ghp_….json`).
                let shown = if sensitive::looks_like_secret(&rel) {
                    "(name hidden)".to_string()
                } else {
                    rel.clone()
                };
                anyhow::anyhow!("Failed to read extension data {}: {}", shown, e)
            })?;
            files.push(DataFile { rel, body });
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    let total: u64 = files.iter().map(|f| f.body.len() as u64).sum();
    if total > MAX_TOTAL_BYTES || files.len() + 4 > MAX_ENTRIES {
        return Err(too_large("extension data is over the backup limits"));
    }

    let config = exportable(config);
    let tree = serde_json::to_value(&config)?;
    let steering = read_steering(&config)?;
    let mut docs = Vec::new();
    for (name, doc) in [
        ("steering/auto.md", steering.auto),
        ("steering/user.md", steering.user),
    ] {
        if let Some(body) = doc {
            docs.push((name, body));
        }
    }
    // Placeholders an earlier build left (it imported a "leave out" backup
    // here): a build that would write them over real values must refuse
    // this file, so it's v2.
    let carries_placeholders = merge::contains_placeholder(&tree)
        || docs
            .iter()
            .any(|(_, b)| String::from_utf8_lossy(b).contains(sensitive::PLACEHOLDER));
    let manifest = BundleManifest::new(
        if carries_placeholders { 2 } else { 1 },
        BundleContents::Full,
    );
    let data: Vec<(String, &[u8])> = files
        .iter()
        .map(|f| (format!("extension-data/{}", f.rel), f.body.as_slice()))
        .collect();
    let docs: Vec<(&str, &[u8])> = docs.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    write_archive(&manifest, &tree, &docs, &data)
}

fn write_archive(
    manifest: &BundleManifest,
    config: &serde_json::Value,
    docs: &[(&str, &[u8])],
    data: &[(String, &[u8])],
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("manifest.json", opts)?;
        zip.write_all(serde_json::to_string_pretty(manifest)?.as_bytes())?;
        // Pretty-printed so a curious user can diff exports.
        zip.start_file("config.json", opts)?;
        zip.write_all(serde_json::to_string_pretty(config)?.as_bytes())?;
        for (name, body) in docs {
            zip.start_file(*name, opts)?;
            zip.write_all(body)?;
        }
        for (name, body) in data {
            zip.start_file(name.as_str(), opts)?;
            zip.write_all(body)?;
        }
        zip.finish()?;
    }
    if buf.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(too_large("the archive is over the backup limits"));
    }
    Ok(buf)
}

// --- Import: read ------------------------------------------------------------------

/// Read a backup file, refusing one over [`MAX_FILE_BYTES`] before reading
/// it (and never reading past the limit if it grows meanwhile).
pub fn read_backup_file(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("Failed to open {:?}", path))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > MAX_FILE_BYTES {
        return Err(too_large(format!(
            "the file is over {} MB",
            MAX_FILE_BYTES / (1024 * 1024)
        )));
    }
    let mut body = Vec::with_capacity(len as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut body)
        .with_context(|| format!("Failed to read {:?}", path))?;
    if body.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large(format!(
            "the file is over {} MB",
            MAX_FILE_BYTES / (1024 * 1024)
        )));
    }
    Ok(body)
}

/// One extension-data file from a backup.
struct IncomingFile {
    /// Validated relative path.
    safe: PathBuf,
    /// The same, `/`-separated.
    rel: String,
    body: Vec<u8>,
}

/// A backup read, decrypted and checked on its own, before anything on
/// this machine is looked at. No locks are needed to make one.
pub struct PreparedImport {
    manifest: BundleManifest,
    kind: BundleKind,
    /// `config.json`, migrated to this build's schema.
    config: serde_json::Value,
    steering_auto: Option<Vec<u8>>,
    steering_user: Option<Vec<u8>>,
    files: Vec<IncomingFile>,
}

/// Decrypt (when the bytes start with the encrypted envelope's magic,
/// whatever the file was called), unpack and check a backup: the input is
/// capped before it's decrypted, entry names are validated before an entry
/// is read, sizes and counts are capped, the manifest must parse, and
/// `config.json` must have a schema this build reads. Errors:
/// [`PassphraseRequired`], [`WrongPassphrase`], [`UnsafeImportPath`],
/// [`BackupTooLarge`], [`UnsupportedConfigSchema`], or a plain message.
pub fn prepare_import(
    bytes: impl Into<Vec<u8>>,
    passphrase: Option<&str>,
) -> Result<PreparedImport> {
    let bytes = bytes.into();
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large(format!(
            "the file is over {} MB",
            MAX_FILE_BYTES / (1024 * 1024)
        )));
    }
    let archive = if bytes.starts_with(ENCRYPTED_MAGIC) {
        let pw = passphrase
            .filter(|p| !p.is_empty())
            .ok_or(PassphraseRequired)?;
        decrypt(bytes, pw)?
    } else {
        bytes
    };
    if archive.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(too_large("the archive is over the backup limits"));
    }
    read_archive(&archive)
}

/// Which archive entry a name is.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum EntryKind {
    Manifest,
    Config,
    SteeringAuto,
    SteeringUser,
    Data,
}

fn entry_kind(name: &str) -> Option<EntryKind> {
    match name {
        "manifest.json" => Some(EntryKind::Manifest),
        "config.json" => Some(EntryKind::Config),
        "steering/auto.md" => Some(EntryKind::SteeringAuto),
        "steering/user.md" => Some(EntryKind::SteeringUser),
        n if n.starts_with("extension-data/") => Some(EntryKind::Data),
        // Unknown entries are ignored, without being read.
        _ => None,
    }
}

/// Read one entry, refusing more than [`MAX_ENTRY_BYTES`] of it or more
/// than [`MAX_TOTAL_BYTES`] in all (counting what it actually inflates
/// to, not what the archive claims).
fn read_capped(entry: impl Read, name: &str, total: &mut u64) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut body)
        .with_context(|| format!("Failed to read {} from archive", name))?;
    let len = body.len() as u64;
    if len > MAX_ENTRY_BYTES {
        return Err(too_large(format!(
            "an entry is over {} MB",
            MAX_ENTRY_BYTES / (1024 * 1024)
        )));
    }
    *total += len;
    if *total > MAX_TOTAL_BYTES {
        return Err(too_large(format!(
            "entries are over {} MB in all",
            MAX_TOTAL_BYTES / (1024 * 1024)
        )));
    }
    Ok(body)
}

/// Validate and migrate a backup's `config.json` the way `Config::load`
/// does, before anything is merged or written.
fn migrate_config(raw: serde_json::Value) -> Result<serde_json::Value> {
    if !raw.is_object() {
        anyhow::bail!("Backup config.json isn't a settings object");
    }
    let stored = raw
        .get("version")
        .and_then(|v| v.as_u64())
        .map_or(1, |n| u32::try_from(n).unwrap_or(u32::MAX));
    crate::config_migrations::migrate(raw).map_err(|e| {
        anyhow::Error::new(UnsupportedConfigSchema {
            stored,
            newer: e.is::<crate::config_migrations::NewerVersionError>(),
        })
    })
}

fn read_archive(bytes: &[u8]) -> Result<PreparedImport> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .context("Backup file isn't a valid Kage archive")?;
    if zip.len() > MAX_ENTRIES {
        return Err(too_large(format!("more than {} entries", MAX_ENTRIES)));
    }

    let mut manifest: Option<BundleManifest> = None;
    let mut config_json: Option<Vec<u8>> = None;
    let mut steering_auto: Option<Vec<u8>> = None;
    let mut steering_user: Option<Vec<u8>> = None;
    let mut files: Vec<IncomingFile> = Vec::new();
    let mut seen_kinds: HashSet<EntryKind> = HashSet::new();
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut seen_data: HashSet<u64> = HashSet::new();
    let mut total = 0u64;

    for i in 0..zip.len() {
        // The name first: nothing is inflated for an entry that's skipped
        // or refused.
        let Some(name) = zip.name_for_index(i).map(str::to_string) else {
            continue;
        };
        if name.ends_with('/') {
            continue;
        }
        let Some(kind) = entry_kind(&name) else {
            continue;
        };
        let incoming = if kind == EntryKind::Data {
            let rel = &name["extension-data/".len()..];
            // A hard refusal for any traversal or name Windows would alias.
            let safe = sanitise_archive_path(rel)
                .map_err(|e| unsafe_path(name.escape_debug().to_string(), e))?;
            let slash = safe
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if sensitive::is_os_junk(&slash) {
                continue;
            }
            if !seen_files.insert(slash.clone()) {
                anyhow::bail!(
                    "Backup is inconsistent: extension-data/{} appears twice",
                    slash
                );
            }
            Some((safe, slash))
        } else {
            if !seen_kinds.insert(kind) {
                anyhow::bail!("Backup is inconsistent: {} appears twice", name);
            }
            None
        };
        let entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        // Two directory entries pointing at the same data would inflate one
        // stream many times over.
        if !seen_data.insert(entry.header_start()) {
            anyhow::bail!("Backup is inconsistent: two entries share the same data");
        }
        let body = read_capped(entry, &name, &mut total)?;
        match (kind, incoming) {
            (EntryKind::Manifest, _) => {
                manifest = Some(serde_json::from_slice(&body).context("Bad manifest.json")?)
            }
            (EntryKind::Config, _) => config_json = Some(body),
            (EntryKind::SteeringAuto, _) => steering_auto = Some(body),
            (EntryKind::SteeringUser, _) => steering_user = Some(body),
            (EntryKind::Data, Some((safe, rel))) => files.push(IncomingFile { safe, rel, body }),
            (EntryKind::Data, None) => {}
        }
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
    let kind = manifest.kind()?;
    if kind == BundleKind::SettingsOnly
        && (steering_auto.is_some() || steering_user.is_some() || !files.is_empty())
    {
        anyhow::bail!("Backup is inconsistent: a settings-only backup carries files");
    }
    let raw: serde_json::Value =
        serde_json::from_slice(&config_json.context("Backup is missing config.json")?)
            .context("Backup config.json was unparseable")?;
    let config = migrate_config(raw)?;
    if kind == BundleKind::Full {
        // A full backup must be a whole, readable config.
        Config::deserialize(&config).context("Backup config.json was unparseable")?;
    }
    Ok(PreparedImport {
        manifest,
        kind,
        config,
        steering_auto,
        steering_user,
        files,
    })
}

// --- Import: apply -------------------------------------------------------------

/// The result of [`apply_import`]: the merged Config (not yet saved), the
/// summary, and the files already written, which the caller keeps
/// ([`CommittedFiles::finish`]) once the Config is saved, or puts back
/// ([`CommittedFiles::rollback`]) if saving fails.
pub struct AppliedImport {
    config: Config,
    summary: ImportSummary,
    files: CommittedFiles,
}

impl AppliedImport {
    pub fn into_parts(self) -> (Config, ImportSummary, CommittedFiles) {
        (self.config, self.summary, self.files)
    }
}

/// Merge a prepared backup into `local` (this machine's config, read under
/// the lock the caller holds until it saves) and write its side-files.
/// Writes are all or nothing (see the module docs).
pub fn apply_import(
    prepared: PreparedImport,
    local: &Config,
    installed: &[InstalledItem],
) -> Result<AppliedImport> {
    apply_import_at(prepared, local, installed, extension_data_root())
}

/// Finish or undo an import a previous run of Kage left midway (killed,
/// power lost). Call before loading the config at startup. Logs and
/// carries on when it can't; the next import tries again.
pub fn recover_interrupted_import() {
    match journal::recover() {
        Ok(journal::Recovery::Nothing) => {}
        Ok(outcome) => log::info!("Interrupted backup import: {:?}", outcome),
        Err(e) => log::error!("Couldn't resolve an interrupted backup import: {:#}", e),
    }
}

/// Whether a steering doc has text of its own: not blank, and not only the
/// auto-steering header or an earlier build's placeholder.
fn steering_has_text(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    let rest = crate::steering_io::strip_auto_header(&text).trim();
    !rest.is_empty() && rest != sensitive::PLACEHOLDER
}

fn apply_import_at(
    p: PreparedImport,
    local: &Config,
    installed: &[InstalledItem],
    ext_root: Option<PathBuf>,
) -> Result<AppliedImport> {
    let lock = journal::ImportLock::acquire()?.ok_or(ImportUnfinished)?;
    journal::recover_locked(&lock).map_err(|e| {
        log::error!("Import refused: an earlier import is unresolved: {:#}", e);
        anyhow::Error::new(ImportUnfinished)
    })?;
    let rules = sensitive::Rules::new(installed);
    let manifest = p.manifest;
    // What the backup says was granted: never applied, only compared with
    // this machine's consent ([`consent`]).
    let claims = consent::grant_claims(&p.config);

    if p.kind == BundleKind::SettingsOnly {
        let secret_data = extensions_with_secret_data(ext_root.as_deref(), &rules)?;
        let (merged, settings) = allowlist::merge(local, &p.config, &rules, &secret_data)?;
        let mut merged = sanitise_imported_config(merged, local);
        let needs_approval = consent::keep_local(&mut merged, local, &claims, installed);
        merged.version = crate::config_migrations::CURRENT_VERSION;
        // Room to spare: ordinary saves later mustn't push it past the limit.
        check_config_size_within(&merged, Config::MAX_CONFIG_SIZE * 3 / 4)?;
        let summary = ImportSummary {
            kind: BundleKind::SettingsOnly,
            shortcuts: merged.shortcuts.len(),
            kage_version: manifest.kage_version.clone(),
            exported_at: manifest.exported_at.clone(),
            settings,
            needs_approval: labels(&needs_approval, installed, &manifest),
            ..Default::default()
        };
        return Ok(AppliedImport {
            config: merged,
            summary,
            files: CommittedFiles::none(),
        });
    }

    let leave_out = manifest.is_leave_out();
    if let Some(root) = ext_root.as_deref() {
        sweep_temps(root);
    }

    // This machine's extension-data files, by the name APFS / NTFS would
    // match: an incoming `Github/ſtate.json` is this machine's
    // `github/state.json`, and is judged (and written) as that.
    let local_files = ext_root
        .as_deref()
        .filter(|r| r.exists())
        .map(list_data_files)
        .transpose()?
        .unwrap_or_default();
    let exact: HashSet<&str> = local_files.iter().map(|(rel, _)| rel.as_str()).collect();
    let folded: HashMap<String, String> = local_files
        .iter()
        .map(|(rel, _)| (sensitive::fold(rel), rel.clone()))
        .collect();
    let alias = |f: &IncomingFile| -> Option<String> {
        if exact.contains(f.rel.as_str()) {
            return Some(f.rel.clone());
        }
        let root = ext_root.as_deref()?;
        let candidate = folded.get(&sensitive::fold(&f.rel))?;
        same_file(&root.join(&f.safe), &root.join(candidate)).then(|| candidate.clone())
    };

    // An earlier build's bundle that says it left a file out must not also
    // carry it: it was made by hand, and would overwrite this machine's copy.
    if leave_out
        && p.files.iter().any(|f| {
            [Some(f.rel.clone()), alias(f)]
                .into_iter()
                .flatten()
                .any(|r| is_excluded_file(&r, &manifest.excluded))
        })
    {
        anyhow::bail!(
            "Backup is inconsistent: it contains extension data its manifest says was left out"
        );
    }

    // --- Extension data. Plan every write, and check every destination,
    // before writing anything: a refused path leaves the disk untouched.
    // On the way, note which of this machine's data files holding a
    // credential the backup replaces with a copy carrying its own.
    let mut data_writes: Vec<(String, PathBuf, Vec<u8>, bool)> = Vec::new();
    let mut replaced: HashSet<String> = HashSet::new();
    if let Some(root) = &ext_root {
        let mut dests: Vec<(PathBuf, Option<String>, IncomingFile)> = Vec::new();
        for f in p.files {
            let local_rel = alias(&f);
            // Write where this machine keeps the file, under its own name.
            let dest = local_rel
                .as_deref()
                .and_then(|r| sanitise_archive_path(r).ok())
                .unwrap_or_else(|| f.safe.clone());
            dests.push((dest, local_rel, f));
        }
        check_no_file_folder_clash(dests.iter().map(|(dest, _, _)| dest.as_path()))?;
        let mut seen = HashSet::new();
        for (dest, _, _) in &dests {
            if !seen.insert(dest.clone()) {
                anyhow::bail!(
                    "Backup is inconsistent: two entries are the same file here ({})",
                    dest.display()
                );
            }
        }
        for (dest, local_rel, f) in dests {
            check_destination_under(root, &dest)?;
            let local_body = local_rel.as_ref().and_then(|_| read_under(root, &dest));
            let declared = [Some(f.rel.as_str()), local_rel.as_deref()]
                .into_iter()
                .flatten()
                .any(|rel| declared_data(&rules, rel));
            // An earlier build's "leave out" backup never carried a
            // sensitive file: one there was put in by hand, and doesn't
            // replace this machine's.
            if leave_out && declared && local_body.as_deref().is_some_and(merge::body_has_content) {
                continue;
            }
            if let merge::FileAction::Replace(body) =
                merge::data_file(f.body, local_body.as_deref(), declared)
            {
                if let Some(rel) = &local_rel {
                    replaced.insert(rel.clone());
                }
                let changes = local_body.as_deref() != Some(body.as_slice());
                data_writes.push((extension_of(&dest), dest, body, changes));
            }
        }
    }
    // The extensions whose credential stays on this machine: a data file
    // holding one that the backup doesn't replace (judged per file: an
    // unrelated file in the backup replaces nothing)…
    let mut keep_whole: BTreeSet<String> = BTreeSet::new();
    for (rel, full) in &local_files {
        if replaced.contains(rel) {
            continue;
        }
        if read_regular_file(full)
            .is_some_and(|body| merge::holds_credential(&body, declared_data(&rules, rel)))
        {
            keep_whole.insert(extension_of(Path::new(rel)));
        }
    }
    keep_whole.remove("");

    // --- Kage's settings and extension settings.
    let mut incoming = p.config;
    let local_tree = serde_json::to_value(local)?;
    merge::host(&mut incoming, &local_tree);
    // Numbers outside what their setting takes are this machine's.
    allowlist::keep_local_numbers_in_range(&mut incoming, &local_tree);
    let mut lost: Vec<String> = Vec::new();
    let mut kept_local: Vec<String> = Vec::new();
    if let Some(obj) = incoming.as_object_mut() {
        let exts = obj
            .entry("extensions")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if !exts.is_object() {
            *exts = serde_json::Value::Object(Default::default());
        }
        let empty = serde_json::Map::new();
        let l = local_tree["extensions"].as_object().unwrap_or(&empty);
        if let Some(m) = exts.as_object_mut() {
            // Scratch values never come from a backup.
            for v in m.values_mut() {
                if let Some(o) = v.as_object_mut() {
                    o.retain(|k, _| !k.starts_with("__"));
                }
            }
            // …or a sensitive setting the backup doesn't carry its own of.
            for (id, lv) in l {
                if keep_whole.contains(&sensitive::fold(id))
                    || merge::settings_credential_stays(id, m.get(id), lv, &rules, leave_out)
                {
                    keep_whole.insert(sensitive::fold(id));
                }
            }
            // Those keep this machine's settings (and data) whole.
            let whole: BTreeSet<String> = l
                .keys()
                .filter(|id| keep_whole.contains(&sensitive::fold(id)))
                .cloned()
                .chain(keep_whole.iter().cloned())
                .collect();
            let outcome = merge::extensions(m, l, &rules, &whole);
            lost = outcome.lost;
            kept_local = outcome.kept;
        }
    }
    let mut merged: Config =
        Config::deserialize(&incoming).context("Backup config.json was unparseable")?;
    merged.version = crate::config_migrations::CURRENT_VERSION;
    allowlist::keep_local_unregistrable_hotkeys(&mut merged, local);
    // Recorded ("ask") rules are history, never a decision: one in the
    // backup mustn't replace this machine's allow or deny. Then this
    // machine's rules the backup lacks are kept.
    strip_history(&mut merged);
    keep_local_tool_rules(&mut merged, local);
    let mut merged = sanitise_imported_config(merged, local);
    let needs_approval = consent::keep_local(&mut merged, local, &claims, installed);
    merged.version = crate::config_migrations::CURRENT_VERSION;
    check_config_size(&merged)?;

    // An extension kept whole gets none of the backup's data files either:
    // any of them could point its credential somewhere else.
    let mut planned: Vec<PlannedWrite> = Vec::new();
    for (id, dest, body, changes) in data_writes {
        if keep_whole.contains(&id) {
            if changes && !kept_local.iter().any(|k| sensitive::fold(k) == id) {
                kept_local.push(id);
            }
            continue;
        }
        if let Some(root) = &ext_root {
            planned.push(PlannedWrite::Data {
                root: root.clone(),
                rel: dest,
                body,
            });
        }
    }
    kept_local.sort();

    // Steering goes where THIS machine keeps it (`local`, never the
    // backup's `user_steering_path`). A copy without text of its own, or
    // still carrying an earlier build's placeholder, never replaces one with
    // text here. The auto doc is Kage's and holds what it learned about the
    // user: owner-only, as everywhere else it is written. The user's own doc
    // keeps the permissions they gave it.
    let mut reenter_refs: Vec<sensitive::SensitiveRef> = Vec::new();
    let mut steering_bytes = 0usize;
    for (body, path, item, perms) in [
        (
            p.steering_auto,
            Config::get_auto_steering_path().ok(),
            sensitive::SensitiveRef::SteeringAuto,
            Perms::Private,
        ),
        (
            p.steering_user,
            resolve_user_steering_path(local),
            sensitive::SensitiveRef::SteeringUser,
            Perms::Inherit,
        ),
    ] {
        let (Some(body), Some(path)) = (body, path) else {
            continue;
        };
        let target = steering_target(&path)?;
        if let Some(dir) = target.parent() {
            sweep_dir(dir);
        }
        if target.is_dir() {
            anyhow::bail!("This machine's steering file {:?} is a folder", target);
        }
        let local_has_text = read_regular_file(&target)
            .as_deref()
            .is_some_and(steering_has_text);
        let redacted = String::from_utf8_lossy(&body).contains(sensitive::PLACEHOLDER);
        if (redacted || !steering_has_text(&body)) && local_has_text {
            continue;
        }
        if redacted {
            reenter_refs.push(item);
        }
        steering_bytes += body.len();
        planned.push(PlannedWrite::Steering {
            target,
            body,
            perms,
        });
    }

    let extensions_written = planned
        .iter()
        .filter(|w| matches!(w, PlannedWrite::Data { .. }))
        .count();
    let files = if planned.is_empty() {
        drop(lock);
        CommittedFiles::none()
    } else {
        let staged = stage(planned)?;
        let mut txn = match journal::Transaction::begin(
            lock,
            staged.files.clone(),
            staged.created_dirs.clone(),
        ) {
            Ok(t) => t,
            Err(e) => {
                staged.discard();
                return Err(e);
            }
        };
        txn.commit()?;
        CommittedFiles { txn: Some(txn) }
    };

    // --- What couldn't be kept.
    let after = ext_root
        .as_deref()
        .filter(|r| r.exists())
        .map(list_data_files)
        .transpose()?
        .unwrap_or_default();
    let mut names = sensitive::display_names(installed);
    for (id, name) in &manifest.extension_names {
        names.entry(id.clone()).or_insert_with(|| name.clone());
    }
    let mut sign_in_again: Vec<String> = Vec::new();
    let mut add_sign_in = |id: &str| {
        let name = sensitive::extension_label(id, &names, &manifest.extension_names);
        if !sign_in_again.contains(&name) {
            sign_in_again.push(name);
        }
    };
    for id in &lost {
        add_sign_in(id);
    }
    let merged_tree = serde_json::to_value(&merged)?;
    let mut reenter: Vec<String> = Vec::new();
    let mut add_reenter = |label: String| {
        if !reenter.contains(&label) {
            reenter.push(label);
        }
    };
    if leave_out {
        for item in &manifest.excluded {
            if !excluded_was_kept(item, &merged, &merged_tree, &after) {
                match item.extension_id() {
                    Some(id) => add_sign_in(id),
                    None => add_reenter(sensitive::excluded_label(item, &merged)),
                }
            }
        }
    }
    for item in &reenter_refs {
        add_reenter(sensitive::excluded_label(item, &merged));
    }
    let mut paths = Vec::new();
    merge::placeholder_paths(&merged_tree, &mut Vec::new(), &mut paths);
    for path in paths {
        add_reenter(sensitive::host_label(&path, &merged));
    }

    let summary = ImportSummary {
        kind: BundleKind::Full,
        shortcuts: merged.shortcuts.len(),
        extensions: extensions_written,
        steering_bytes,
        kage_version: manifest.kage_version.clone(),
        exported_at: manifest.exported_at.clone(),
        settings: 0,
        left_out: if leave_out {
            manifest.excluded.len()
        } else {
            0
        },
        sign_in_again,
        reenter,
        needs_approval: labels(&needs_approval, installed, &manifest),
        kept_local: labels(&kept_local, installed, &manifest),
    };
    merged.version = crate::config_migrations::CURRENT_VERSION;
    Ok(AppliedImport {
        config: merged,
        summary,
        files,
    })
}

/// Keep this machine's tool-permission rules a backup doesn't carry (a
/// backup adds and updates rules but never removes one of this machine's).
/// A rule the backup has (same title) wins.
fn keep_local_tool_rules(merged: &mut Config, local: &Config) {
    for rule in &local.tool_permissions.tools {
        if rule.policy == crate::config::PolicyKind::Ask {
            continue;
        }
        if !merged
            .tool_permissions
            .tools
            .iter()
            .any(|t| t.title == rule.title)
        {
            merged.tool_permissions.tools.push(rule.clone());
        }
    }
}

/// The (folded) extension id an extension-data path belongs to: its first
/// folder, or `""` for a file directly under the root.
fn extension_of(rel: &Path) -> String {
    let parts: Vec<_> = rel.components().collect();
    match parts.as_slice() {
        [first, _, ..] => sensitive::fold(&first.as_os_str().to_string_lossy()),
        _ => String::new(),
    }
}

/// Display names for extension ids, for the summary.
fn labels(ids: &[String], installed: &[InstalledItem], manifest: &BundleManifest) -> Vec<String> {
    let names = sensitive::display_names(installed);
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        let name = sensitive::extension_label(id, &names, &manifest.extension_names);
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Whether the data file at `rel` is one an earlier build's manifest says
/// it left out. Stand-in names can't be matched.
fn is_excluded_file(rel: &str, excluded: &[sensitive::SensitiveRef]) -> bool {
    let folded = sensitive::fold(rel);
    excluded.iter().any(|item| match item {
        sensitive::SensitiveRef::ExtensionData {
            extension_id, file, ..
        } => {
            !sensitive::is_opaque(file)
                && !sensitive::is_opaque(extension_id)
                && sensitive::fold(&data_file_rel(extension_id, file)) == folded
        }
        _ => false,
    })
}

fn data_file_rel(extension_id: &str, file: &str) -> String {
    if extension_id.is_empty() {
        file.to_string()
    } else {
        format!("{}/{}", extension_id, file)
    }
}

/// Whether this machine still has a value for an item an earlier build's
/// backup left out. A stand-in name counts as kept when the extension (or
/// the setting's parent) here holds anything.
fn excluded_was_kept(
    item: &sensitive::SensitiveRef,
    merged: &Config,
    tree: &serde_json::Value,
    local_files: &[(String, PathBuf)],
) -> bool {
    use sensitive::SensitiveRef as R;
    let file_has_content =
        |full: &PathBuf| read_regular_file(full).is_some_and(|b| merge::body_has_content(&b));
    match item {
        R::ExtensionData {
            extension_id, file, ..
        } => {
            if sensitive::is_opaque(file) || sensitive::is_opaque(extension_id) {
                let id = sensitive::fold(extension_id);
                return local_files.iter().any(|(rel, full)| {
                    rel.split_once('/').is_some_and(|(d, _)| {
                        sensitive::is_opaque(extension_id) || sensitive::fold(d) == id
                    }) && file_has_content(full)
                });
            }
            let want = sensitive::fold(&data_file_rel(extension_id, file));
            local_files
                .iter()
                .any(|(rel, full)| sensitive::fold(rel) == want && file_has_content(full))
        }
        R::ExtensionConfig {
            extension_id,
            field,
        } => merged
            .extensions
            .iter()
            .filter(|(id, _)| {
                sensitive::is_opaque(extension_id)
                    || sensitive::fold(id) == sensitive::fold(extension_id)
            })
            .any(|(_, v)| {
                if field.is_empty() || sensitive::is_opaque(field) {
                    merge::has_content(v)
                } else {
                    v.get(field).is_some_and(merge::has_content)
                }
            }),
        R::HostSetting { path } => {
            let segs = sensitive::parse_pointer(path);
            let cut = segs
                .iter()
                .position(|s| sensitive::is_opaque(s))
                .unwrap_or(segs.len());
            let mut cur = tree;
            for seg in &segs[..cut] {
                cur = match cur {
                    serde_json::Value::Object(o) => match o.get(seg) {
                        Some(v) => v,
                        None => return false,
                    },
                    serde_json::Value::Array(a) => {
                        match seg.parse::<usize>().ok().and_then(|i| a.get(i)) {
                            Some(v) => v,
                            None => return false,
                        }
                    }
                    _ => return false,
                };
            }
            merge::has_content(cur) && !merge::contains_placeholder(cur)
        }
        R::ToolPermissionRules => merged
            .tool_permissions
            .tools
            .iter()
            .any(|t| t.policy != crate::config::PolicyKind::Ask),
        R::SteeringAuto | R::SteeringUser => true,
        R::Unknown => false,
    }
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

/// The steering docs a backup of `config` carries (absent files are `None`).
#[derive(Default)]
struct SteeringDocs {
    auto: Option<Vec<u8>>,
    user: Option<Vec<u8>>,
}

fn read_steering(config: &Config) -> Result<SteeringDocs> {
    let mut docs = SteeringDocs::default();
    if let Ok(auto_path) = Config::get_auto_steering_path() {
        if auto_path.exists() {
            docs.auto = Some(std::fs::read(&auto_path).context("Failed to read auto-steering")?);
        }
    }
    if let Some(user_path) = resolve_user_steering_path(config) {
        if user_path.exists() {
            docs.user = Some(std::fs::read(&user_path).context("Failed to read user-steering")?);
        }
    }
    Ok(docs)
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
/// letters and other `:` (alternate data streams), `~` (8.3 short names),
/// `<>|?*"`, NUL and control characters, names Windows would silently
/// alias (trailing `.` or space) or treat as a device (`CON`, `NUL`,
/// `COM1`, …), and paths that name nothing.
pub fn sanitise_archive_path(rel: &str) -> Result<PathBuf> {
    if rel.is_empty() {
        anyhow::bail!("empty path");
    }
    if rel.starts_with('/') {
        anyhow::bail!("absolute path in archive: {}", rel.escape_debug());
    }
    // `~` makes a Windows 8.3 short name (`GITHUB~1`) that aliases another
    // folder; `<>|?*"` can't be created there. Kage never writes them.
    if rel.chars().any(|c| {
        matches!(c, '\\' | ':' | '~' | '<' | '>' | '|' | '?' | '*' | '"') || c.is_control()
    }) {
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

/// Read `path` only if it is a regular file (not a symlink).
fn read_regular_file(path: &Path) -> Option<Vec<u8>> {
    let md = std::fs::symlink_metadata(path).ok()?;
    if is_link(&md) || !md.file_type().is_file() {
        return None;
    }
    std::fs::read(path).ok()
}

/// This machine's file at `rel` under `root`, if every component is a real
/// directory and the file a regular one (no symlinks followed).
fn read_under(root: &Path, rel: &Path) -> Option<Vec<u8>> {
    let mut cur = root.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for (i, part) in parts.iter().enumerate() {
        cur.push(part);
        let md = std::fs::symlink_metadata(&cur).ok()?;
        if is_link(&md) {
            return None;
        }
        if i + 1 < parts.len() && !md.is_dir() {
            return None;
        }
    }
    read_regular_file(&cur)
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
/// folder (`ext/a` and `ext/a/b.json`), compared the way Windows and macOS
/// compare names ([`sensitive::fold`]). Caught here, the import fails
/// before anything is written rather than midway.
fn check_no_file_folder_clash<'a>(rels: impl Iterator<Item = &'a Path>) -> Result<()> {
    let key = |p: &Path| {
        p.components()
            .map(|c| sensitive::fold(&c.as_os_str().to_string_lossy()))
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut files = HashSet::new();
    let mut folders = HashSet::new();
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

/// Make sure `dir` is a real directory (not a link), creating it if absent
/// and recording it in `created`.
fn ensure_real_dir(dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(md) => return refuse_link_or_kind(dir, &md, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("Failed to inspect {:?}", dir)),
    }
    match std::fs::create_dir(dir) {
        Ok(()) => created.push(dir.to_path_buf()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("Failed to create {:?}", dir)),
    }
    let md =
        std::fs::symlink_metadata(dir).with_context(|| format!("Failed to inspect {:?}", dir))?;
    refuse_link_or_kind(dir, &md, true)
}

/// Create `dir` and any missing parents (steering docs live outside the
/// extension-data root, where the user may keep links of their own),
/// recording what was created.
fn create_dirs(dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let mut missing = Vec::new();
    let mut cur = Some(dir);
    while let Some(d) = cur.filter(|d| !d.as_os_str().is_empty()) {
        if d.exists() {
            break;
        }
        missing.push(d.to_path_buf());
        cur = d.parent();
    }
    for d in missing.into_iter().rev() {
        match std::fs::create_dir(&d) {
            Ok(()) => created.push(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("Failed to create {:?}", d)),
        }
    }
    Ok(())
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

// --- Import: write -------------------------------------------------------------

/// One file import will write.
enum PlannedWrite {
    /// Under the extension-data root, creating directories one level at a
    /// time without following a link.
    Data {
        root: PathBuf,
        rel: PathBuf,
        body: Vec<u8>,
    },
    /// A steering doc at this machine's chosen target.
    Steering {
        target: PathBuf,
        body: Vec<u8>,
        perms: Perms,
    },
}

/// Names of an import's own temp files. Never backed up
/// (`sensitive::is_os_junk`); swept by the next import.
pub(crate) const TEMP_PREFIX: &str = ".kage-import.";

/// A unique name for a temp file next to `target`. A rollback copy
/// (`bak`) carries the target's name when it fits, so a person looking at
/// the folder can tell what it is; a staged file doesn't, so a long target
/// name can't push it past the file system's limit.
pub(super) fn sibling_temp(target: &Path, suffix: &str) -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = target
        .parent()
        .with_context(|| format!("No parent directory for {:?}", target))?;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned());
    let base = format!("{}{}-{}", TEMP_PREFIX, std::process::id(), seq);
    Ok(match name {
        Some(n) if suffix == "bak" && base.len() + n.len() + 6 <= 255 => {
            dir.join(format!("{}~{}.{}", base, n, suffix))
        }
        _ => dir.join(format!("{}.{}", base, suffix)),
    })
}

/// Delete staged files an import left behind under `root` (it stopped
/// before its journal was written, or the journal is resolved). Run with
/// the import lock held, after [`journal::recover_locked`]. A rollback copy
/// is never deleted here: without a committed journal there's no telling
/// whether it's the only copy of a file. One whose original is missing goes
/// back in its place; any other is left alone (it's never backed up).
fn sweep_temps(root: &Path) {
    if let Ok(files) = walk_files(root) {
        sweep(files.into_iter().map(|(_, full)| full).collect());
    }
}

/// [`sweep_temps`] for one folder only (a steering doc's, which can be
/// anywhere the user keeps it).
fn sweep_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    sweep(
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .collect(),
    );
}

fn sweep(files: Vec<PathBuf>) {
    for path in files {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let Some(rest) = name.strip_prefix(TEMP_PREFIX) else {
            continue;
        };
        if rest.ends_with(".tmp") {
            let _ = std::fs::remove_file(&path);
        } else if let Some(target) = rest
            .strip_suffix(".bak")
            .and_then(|r| r.split_once('~'))
            .map(|(_, t)| t)
            .filter(|t| !t.is_empty() && !t.contains(['/', '\\']))
        {
            let original = path.with_file_name(target);
            if std::fs::symlink_metadata(&original).is_err() {
                let _ = std::fs::rename(&path, &original);
            }
        }
    }
}

/// Whether two paths are the same file on this machine (both exist and,
/// on Unix, have the same device and inode; elsewhere a name that resolves
/// at all on a case-insensitive NTFS folder is the same file).
fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ma.dev() == mb.dev() && ma.ino() == mb.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (ma, mb);
        true
    }
}
/// Write `body` to a fresh temp file next to `target` (`create_new`, so an
/// existing file or link there is never opened), flushed to disk. It is
/// owner-only; with [`Perms::Inherit`], an existing `target` passes on its
/// permissions instead (the user's own steering doc keeps the mode they
/// gave it).
fn write_temp(target: &Path, body: &[u8], perms: Perms) -> Result<PathBuf> {
    let tmp = sibling_temp(target, "tmp")?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = perms;
    let result = (|| -> Result<()> {
        let mut file = opts
            .open(&tmp)
            .with_context(|| format!("Failed to create {:?}", tmp))?;
        file.write_all(body)
            .with_context(|| format!("Failed to write {:?}", tmp))?;
        #[cfg(unix)]
        if perms == Perms::Inherit {
            if let Ok(md) = std::fs::symlink_metadata(target) {
                if md.is_file() {
                    file.set_permissions(md.permissions())
                        .with_context(|| format!("Failed to set permissions on {:?}", tmp))?;
                }
            }
        }
        file.sync_all()
            .with_context(|| format!("Failed to flush {:?}", tmp))
    })();
    match result {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Files written to temp names next to their targets; nothing replaced
/// yet.
#[derive(Default)]
struct Staged {
    /// (target, temp file).
    files: Vec<(PathBuf, PathBuf)>,
    /// Directories created for them, in creation order.
    created_dirs: Vec<PathBuf>,
}

fn remove_created_dirs(dirs: &[PathBuf]) {
    for d in dirs.iter().rev() {
        // Only if still empty: never removes anything else.
        let _ = std::fs::remove_dir(d);
    }
}

/// Write every planned file to a temp file next to its target, creating
/// the directories it needs. On any failure every temp file and created
/// directory is removed and nothing has been replaced.
fn stage(plan: Vec<PlannedWrite>) -> Result<Staged> {
    let mut staged = Staged::default();
    for write in plan {
        let result = (|| -> Result<(PathBuf, PathBuf)> {
            match write {
                PlannedWrite::Data { root, rel, body } => {
                    let parts: Vec<_> = rel.components().collect();
                    let Some((last, dirs)) = parts.split_last() else {
                        anyhow::bail!("Empty destination under {:?}", root);
                    };
                    // Above the root is this machine's own config folder.
                    if let Some(parent) = root.parent() {
                        create_dirs(parent, &mut staged.created_dirs)?;
                    }
                    let mut cur = root.clone();
                    ensure_real_dir(&cur, &mut staged.created_dirs)?;
                    for part in dirs {
                        cur.push(part);
                        ensure_real_dir(&cur, &mut staged.created_dirs)?;
                    }
                    let target = cur.join(last);
                    if let Ok(md) = std::fs::symlink_metadata(&target) {
                        refuse_link_or_kind(&target, &md, false)?;
                    }
                    // Owner-only, like every other write of extension
                    // data: it can hold an extension's API keys.
                    let tmp = write_temp(&target, &body, Perms::Private)?;
                    Ok((target, tmp))
                }
                PlannedWrite::Steering {
                    target,
                    body,
                    perms,
                } => {
                    if let Some(parent) = target.parent() {
                        create_dirs(parent, &mut staged.created_dirs)?;
                    }
                    let tmp = write_temp(&target, &body, perms)?;
                    Ok((target, tmp))
                }
            }
        })();
        match result {
            Ok(pair) => staged.files.push(pair),
            Err(e) => {
                staged.discard();
                return Err(e);
            }
        }
    }
    Ok(staged)
}

impl Staged {
    fn discard(self) {
        for (_, tmp) in &self.files {
            let _ = std::fs::remove_file(tmp);
        }
        remove_created_dirs(&self.created_dirs);
    }
}

/// Files an import has put in place, with what they replaced (recorded in
/// the import journal). Dropped without [`Self::finish`], it puts
/// everything back.
pub struct CommittedFiles {
    txn: Option<journal::Transaction>,
}

impl CommittedFiles {
    fn none() -> Self {
        Self { txn: None }
    }

    /// Keep the new files, once the config that goes with them is saved:
    /// the commit is recorded, then what they replaced is deleted. An error
    /// means the commit couldn't be recorded and the files were put back:
    /// the caller restores the config it saved.
    pub fn finish(mut self) -> Result<()> {
        match self.txn.take() {
            Some(txn) => txn.finish(),
            None => Ok(()),
        }
    }

    /// Put every replaced file back and remove the new ones (the caller
    /// couldn't save the Config that goes with them).
    pub fn rollback(mut self) {
        if let Some(txn) = self.txn.take() {
            txn.rollback();
        }
    }

    /// Test helper: what a crash between committing files and saving the
    /// config leaves behind (the journal says "applying").
    #[cfg(test)]
    fn abandon(mut self) {
        if let Some(txn) = self.txn.take() {
            txn.abandon();
        }
    }
}

/// Write `body` to `rel` under `root` the way import does (staged, then
/// committed).
#[cfg(test)]
fn write_under(root: &Path, rel: &Path, body: &[u8]) -> Result<()> {
    let staged = stage(vec![PlannedWrite::Data {
        root: root.to_path_buf(),
        rel: rel.to_path_buf(),
        body: body.to_vec(),
    }])?;
    let lock = journal::ImportLock::acquire()?.expect("no other process imports in tests");
    let mut txn = journal::Transaction::begin(lock, staged.files.clone(), staged.created_dirs)?;
    txn.commit()?;
    txn.finish()
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

/// Decrypt an envelope in place (no second copy of a large backup). The
/// caller has already capped its size.
fn decrypt(mut envelope: Vec<u8>, passphrase: &str) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{AeadInPlace, KeyInit},
        Aes256Gcm, Nonce,
    };

    let header = ENCRYPTED_MAGIC.len() + SALT_LEN + NONCE_LEN;
    if envelope.len() < header + TAG_LEN {
        anyhow::bail!("Backup file is too small to be a valid encrypted bundle");
    }
    if !envelope.starts_with(ENCRYPTED_MAGIC) {
        anyhow::bail!("Backup file is not an encrypted Kage bundle");
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&envelope[ENCRYPTED_MAGIC.len()..ENCRYPTED_MAGIC.len() + SALT_LEN]);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    nonce_bytes.copy_from_slice(&envelope[ENCRYPTED_MAGIC.len() + SALT_LEN..header]);
    envelope.drain(..header);

    let key = derive_key(passphrase.as_bytes(), &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow::anyhow!("AES init: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    // GCM auth failure is the same surface as a wrong passphrase from the
    // user's perspective; the UI asks again.
    cipher
        .decrypt_in_place(nonce, b"", &mut envelope)
        .map_err(|_| anyhow::Error::new(WrongPassphrase))?;
    Ok(envelope)
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
mod tests;
