//! Extension and theme discovery, management, and store API.
//!
//! Extensions, themes, and command packs install under the per-user Kage
//! config directory. Store archives are verified by their caller, then
//! extracted and installed through the same path as local packages.

mod archive;
mod discovery;
mod install;

pub use archive::{extract_zip, install_from_zip};
pub use discovery::discover_items;
pub use install::{install_from_directory, uninstall};

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
    let config_dir = dirs::config_dir().context("Failed to get config directory")?;
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
    fn manifest_round_trip_drops_no_known_keys() {
        let json = r#"{
            "id": "fixture", "name": "Fixture", "version": "1.0.0",
            "type": "extension", "description": "d", "icon": "i",
            "author": "kage", "preview": "p.png", "sandboxVendor": ["math"],
            "permissions": ["storage"], "commands": [],
            "config": {"enabled": true},
            "contributes": {
                "searchProvider": "./search.js", "settingsProvider": "./settings.js",
                "settingsModule": "./legacy.js", "css": ["style.css"],
                "widgets": [{"id": "w", "slot": "main", "module": "./w.js"}],
                "themes": {"dark": "dark.json", "light": "light.json"},
                "toolbarButtons": "./toolbar.js", "messageFormatters": "./fmt.js",
                "toolProvider": "./tools.js", "triggerProvider": "./triggers.js"
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
