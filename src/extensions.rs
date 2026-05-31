//! Extension and theme discovery, management, and store API.
//!
//! Every extension and theme installs to the per-user config dir:
//!   - Extensions: `<config_dir>/kage/extensions/`
//!   - Themes:     `<config_dir>/kage/themes/`
//!
//! Items are fetched from the store catalog
//! (`https://nachmore.github.io/Kage-Extensions/` by default), verified
//! against their SHA-256, and extracted on install. There used to be a
//! second "bundled" path that shipped a small set of read-only extensions
//! inside the binary; that complicated the security model and made
//! first-party extensions feel different from third-party ones, so it
//! was removed in favour of one uniform install path.

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
///
/// - lowercase ASCII letters, digits, `-`, and `_`
/// - must start with a letter or digit (no leading dot, dash, or underscore)
/// - 1..=64 characters
///
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
// Store URL validation (used by store_install / store_get_catalog /
// store_get_detail / save_store_url Tauri commands)
// ---------------------------------------------------------------------------

/// Validate a store URL. HTTPS is allowed unconditionally; HTTP is allowed
/// only for localhost-equivalent hosts (the dev server).
///
/// Pre-fix the check was `url.starts_with("http://localhost")` /
/// `url.starts_with("http://127.0.0.1")`, which is suffix-abusable: a URL
/// like `http://localhost.attacker.com/store` or `http://127.0.0.1.evil.example/`
/// matches the prefix but resolves to an attacker-controlled host. The fix
/// is to parse the URL and compare the host component exactly.
pub fn validate_store_url(url: &str) -> Result<()> {
    let parsed =
        url::Url::parse(url).with_context(|| format!("Store URL is not a valid URL: {}", url))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            // host_str returns Some("localhost") / Some("127.0.0.1") / Some("[::1]")
            // for the loopback hostnames; suffix-abuse cases like
            // "localhost.attacker.com" return Some("localhost.attacker.com")
            // and fail this exact-equality check.
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

/// Canonical list of valid capability names. Must stay in sync with
/// `CAPABILITIES` in `ui/js/shared/extension-permissions.js` — the Rust
/// side is now authoritative; the JS list is a mirror for rendering.
///
/// Unknown / misspelled capabilities in a manifest are dropped by
/// [`normalize_permissions`] at install time. This means a typo like
/// `"strage"` can never become a recorded grant, regardless of which
/// install path a package came through.
pub const VALID_CAPABILITIES: &[&str] = &[
    "storage",
    "clipboard",
    "urls",
    "launch",
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

/// Legacy capability aliases. A manifest that declares one of these
/// gets the expansion stored as the actual grant. Keep in sync with
/// `LEGACY_PERMISSION_ALIASES` in `ui/js/shared/extension-permissions.js`.
///
/// `shell` historically bundled URL-handoff + arbitrary file/app launch
/// under one badge labelled "Open URLs, file paths, and launch other
/// apps." That description was a bigger surface than most extensions
/// actually need (most call only `open_url`), so it was split into
/// `urls` and `launch`. Manifests that still say `shell` get both for
/// backwards compatibility.
fn legacy_aliases(cap: &str) -> Option<&'static [&'static str]> {
    match cap {
        "shell" => Some(&["urls", "launch"]),
        _ => None,
    }
}

/// Filter a raw permission list from a manifest down to a deduped,
/// lowercase set of known capabilities. Unknown entries are logged
/// and dropped; legacy aliases (see [`legacy_aliases`]) are expanded
/// into their current-cap equivalents. The returned vector is safe to
/// store as a grant.
///
/// This is the single point of authority for what can land in
/// `config.extension_grants[*].granted`. Both the welcome batch path
/// (`install_and_commit_direct`) and the store path
/// (`commit_extension_install`) should funnel through it — drift
/// between the two is what lets silent privilege escalation sneak in.
pub fn normalize_permissions(raw: &[String], context: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    let mut push = |cap: String, out: &mut Vec<String>| {
        if seen.insert(cap.clone()) {
            out.push(cap);
        }
    };
    for entry in raw {
        let cap = entry.trim().to_lowercase();
        if cap.is_empty() {
            continue;
        }
        if let Some(expanded) = legacy_aliases(&cap) {
            log::warn!(
                "Extension '{}': capability '{}' is deprecated; expanding to {}. \
                 Update manifest.json to declare these directly.",
                context,
                cap,
                expanded.join(" + ")
            );
            for e in expanded {
                if VALID_CAPABILITIES.contains(e) {
                    push((*e).to_string(), &mut out);
                }
            }
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
        push(cap, &mut out);
    }
    out
}

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
    /// for the full list. When the field is absent, the frontend falls
    /// back to a legacy-safe default of `storage` only — pre-permissions
    /// manifests can still load, but they can't do anything else.
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
fn scan_directory(dir: &PathBuf, enabled_states: &HashMap<String, bool>) -> Vec<InstalledItem> {
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
                    let enabled = enabled_states.get(&manifest.id).copied().unwrap_or(true);
                    items.push(InstalledItem {
                        path: path.to_string_lossy().to_string(),
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

/// Discover all installed items of a given kind. `kind` is "extension",
/// "theme", or "commands". Reads from the per-user install dir under
/// `<config_dir>/kage/<subdir>/`. Was previously bundled+user-merged with
/// the binary shipping a small set of in-tree extensions; that mode is
/// gone now — every extension goes through the normal store install path.
pub fn discover_items(kind: &str, enabled_states: &HashMap<String, bool>) -> Vec<InstalledItem> {
    let mut by_id: HashMap<String, InstalledItem> = HashMap::new();

    if let Ok(subdir) = kind_to_subdir(kind) {
        if let Ok(user_dir) = user_item_dir(subdir) {
            for item in scan_directory(&user_dir, enabled_states) {
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
    let content =
        fs::read_to_string(&manifest_path).context("No manifest.json found in source directory")?;
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

    info!(
        "Installed {} '{}' v{}",
        manifest.kind, manifest.id, manifest.version
    );

    Ok(InstalledItem {
        path: target_dir.to_string_lossy().to_string(),
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

    let file = fs::File::open(zip_path).context("Failed to open zip file")?;
    let mut archive = zip::ZipArchive::new(file).context("Failed to read zip archive")?;

    // Make sure the target exists so we can canonicalize it once up-front.
    fs::create_dir_all(target_dir).ok();
    let canonical_target = target_dir
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize target {}", target_dir.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("Failed to read zip entry")?;

        let entry_path = entry
            .enclosed_name()
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
        let anchor_canon = anchor.canonicalize().unwrap_or(anchor);
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

    info!(
        "Extracted zip to {:?} ({} entries)",
        target_dir,
        archive.len()
    );
    Ok(())
}

/// Install an extension from a .zip file downloaded from the store.
/// Extracts to a temp directory, reads the manifest, then installs to the correct location.
pub fn install_from_zip(zip_path: &PathBuf) -> Result<InstalledItem> {
    // Extract to a temp directory
    let temp_dir = std::env::temp_dir().join(format!(
        "kage-ext-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("tmp")
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

fn try_load_theme_variant(
    theme_dir: &std::path::Path,
    variant: &str,
) -> Result<Option<serde_json::Value>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_permissions_drops_unknown() {
        let raw = vec![
            "storage".to_string(),
            "strage".to_string(), // typo
            "clipboard".to_string(),
        ];
        let out = normalize_permissions(&raw, "test");
        assert_eq!(out, vec!["storage", "clipboard"]);
    }

    #[test]
    fn normalize_permissions_dedupes_and_lowercases() {
        let raw = vec![
            "Storage".to_string(),
            "  storage  ".to_string(),
            "STORAGE".to_string(),
        ];
        let out = normalize_permissions(&raw, "test");
        assert_eq!(out, vec!["storage"]);
    }

    #[test]
    fn normalize_permissions_preserves_order() {
        let raw = vec![
            "calendar".to_string(),
            "storage".to_string(),
            "agent".to_string(),
        ];
        let out = normalize_permissions(&raw, "test");
        assert_eq!(out, vec!["calendar", "storage", "agent"]);
    }

    #[test]
    fn normalize_permissions_handles_empty_and_whitespace() {
        let raw = vec!["".to_string(), "   ".to_string(), "storage".to_string()];
        let out = normalize_permissions(&raw, "test");
        assert_eq!(out, vec!["storage"]);
    }

    #[test]
    fn every_valid_capability_passes() {
        let raw: Vec<String> = VALID_CAPABILITIES.iter().map(|s| s.to_string()).collect();
        let out = normalize_permissions(&raw, "test");
        assert_eq!(out.len(), VALID_CAPABILITIES.len());
    }
}
