//! What extensions declare about their settings and data, the names import
//! matches against this machine's files, and the labels the backup UI shows.
//!
//! Nothing here decides what an unencrypted backup contains by looking for
//! secrets: a settings-only backup holds only what [`super::allowlist`]
//! names. [`looks_like_secret`] is defence in depth for those allowlisted
//! strings, nothing more.

use crate::config::Config;
use crate::extensions::{ConfigFieldDecl, ConfigFieldType, InstalledItem, SensitiveKeyPattern};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::LazyLock;

/// The placeholder an earlier ("leave out") build wrote where it left a
/// value out. Import keeps this machine's value wherever a backup still
/// carries it.
pub const PLACEHOLDER: &str = "__REMOVED_FROM_BACKUP__";

/// Prefix of the stand-in names an earlier build wrote for names that
/// looked like credentials (random per bundle, never matchable).
const OPAQUE_PREFIX: &str = "hidden:";

/// Whether `s` is one of an earlier build's stand-in names.
pub fn is_opaque(s: &str) -> bool {
    s.starts_with(OPAQUE_PREFIX)
}

/// An item an earlier build's "leave out" backup lists in its manifest
/// (`excluded`). Read only, to keep this machine's values for them and to
/// report what it couldn't keep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SensitiveRef {
    /// `extension-data/<extension_id>/<file>`.
    ExtensionData {
        extension_id: String,
        #[serde(default)]
        key: String,
        file: String,
    },
    /// `config.extensions[<extension_id>][<field>]`; an empty `field` is
    /// the extension's whole settings value.
    ExtensionConfig {
        extension_id: String,
        field: String,
    },
    /// One of Kage's own settings, as an RFC 6901 pointer into config.json.
    HostSetting {
        path: String,
    },
    /// Allowed or denied tool-permission rules.
    ToolPermissionRules,
    SteeringAuto,
    SteeringUser,
    /// A kind this build doesn't know. Always reported.
    #[serde(other)]
    Unknown,
}

impl SensitiveRef {
    /// The owning extension, or `None` for Kage's own settings.
    pub fn extension_id(&self) -> Option<&str> {
        match self {
            Self::ExtensionData { extension_id, .. }
            | Self::ExtensionConfig { extension_id, .. } => Some(extension_id),
            _ => None,
        }
    }
}

/// `/a/b~1c` → `["a", "b/c"]` (RFC 6901).
pub fn parse_pointer(p: &str) -> Vec<String> {
    p.split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect()
}

// --- Names -------------------------------------------------------------------

/// A name as APFS and NTFS compare it: Unicode case-folded (`ß` = `ss`,
/// `ſ` = `s`, `Spotify` = `spotify`) and NFC-normalised (a decomposed `é`
/// is the precomposed one). Used wherever an imported name is matched
/// against this machine's files or an extension's id.
pub fn fold(name: &str) -> String {
    use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
    let decomposed = DecomposingNormalizerBorrowed::new_nfd().normalize(name);
    // Upper then lower, twice, folds the special cases (`ß` → `SS` → `ss`,
    // `ẞ` → `ß` → `SS` → `ss`, `ς` → `Σ` → `σ`).
    let mut folded = decomposed.to_uppercase().to_lowercase();
    folded = folded.to_uppercase().to_lowercase();
    ComposingNormalizerBorrowed::new_nfc()
        .normalize(&folded)
        .into_owned()
}

/// Files an OS drops into any folder it shows (Finder, Explorer, KDE), and
/// an import's own temp and rollback files: never extension data, never
/// backed up.
pub fn is_os_junk(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let lower = name.to_ascii_lowercase();
    name.starts_with(super::TEMP_PREFIX)
        || name.starts_with("._")
        || matches!(
            lower.as_str(),
            ".ds_store" | "thumbs.db" | "ehthumbs.db" | "desktop.ini" | ".directory" | ".localized"
        )
        || name == "Icon\r"
}

// --- Defence in depth ----------------------------------------------------------

