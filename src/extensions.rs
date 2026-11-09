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
        out.push(ConfigFieldDecl {
            field: field.clone(),
            ty,
            sensitive,
            plain: plain && !sensitive,
            choices,
        });
    }
    out
}

/// Most values a manifest `config.<field>.enum` may list.
pub const MAX_CONFIG_ENUM_VALUES: usize = 64;
/// Longest string value a manifest `config.<field>.enum` may list.
pub const MAX_CONFIG_ENUM_LEN: usize = 128;

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
