//! Extension and theme discovery, management, and store API.
//!
//! Extensions, themes, and command packs install under the per-user Kage
//! config directory. Store archives are verified by their caller, then
//! extracted and installed through the same path as local packages.

mod archive;
mod discovery;
mod install;
// `ext_view_open` will use it; the app doesn't open views yet.
#[allow(dead_code)]
pub mod views;

pub use archive::{extract_zip, install_from_zip};
pub use discovery::{discover_all_items, discover_items};
pub use install::{
    commit_pending, discard_pending, install_from_directory, parked_upgrade_dir, uninstall,
    InstallMode, Installed,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Validate an extension/theme/command-pack identifier before it is used as a
/// path component.
pub fn validate_extension_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        anyhow::bail!("Extension id must be 1..=64 characters: {:?}", id);
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        anyhow::bail!(
            "Extension id must start with an ASCII letter or digit: {:?}",
            id
        );
    }
    for c in chars {
        let ok = c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if !ok {
            anyhow::bail!(
                "Extension id contains a disallowed character {:?}: {:?}",
                c,
                id
            );
        }
        if c.is_ascii_uppercase() {
            anyhow::bail!("Extension id must be lowercase: {:?}", id);
        }
    }
    if first.is_ascii_uppercase() {
        anyhow::bail!("Extension id must be lowercase: {:?}", id);
    }
    Ok(())
}

/// Validate a store URL. HTTPS is allowed unconditionally; HTTP is allowed
/// only for localhost-equivalent hosts used by a development server.
pub fn validate_store_url(url: &str) -> Result<()> {
    let parsed =
        url::Url::parse(url).with_context(|| format!("Store URL is not a valid URL: {}", url))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = parsed.host_str().unwrap_or("");
            if matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1") {
                Ok(())
            } else {
                anyhow::bail!(
                    "Store URL must use HTTPS (got: {}). HTTP is only allowed for localhost.",
                    url
                )
            }
        }
        other => anyhow::bail!(
            "Store URL must use HTTPS (got scheme: {}). HTTP is only allowed for localhost.",
            other
        ),
    }
}

/// Validate that a data key is safe for use as a filename within an
/// extension's data directory.
pub fn validate_data_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 128 {
        anyhow::bail!("Data key must be 1-128 characters");
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        anyhow::bail!("Data key contains invalid characters (allowed: a-z, 0-9, -, _, .)");
    }
    if key.contains("..") {
        anyhow::bail!("Data key must not contain '..'");
    }
    Ok(())
}

/// Resolve the per-extension data path under `root` for (extension_id, key).
pub fn resolve_extension_data_path(root: &Path, extension_id: &str, key: &str) -> Result<PathBuf> {
    validate_extension_id(extension_id).context("Invalid extension id")?;
    validate_data_key(key).context("Invalid data key")?;
    let dir = root.join(extension_id);
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create extension data dir {:?}", dir))?;
    }
    Ok(dir.join(format!("{}.json", key)))
}

/// Most entries a manifest's `sensitiveData` may declare. Entries past this
/// index are ignored with a warning. Mirrored by
/// `SENSITIVE_DATA_MAX_ENTRIES` in `ui/js/shared/extension-sensitive-data.js`.
pub const MAX_SENSITIVE_DATA_ENTRIES: usize = 32;

/// Safety-net pattern for extension-data keys and extension setting names
/// the manifest doesn't declare sensitive: a name like this is treated as
/// sensitive anyway (docs/EXTENSIONS.md § Sensitive data and backups).
/// Mirrored by `SENSITIVE_NAME_RE` in `ui/js/shared/extension-sensitive-data.js`.
pub const SENSITIVE_NAME_PATTERN: &str = r"(?i)(token|secret|credential|creds|password|passwd|passphrase|passcode|api[-_\s]?key|license[-_]?key|account[-_]?key|private[-_]?key|access[-_]?key|auth|oauth|session|cookie|totp|otp[-_]?(secret|seed)|mnemonic|(^|[-_.])(pin|pass|pwd)$)";

static SENSITIVE_NAME_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(SENSITIVE_NAME_PATTERN).expect("SENSITIVE_NAME_PATTERN must compile")
});

/// Whether an extension-data key or setting name looks like it holds a
/// credential (the safety net applied on top of what the manifest declares).
pub fn is_sensitive_name(name: &str) -> bool {
    SENSITIVE_NAME_RE.is_match(name)
}

/// One `sensitiveData` entry: an exact storage key (`"creds"`) or a prefix
/// glob ending in `*` (`"token_*"`; a bare `"*"` covers every key). Matching
/// ignores ASCII case, because on macOS and Windows `Creds` and `creds` are
/// the same data file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensitiveKeyPattern {
    Exact(String),
    Prefix(String),
}

impl SensitiveKeyPattern {
    /// Parse one manifest entry. The key (or the prefix before `*`) must be
    /// a valid data key — the same rule `save_extension_data` applies.
    pub fn parse(entry: &str) -> Result<Self> {
        match entry.strip_suffix('*') {
            Some(prefix) => {
                if prefix.contains('*') {
                    anyhow::bail!("'*' is only allowed once, at the end");
                }
                if !prefix.is_empty() {
                    validate_data_key(prefix)?;
                }
                Ok(Self::Prefix(prefix.to_ascii_lowercase()))
            }
            None => {
                validate_data_key(entry)?;
                Ok(Self::Exact(entry.to_ascii_lowercase()))
            }
        }
    }

    pub fn matches(&self, key: &str) -> bool {
        let key = key.to_ascii_lowercase();
        match self {
            Self::Exact(k) => key == *k,
            Self::Prefix(p) => key.starts_with(p.as_str()),
        }
    }
}

