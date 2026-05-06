//! Extension and theme discovery, management, and store API.
//!
//! Extensions live in two locations:
//! - Bundled: `<app_resource_dir>/extensions/` (read-only, ships with app)
//! - User:    `<config_dir>/kage/extensions/` (user-installed)
//!
//! Themes live similarly:
//! - Bundled: `<app_resource_dir>/themes/`
//! - User:    `<config_dir>/kage/themes/`
//!
//! User-installed items take precedence over bundled ones with the same ID.

use anyhow::{Context, Result};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Identifier validation
// ---------------------------------------------------------------------------

/// Validate an extension/theme/command-pack identifier before it's used as a
/// path component. Without this check, a hostile manifest with `id: "../foo"`
/// (or `"id": "/etc/something"`) would let install_from_directory build a
/// target path that escapes the extensions directory — and the function
/// would then call fs::remove_dir_all on that escaped path before copying
/// over it. That's an arbitrary directory delete on install.
///
/// Rules:
/// - lowercase ASCII letters, digits, `-`, and `_`
/// - must start with a letter or digit (no leading dot, dash, or underscore)
/// - 1..=64 characters
/// These are tighter than the manifest format technically allows, but match
/// the convention every shipped extension follows and reject every Unicode
/// directory-traversal trick we know about.
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
    // The first-char check above already excludes uppercase (alphanumeric
    // + later loop catches it), but be explicit:
    if first.is_ascii_uppercase() {
        anyhow::bail!("Extension id must be lowercase: {:?}", id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-extension data layout (used by save_extension_data / load_extension_data
// / delete_extension_data Tauri commands)
// ---------------------------------------------------------------------------
//
// Each entry lives at <root>/<extension_id>/<key>.json. The extension_id
// scope is enforced by the JS sandbox host bridge — it overrides whatever
// the sandboxed caller supplies — so an extension with the `storage`
// capability can't read or write another extension's data.

/// Validate that a data key is safe for use as a filename within an
/// extension's data directory.
pub fn validate_data_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 128 {
        anyhow::bail!("Data key must be 1-128 characters");
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        anyhow::bail!("Data key contains invalid characters (allowed: a-z, 0-9, -, _, .)");
    }
    if key.contains("..") {
        anyhow::bail!("Data key must not contain '..'");
    }
    Ok(())
}

/// Resolve the per-extension data path under `root` for (extension_id, key).
/// Validates both inputs, ensures the per-extension dir exists, and returns
/// `<root>/<extension_id>/<key>.json`.
pub fn resolve_extension_data_path(
    root: &std::path::Path,
    extension_id: &str,
    key: &str,
) -> Result<std::path::PathBuf> {
    validate_extension_id(extension_id).context("Invalid extension id")?;
    validate_data_key(key).context("Invalid data key")?;
    let dir = root.join(extension_id);
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create extension data dir {:?}", dir))?;
    }
    Ok(dir.join(format!("{}.json", key)))
}

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
    /// Capabilities this extension is requesting. See docs/EXTENSIONS.md
    /// for the full list. When the field is absent, the frontend falls back
    /// to a legacy-safe default — bundled extensions get a broad set,
    /// user-installed ones get `storage` only.
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    /// For command packs: the commands themselves
    #[serde(default)]
    pub commands: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionContributes {
    #[serde(default)]
    pub search_provider: Option<String>,
    /// Declarative settings provider (sandboxed). Use this for all new
    /// extensions. The legacy `settings_module` field is kept for reading
    /// old manifests but is no longer loaded.
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

// ---------------------------------------------------------------------------
// Directory helpers
// ---------------------------------------------------------------------------

/// Get a user directory under `<config_dir>/kage/<subdir>/`
pub fn user_item_dir(subdir: &str) -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Failed to get config directory")?;
    Ok(config_dir.join("kage").join(subdir))
}

/// Map an item kind ("extension", "theme", "commands") to its user directory name.
pub fn kind_to_subdir(kind: &str) -> Result<&'static str> {
    match kind {
        "extension" => Ok("extensions"),
        "theme" => Ok("themes"),
        "commands" => Ok("command-packs"),
        other => anyhow::bail!("Unknown item type: {}", other),
    }
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

/// Discover all installed items of a given kind (bundled + user). User items override bundled by ID.
/// `kind` is "extension", "theme", or "commands".
pub fn discover_items(
    kind: &str,
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

    // User items override bundled
    if let Ok(subdir) = kind_to_subdir(kind) {
        if let Ok(user_dir) = user_item_dir(subdir) {
            for item in scan_directory(&user_dir, false, enabled_states) {
                by_id.insert(item.manifest.id.clone(), item);
            }
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

    // Reject hostile manifest ids before they reach any filesystem op.
    // See validate_extension_id for why this matters.
    validate_extension_id(&manifest.id).context("Invalid extension id in manifest")?;

    let subdir = kind_to_subdir(&manifest.kind)?;
    let target_base = user_item_dir(subdir)?;

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
    // Same defense as install_from_directory: never let a frontend-supplied
    // id reach fs::remove_dir_all without being checked first.
    validate_extension_id(id).context("Invalid extension id")?;

    let subdir = kind_to_subdir(kind)?;
    let base = user_item_dir(subdir)?;

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
// Zip extraction (for store installs)
// ---------------------------------------------------------------------------

/// Extract a .zip archive to a target directory with Zip Slip protection.
/// Returns the path to the extracted directory.
pub fn extract_zip(zip_path: &PathBuf, target_dir: &PathBuf) -> Result<()> {
    use std::io;
    use std::path::Component;

    let file = fs::File::open(zip_path)
        .context("Failed to open zip file")?;
    let mut archive = zip::ZipArchive::new(file)
        .context("Failed to read zip archive")?;

    // Make sure the target exists so we can canonicalize it once up-front.
    fs::create_dir_all(target_dir).ok();
    let canonical_target = target_dir.canonicalize()
        .with_context(|| format!("Failed to canonicalize target {}", target_dir.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)
            .context("Failed to read zip entry")?;

        let entry_path = entry.enclosed_name()
            .context("Zip entry has invalid path (possible Zip Slip attack)")?
            .to_owned();

        // Reject absolute paths, prefixes, and any `..` components up front. This
        // defends against bugs in `enclosed_name` as well as symlink-based attacks.
        for comp in entry_path.components() {
            match comp {
                Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                    anyhow::bail!(
                        "Zip Slip: entry '{}' contains forbidden path component",
                        entry_path.display()
                    );
                }
                _ => {}
            }
        }

        let out_path = target_dir.join(&entry_path);

        // Build the canonical resolved path by canonicalizing the deepest existing
        // ancestor (to resolve any symlinks in the target) and appending the
        // remaining components verbatim. Never call canonicalize on `out_path`
        // itself until we've confirmed containment — that would follow a
        // malicious symlink to somewhere else.
        let mut anchor = out_path.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        while !anchor.exists() {
            match anchor.file_name() {
                Some(name) => tail.push(name.to_os_string()),
                None => break,
            }
            if !anchor.pop() {
                break;
            }
        }
        let anchor_canon = anchor
            .canonicalize()
            .unwrap_or(anchor);
        let mut resolved = anchor_canon;
        for name in tail.into_iter().rev() {
            resolved.push(name);
        }

        if !resolved.starts_with(&canonical_target) {
            anyhow::bail!(
                "Zip Slip detected: entry '{}' would extract outside target directory",
                entry_path.display()
            );
        }

        // Never write through a symlink that already exists at the destination.
        if let Ok(meta) = fs::symlink_metadata(&resolved) {
            if meta.file_type().is_symlink() {
                anyhow::bail!(
                    "Zip Slip: refusing to write through existing symlink at '{}'",
                    resolved.display()
                );
            }
        }

        if entry.is_dir() {
            fs::create_dir_all(&resolved)?;
        } else {
            if let Some(parent) = resolved.parent() {
                fs::create_dir_all(parent)?;
                // Reject symlinked parent directories that could redirect writes.
                if let Ok(meta) = fs::symlink_metadata(parent) {
                    if meta.file_type().is_symlink() {
                        anyhow::bail!(
                            "Zip Slip: refusing to extract into symlinked directory '{}'",
                            parent.display()
                        );
                    }
                }
            }
            let mut outfile = fs::File::create(&resolved)
                .with_context(|| format!("Failed to create file: {}", resolved.display()))?;
            io::copy(&mut entry, &mut outfile)
                .with_context(|| format!("Failed to write file: {}", resolved.display()))?;
        }
    }

    info!("Extracted zip to {:?} ({} entries)", target_dir, archive.len());
    Ok(())
}

/// Install an extension from a .zip file downloaded from the store.
/// Extracts to a temp directory, reads the manifest, then installs to the correct location.
pub fn install_from_zip(zip_path: &PathBuf) -> Result<InstalledItem> {
    // Extract to a temp directory
    let temp_dir = std::env::temp_dir().join(format!(
        "kage-ext-{}",
        uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("tmp")
    ));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    extract_zip(zip_path, &temp_dir)?;

    // The zip might contain files directly or inside a single subdirectory.
    // Find the manifest.json — check root first, then one level deep.
    let manifest_dir = if temp_dir.join("manifest.json").exists() {
        temp_dir.clone()
    } else {
        // Check for a single subdirectory containing manifest.json
        let mut found = None;
        if let Ok(entries) = fs::read_dir(&temp_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("manifest.json").exists() {
                    found = Some(path);
                    break;
                }
            }
        }
        found.context("No manifest.json found in zip archive (checked root and one level deep)")?
    };

    // Install from the extracted directory
    let result = install_from_directory(&manifest_dir);

    // Cleanup temp directory
    let _ = fs::remove_dir_all(&temp_dir);

    result
}

// ---------------------------------------------------------------------------
// Theme color loading
// ---------------------------------------------------------------------------

/// Load theme colors from a theme's JSON file.
/// Returns the colors map or None if not found.
pub fn load_theme_colors(theme_id: &str, variant: &str, bundled_dir: Option<&PathBuf>) -> Result<Option<serde_json::Value>> {
    // Check user themes first
    if let Ok(user_dir) = user_item_dir("themes") {
        let theme_dir = user_dir.join(theme_id);
        log::info!("load_theme_colors: checking user dir {:?}", theme_dir);
        if let Some(colors) = try_load_theme_variant(&theme_dir, variant)? {
            return Ok(Some(colors));
        }
    }

    // Then bundled
    if let Some(dir) = bundled_dir {
        let theme_dir = dir.join(theme_id);
        log::info!("load_theme_colors: checking bundled dir {:?}", theme_dir);
        if let Some(colors) = try_load_theme_variant(&theme_dir, variant)? {
            return Ok(Some(colors));
        }
    }

    log::warn!("load_theme_colors: theme '{}' ({}) not found in any directory", theme_id, variant);
    Ok(None)
}

fn try_load_theme_variant(theme_dir: &std::path::Path, variant: &str) -> Result<Option<serde_json::Value>> {
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