/// Well-known token and key formats, credential labels, credentials in URLs.
static SECRET_SHAPE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?x)
          (?:gh[poursa]|ghr)_[A-Za-z0-9]{16,}
        | github_pat_[A-Za-z0-9_]{16,}
        | glpat-[A-Za-z0-9_-]{16,}
        | xox[abposre]-[A-Za-z0-9-]{8,}
        | sk-(?:ant-|proj-|live_|test_)?[A-Za-z0-9_-]{16,}
        | (?:xai|gsk|whsec|dckr_pat|hvs|ya29|npm|pypi|shpat|rk_live|pk_live)[._-][A-Za-z0-9_.-]{12,}
        | AIza[0-9A-Za-z_-]{20,}
        | (?:AKIA|ASIA)[0-9A-Z]{16}
        | eyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}
        | -----BEGIN
        | AGE-SECRET-KEY-
        | PuTTY-User-Key-File
        | (?i:\b(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,})
        | (?i:(?:pass(?:word|wd|phrase|code)?|pwd|secret|token|api[-_\s]?key|auth|credential|private[-_\s]?key)\W{0,3}\s*[:=])
        | ://[^/\s:@]+:[^/\s@]*@
        | (?i:[?&;][^=&\s]*(?:token|key|secret|sig|signature|auth|pass|password|code|session)[^=&\s]*=)
        ",
    )
    .expect("SECRET_SHAPE must compile")
});

/// A long run of token characters mixing letters and digits.
static TOKEN_RUN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"[A-Za-z0-9+/=_.~-]{24,}").expect("TOKEN_RUN must compile")
});

/// Whether an allowlisted string looks like it holds a password, token or
/// key. Defence in depth only: the allowlist decides what an unencrypted
/// backup holds; a value flagged here is left out of it as well.
pub fn looks_like_secret(s: &str) -> bool {
    if s.contains(PLACEHOLDER) || SECRET_SHAPE.is_match(s) {
        return true;
    }
    TOKEN_RUN.find_iter(s).any(|m| {
        let run = m.as_str();
        run.bytes().any(|b| b.is_ascii_digit()) && run.bytes().any(|b| b.is_ascii_alphabetic())
    })
}

// --- Manifest rules ------------------------------------------------------------

/// What the installed manifests say about one extension id.
#[derive(Clone, Default)]
struct ExtensionMeta {
    patterns: Vec<SensitiveKeyPattern>,
    /// `config` entries by field, one per manifest that declares it (every
    /// kind: used to make a setting stricter).
    fields: HashMap<String, Vec<ConfigFieldDecl>>,
    /// The same, from installed *extensions* only: only these can make a
    /// setting plain.
    extension_fields: HashMap<String, Vec<ConfigFieldDecl>>,
    /// How many installed extension folders have this id.
    extension_items: usize,
    /// An installed *extension* has this id (a theme runs no code and can't
    /// stand in for its manifest).
    extension: bool,
    /// An installed theme has this id.
    theme: bool,
    /// The installed items' versions.
    versions: Vec<String>,
}

/// The installed manifests' declarations, read once, keyed by folded id.
/// Every item claiming an id (an extension and a theme, or two folders —
/// see `extensions::discover_all_items`) adds its declarations. For a
/// setting to go into an unencrypted backup, every installed extension
/// folder for the id must declare and allow it, and no other item claiming
/// the id may forbid
/// it.
#[derive(Clone, Default)]
pub struct Rules {
    meta: HashMap<String, ExtensionMeta>,
}

/// Whether an extension setting may go into an unencrypted backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plain {
    /// Yes.
    Include,
    /// Its manifest allows it, but the value looks like a secret (or is
    /// too long): left out, and the user is told.
    Flagged,
    /// No: undeclared, sensitive, free-form, the wrong type, or no manifest.
    No,
}

/// Longest string an unencrypted backup copies for an extension setting.
pub const MAX_PLAIN_STRING: usize = 512;

impl Rules {
    pub fn new(installed: &[InstalledItem]) -> Self {
        let mut meta: HashMap<String, ExtensionMeta> = HashMap::new();
        for item in installed {
            let entry = meta.entry(fold(&item.manifest.id)).or_default();
            let is_extension = item.manifest.kind == "extension";
            entry.extension |= is_extension;
            entry.extension_items += usize::from(is_extension);
            entry.theme |= item.manifest.kind == "theme";
            if !entry.versions.contains(&item.manifest.version) {
                entry.versions.push(item.manifest.version.clone());
            }
            for p in item.manifest.sensitive_data_patterns() {
                if !entry.patterns.contains(&p) {
                    entry.patterns.push(p);
                }
            }
            for decl in item.manifest.config_field_decls() {
                if is_extension {
                    entry
                        .extension_fields
                        .entry(decl.field.clone())
                        .or_default()
                        .push(decl.clone());
                }
                entry
                    .fields
                    .entry(decl.field.clone())
                    .or_default()
                    .push(decl);
            }
        }
        Self { meta }
    }