/// Validate a manifest's raw `sensitiveData` value. Malformed entries are
/// dropped with a warning, like unknown capabilities in
/// [`normalize_permissions`]: a not-an-array value, non-strings, entries
/// that aren't valid data keys, and anything past
/// [`MAX_SENSITIVE_DATA_ENTRIES`].
pub fn normalize_sensitive_data(
    raw: Option<&serde_json::Value>,
    context: &str,
) -> Vec<SensitiveKeyPattern> {
    let list = match raw {
        None | Some(serde_json::Value::Null) => return Vec::new(),
        Some(serde_json::Value::Array(list)) => list,
        Some(_) => {
            log::warn!(
                "Extension '{}': sensitiveData must be an array of storage keys — ignored",
                context
            );
            return Vec::new();
        }
    };
    let mut out: Vec<SensitiveKeyPattern> = Vec::new();
    for (i, entry) in list.iter().enumerate() {
        if i >= MAX_SENSITIVE_DATA_ENTRIES {
            log::warn!(
                "Extension '{}': sensitiveData has more than {} entries; the rest are ignored",
                context,
                MAX_SENSITIVE_DATA_ENTRIES
            );
            break;
        }
        let Some(text) = entry.as_str() else {
            log::warn!(
                "Extension '{}': sensitiveData entry {} is not a string — ignored",
                context,
                i
            );
            continue;
        };
        match SensitiveKeyPattern::parse(text) {
            Ok(pattern) => {
                if !out.contains(&pattern) {
                    out.push(pattern);
                }
            }
            Err(e) => log::warn!(
                "Extension '{}': invalid sensitiveData entry {:?} ({}) — ignored",
                context,
                text,
                e
            ),
        }
    }
    out
}

/// The type a manifest `config` entry declares (docs/EXTENSIONS.md § Config
/// Schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFieldType {
    Boolean,
    Number,
    String,
}

/// One manifest `config` entry, as an unencrypted settings-only backup
/// reads it (docs/EXTENSIONS.md § Sensitive data and backups).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigFieldDecl {
    pub field: String,
    /// `None`: missing or not one of the supported types.
    pub ty: Option<ConfigFieldType>,
    /// `sensitive: true`.
    pub sensitive: bool,
    /// `backup: "plain"`: a free-form string the author says is safe to
    /// write to an unencrypted backup.
    pub plain: bool,
    /// `enum: [...]`: the only values the setting takes (strings, numbers
    /// or booleans).
    pub choices: Option<Vec<serde_json::Value>>,
    /// `sync: "shared" | "os" | "machine" | "never"`: where folder sync
    /// puts the setting (docs/design/SYNC.md §12.1). `None`: the default
    /// rule.
    pub sync: Option<SyncScopeDecl>,
}

/// A manifest `config.<field>.sync` value (docs/design/SYNC.md §12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncScopeDecl {
    /// Synced, one value for all computers.
    Shared,
    /// Synced, a new value applies to this kind of computer.
    Os,
    /// Never leaves this computer.
    Machine,
    /// Never synced.
    Never,
}

/// Every entry of a manifest's `config` block, read for backups. Malformed
/// `sensitive`, `backup` and `enum` values are ignored with a warning, like
/// unknown capabilities; `sensitive: true` wins over `backup: "plain"`.
pub fn config_field_decls(
    config: Option<&serde_json::Value>,
    context: &str,
) -> Vec<ConfigFieldDecl> {
    let Some(schema) = config.and_then(|c| c.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (field, entry) in schema {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let ty = match entry.get("type").and_then(|t| t.as_str()) {
            Some("boolean") => Some(ConfigFieldType::Boolean),
            Some("number") => Some(ConfigFieldType::Number),
            Some("string") => Some(ConfigFieldType::String),
            _ => None,
        };
        let sensitive = match entry.get("sensitive") {
            None | Some(serde_json::Value::Bool(false)) => false,
            Some(serde_json::Value::Bool(true)) => true,
            Some(_) => {
                log::warn!(
                    "Extension '{}': config.{}.sensitive must be true or false — ignored",
                    context,
                    field
                );
                false
            }
        };
        let plain = match entry.get("backup") {
            None => false,
            Some(serde_json::Value::String(s)) if s == "plain" => true,
            Some(_) => {
                log::warn!(
                    "Extension '{}': config.{}.backup must be \"plain\" — ignored",
                    context,
                    field
                );
                false
            }
        };
        if plain && sensitive {
            log::warn!(
                "Extension '{}': config.{} is both sensitive and backup \"plain\"; it's treated as sensitive",
                context,
                field
            );
        }
        let choices = match entry.get("enum") {
            None => None,
            Some(serde_json::Value::Array(values))
                if !values.is_empty()
                    && values.len() <= MAX_CONFIG_ENUM_VALUES
                    && values.iter().all(|v| {
                        v.is_boolean()
                            || v.is_number()
                            || v.as_str().is_some_and(|s| s.len() <= MAX_CONFIG_ENUM_LEN)
                    }) =>
            {
                Some(values.clone())
            }
            Some(_) => {
                log::warn!(
                    "Extension '{}': config.{}.enum must be a list of up to {} strings, numbers or booleans — ignored",
                    context,
                    field,
                    MAX_CONFIG_ENUM_VALUES
                );
                None
            }
        };
        let sync = match entry.get("sync") {
            None => None,
            Some(serde_json::Value::String(s)) if s == "shared" => Some(SyncScopeDecl::Shared),
            Some(serde_json::Value::String(s)) if s == "os" => Some(SyncScopeDecl::Os),
            Some(serde_json::Value::String(s)) if s == "machine" => Some(SyncScopeDecl::Machine),
            Some(serde_json::Value::String(s)) if s == "never" => Some(SyncScopeDecl::Never),
            Some(_) => {
                log::warn!(
                    "Extension '{}': config.{}.sync must be \"shared\", \"os\", \"machine\" or \"never\" — ignored",
                    context,
                    field
                );
                None
            }
        };
        out.push(ConfigFieldDecl {
            field: field.clone(),
            ty,
            sensitive,
            plain: plain && !sensitive,
            choices,
            sync,
        });
    }
    out
}

/// Most values a manifest `config.<field>.enum` may list.
pub const MAX_CONFIG_ENUM_VALUES: usize = 64;
/// Longest string value a manifest `config.<field>.enum` may list.
pub const MAX_CONFIG_ENUM_LEN: usize = 128;

// --- The manifest `sync` block (docs/design/SYNC.md §12.2) -------------------

/// Longest `sync.description`, in characters. Rust checks the raw text;
/// the install prompt checks it again after `__MSG_…__` resolution.
pub const MAX_SYNC_DESCRIPTION: usize = 140;
/// Most `sync.keys` entries.
pub const MAX_SYNC_KEYS: usize = 32;
/// Most `sync.lists` entries.
pub const MAX_SYNC_LISTS: usize = 16;
/// Most `sync.collections` entries.
pub const MAX_SYNC_COLLECTIONS: usize = 16;
/// A collection record's default size limit.
pub const DEFAULT_RECORD_BYTES: u64 = 64 * 1024;
/// The largest `maxBytes` of a collection, and of a `large` one.
pub const MAX_RECORD_BYTES: u64 = 1024 * 1024;
pub const MAX_LARGE_RECORD_BYTES: u64 = 4 * 1024 * 1024;

/// One `sync.lists` entry: the key's file holds a list of items with ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncListDecl {
    /// The top-level field holding the array, or `""` when the file is
    /// itself an array.
    pub array: String,
    /// The element field holding its unique id.
    pub id: String,
    /// The element field that names it in the conflicts list.
    pub label: Option<String>,
}

