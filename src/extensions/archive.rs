use super::{install_from_directory, InstalledItem};
use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::PathBuf;

/// Per-entry decompressed size cap for store/sideloaded extension zips.
const ZIP_MAX_ENTRY_BYTES: u64 = 50 * 1024 * 1024; // 50 MB
/// Cumulative decompressed size cap across the whole archive.
const ZIP_MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024; // 100 MB

/// Extract a .zip archive to a target directory with Zip Slip protection and
/// a decompression budget (per-entry and cumulative).
pub fn extract_zip(zip_path: &PathBuf, target_dir: &PathBuf) -> Result<()> {
    use std::io;
    use std::io::Read;
    use std::path::Component;

    let file = fs::File::open(zip_path).context("Failed to open zip file")?;
    let mut archive = zip::ZipArchive::new(file).context("Failed to read zip archive")?;

    const ZIP_MAX_ENTRIES: usize = 10_000;
    if archive.len() > ZIP_MAX_ENTRIES {
        anyhow::bail!(
            "Zip archive has {} entries (max {}) — refusing to extract",
            archive.len(),
            ZIP_MAX_ENTRIES
        );
    }

    let mut total_bytes: u64 = 0;
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
            let entry_budget = ZIP_MAX_ENTRY_BYTES.min(ZIP_MAX_TOTAL_BYTES - total_bytes);
            let written = io::copy(&mut (&mut entry).take(entry_budget + 1), &mut outfile)
                .with_context(|| format!("Failed to write file: {}", resolved.display()))?;
            if written > entry_budget {
                drop(outfile);
                fs::remove_file(&resolved).ok();
                anyhow::bail!(
                    "Zip entry '{}' exceeds the decompression budget ({} MB per entry, {} MB total) — aborting extraction",
                    entry_path.display(),
                    ZIP_MAX_ENTRY_BYTES / (1024 * 1024),
                    ZIP_MAX_TOTAL_BYTES / (1024 * 1024)
                );
            }
            total_bytes += written;
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
pub fn install_from_zip(zip_path: &PathBuf) -> Result<InstalledItem> {
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

    let result = (|| {
        super::extract_zip(zip_path, &temp_dir)?;
        let manifest_dir = if temp_dir.join("manifest.json").exists() {
            temp_dir.clone()
        } else {
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
            found.context(
                "No manifest.json found in zip archive (checked root and one level deep)",
            )?
        };
        install_from_directory(&manifest_dir)
    })();

    let _ = fs::remove_dir_all(&temp_dir);
    result
}