    fn get(&self, id: &str) -> Option<&ExtensionMeta> {
        self.meta.get(&fold(id))
    }

    /// Whether an installed extension has this id, so what it declares is
    /// known.
    pub fn is_installed(&self, id: &str) -> bool {
        !id.is_empty() && self.get(id).is_some_and(|m| m.extension)
    }

    /// Whether any installed item (extension, theme, command pack) has
    /// this exact id.
    pub fn is_installed_item(&self, id: &str) -> bool {
        self.meta.contains_key(&fold(id)) && fold(id) == id
    }

    /// Whether an installed theme has this exact id.
    pub fn is_installed_theme(&self, id: &str) -> bool {
        self.is_installed_item(id) && self.get(id).is_some_and(|m| m.theme)
    }

    /// Whether an installed item with this id has this version.
    pub fn has_version(&self, id: &str, version: &str) -> bool {
        self.is_installed_item(id)
            && self
                .get(id)
                .is_some_and(|m| m.versions.iter().any(|v| v == version))
    }

    /// Whether a top-level setting is one whose value grants access: a
    /// manifest marks it `sensitive: true`, or its name matches the safety
    /// net ([`crate::extensions::is_sensitive_name`]).
    pub fn is_sensitive_setting(&self, id: &str, field: &str) -> bool {
        crate::extensions::is_sensitive_name(field)
            || self.get(id).is_some_and(|m| {
                m.fields
                    .get(field)
                    .is_some_and(|decls| decls.iter().any(|d| d.sensitive))
            })
    }

    /// Whether a data file (by storage key, `file` minus `.json`) is one a
    /// manifest declares in `sensitiveData`, or whose name matches the
    /// safety net.
    pub fn is_sensitive_data(&self, id: &str, key: &str) -> bool {
        crate::extensions::is_sensitive_name(key)
            || self
                .get(id)
                .is_some_and(|m| m.patterns.iter().any(|p| p.matches(key)))
    }

    /// Whether extension `id`'s top-level setting `field` with `value` may
    /// go into an unencrypted settings-only backup: the extension is
    /// installed, every installed extension folder for the id declares the
    /// field, no manifest of any kind claiming the id marks it sensitive or
    /// refuses the value, and the value is a boolean, a number, one of a
    /// declared `enum`, or a string the manifest marks `backup: "plain"`.
    /// A theme (or other item) sharing the id can only make a setting
    /// stricter. A field named like a credential (the safety net, or a
    /// `sensitiveData` pattern of the extension's), or whose name itself
    /// looks like a secret, is never plain, whatever its declared type:
    /// that check comes before any declared shape is looked at.
    pub fn plain_setting(&self, id: &str, field: &str, value: &serde_json::Value) -> Plain {
        if is_host_managed(field) || !self.is_installed(id) {
            return Plain::No;
        }
        if self.is_sensitive_setting(id, field)
            || self.is_sensitive_data(id, field)
            || looks_like_secret(field)
        {
            return Plain::No;
        }
        let Some(meta) = self.get(id) else {
            return Plain::No;
        };
        let Some(own) = meta.extension_fields.get(field) else {
            return Plain::No;
        };
        if own.len() != meta.extension_items {
            return Plain::No;
        }
        let Some(decls) = meta.fields.get(field) else {
            return Plain::No;
        };
        if decls.is_empty() || decls.iter().any(|d| d.sensitive) {
            return Plain::No;
        }
        let mut verdict = Plain::Include;
        for decl in decls {
            match decl_allows(decl, value) {
                Plain::No => return Plain::No,
                Plain::Flagged => verdict = Plain::Flagged,
                Plain::Include => {}
            }
        }
        verdict
    }
}

/// Saved setting keys the host owns: scratch (`__`) values and the legacy
/// `enabled` switch.
pub fn is_host_managed(field: &str) -> bool {
    field.starts_with("__") || field == "enabled"
}