/// One `sync.collections` entry: records the extension pushes and pulls.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncCollectionDecl {
    /// The record is one cell.
    pub whole: bool,
    pub together: Vec<Vec<String>>,
    pub sets: Vec<String>,
    pub maps: Vec<String>,
    /// Fields the host drops before storing a record.
    pub local: Vec<String>,
    pub label: Option<String>,
    /// The field that marks a record private.
    pub private: Option<String>,
    pub max_bytes: u64,
    /// Records may be up to 4 MiB and are stored as blobs.
    pub large: bool,
}

/// The validated `sync` block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncDecl {
    pub description: Option<String>,
    pub schema: u32,
    /// Keys mode: storage keys, exact or with a trailing `*`.
    pub keys: Vec<SensitiveKeyPattern>,
    /// Lists mode, by storage key.
    pub lists: std::collections::BTreeMap<String, SyncListDecl>,
    /// Collections mode, by name.
    pub collections: std::collections::BTreeMap<String, SyncCollectionDecl>,
}

impl SyncDecl {
    /// Whether keys mode covers `key`. A lists key never is.
    pub fn key_syncs(&self, key: &str) -> bool {
        !self.lists.contains_key(key) && self.keys.iter().any(|p| p.matches(key))
    }
}

/// Whether `name` is a valid collection name: `^[a-z][a-z0-9_]{0,31}$`.
pub fn valid_collection_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `name` can be a top-level record field a cell carries.
pub fn valid_sync_field(name: &str) -> bool {
    crate::sync::split::valid_field_name(name) && name.len() <= 128
}

fn sync_field_list(v: Option<&serde_json::Value>) -> Option<Vec<String>> {
    match v {
        None => Some(Vec::new()),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .filter(|s| valid_sync_field(s))
                    .map(str::to_string)
            })
            .collect(),
        Some(_) => None,
    }
}

fn optional_sync_field(v: Option<&serde_json::Value>) -> Result<Option<String>, ()> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if valid_sync_field(s) => Ok(Some(s.clone())),
        Some(_) => Err(()),
    }
}

/// One collection entry, or why it is ignored.
fn sync_collection(raw: &serde_json::Value) -> Result<SyncCollectionDecl, &'static str> {
    let o = raw.as_object().ok_or("must be an object")?;
    let flag = |k: &str| match o.get(k) {
        None => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => Err("whole and large must be true or false"),
    };
    let whole = flag("whole")?;
    let large = flag("large")?;
    let cap = if large {
        MAX_LARGE_RECORD_BYTES
    } else {
        MAX_RECORD_BYTES
    };
    let max_bytes = match o.get("maxBytes") {
        None => DEFAULT_RECORD_BYTES,
        Some(v) => match v.as_u64() {
            Some(n) if (1..=cap).contains(&n) => n,
            _ => return Err("maxBytes is out of range"),
        },
    };
    let together: Vec<Vec<String>> = match o.get("together") {
        None => Vec::new(),
        Some(serde_json::Value::Array(groups)) => groups
            .iter()
            .map(|g| sync_field_list(Some(g)).filter(|g| !g.is_empty()))
            .collect::<Option<_>>()
            .ok_or("together must be a list of lists of field names")?,
        Some(_) => return Err("together must be a list of lists of field names"),
    };
    let sets = sync_field_list(o.get("sets")).ok_or("sets must be a list of field names")?;
    let maps = sync_field_list(o.get("maps")).ok_or("maps must be a list of field names")?;
    let local = sync_field_list(o.get("local")).ok_or("local must be a list of field names")?;
    let mut seen = std::collections::BTreeSet::new();
    for f in together
        .iter()
        .flatten()
        .chain(&sets)
        .chain(&maps)
        .chain(&local)
    {
        if f == "id" {
            return Err("can't name the id field");
        }
        if !seen.insert(f.as_str()) {
            return Err("has fields in more than one of together, sets, maps and local");
        }
    }
    let label = optional_sync_field(o.get("label")).map_err(|_| "label must be a field name")?;
    let private =
        optional_sync_field(o.get("private")).map_err(|_| "private must be a field name")?;
    Ok(SyncCollectionDecl {
        whole,
        together,
        sets,
        maps,
        local,
        label,
        private,
        max_bytes,
        large,
    })
}

/// Whether a storage key (or a key pattern's literal part) names
/// credentials: the manifest's `sensitiveData` or the safety net.
fn sensitive_key(key: &str, sensitive: &[SensitiveKeyPattern]) -> bool {
    is_sensitive_name(key) || sensitive.iter().any(|p| p.matches(key))
}

