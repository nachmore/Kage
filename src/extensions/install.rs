use super::{
    kind_to_subdir, user_item_dir, validate_extension_id, ExtensionManifest, InstalledItem,
};
use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::PathBuf;

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

    if target_dir.exists() {
        fs::remove_dir_all(&target_dir).context("Failed to remove existing installation")?;
    }

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
    // Never let a frontend-supplied id reach fs::remove_dir_all unchecked.
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