/// One manifest entry's verdict on `value` (see [`Rules::plain_setting`],
/// which has already refused a field named like a credential). Every
/// string it accepts, a declared choice included, is checked by
/// [`looks_like_secret`].
fn decl_allows(decl: &ConfigFieldDecl, value: &serde_json::Value) -> Plain {
    if crate::extensions::is_sensitive_name(&decl.field) {
        return Plain::No;
    }
    let accepted = match &decl.choices {
        Some(choices) => choices.iter().any(|c| c == value),
        None => {
            matches!(
                (decl.ty, value),
                (Some(ConfigFieldType::Boolean), serde_json::Value::Bool(_))
                    | (Some(ConfigFieldType::Number), serde_json::Value::Number(_))
            ) || (decl.plain
                && matches!(
                    (decl.ty, value),
                    (Some(ConfigFieldType::String), serde_json::Value::String(_))
                ))
        }
    };
    if !accepted {
        return Plain::No;
    }
    match value {
        serde_json::Value::String(s)
            if s.chars().count() > MAX_PLAIN_STRING || looks_like_secret(s) =>
        {
            Plain::Flagged
        }
        _ => Plain::Include,
    }
}

// --- Labels ------------------------------------------------------------------

/// Bidi-isolate a user-supplied name inside a localised sentence (FSI …
/// PDI). A name that looks like a credential, or an earlier build's
/// stand-in, is never shown.
pub fn isolate(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() || looks_like_secret(name) || is_opaque(name) {
        format!(
            "\u{2068}{}\u{2069}",
            crate::t!("settings.about.backup.sensitive.unnamed")
        )
    } else {
        format!("\u{2068}{}\u{2069}", name)
    }
}

/// A one-line, value-free description of a Kage setting an import couldn't
/// fill (it still holds an earlier build's placeholder), in the active
/// language. Names come from `config` (the merged one).
pub fn host_label(path: &[String], config: &Config) -> String {
    let p: Vec<&str> = path.iter().map(String::as_str).collect();
    let at = |s: &str| s.parse::<usize>().ok();
    let connection = |i: &str| {
        isolate(
            at(i)
                .and_then(|i| config.acp.connections.get(i))
                .map_or("", |c| c.name.as_str()),
        )
    };
    let shortcut = |i: &str| {
        isolate(at(i).and_then(|i| config.shortcuts.get(i)).map_or("", |s| {
            if s.name.trim().is_empty() {
                s.shortcut.as_str()
            } else {
                s.name.as_str()
            }
        }))
    };
    let automation = |i: &str| {
        isolate(
            at(i)
                .and_then(|i| config.macros.get(i))
                .map_or("", |m| m.name.as_str()),
        )
    };
    let step = |j: &str| at(j).map(|j| (j + 1).to_string()).unwrap_or_default();
    let site = |i: &str| {
        isolate(
            at(i)
                .and_then(|i| config.internal_sites.get(i))
                .map_or("", |s| s.name.as_str()),
        )
    };
    match p.as_slice() {
        ["acp", "connections", i, "mode", "spawn_command"] => crate::t!(
            "settings.about.backup.sensitive.item.agent_command",
            "name" => &connection(i)
        ),
        ["acp", "connections", i, "ollama_settings", "base_url"] => crate::t!(
            "settings.about.backup.sensitive.item.agent_ollama_url",
            "name" => &connection(i)
        ),
        ["acp", "connections", i, ..] => crate::t!(
            "settings.about.backup.sensitive.item.agent_connection",
            "name" => &connection(i)
        ),
        ["store_url"] => crate::t!("settings.about.backup.sensitive.item.store_url"),
        ["store_sources", i, ..] => crate::t!(
            "settings.about.backup.sensitive.item.store_source_url",
            "name" => &isolate(
                at(i)
                    .and_then(|i| config.store_sources.get(i))
                    .map_or("", |s| s.name.as_str())
            )
        ),
        ["shortcuts", i, "url"] => crate::t!(
            "settings.about.backup.sensitive.item.shortcut_url",
            "name" => &shortcut(i)
        ),
        ["shortcuts", i, "arguments"] => crate::t!(
            "settings.about.backup.sensitive.item.shortcut_arguments",
            "name" => &shortcut(i)
        ),
        ["shortcuts", i, "script"] => crate::t!(
            "settings.about.backup.sensitive.item.shortcut_script",
            "name" => &shortcut(i)
        ),
        ["shortcuts", i, "prompt"] => crate::t!(
            "settings.about.backup.sensitive.item.shortcut_prompt",
            "name" => &shortcut(i)
        ),
        ["shortcuts", i, ..] => crate::t!(
            "settings.about.backup.sensitive.item.shortcut",
            "name" => &shortcut(i)
        ),
        ["macros", i, "steps", j, "script"] => crate::t!(
            "settings.about.backup.sensitive.item.macro_step_script",
            "name" => &automation(i),
            "step" => &step(j)
        ),
        ["macros", i, "steps", j, "prompt"] => crate::t!(
            "settings.about.backup.sensitive.item.macro_step_prompt",
            "name" => &automation(i),
            "step" => &step(j)
        ),
        ["macros", i, "steps", j, ..] => crate::t!(
            "settings.about.backup.sensitive.item.macro_step",
            "name" => &automation(i),
            "step" => &step(j)
        ),
        ["macros", i, ..] => crate::t!(
            "settings.about.backup.sensitive.item.macro",
            "name" => &automation(i)
        ),
        ["context_rules", i, rest @ ..] => {
            let name = isolate(
                at(i)
                    .and_then(|i| config.context_rules.get(i))
                    .map_or("", |r| r.friendly_name.as_str()),
            );
            if rest == ["steering"] {
                crate::t!(
                    "settings.about.backup.sensitive.item.context_rule_steering",
                    "name" => &name
                )
            } else {
                crate::t!(
                    "settings.about.backup.sensitive.item.context_rule",
                    "name" => &name
                )
            }
        }
        ["internal_sites", i, "proxy", "script"] => crate::t!(
            "settings.about.backup.sensitive.item.internal_site_script",
            "name" => &site(i)
        ),
        ["internal_sites", i, "sample_url"] => crate::t!(
            "settings.about.backup.sensitive.item.internal_site_sample_url",
            "name" => &site(i)
        ),
        ["internal_sites", i, ..] => crate::t!(
            "settings.about.backup.sensitive.item.internal_site",
            "name" => &site(i)
        ),
        ["quick_actions", "custom_actions", i, ..] => crate::t!(
            "settings.about.backup.sensitive.item.quick_action",
            "name" => &isolate(
                at(i)
                    .and_then(|i| config.quick_actions.custom_actions.get(i))
                    .map_or("", |a| a.label.as_str())
            )
        ),
        ["tool_permissions", ..] => {
            crate::t!("settings.about.backup.sensitive.item.tool_permission_rules")
        }
        _ => setting_path_label(path),
    }
}