/// Validate a manifest's raw `sync` block, following the
/// [`normalize_sensitive_data`] contract: a malformed part is ignored with
/// a warning and the rest still loads. A block on a manifest without the
/// `sync` permission (`permissions`, normalised) is ignored: only
/// settings sync then. Keys and lists that `sensitiveData` or the safety
/// net match are dropped: they never sync.
pub fn normalize_sync(
    raw: Option<&serde_json::Value>,
    permissions: &[String],
    sensitive: &[SensitiveKeyPattern],
    context: &str,
) -> Option<SyncDecl> {
    let obj = match raw {
        None | Some(serde_json::Value::Null) => return None,
        Some(serde_json::Value::Object(o)) => o,
        Some(_) => {
            log::warn!("Extension '{}': sync must be an object — ignored", context);
            return None;
        }
    };
    if !permissions.iter().any(|p| p == "sync") {
        log::warn!(
            "Extension '{}': a sync block without the sync permission is ignored; only settings sync",
            context
        );
        return None;
    }
    let mut out = SyncDecl {
        schema: 1,
        ..SyncDecl::default()
    };
    match obj.get("description") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::String(s)) if s.chars().count() <= MAX_SYNC_DESCRIPTION => {
            out.description = Some(s.clone());
        }
        Some(_) => log::warn!(
            "Extension '{}': sync.description must be text of at most {} characters — ignored",
            context,
            MAX_SYNC_DESCRIPTION
        ),
    }
    if let Some(v) = obj.get("schema") {
        match v.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(n) if n >= 1 => out.schema = n,
            _ => log::warn!(
                "Extension '{}': sync.schema must be a whole number from 1 — using 1",
                context
            ),
        }
    }
    normalize_sync_keys(obj.get("keys"), sensitive, context, &mut out);
    normalize_sync_lists(obj.get("lists"), sensitive, context, &mut out);
    match obj.get("collections") {
        None => {}
        Some(serde_json::Value::Object(colls)) => {
            for (i, (name, decl)) in colls.iter().enumerate() {
                if i >= MAX_SYNC_COLLECTIONS {
                    log::warn!(
                        "Extension '{}': sync.collections has more than {} entries; the rest are ignored",
                        context,
                        MAX_SYNC_COLLECTIONS
                    );
                    break;
                }
                if !valid_collection_name(name) {
                    log::warn!(
                        "Extension '{}': sync.collections name {:?} is invalid — ignored",
                        context,
                        name
                    );
                    continue;
                }
                match sync_collection(decl) {
                    Ok(d) => {
                        out.collections.insert(name.clone(), d);
                    }
                    Err(why) => log::warn!(
                        "Extension '{}': sync.collections.{} {} — ignored",
                        context,
                        name,
                        why
                    ),
                }
            }
        }
        Some(_) => log::warn!(
            "Extension '{}': sync.collections must be an object — ignored",
            context
        ),
    }
    Some(out)
}

fn normalize_sync_keys(
    raw: Option<&serde_json::Value>,
    sensitive: &[SensitiveKeyPattern],
    context: &str,
    out: &mut SyncDecl,
) {
    let list = match raw {
        None => return,
        Some(serde_json::Value::Array(list)) => list,
        Some(_) => {
            log::warn!(
                "Extension '{}': sync.keys must be a list — ignored",
                context
            );
            return;
        }
    };
    for (i, entry) in list.iter().enumerate() {
        if i >= MAX_SYNC_KEYS {
            log::warn!(
                "Extension '{}': sync.keys has more than {} entries; the rest are ignored",
                context,
                MAX_SYNC_KEYS
            );
            break;
        }
        let Some(text) = entry.as_str() else {
            log::warn!(
                "Extension '{}': sync.keys entry {} is not text — ignored",
                context,
                i
            );
            continue;
        };
        match SensitiveKeyPattern::parse(text) {
            Ok(p) => {
                let literal = text.trim_end_matches('*');
                if !literal.is_empty() && sensitive_key(literal, sensitive) {
                    log::warn!(
                        "Extension '{}': sync.keys entry {:?} names sensitive data; it never syncs",
                        context,
                        text
                    );
                } else if !out.keys.contains(&p) {
                    out.keys.push(p);
                }
            }
            Err(e) => log::warn!(
                "Extension '{}': invalid sync.keys entry {:?} ({}) — ignored",
                context,
                text,
                e
            ),
        }
    }
}

fn normalize_sync_lists(
    raw: Option<&serde_json::Value>,
    sensitive: &[SensitiveKeyPattern],
    context: &str,
    out: &mut SyncDecl,
) {
    let lists = match raw {
        None => return,
        Some(serde_json::Value::Object(lists)) => lists,
        Some(_) => {
            log::warn!(
                "Extension '{}': sync.lists must be an object — ignored",
                context
            );
            return;
        }
    };
    for (i, (key, decl)) in lists.iter().enumerate() {
        if i >= MAX_SYNC_LISTS {
            log::warn!(
                "Extension '{}': sync.lists has more than {} entries; the rest are ignored",
                context,
                MAX_SYNC_LISTS
            );
            break;
        }
        if validate_data_key(key).is_err() || sensitive_key(key, sensitive) {
            log::warn!(
                "Extension '{}': sync.lists key {:?} is not a storage key it may sync — ignored",
                context,
                key
            );
            continue;
        }
        let parsed = decl.as_object().and_then(|o| {
            let array = match o.get("array") {
                Some(serde_json::Value::String(a)) if a.is_empty() || valid_sync_field(a) => {
                    a.clone()
                }
                _ => return None,
            };
            let id = o
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| valid_sync_field(s))?
                .to_string();
            let label = optional_sync_field(o.get("label")).ok()?;
            Some(SyncListDecl { array, id, label })
        });
        match parsed {
            Some(d) => {
                out.lists.insert(key.clone(), d);
            }
            None => log::warn!(
                "Extension '{}': sync.lists.{} needs array and id field names — ignored",
                context,
                key
            ),
        }
    }
}

/// The lock `save_extension_data`, `delete_extension_data` and folder
/// sync's projection take around a write of one data key, so a synced
/// value never lands half-way through an extension's own save
/// (docs/design/SYNC.md §12.4). Keys fold to lowercase, as the macOS and
/// Windows file systems do.
pub fn data_key_lock(extension_id: &str, key: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    type Locks = Mutex<HashMap<(String, String), Arc<Mutex<()>>>>;
    static LOCKS: std::sync::LazyLock<Locks> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut map = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 4096 {
        // Locks nobody holds can go; they are made again on demand.
        map.retain(|_, l| Arc::strong_count(l) > 1);
    }
    map.entry((extension_id.to_string(), key.to_ascii_lowercase()))
        .or_default()
        .clone()
}

/// Canonical list of valid capability names. Must stay in sync with
/// `CAPABILITIES` in `ui/js/shared/extension-permissions.js`.
pub const VALID_CAPABILITIES: &[&str] = &[
    "storage",
    "clipboard",
    "urls",
    "launch",
    "network",
    "oauth",
    "filesystem",
    "window",
    "windows",
    "notifications",
    "calendar",
    "session",
    "agent",
    "activity",
    "automation",
    "tts",
    "views",
    "file_text",
    "agent_background",
    "sync",
];

