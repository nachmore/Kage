//! Extension and theme discovery, management, and store API.
//!
//! Extensions live in two locations:
//! - Bundled: `<app_resource_dir>/extensions/` (read-only, ships with app)
//! - User:    `<config_dir>/kiro-assistant/extensions/` (user-installed)
//!
//! Themes live similarly:
//! - Bundled: `<app_resource_dir>/themes/`
//! - User:    `<config_dir>/kiro-assistant/themes/`
//!
//! User-installed items take precedence over bundled ones with the same ID.

use anyhow::{Context, Result};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Manifest types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub kind: String, // "extension" | "theme" | "commands"
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
    /// For command packs: the commands themselves
    #[serde(default)]
    pub commands: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionContributes {
    #[serde(default)]
    pub search_provider: Option<String>,
    #[serde(default)]
    pub settings_module: Option<String>,
    #[serde(default)]
    pub css: Option<Vec<String>>,
    #[serde(default)]
    pub widgets: Option<Vec<WidgetContribution>>,
    #[serde(default)]
    pub themes: Option<ThemeContributes>,
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

/// Runtime info about a discovered extension/theme, including where it lives on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledItem {
    pub manifest: ExtensionManifest,
    /// Absolute path to the item's directory
    pub path: String,
    /// Whether this is a bundled (read-only) item or user-installed
    pub bundled: bool,
    /// Whether the user has enabled this item (default true)
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Store catalog types (matches the mock API shape)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct StoreCatalogResponse {
    pub items: Vec<StoreCatalogItem>,
    pub total: u32,
    pub page: u32,
    #[serde(rename = "pageSize")]
    pub page_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct StoreCatalogItem {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub downloads: u32,
    #[serde(default)]
    pub rating: f32,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct StoreCatalogDetail {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub readme: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub downloads: u32,
    #[serde(default)]
    pub rating: f32,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub manifest: Option<serde_json::Value>,
    #[serde(default)]
    pub size: u64,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Directory helpers
// ---------------------------------------------------------------------------

/// Get the user extensions directory: `<config_dir>/kiro-assistant/extensions/`
pub fn user_extensions_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Failed to get config directory")?;
    Ok(config_dir.join("kiro-assistant").join("extensions"))
}

/// Get the user themes directory: `<config_dir>/kiro-assistant/themes/`
pub fn user_themes_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Failed to get config directory")?;
    Ok(config_dir.join("kiro-assistant").join("themes"))
}

/// Get the user command packs directory: `<config_dir>/kiro-assistant/command-packs/`
pub fn user_command_packs_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Failed to get config directory")?;
    Ok(config_dir.join("kiro-assistant").join("command-packs"))
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Scan a directory for manifest.json files, returning discovered items.
fn scan_directory(dir: &PathBuf, bundled: bool, enabled_states: &HashMap<String, bool>) -> Vec<InstalledItem> {
    let mut items = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return items,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        match fs::read_to_string(&manifest_path) {
            Ok(content) => match serde_json::from_str::<ExtensionManifest>(&content) {
                Ok(manifest) => {
                    let enabled = enabled_states
                        .get(&manifest.id)
                        .copied()
                        .unwrap_or(true);
                    items.push(InstalledItem {
                        path: path.to_string_lossy().to_string(),
                        bundled,
                        enabled,
                        manifest,
                    });
                }
                Err(e) => {
                    warn!("Invalid manifest at {:?}: {}", manifest_path, e);
                }
            },
            Err(e) => {
                warn!("Failed to read {:?}: {}", manifest_path, e);
            }
        }
    }
    items
}

/// Discover all installed extensions (bundled + user). User items override bundled by ID.
pub fn discover_extensions(
    bundled_dir: Option<&PathBuf>,
    enabled_states: &HashMap<String, bool>,
) -> Vec<InstalledItem> {
    let mut by_id: HashMap<String, InstalledItem> = HashMap::new();

    // Bundled first (lower priority)
    if let Some(dir) = bundled_dir {
        for item in scan_directory(dir, true, enabled_states) {
            by_id.insert(item.manifest.id.clone(), item);
        }
    }

    // User extensions override bundled
    if let Ok(user_dir) = user_extensions_dir() {
        for item in scan_directory(&user_dir, false, enabled_states) {
            by_id.insert(item.manifest.id.clone(), item);
        }
    }

    let mut items: Vec<InstalledItem> = by_id.into_values().collect();
    items.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    items
}

/// Discover all installed themes (bundled + user). User items override bundled by ID.
pub fn discover_themes(
    bundled_dir: Option<&PathBuf>,
    enabled_states: &HashMap<String, bool>,
) -> Vec<InstalledItem> {
    let mut by_id: HashMap<String, InstalledItem> = HashMap::new();

    if let Some(dir) = bundled_dir {
        for item in scan_directory(dir, true, enabled_states) {
            by_id.insert(item.manifest.id.clone(), item);
        }
    }

    if let Ok(user_dir) = user_themes_dir() {
        for item in scan_directory(&user_dir, false, enabled_states) {
            by_id.insert(item.manifest.id.clone(), item);
        }
    }

    let mut items: Vec<InstalledItem> = by_id.into_values().collect();
    items.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    items
}

/// Discover all installed command packs.
pub fn discover_command_packs(
    bundled_dir: Option<&PathBuf>,
    enabled_states: &HashMap<String, bool>,
) -> Vec<InstalledItem> {
    let mut by_id: HashMap<String, InstalledItem> = HashMap::new();

    if let Some(dir) = bundled_dir {
        for item in scan_directory(dir, true, enabled_states) {
            by_id.insert(item.manifest.id.clone(), item);
        }
    }

    if let Ok(user_dir) = user_command_packs_dir() {
        for item in scan_directory(&user_dir, false, enabled_states) {
            by_id.insert(item.manifest.id.clone(), item);
        }
    }

    let mut items: Vec<InstalledItem> = by_id.into_values().collect();
    items.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    items
}

// ---------------------------------------------------------------------------
// Installation / Uninstallation
// ---------------------------------------------------------------------------

/// Install an extension/theme/command-pack from a downloaded directory.
/// `source_dir` should contain a valid manifest.json.
pub fn install_from_directory(source_dir: &PathBuf) -> Result<InstalledItem> {
    let manifest_path = source_dir.join("manifest.json");
    let content = fs::read_to_string(&manifest_path)
        .context("No manifest.json found in source directory")?;
    let manifest: ExtensionManifest =
        serde_json::from_str(&content).context("Invalid manifest.json")?;

    let target_base = match manifest.kind.as_str() {
        "extension" => user_extensions_dir()?,
        "theme" => user_themes_dir()?,
        "commands" => user_command_packs_dir()?,
        other => anyhow::bail!("Unknown extension type: {}", other),
    };

    let target_dir = target_base.join(&manifest.id);

    // Remove existing if present
    if target_dir.exists() {
        fs::remove_dir_all(&target_dir).context("Failed to remove existing installation")?;
    }

    // Copy the directory
    copy_dir_recursive(source_dir, &target_dir)?;

    info!("Installed {} '{}' v{}", manifest.kind, manifest.id, manifest.version);

    Ok(InstalledItem {
        path: target_dir.to_string_lossy().to_string(),
        bundled: false,
        enabled: true,
        manifest,
    })
}

/// Uninstall a user-installed item by ID and type.
pub fn uninstall(id: &str, kind: &str) -> Result<()> {
    let base = match kind {
        "extension" => user_extensions_dir()?,
        "theme" => user_themes_dir()?,
        "commands" => user_command_packs_dir()?,
        other => anyhow::bail!("Unknown type: {}", other),
    };

    let target = base.join(id);
    if !target.exists() {
        anyhow::bail!("Item '{}' is not installed", id);
    }

    fs::remove_dir_all(&target).context("Failed to remove installation directory")?;
    info!("Uninstalled {} '{}'", kind, id);
    Ok(())
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Theme color loading
// ---------------------------------------------------------------------------

/// Load theme colors from a theme's JSON file.
/// Returns the colors map or None if not found.
pub fn load_theme_colors(theme_id: &str, variant: &str, bundled_dir: Option<&PathBuf>) -> Result<Option<serde_json::Value>> {
    // Check user themes first
    if let Ok(user_dir) = user_themes_dir() {
        let theme_dir = user_dir.join(theme_id);
        if let Some(colors) = try_load_theme_variant(&theme_dir, variant)? {
            return Ok(Some(colors));
        }
    }

    // Then bundled
    if let Some(dir) = bundled_dir {
        let theme_dir = dir.join(theme_id);
        if let Some(colors) = try_load_theme_variant(&theme_dir, variant)? {
            return Ok(Some(colors));
        }
    }

    Ok(None)
}

fn try_load_theme_variant(theme_dir: &PathBuf, variant: &str) -> Result<Option<serde_json::Value>> {
    // First read the manifest to find the variant file path
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

    let variant_file = match variant_file {
        Some(f) => f,
        None => return Ok(None),
    };

    let variant_path = theme_dir.join(variant_file);
    if !variant_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&variant_path)?;
    let theme_data: serde_json::Value = serde_json::from_str(&content)?;
    Ok(theme_data.get("colors").cloned())
}