/// "Setting “ui › theme”", hiding any segment that looks like a credential.
pub fn setting_path_label(path: &[String]) -> String {
    let shown: Vec<String> = path
        .iter()
        .map(|s| {
            if is_opaque(s) || looks_like_secret(s) {
                "…".to_string()
            } else {
                s.clone()
            }
        })
        .collect();
    crate::t!(
        "settings.about.backup.sensitive.item.host_setting",
        "path" => &format!("\u{2066}{}\u{2069}", shown.join(" › "))
    )
}

/// The label for one of an earlier build's `excluded` entries that isn't an
/// extension's (those are reported by extension name).
pub fn excluded_label(item: &SensitiveRef, config: &Config) -> String {
    match item {
        SensitiveRef::HostSetting { path } => host_label(&parse_pointer(path), config),
        SensitiveRef::ToolPermissionRules => {
            crate::t!("settings.about.backup.sensitive.item.tool_permission_rules")
        }
        SensitiveRef::SteeringAuto => {
            crate::t!("settings.about.backup.sensitive.item.steering_auto")
        }
        SensitiveRef::SteeringUser => {
            crate::t!("settings.about.backup.sensitive.item.steering_user")
        }
        _ => crate::t!("settings.about.backup.sensitive.item.unknown"),
    }
}

/// An installed extension's name, with a `__MSG_name__` token resolved from
/// its `_locales/` (active language, then its base language, then `en`).
/// Falls back to the id rather than show a raw token.
pub fn display_name(item: &InstalledItem) -> String {
    let id = &item.manifest.id;
    let name = item.manifest.name.trim();
    let Some(key) = name
        .strip_prefix("__MSG_")
        .and_then(|s| s.strip_suffix("__"))
    else {
        return if name.is_empty() {
            id.clone()
        } else {
            name.to_string()
        };
    };
    let lang = crate::i18n::active_language();
    let mut candidates = vec![lang.clone()];
    if let Some((stem, _)) = lang.split_once('-') {
        candidates.push(stem.to_string());
    }
    candidates.push("en".to_string());
    let locales = PathBuf::from(&item.path).join("_locales");
    for cand in candidates {
        if !cand.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(locales.join(&cand).join("messages.json")) else {
            continue;
        };
        let Ok(catalog) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(message) = catalog
            .get(key)
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .filter(|m| !m.trim().is_empty())
        {
            return message.to_string();
        }
    }
    id.clone()
}