/// Filter a raw permission list to a deduped, lowercase set of known
/// capabilities. Unknown entries are logged and dropped.
pub fn normalize_permissions(raw: &[String], context: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        let cap = entry.trim().to_lowercase();
        if cap.is_empty() {
            continue;
        }
        if !VALID_CAPABILITIES.contains(&cap.as_str()) {
            log::warn!(
                "Extension '{}': unknown capability '{}' — ignored",
                context,
                cap
            );
            continue;
        }
        if seen.insert(cap.clone()) {
            out.push(cap);
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub contributes: Option<ExtensionContributes>,
    #[serde(default, rename = "sandboxVendor")]
    pub sandbox_vendor: Option<Vec<String>>,
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    #[serde(default)]
    pub commands: Option<Vec<serde_json::Value>>,
    /// Storage keys whose `save_extension_data` files grant access if
    /// leaked (tokens, credentials). Kept raw so a malformed value can't
    /// make the whole manifest unreadable; read it through
    /// [`ExtensionManifest::sensitive_data_patterns`].
    #[serde(default, rename = "sensitiveData")]
    pub sensitive_data: Option<serde_json::Value>,
    /// The `agentBackground` block: why and for what the extension asks
    /// the agent in the background (`agent_background` capability). Kept
    /// raw, like `sensitiveData`; read it through
    /// [`ExtensionManifest::agent_background_decl`].
    #[serde(default, rename = "agentBackground")]
    pub agent_background: Option<serde_json::Value>,
    /// The `sync` block: what of the extension's data folder sync copies
    /// (`sync` capability, docs/design/SYNC.md §12.2). Kept raw, like
    /// `sensitiveData`; read it through [`ExtensionManifest::sync_decl`].
    #[serde(default)]
    pub sync: Option<serde_json::Value>,
}

impl ExtensionManifest {
    /// The validated `sensitiveData` patterns (see [`normalize_sensitive_data`]).
    pub fn sensitive_data_patterns(&self) -> Vec<SensitiveKeyPattern> {
        normalize_sensitive_data(self.sensitive_data.as_ref(), &self.id)
    }

    /// Every `config` entry, as backups read it (see [`config_field_decls`]).
    pub fn config_field_decls(&self) -> Vec<ConfigFieldDecl> {
        config_field_decls(self.config.as_ref(), &self.id)
    }

    /// The `agentBackground` block, normalised. `None` when it is absent
    /// or malformed: background agent calls then fail `not_available`
    /// (docs/design/EXTENSION_AGENT_CALLS.md §3.2).
    pub fn agent_background_decl(&self) -> Option<crate::agent_calls::AgentBackgroundDecl> {
        self.agent_background
            .as_ref()
            .and_then(crate::agent_calls::AgentBackgroundDecl::from_manifest)
    }

    /// The validated `sync` block (see [`normalize_sync`]). `None` when it
    /// is absent, malformed, or the manifest doesn't ask for `sync`.
    pub fn sync_decl(&self) -> Option<SyncDecl> {
        let perms = normalize_permissions(self.permissions.as_deref().unwrap_or(&[]), &self.id);
        normalize_sync(
            self.sync.as_ref(),
            &perms,
            &self.sensitive_data_patterns(),
            &self.id,
        )
    }

    /// The valid `contributes.views` entries (see [`views::normalize_views`]).
    pub fn views(&self) -> Vec<views::ViewDecl> {
        views::normalize_views(
            self.contributes.as_ref().and_then(|c| c.views.as_ref()),
            &self.name,
            &self.icon,
            &self.id,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionContributes {
    #[serde(default)]
    pub search_provider: Option<String>,
    #[serde(default)]
    pub settings_provider: Option<String>,
    #[serde(default)]
    pub settings_module: Option<String>,
    #[serde(default)]
    pub css: Option<Vec<String>>,
    #[serde(default)]
    pub widgets: Option<Vec<WidgetContribution>>,
    #[serde(default)]
    pub themes: Option<ThemeContributes>,
    #[serde(default)]
    pub toolbar_buttons: Option<String>,
    #[serde(default)]
    pub message_formatters: Option<String>,
    #[serde(default)]
    pub tool_provider: Option<String>,
    #[serde(default)]
    pub trigger_provider: Option<String>,
    /// Generic mascot activities the extension may hint ("music", "meeting",
    /// "timer"). Validated and enforced frontend-side by the extension
    /// sandbox host; modelled here only so the field survives the
    /// manifest round-trip at install time instead of being silently dropped.
    #[serde(default)]
    pub mascot_activities: Option<Vec<String>>,
    /// Windows the extension draws itself (`views` capability). Kept raw
    /// so a malformed value can't make the manifest unreadable; read it
    /// through [`ExtensionManifest::views`].
    #[serde(default)]
    pub views: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetContribution {
    pub id: String,
    pub slot: String,
    pub module: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeContributes {
    #[serde(default)]
    pub dark: Option<String>,
    #[serde(default)]
    pub light: Option<String>,
}

/// Runtime info about a discovered extension/theme, including where it lives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledItem {
    pub manifest: ExtensionManifest,
    pub path: String,
    pub enabled: bool,
}

/// Get a user directory under `<config_dir>/kage/<subdir>/`.
pub fn user_item_dir(subdir: &str) -> Result<PathBuf> {
    let config_dir = crate::paths::config_dir().context("Failed to get config directory")?;
    Ok(config_dir.join("kage").join(subdir))
}

/// Map an item kind to its user directory name.
pub fn kind_to_subdir(kind: &str) -> Result<&'static str> {
    match kind {
        "extension" => Ok("extensions"),
        "theme" => Ok("themes"),
        "commands" => Ok("command-packs"),
        other => anyhow::bail!("Unknown item type: {}", other),
    }
}

/// Load theme colors from a theme's JSON file.
pub fn load_theme_colors(theme_id: &str, variant: &str) -> Result<Option<serde_json::Value>> {
    if let Ok(user_dir) = user_item_dir("themes") {
        let theme_dir = user_dir.join(theme_id);
        log::info!("load_theme_colors: checking user dir {:?}", theme_dir);
        if let Some(colors) = try_load_theme_variant(&theme_dir, variant)? {
            return Ok(Some(colors));
        }
    }

    log::warn!(
        "load_theme_colors: theme '{}' ({}) not found",
        theme_id,
        variant
    );
    Ok(None)
}

fn try_load_theme_variant(theme_dir: &Path, variant: &str) -> Result<Option<serde_json::Value>> {
    let manifest_path = theme_dir.join("manifest.json");
    if !manifest_path.exists() {
        return Ok(None);
    }

    let manifest_content = fs::read_to_string(&manifest_path)?;
    let manifest: ExtensionManifest = serde_json::from_str(&manifest_content)?;
    let variant_file = manifest
        .contributes
        .as_ref()
        .and_then(|c| c.themes.as_ref())
        .and_then(|t| match variant {
            "dark" => t.dark.as_ref(),
            "light" => t.light.as_ref(),
            _ => t.dark.as_ref(),
        });

    let Some(variant_file) = variant_file else {
        return Ok(None);
    };
    let variant_path = theme_dir.join(variant_file);
    if !variant_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&variant_path)?;
    let theme_data: serde_json::Value = serde_json::from_str(&content)?;
    Ok(theme_data.get("colors").cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_permissions_filters_and_preserves_order() {
        let raw = vec![
            "Storage".to_string(),
            "strage".to_string(),
            "  storage  ".to_string(),
            "calendar".to_string(),
        ];
        assert_eq!(
            normalize_permissions(&raw, "test"),
            vec!["storage", "calendar"]
        );
    }

    #[test]
    fn every_valid_capability_passes() {
        let raw: Vec<String> = VALID_CAPABILITIES.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            normalize_permissions(&raw, "test").len(),
            VALID_CAPABILITIES.len()
        );
    }

    #[test]
    fn views_is_a_valid_capability() {
        assert!(VALID_CAPABILITIES.contains(&"views"));
        assert_eq!(
            normalize_permissions(&["Views".to_string()], "test"),
            vec!["views"]
        );
    }

    #[test]
    fn agent_background_is_a_valid_capability() {
        assert!(VALID_CAPABILITIES.contains(&"agent_background"));
        assert_eq!(
            normalize_permissions(&[" Agent_Background ".to_string()], "test"),
            vec!["agent_background"]
        );
    }

    #[test]
    fn agent_background_block_round_trips_and_normalizes() {
        let m = parse_manifest(serde_json::json!({
            "permissions": ["storage", "agent_background"],
            "agentBackground": {
                "reason": "__MSG_manifest.agent_background_reason__",
                "purposes": ["organize"],
                "maxPromptChars": 16000,
                "dailyCalls": 40,
                "future": true
            }
        }));
        let decl = m.agent_background_decl().expect("a valid block");
        assert_eq!(decl.reason, "__MSG_manifest.agent_background_reason__");
        assert_eq!(decl.purposes, vec!["organize"]);
        assert_eq!(decl.max_prompt_chars, Some(16000));
        assert_eq!(decl.daily_calls, Some(40));
        let value = serde_json::to_value(&m).expect("serialize");
        assert_eq!(
            value["agentBackground"]["future"], true,
            "the raw value round-trips untouched"
        );
    }

    #[test]
    fn a_malformed_agent_background_block_means_not_declared() {
        for raw in [
            serde_json::json!("organize"),
            serde_json::json!({ "purposes": ["organize"] }),
            serde_json::json!({ "reason": "Why", "purposes": [] }),
            serde_json::json!({ "reason": "Why", "purposes": ["Bad-Id"] }),
        ] {
            let m = parse_manifest(serde_json::json!({ "agentBackground": raw }));
            assert!(m.agent_background_decl().is_none(), "{raw}");
        }
        assert!(parse_manifest(serde_json::json!({}))
            .agent_background_decl()
            .is_none());
    }

    #[test]
    fn manifest_views_survive_round_trip_and_normalize() {
        let json = r#"{
            "id": "mindmap", "name": "Mind Map", "version": "1.0.0",
            "type": "extension", "icon": "🧠",
            "contributes": { "views": [
                {"id": "explorer", "module": "./explorer.js", "extra": 1},
                {"id": "Bad", "module": "./bad.js"}
            ] }
        }"#;
        let manifest: ExtensionManifest = serde_json::from_str(json).expect("parse");
        let value = serde_json::to_value(&manifest).expect("serialize");
        assert_eq!(
            value["contributes"]["views"][0]["extra"], 1,
            "the raw value round-trips untouched"
        );
        let views = manifest.views();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].id, "explorer");
        assert_eq!(views[0].title, "Mind Map");
        assert_eq!(views[0].icon, "🧠");
    }

    #[test]
    fn malformed_views_leave_the_manifest_readable() {
        let m = parse_manifest(serde_json::json!({
            "contributes": { "views": "explorer" }
        }));
        assert!(m.views().is_empty());
        assert!(parse_manifest(serde_json::json!({})).views().is_empty());
    }

    #[test]
    fn manifest_preserves_sandbox_vendor_round_trip() {
        let json = r#"{
            "id": "math", "name": "Math", "version": "1.0.0",
            "type": "extension", "sandboxVendor": ["math"]
        }"#;
        let manifest: ExtensionManifest = serde_json::from_str(json).expect("parse");
        assert_eq!(
            manifest.sandbox_vendor.as_deref(),
            Some(&["math".to_string()][..])
        );
        let value = serde_json::to_value(&manifest).expect("serialize");
        assert_eq!(value["sandboxVendor"], serde_json::json!(["math"]));
    }

    #[test]
    fn manifest_preserves_mascot_activities_round_trip() {
        let json = r#"{
            "id": "player", "name": "Player", "version": "1.0.0",
            "type": "extension",
            "contributes": { "mascotActivities": ["music"] }
        }"#;
        let manifest: ExtensionManifest = serde_json::from_str(json).expect("parse");
        assert_eq!(
            manifest
                .contributes
                .as_ref()
                .and_then(|c| c.mascot_activities.as_deref()),
            Some(&["music".to_string()][..])
        );
        let value = serde_json::to_value(&manifest).expect("serialize");
        assert_eq!(
            value["contributes"]["mascotActivities"],
            serde_json::json!(["music"])
        );
    }

    fn parse_manifest(extra: serde_json::Value) -> ExtensionManifest {
        let mut base = serde_json::json!({
            "id": "fixture", "name": "Fixture", "version": "1.0.0", "type": "extension",
        });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).expect("manifest must parse")
    }

    #[test]
    fn sensitive_data_absent_means_nothing_declared() {
        let m = parse_manifest(serde_json::json!({}));
        assert!(m.sensitive_data_patterns().is_empty());
        assert!(m.config_field_decls().is_empty());
    }

    #[test]
    fn sensitive_data_parses_exact_keys_and_prefix_globs() {
        let m = parse_manifest(serde_json::json!({
            "sensitiveData": ["creds", "token_*", "Creds", "*"]
        }));
        let patterns = m.sensitive_data_patterns();
        assert_eq!(
            patterns,
            vec![
                SensitiveKeyPattern::Exact("creds".into()),
                SensitiveKeyPattern::Prefix("token_".into()),
                SensitiveKeyPattern::Prefix(String::new()),
            ],
            "duplicates (case-insensitively) collapse"
        );
        let exact = &patterns[0];
        assert!(exact.matches("creds"));
        assert!(
            exact.matches("CREDS"),
            "case-insensitive like the macOS/Windows FS"
        );
        assert!(!exact.matches("creds2"));
        let glob = &patterns[1];
        assert!(glob.matches("token_spotify"));
        assert!(glob.matches("token_"));
        assert!(!glob.matches("my_token_x"));
        assert!(patterns[2].matches("anything"));
    }

    #[test]
    fn sensitive_data_drops_malformed_entries() {
        let m = parse_manifest(serde_json::json!({
            "sensitiveData": [
                "ok", 42, null, "", "../escape", "a/b", "spaces here", "to*ken",
                "**", "a..b*", "x".repeat(129), "fine_*"
            ]
        }));
        assert_eq!(
            m.sensitive_data_patterns(),
            vec![
                SensitiveKeyPattern::Exact("ok".into()),
                SensitiveKeyPattern::Prefix("fine_".into()),
            ]
        );
    }

    #[test]
    fn sensitive_data_wrong_type_is_ignored_not_fatal() {
        // A malformed value must not make the manifest unreadable (that
        // would hide an installed extension that loads fine on old hosts).
        for raw in [
            serde_json::json!("creds"),
            serde_json::json!({ "creds": true }),
            serde_json::json!(true),
        ] {
            let m = parse_manifest(serde_json::json!({ "sensitiveData": raw }));
            assert!(m.sensitive_data_patterns().is_empty());
        }
    }

    #[test]
    fn sensitive_data_caps_at_32_entries() {
        let entries: Vec<String> = (0..40).map(|i| format!("key{}", i)).collect();
        let m = parse_manifest(serde_json::json!({ "sensitiveData": entries }));
        let patterns = m.sensitive_data_patterns();
        assert_eq!(patterns.len(), MAX_SENSITIVE_DATA_ENTRIES);
        assert_eq!(
            patterns.last(),
            Some(&SensitiveKeyPattern::Exact("key31".into()))
        );
    }

    #[test]
    fn sensitive_config_fields_need_a_boolean_true() {
        let m = parse_manifest(serde_json::json!({
            "config": {
                "api_key": { "type": "string", "default": "", "sensitive": true },
                "calendar_url": { "type": "string", "sensitive": true },
                "units": { "type": "string", "sensitive": false },
                "sloppy": { "type": "string", "sensitive": "yes" },
                "plain": { "type": "boolean", "default": true }
            }
        }));
        let mut fields: Vec<String> = m
            .config_field_decls()
            .into_iter()
            .filter(|d| d.sensitive)
            .map(|d| d.field)
            .collect();
        fields.sort();
        assert_eq!(fields, vec!["api_key", "calendar_url"]);
    }

    #[test]
    fn config_field_decls_read_types_flags_and_choices() {
        let m = parse_manifest(serde_json::json!({
            "config": {
                "units": { "type": "string", "default": "metric", "enum": ["metric", "imperial"] },
                "city": { "type": "string", "default": "", "backup": "plain" },
                "api_key": { "type": "string", "sensitive": true, "backup": "plain" },
                "badge": { "type": "boolean", "default": true },
                "limit": { "type": "number" },
                "odd": { "type": "date", "backup": "yes", "enum": "metric" },
                "big_enum": { "type": "string", "enum": (0..65).map(|i| i.to_string()).collect::<Vec<_>>() },
                "not_an_object": true
            }
        }));
        let mut decls = m.config_field_decls();
        decls.sort_by(|a, b| a.field.cmp(&b.field));
        let by = |f: &str| decls.iter().find(|d| d.field == f).unwrap();
        assert_eq!(decls.len(), 7, "a non-object entry is skipped");
        assert_eq!(
            by("units").choices,
            Some(vec![
                serde_json::json!("metric"),
                serde_json::json!("imperial")
            ])
        );
        assert!(by("city").plain && !by("city").sensitive);
        assert!(
            by("api_key").sensitive && !by("api_key").plain,
            "sensitive wins"
        );
        assert_eq!(by("badge").ty, Some(ConfigFieldType::Boolean));
        assert_eq!(by("limit").ty, Some(ConfigFieldType::Number));
        let odd = by("odd");
        assert_eq!(
            (odd.ty, odd.plain, odd.choices.clone()),
            (None, false, None)
        );
        assert_eq!(by("big_enum").choices, None, "too many choices");
    }

    #[test]
    fn safety_net_matches_the_contract_pattern() {
        for name in [
            "token",
            "refresh_token",
            "SECRET",
            "credentials",
            "creds",
            "password",
            "passwd",
            "api_key",
            "api-key",
            "apikey",
            "ApiKey",
            "auth",
            "oauth_state",
            "session",
            "cookie_jar",
            "passphrase",
            "pin",
            "wifi_pass",
            "db_pwd",
            "licenseKey",
            "API Key",
            "totp_seed",
            "AccountKey",
        ] {
            assert!(is_sensitive_name(name), "{} should match", name);
        }
        for name in [
            "todos",
            "bookmarks",
            "location",
            "watchlist",
            "units",
            "trigger",
            "pinned",
            "spinner",
            "bypass",
            "compass",
            "hotkey",
        ] {
            assert!(!is_sensitive_name(name), "{} should not match", name);
        }
    }

    #[test]
    fn sync_is_a_valid_capability() {
        assert!(VALID_CAPABILITIES.contains(&"sync"));
    }

    fn sync_manifest(sync: serde_json::Value) -> ExtensionManifest {
        parse_manifest(serde_json::json!({
            "permissions": ["storage", "sync"],
            "sensitiveData": ["creds", "auth_*"],
            "sync": sync
        }))
    }

    #[test]
    fn a_full_sync_block_normalizes() {
        let d = sync_manifest(serde_json::json!({
            "description": "__MSG_sync.description__",
            "schema": 2,
            "keys": ["prefs", "view-*"],
            "lists": { "todos": { "array": "items", "id": "id", "label": "title" },
                       "plain": { "array": "", "id": "key" } },
            "collections": {
                "items": {
                    "label": "title", "private": "private", "maxBytes": 65536,
                    "together": [["status", "doneAt"], ["title", "titleSource"]],
                    "sets": ["entityIds"], "maps": ["mentionOrigin"], "local": ["rev"]
                },
                "bodies": { "whole": true, "large": true, "maxBytes": 4194304 }
            }
        }))
        .sync_decl()
        .expect("a valid block");
        assert_eq!(d.description.as_deref(), Some("__MSG_sync.description__"));
        assert_eq!(d.schema, 2);
        assert!(d.key_syncs("prefs") && d.key_syncs("view-main") && !d.key_syncs("other"));
        assert!(
            !d.key_syncs("todos"),
            "a lists key is never a keys-mode key"
        );
        assert_eq!(d.lists["todos"].label.as_deref(), Some("title"));
        assert_eq!(d.lists["plain"].array, "");
        let items = &d.collections["items"];
        assert_eq!(items.together.len(), 2);
        assert_eq!(items.max_bytes, 65_536);
        assert!(!items.whole && !items.large);
        let bodies = &d.collections["bodies"];
        assert!(bodies.whole && bodies.large);
        assert_eq!(bodies.max_bytes, MAX_LARGE_RECORD_BYTES);
    }

    #[test]
    fn a_malformed_sync_part_is_ignored_and_the_rest_loads() {
        let d = sync_manifest(serde_json::json!({
            "description": "x".repeat(141),
            "schema": 0,
            "keys": ["prefs", "creds", "auth_token", "token", "bad key", 3, "ok-*"],
            "lists": {
                "todos": { "array": "items", "id": "id" },
                "nolist": { "array": "items" },
                "creds": { "array": "", "id": "id" },
                "../x": { "array": "", "id": "id" }
            },
            "collections": {
                "Items": {},
                "overlap": { "sets": ["a"], "local": ["a"] },
                "idfield": { "sets": ["id"] },
                "big": { "maxBytes": 2000000 },
                "bigok": { "maxBytes": 2000000, "large": true },
                "fine": {}
            }
        }))
        .sync_decl()
        .expect("the valid parts load");
        assert_eq!(d.description, None, "over 140 characters");
        assert_eq!(d.schema, 1);
        assert_eq!(
            d.keys,
            vec![
                SensitiveKeyPattern::Exact("prefs".into()),
                SensitiveKeyPattern::Prefix("ok-".into())
            ],
            "sensitive keys (declared or by the safety net) never sync"
        );
        assert_eq!(d.lists.keys().collect::<Vec<_>>(), ["todos"]);
        let colls: Vec<&String> = d.collections.keys().collect();
        assert_eq!(colls, ["bigok", "fine"]);
        assert_eq!(d.collections["fine"].max_bytes, DEFAULT_RECORD_BYTES);
    }

    #[test]
    fn a_sync_block_needs_the_sync_permission() {
        let m = parse_manifest(serde_json::json!({
            "permissions": ["storage"],
            "sync": { "keys": ["prefs"] }
        }));
        assert!(m.sync_decl().is_none(), "only settings sync without it");
        assert!(parse_manifest(serde_json::json!({})).sync_decl().is_none());
        let m = parse_manifest(serde_json::json!({
            "permissions": ["sync"],
            "sync": ["prefs"]
        }));
        assert!(m.sync_decl().is_none(), "not an object");
        let too_many: Vec<String> = (0..40).map(|i| format!("k{i}")).collect();
        let d = sync_manifest(serde_json::json!({ "keys": too_many }))
            .sync_decl()
            .unwrap();
        assert_eq!(d.keys.len(), MAX_SYNC_KEYS);
    }

    #[test]
    fn config_fields_read_their_sync_scope() {
        let m = parse_manifest(serde_json::json!({
            "config": {
                "a": { "type": "string", "sync": "shared" },
                "b": { "type": "string", "sync": "os" },
                "c": { "type": "boolean", "sync": "machine" },
                "d": { "type": "boolean", "sync": "never" },
                "e": { "type": "boolean", "sync": "everywhere" },
                "f": { "type": "boolean" }
            }
        }));
        let by = |f: &str| {
            m.config_field_decls()
                .into_iter()
                .find(|d| d.field == f)
                .unwrap()
                .sync
        };
        assert_eq!(by("a"), Some(SyncScopeDecl::Shared));
        assert_eq!(by("b"), Some(SyncScopeDecl::Os));
        assert_eq!(by("c"), Some(SyncScopeDecl::Machine));
        assert_eq!(by("d"), Some(SyncScopeDecl::Never));
        assert_eq!(by("e"), None, "an unknown value is ignored");
        assert_eq!(by("f"), None);
    }

    #[test]
    fn the_data_key_lock_is_shared_per_key_ignoring_case() {
        let a = data_key_lock("lock-test", "Prefs");
        let b = data_key_lock("lock-test", "prefs");
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        assert!(!std::sync::Arc::ptr_eq(
            &a,
            &data_key_lock("lock-test", "other")
        ));
    }

    #[test]
    fn manifest_round_trip_drops_no_known_keys() {
        let json = r#"{
            "id": "fixture", "name": "Fixture", "version": "1.0.0",
            "type": "extension", "description": "d", "icon": "i",
            "author": "kage", "preview": "p.png", "sandboxVendor": ["math"],
            "sensitiveData": ["creds"],
            "permissions": ["storage"], "commands": [],
            "config": {"enabled": true},
            "contributes": {
                "searchProvider": "./search.js", "settingsProvider": "./settings.js",
                "settingsModule": "./legacy.js", "css": ["style.css"],
                "widgets": [{"id": "w", "slot": "main", "module": "./w.js"}],
                "themes": {"dark": "dark.json", "light": "light.json"},
                "toolbarButtons": "./toolbar.js", "messageFormatters": "./fmt.js",
                "toolProvider": "./tools.js", "triggerProvider": "./triggers.js",
                "mascotActivities": ["music"],
                "views": [{"id": "explorer", "title": "Explorer", "module": "./explorer.js"}]
            }
        }"#;
        let input: serde_json::Value = serde_json::from_str(json).expect("parse input");
        let manifest: ExtensionManifest = serde_json::from_str(json).expect("parse manifest");
        let output = serde_json::to_value(&manifest).expect("serialize");
        for key in input.as_object().unwrap().keys() {
            assert!(output.as_object().unwrap().contains_key(key));
        }
        for key in input["contributes"].as_object().unwrap().keys() {
            assert!(output["contributes"].as_object().unwrap().contains_key(key));
        }
    }
}