/// Installed extensions' display names, by id (the first item for an id).
pub fn display_names(installed: &[InstalledItem]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for item in installed {
        names
            .entry(item.manifest.id.clone())
            .or_insert_with(|| display_name(item));
    }
    names
}

/// The name to show for an extension id: installed name, then the name a
/// bundle recorded, then the id. Unknown, empty and stand-in ids get a
/// generic group name.
pub fn extension_label(
    id: &str,
    installed: &HashMap<String, String>,
    recorded: &BTreeMap<String, String>,
) -> String {
    if id.is_empty() || is_opaque(id) || looks_like_secret(id) {
        return crate::t!("settings.about.backup.sensitive.group.other");
    }
    installed
        .get(id)
        .or_else(|| recorded.get(id))
        .filter(|n| !n.trim().is_empty() && !looks_like_secret(n))
        .cloned()
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn installed(manifest: Value) -> InstalledItem {
        InstalledItem {
            manifest: serde_json::from_value(manifest).unwrap(),
            path: String::new(),
            enabled: true,
        }
    }

    fn weather() -> InstalledItem {
        installed(json!({
            "id": "weather", "name": "Weather", "version": "1.0.0", "type": "extension",
            "sensitiveData": ["creds"],
            "config": {
                "show_badge": { "type": "boolean", "default": true },
                "limit": { "type": "number", "default": 10 },
                "units": { "type": "string", "default": "metric", "enum": ["metric", "imperial"] },
                "city": { "type": "string", "default": "", "backup": "plain" },
                "note": { "type": "string", "default": "" },
                "api_key": { "type": "string", "default": "", "sensitive": true },
                "pin": { "type": "number", "default": 0 },
                "token_label": { "type": "string", "default": "", "backup": "plain" },
                "both": { "type": "string", "default": "", "sensitive": true, "backup": "plain" },
                "nested": { "type": "string", "default": "", "backup": "plain" }
            }
        }))
    }

    #[test]
    fn only_declared_scalar_settings_are_plain() {
        let rules = Rules::new(&[weather()]);
        let ok = |f: &str, v: Value| rules.plain_setting("weather", f, &v);
        assert_eq!(ok("show_badge", json!(false)), Plain::Include);
        assert_eq!(ok("limit", json!(25)), Plain::Include);
        assert_eq!(ok("units", json!("imperial")), Plain::Include);
        assert_eq!(
            ok("units", json!("kelvin")),
            Plain::No,
            "not a declared choice"
        );
        assert_eq!(ok("city", json!("Lisbon")), Plain::Include);
        assert_eq!(
            ok("note", json!("free text")),
            Plain::No,
            "free-form, no flag"
        );
        assert_eq!(ok("api_key", json!("k")), Plain::No);
        assert_eq!(ok("both", json!("x")), Plain::No, "sensitive wins");
        assert_eq!(
            ok("pin", json!(1234)),
            Plain::No,
            "a number named like a PIN"
        );
        assert_eq!(
            ok("token_label", json!("x")),
            Plain::No,
            "named like a credential"
        );
        assert_eq!(ok("undeclared", json!(true)), Plain::No);
        assert_eq!(ok("limit", json!("25")), Plain::No, "wrong type");
        assert_eq!(ok("nested", json!({ "a": 1 })), Plain::No);
        assert_eq!(ok("__scratch", json!(true)), Plain::No);
        assert_eq!(
            ok("city", json!("ghp_aaaaaaaaaaaaaaaaaaaaaaaa")),
            Plain::Flagged
        );
        assert_eq!(ok("city", json!("x".repeat(600))), Plain::Flagged);
        assert_eq!(
            rules.plain_setting("not-installed", "show_badge", &json!(true)),
            Plain::No
        );
    }

    #[test]
    fn a_credential_named_field_is_never_plain_whatever_its_declared_shape() {
        // The review's probes: `api_key` declared as an enum holding a
        // token, a numeric `pin` enum, and the same names as plain types.
        let token = "ghp_0123456789abcdefABCDEF0123";
        let rules = Rules::new(&[installed(json!({
            "id": "vault", "name": "Vault", "version": "1.0.0", "type": "extension",
            "sensitiveData": ["creds", "acct_*"],
            "config": {
                "api_key": { "type": "string", "enum": [token, "none"] },
                "pin": { "type": "number", "enum": [1234, 0] },
                "keep_session": { "type": "boolean" },
                "auth_method": { "type": "string", "enum": ["oauth", "pat"] },
                "creds": { "type": "boolean" },
                "acct_id": { "type": "number" },
                "mode": { "type": "string", "enum": [token, "fast"] },
                "ghp_0123456789abcdefABCDEF0123": { "type": "boolean" },
                "units": { "type": "string", "enum": ["metric", "imperial"] }
            }
        }))]);
        let plain = |f: &str, v: Value| rules.plain_setting("vault", f, &v);
        assert_eq!(plain("api_key", json!(token)), Plain::No);
        assert_eq!(plain("api_key", json!("none")), Plain::No);
        assert_eq!(plain("pin", json!(1234)), Plain::No);
        assert_eq!(plain("keep_session", json!(true)), Plain::No);
        assert_eq!(plain("auth_method", json!("oauth")), Plain::No);
        // Named like one of the extension's own sensitiveData keys.
        assert_eq!(plain("creds", json!(true)), Plain::No);
        assert_eq!(plain("acct_id", json!(7)), Plain::No);
        // A field whose name is itself a token.
        assert_eq!(
            plain("ghp_0123456789abcdefABCDEF0123", json!(true)),
            Plain::No
        );
        // A declared choice that looks like a secret is left out, flagged.
        assert_eq!(plain("mode", json!(token)), Plain::Flagged);
        assert_eq!(plain("mode", json!("fast")), Plain::Include);
        assert_eq!(plain("units", json!("metric")), Plain::Include);
    }

    #[test]
    fn every_manifest_for_an_id_must_allow_a_plain_setting() {
        let theme = installed(json!({
            "id": "weather", "name": "Weather theme", "version": "1.0.0", "type": "theme",
            "config": { "city": { "type": "string", "sensitive": true } }
        }));
        let rules = Rules::new(&[weather(), theme]);
        assert_eq!(
            rules.plain_setting("weather", "city", &json!("Lisbon")),
            Plain::No
        );
        assert!(rules.is_sensitive_setting("weather", "city"));
        // A theme alone can't make an extension's settings plain.
        let only_theme = installed(json!({
            "id": "skin", "name": "Skin", "version": "1.0.0", "type": "theme",
            "config": { "flag": { "type": "boolean" } }
        }));
        let rules = Rules::new(&[only_theme]);
        assert_eq!(rules.plain_setting("skin", "flag", &json!(true)), Plain::No);
        // A theme can't make a setting the extension doesn't declare plain.
        let notes = installed(json!({
            "id": "notes", "name": "Notes", "version": "1.0.0", "type": "extension"
        }));
        let permissive_theme = installed(json!({
            "id": "notes", "name": "Notes theme", "version": "1.0.0", "type": "theme",
            "config": { "draft": { "type": "string", "backup": "plain" },
                        "account_number": { "type": "number" } }
        }));
        let rules = Rules::new(&[notes.clone(), permissive_theme]);
        assert_eq!(
            rules.plain_setting("notes", "draft", &json!("private")),
            Plain::No
        );
        assert_eq!(
            rules.plain_setting("notes", "account_number", &json!(1)),
            Plain::No
        );
        // Nor can a second extension folder for the id, while one doesn't.
        let second = installed(json!({
            "id": "notes", "name": "Notes copy", "version": "1.0.1", "type": "extension",
            "config": { "draft": { "type": "string", "backup": "plain" } }
        }));
        let rules = Rules::new(&[notes, second.clone()]);
        assert_eq!(
            rules.plain_setting("notes", "draft", &json!("private")),
            Plain::No
        );
        let rules = Rules::new(&[second.clone(), second]);
        assert_eq!(
            rules.plain_setting("notes", "draft", &json!("ok")),
            Plain::Include
        );
    }

    #[test]
    fn sensitive_data_and_settings_follow_declarations_and_the_safety_net() {
        let rules = Rules::new(&[weather()]);
        assert!(rules.is_sensitive_data("weather", "creds"));
        assert!(rules.is_sensitive_data("weather", "refresh_token"));
        assert!(!rules.is_sensitive_data("weather", "forecast"));
        assert!(rules.is_sensitive_setting("weather", "api_key"));
        assert!(rules.is_sensitive_setting("anything", "session_cookie"));
        assert!(!rules.is_sensitive_setting("weather", "units"));
    }

    #[test]
    fn the_defence_in_depth_scanner_catches_the_reviewed_shapes() {
        for s in [
            "ghp_0123456789abcdefABCDEF0123",
            "sk-ant-api03-abcdefghijklmnop",
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQ",
            "PuTTY-User-Key-File-3: ssh-ed25519",
            "xai-abcdefghijklmnopqrstu",
            "gsk_abcdefghijklmnopqrstu",
            "whsec_abcdefghijklmnopqrstu",
            "dckr_pat_abcdefghijklmnop",
            "hvs.CAESIabcdefghijklmnop",
            "https://me:$PASSWORD123@store.example.com/x.json",
            "https://store.example.com/x.json?auth[token]=Qz8vRt2LmN",
            "password: sunshinebeachwaves",
            "**Password:** Qz8vRt2LmN",
            "Bearer Qz8vRt2LmNabcdef",
            "Qz8vRt2LmNQz8vRt2LmNQz8vRt2LmN",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcdefghij",
        ] {
            assert!(looks_like_secret(s), "{}", s);
        }
        for s in [
            "system",
            "dark",
            "catppuccin-mocha",
            "en",
            "pt-BR",
            "HH:mm",
            "ddd, MMM D",
            "Lisbon",
            "San Francisco",
            "WindowsTerminal",
            "Space",
        ] {
            assert!(!looks_like_secret(s), "{}", s);
        }
    }

    #[test]
    fn names_that_look_like_credentials_are_never_shown() {
        assert!(isolate("ghp_0123456789abcdefABCDEF0123")
            .contains(&crate::t!("settings.about.backup.sensitive.unnamed")));
        assert!(isolate("Work agent").contains("Work agent"));
        let names = HashMap::new();
        assert_eq!(
            extension_label("hidden:abc", &names, &BTreeMap::new()),
            crate::t!("settings.about.backup.sensitive.group.other")
        );
        assert_eq!(extension_label("todos", &names, &BTreeMap::new()), "todos");
    }

    #[test]
    fn display_name_resolves_msg_tokens_and_falls_back_to_id() {
        let dir = tempfile::tempdir().unwrap();
        let en = dir.path().join("_locales").join("en");
        std::fs::create_dir_all(&en).unwrap();
        std::fs::write(
            en.join("messages.json"),
            r#"{"extName":{"message":"Weather Pro"}}"#,
        )
        .unwrap();
        let mut item = installed(json!({
            "id": "weather", "name": "__MSG_extName__", "version": "1.0.0", "type": "extension"
        }));
        item.path = dir.path().to_string_lossy().into_owned();
        assert_eq!(display_name(&item), "Weather Pro");
        item.manifest.name = "__MSG_missing__".into();
        assert_eq!(display_name(&item), "weather");
    }

    #[test]
    fn earlier_excluded_entries_parse_and_unknown_kinds_survive() {
        let v: Vec<SensitiveRef> = serde_json::from_value(json!([
            { "kind": "extension_data", "extension_id": "github", "key": "token", "file": "token.json" },
            { "kind": "extension_config", "extension_id": "github", "field": "personal_token" },
            { "kind": "host_setting", "path": "/acp/connections/0/mode/spawn_command" },
            { "kind": "tool_permission_rules" },
            { "kind": "steering_user" },
            { "kind": "something_new", "x": 1 }
        ]))
        .unwrap();
        assert_eq!(v.len(), 6);
        assert_eq!(v[5], SensitiveRef::Unknown);
        assert_eq!(v[0].extension_id(), Some("github"));
        assert_eq!(
            parse_pointer("/acp/connections/0/a~1b"),
            vec!["acp", "connections", "0", "a/b"]
        );
    }

    #[test]
    fn os_clutter_and_import_temps_are_junk() {
        for junk in [
            ".DS_Store",
            "ext/Thumbs.db",
            "ext/._notes.json",
            "ext/.kage-import.1-2.tmp",
        ] {
            assert!(is_os_junk(junk), "{}", junk);
        }
        assert!(!is_os_junk("ext/notes.json"));
    }

    #[test]
    fn fold_matches_the_way_apfs_and_ntfs_compare_names() {
        assert_eq!(fold("Github/ſtate.json"), fold("github/state.json"));
        assert_eq!(fold("Straße"), fold("STRASSE"));
        assert_eq!(fold("e\u{301}"), fold("\u{e9}"));
    }
}
