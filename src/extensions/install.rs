use super::{
    kind_to_subdir, user_item_dir, validate_extension_id, ExtensionManifest, InstalledItem,
};
use crate::lock_ext::LockExt;
use anyhow::{Context, Result};
use log::{info, warn};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, TryLockError};

/// Item kinds whose install dirs can hold a parked upgrade.
const ITEM_KINDS: [&str; 3] = ["extension", "theme", "commands"];

// Hidden sibling dirs (`.<id>.<suffix>`) used while replacing an install.
// Ids can't start with '.', and discovery skips dot-dirs, so none of these
// can ever be loaded as an item.
const STAGING_SUFFIX: &str = "staging";
const BACKUP_SUFFIX: &str = "bak";
const PENDING_SUFFIX: &str = "pending";
const REMOVING_SUFFIX: &str = "removing";

/// Serialises every swap and records which parked upgrades belong to a
/// permission prompt in this run. In-memory on purpose: a `.pending` dir
/// left by an earlier run has no prompt waiting on it, so it must not turn
/// a later uninstall into a no-op discard.
static PARKED_UPGRADES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// How an install treats a copy of the same item that is already on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// Swap the new files in now (auto-update, welcome batch).
    Replace,
    /// Leave an existing install live and park the new files beside it
    /// until the user approves (`commit_pending`) or declines
    /// (`discard_pending`) the upgrade prompt. A fresh install is written
    /// live either way, so declining one still rolls back via `uninstall`.
    DeferUpgrade,
}

/// An install result: the new manifest, plus whether it is still parked.
#[derive(Debug)]
pub struct Installed {
    pub item: InstalledItem,
    /// True when an older version is still live and the new files wait for
    /// `commit_pending`.
    pub deferred: bool,
}

fn sibling(base: &Path, id: &str, suffix: &str) -> PathBuf {
    base.join(format!(".{}.{}", id, suffix))
}

fn read_manifest(source_dir: &Path) -> Result<ExtensionManifest> {
    let content = fs::read_to_string(source_dir.join("manifest.json"))
        .context("No manifest.json found in source directory")?;
    let manifest: ExtensionManifest =
        serde_json::from_str(&content).context("Invalid manifest.json")?;

    // Reject hostile manifest ids before they reach any filesystem op.
    // See validate_extension_id for why this matters.
    validate_extension_id(&manifest.id).context("Invalid extension id in manifest")?;
    Ok(manifest)
}

/// Install an extension/theme/command-pack from a downloaded directory.
/// `source_dir` should contain a valid manifest.json.
pub fn install_from_directory(source_dir: &Path, mode: InstallMode) -> Result<Installed> {
    let manifest = read_manifest(source_dir)?;
    let base = user_item_dir(kind_to_subdir(&manifest.kind)?)?;
    let installed = install_into(&base, source_dir, manifest, mode)?;

    let m = &installed.item.manifest;
    if installed.deferred {
        info!("Staged update for {} '{}' v{}", m.kind, m.id, m.version);
    } else {
        info!("Installed {} '{}' v{}", m.kind, m.id, m.version);
    }
    Ok(installed)
}

fn install_into(
    base: &Path,
    source_dir: &Path,
    manifest: ExtensionManifest,
    mode: InstallMode,
) -> Result<Installed> {
    let target_dir = base.join(&manifest.id);
    let mut parked_upgrades = PARKED_UPGRADES.lock_or_recover();

    fs::create_dir_all(base).context("Failed to create install directory")?;
    recover_leftovers(base, &manifest.id, &target_dir);

    // A newer stage supersedes any upgrade still parked for this id.
    let parked = sibling(base, &manifest.id, PENDING_SUFFIX);
    take_parked(&mut parked_upgrades, &parked);
    remove_if_exists(&parked);

    let deferred = mode == InstallMode::DeferUpgrade && target_dir.exists();
    if deferred {
        copy_fresh(source_dir, &parked)?;
        parked_upgrades.push(parked);
    } else {
        // Copy beside the target first (same volume as the target, unlike
        // the temp extraction dir) so the swap itself is just renames.
        let staging = sibling(base, &manifest.id, STAGING_SUFFIX);
        copy_fresh(source_dir, &staging)?;
        swap_in(base, &manifest.id, &staging, &target_dir)?;
    }

    Ok(Installed {
        item: InstalledItem {
            path: target_dir.to_string_lossy().to_string(),
            enabled: true,
            manifest,
        },
        deferred,
    })
}

/// Apply an upgrade parked by an `InstallMode::DeferUpgrade` install, once
/// the user has approved it. Returns false when nothing was parked for `id`.
pub fn commit_pending(id: &str) -> Result<bool> {
    validate_extension_id(id).context("Invalid extension id")?;
    for kind in ITEM_KINDS {
        let base = user_item_dir(kind_to_subdir(kind)?)?;
        if commit_pending_in(&base, id)? {
            info!("Applied staged update for {} '{}'", kind, id);
            return Ok(true);
        }
    }
    Ok(false)
}

fn commit_pending_in(base: &Path, id: &str) -> Result<bool> {
    let mut parked_upgrades = PARKED_UPGRADES.lock_or_recover();
    let parked = sibling(base, id, PENDING_SUFFIX);
    if !take_parked(&mut parked_upgrades, &parked) {
        return Ok(false);
    }
    swap_in(base, id, &parked, &base.join(id))?;
    Ok(true)
}

/// The dir holding an upgrade parked for `id` under `base`, if one is
/// waiting on a prompt in this run. The install prompt reads the new
/// version's files (e.g. its locale catalog) from here.
pub fn parked_upgrade_dir(base: &Path, id: &str) -> Option<PathBuf> {
    let parked_upgrades = PARKED_UPGRADES.lock_or_recover();
    let parked = sibling(base, id, PENDING_SUFFIX);
    parked_upgrades.contains(&parked).then_some(parked)
}

/// Drop an upgrade parked for `id` (the user declined it), leaving the live
/// version untouched. Returns false when nothing was parked for `id`.
pub fn discard_pending(id: &str, kind: &str) -> Result<bool> {
    validate_extension_id(id).context("Invalid extension id")?;
    let base = user_item_dir(kind_to_subdir(kind)?)?;
    let discarded = discard_pending_in(&base, id);
    if discarded {
        info!("Discarded staged update for {} '{}'", kind, id);
    }
    Ok(discarded)
}

fn discard_pending_in(base: &Path, id: &str) -> bool {
    let mut parked_upgrades = PARKED_UPGRADES.lock_or_recover();
    let parked = sibling(base, id, PENDING_SUFFIX);
    let was_parked = take_parked(&mut parked_upgrades, &parked);
    if was_parked {
        remove_if_exists(&parked);
    }
    was_parked
}

/// Uninstall a user-installed item by ID and type.
pub fn uninstall(id: &str, kind: &str) -> Result<()> {
    // Never let a frontend-supplied id reach fs::remove_dir_all unchecked.
    validate_extension_id(id).context("Invalid extension id")?;

    let base = user_item_dir(kind_to_subdir(kind)?)?;
    uninstall_in(&base, id)?;
    info!("Uninstalled {} '{}'", kind, id);
    Ok(())
}

fn uninstall_in(base: &Path, id: &str) -> Result<()> {
    let mut parked_upgrades = PARKED_UPGRADES.lock_or_recover();
    let target = base.join(id);
    recover_leftovers(base, id, &target);
    if !target.exists() {
        anyhow::bail!("Item '{}' is not installed", id);
    }

    // Rename first: it either fails whole (file open on Windows) or hides
    // the item at once, so a partial delete can't leave a half-present
    // extension for discovery to load.
    let doomed = sibling(base, id, REMOVING_SUFFIX);
    remove_if_exists(&doomed);
    rename_dir(&target, &doomed).context("Failed to remove installation directory")?;
    remove_if_exists(&doomed);

    let parked = sibling(base, id, PENDING_SUFFIX);
    take_parked(&mut parked_upgrades, &parked);
    remove_if_exists(&parked);
    Ok(())
}

/// Move `staged` into `target`, keeping the old version until the new one
/// is in place. On any failure the old version stays (or is put back) and
/// `staged` is removed.
fn swap_in(base: &Path, id: &str, staged: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        // With no old version to protect, a rename that still fails (a
        // scanner holding the fresh files past the retries) falls back to
        // copying straight into place, as fresh installs did before staging.
        let moved = match rename_dir(staged, target) {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!(
                    "Failed to rename {:?} into place ({}); copying instead",
                    staged, e
                );
                copy_fresh(staged, target)
            }
        };
        remove_if_exists(staged);
        return moved.context("Failed to move new installation into place");
    }

    let backup = sibling(base, id, BACKUP_SUFFIX);
    remove_if_exists(&backup);
    // Fails on Windows while a file inside is open — bail with the old
    // version intact rather than falling back to a partial delete.
    if let Err(e) = rename_dir(target, &backup) {
        remove_if_exists(staged);
        return Err(e).context("Failed to move existing installation aside");
    }
    if let Err(e) = rename_dir(staged, target) {
        if let Err(restore) = rename_dir(&backup, target) {
            warn!(
                "Failed to restore previous installation {:?}: {}",
                backup, restore
            );
        }
        remove_if_exists(staged);
        return Err(e).context("Failed to move new installation into place");
    }
    remove_if_exists(&backup);
    Ok(())
}

/// `fs::rename` with a retry. Windows refuses to rename a directory whose
/// files were just written while antivirus / indexer handles are open,
/// which is exactly the state a fresh staging copy is in, so those errors
/// get a budget of several seconds; anything else gets a short one. A real
/// lock (a running binary inside) still fails after the last attempt.
fn rename_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    const RETRIES: u64 = 5;
    // ~7.75 s total with the capped back-off below.
    const IN_USE_RETRIES: u64 = 20;
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                let budget = if is_file_in_use(&e) {
                    IN_USE_RETRIES
                } else {
                    RETRIES
                };
                if attempt >= budget {
                    return Err(e);
                }
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis((50 * attempt).min(500)));
            }
            result => return result,
        }
    }
}

/// Windows' "a file inside is open" family: ACCESS_DENIED (mapped to
/// PermissionDenied), ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33).
fn is_file_in_use(e: &std::io::Error) -> bool {
    cfg!(windows)
        && (e.kind() == std::io::ErrorKind::PermissionDenied
            || matches!(e.raw_os_error(), Some(32 | 33)))
}

/// Clean up after a crash mid-swap for `id`: put the backup back if the live
/// dir is missing, then drop stale staging/backup/removal dirs. Parked
/// upgrades are handled by the callers, which know whether one is wanted.
fn recover_leftovers(base: &Path, id: &str, target: &Path) {
    let backup = sibling(base, id, BACKUP_SUFFIX);
    if backup.exists() && !target.exists() {
        match fs::rename(&backup, target) {
            Ok(()) => warn!("Restored interrupted install of '{}' from backup", id),
            Err(e) => warn!("Failed to restore backup {:?}: {}", backup, e),
        }
    }
    for suffix in [BACKUP_SUFFIX, STAGING_SUFFIX, REMOVING_SUFFIX] {
        remove_if_exists(&sibling(base, id, suffix));
    }
}

/// Sweep install leftovers under `base` that nothing in this run owns:
/// staging/backup/removal dirs from a crash mid-swap (restoring the backup
/// when the live dir is gone) and `.pending` upgrades whose prompt died with
/// an earlier run. Run from discovery so a crash doesn't leave dead copies
/// on disk until the same id happens to be installed or uninstalled again.
pub(super) fn sweep_leftovers(base: &Path) {
    // An install holds this lock across its whole copy + swap, so its own
    // staging/parked dir must not be swept from under it. Rather than stall
    // discovery behind a multi-second copy, skip: the next discovery sweeps.
    let parked_upgrades = match PARKED_UPGRADES.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    sweep_leftovers_in(base, &parked_upgrades);
}

fn sweep_leftovers_in(base: &Path, parked_upgrades: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    let mut crashed_ids = BTreeSet::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        // Only `.<valid id>.<known suffix>` is ours; anything else that
        // happens to start with a dot is left alone.
        let Some((id, suffix)) = name
            .to_str()
            .and_then(|n| n.strip_prefix('.'))
            .and_then(|n| n.rsplit_once('.'))
        else {
            continue;
        };
        if validate_extension_id(id).is_err() {
            continue;
        }
        let path = entry.path();
        match suffix {
            // Liveness is the in-memory parked list: a prompt from this run
            // is still waiting on its dir; any other `.pending` is orphaned.
            PENDING_SUFFIX if !parked_upgrades.contains(&path) => {
                info!("Removing orphaned staged update {:?}", path);
                remove_if_exists(&path);
            }
            BACKUP_SUFFIX | STAGING_SUFFIX | REMOVING_SUFFIX => {
                info!("Cleaning up interrupted install leftover {:?}", path);
                crashed_ids.insert(id.to_string());
            }
            _ => {}
        }
    }
    // Same recovery an install of the id would run, so a backup whose live
    // dir vanished mid-swap is restored rather than deleted.
    for id in crashed_ids {
        recover_leftovers(base, &id, &base.join(&id));
    }
}

fn take_parked(parked_upgrades: &mut Vec<PathBuf>, dir: &Path) -> bool {
    let before = parked_upgrades.len();
    parked_upgrades.retain(|p| p.as_path() != dir);
    parked_upgrades.len() != before
}

fn remove_if_exists(dir: &Path) {
    if dir.exists() {
        if let Err(e) = fs::remove_dir_all(dir) {
            warn!("Failed to remove {:?}: {}", dir, e);
        }
    }
}

/// Copy `src` into a fresh `dst`, removing any partial copy on failure.
fn copy_fresh(src: &Path, dst: &Path) -> Result<()> {
    remove_if_exists(dst);
    if let Err(e) = copy_dir_recursive(src, dst) {
        remove_if_exists(dst);
        return Err(e.context("Failed to copy extension files"));
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(dir: &Path, id: &str, version: &str) {
        fs::create_dir_all(dir).unwrap();
        let manifest = serde_json::json!({
            "id": id, "name": id, "version": version, "type": "extension",
        });
        fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        fs::write(dir.join("main.js"), version).unwrap();
    }

    fn installed_version(base: &Path, id: &str) -> String {
        fs::read_to_string(base.join(id).join("main.js")).unwrap()
    }

    fn install(base: &Path, src: &Path, mode: InstallMode) -> Installed {
        let manifest = read_manifest(src).unwrap();
        install_into(base, src, manifest, mode).unwrap()
    }

    /// Only the live dir should remain once a swap settles.
    fn entry_names(base: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(base)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn fresh_install_is_live_even_when_deferring() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, src) = (tmp.path().join("ext"), tmp.path().join("src"));
        write_package(&src, "fresh", "1.0.0");

        let installed = install(&base, &src, InstallMode::DeferUpgrade);
        assert!(!installed.deferred);
        assert_eq!(installed_version(&base, "fresh"), "1.0.0");
        assert_eq!(entry_names(&base), vec!["fresh"]);
    }

    #[test]
    fn replace_swaps_new_version_in_without_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, src) = (tmp.path().join("ext"), tmp.path().join("src"));
        write_package(&src, "upd", "1.0.0");
        install(&base, &src, InstallMode::Replace);
        write_package(&src, "upd", "2.0.0");

        let installed = install(&base, &src, InstallMode::Replace);
        assert!(!installed.deferred);
        assert_eq!(installed_version(&base, "upd"), "2.0.0");
        assert_eq!(entry_names(&base), vec!["upd"]);
    }

    #[test]
    fn declined_upgrade_keeps_the_old_version() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, src) = (tmp.path().join("ext"), tmp.path().join("src"));
        write_package(&src, "keep", "1.0.0");
        install(&base, &src, InstallMode::Replace);
        write_package(&src, "keep", "2.0.0");

        let installed = install(&base, &src, InstallMode::DeferUpgrade);
        assert!(installed.deferred);
        assert_eq!(installed.item.manifest.version, "2.0.0");
        assert_eq!(installed_version(&base, "keep"), "1.0.0");

        assert!(discard_pending_in(&base, "keep"));
        assert_eq!(installed_version(&base, "keep"), "1.0.0");
        assert_eq!(entry_names(&base), vec!["keep"]);
        // Nothing left to discard, so a real uninstall would proceed.
        assert!(!discard_pending_in(&base, "keep"));
    }

    #[test]
    fn approved_upgrade_swaps_in_on_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, src) = (tmp.path().join("ext"), tmp.path().join("src"));
        write_package(&src, "approve", "1.0.0");
        install(&base, &src, InstallMode::Replace);
        write_package(&src, "approve", "2.0.0");
        install(&base, &src, InstallMode::DeferUpgrade);

        assert!(commit_pending_in(&base, "approve").unwrap());
        assert_eq!(installed_version(&base, "approve"), "2.0.0");
        assert_eq!(entry_names(&base), vec!["approve"]);
        assert!(!commit_pending_in(&base, "approve").unwrap());
    }

    #[test]
    fn parked_upgrade_dir_points_at_the_waiting_version() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, src) = (tmp.path().join("ext"), tmp.path().join("src"));
        write_package(&src, "loc", "1.0.0");
        install(&base, &src, InstallMode::Replace);
        assert!(parked_upgrade_dir(&base, "loc").is_none());
        write_package(&src, "loc", "2.0.0");
        install(&base, &src, InstallMode::DeferUpgrade);

        let parked = parked_upgrade_dir(&base, "loc").unwrap();
        assert_eq!(fs::read_to_string(parked.join("main.js")).unwrap(), "2.0.0");
        assert!(discard_pending_in(&base, "loc"));
        assert!(parked_upgrade_dir(&base, "loc").is_none());
    }

    #[test]
    fn leftover_pending_from_earlier_run_does_not_count_as_parked() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("ext");
        write_package(&base.join("stale"), "stale", "1.0.0");
        write_package(&sibling(&base, "stale", PENDING_SUFFIX), "stale", "2.0.0");

        assert!(!discard_pending_in(&base, "stale"));
        uninstall_in(&base, "stale").unwrap();
        assert!(entry_names(&base).is_empty());
    }

    #[test]
    fn interrupted_swap_restores_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("ext");
        write_package(&sibling(&base, "crash", BACKUP_SUFFIX), "crash", "1.0.0");
        write_package(&sibling(&base, "crash", STAGING_SUFFIX), "crash", "2.0.0");

        recover_leftovers(&base, "crash", &base.join("crash"));
        assert_eq!(installed_version(&base, "crash"), "1.0.0");
        assert_eq!(entry_names(&base), vec!["crash"]);
    }

    #[test]
    fn sweep_drops_crash_leftovers_and_orphaned_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("ext");
        // Live item with every kind of stale sibling beside it.
        write_package(&base.join("live"), "live", "1.0.0");
        for suffix in [
            BACKUP_SUFFIX,
            STAGING_SUFFIX,
            REMOVING_SUFFIX,
            PENDING_SUFFIX,
        ] {
            write_package(&sibling(&base, "live", suffix), "live", "0.0.0");
        }
        // Crash after moving the live dir aside: the backup must come back.
        write_package(&sibling(&base, "crash", BACKUP_SUFFIX), "crash", "1.0.0");
        write_package(&sibling(&base, "crash", STAGING_SUFFIX), "crash", "2.0.0");
        // Not ours: unknown suffix and an invalid id are left alone.
        fs::create_dir_all(base.join(".live.other")).unwrap();
        fs::create_dir_all(base.join(".Bad.staging")).unwrap();

        sweep_leftovers_in(&base, &[]);
        assert_eq!(installed_version(&base, "live"), "1.0.0");
        assert_eq!(installed_version(&base, "crash"), "1.0.0");
        assert_eq!(
            entry_names(&base),
            vec![".Bad.staging", ".live.other", "crash", "live"]
        );
    }

    #[test]
    fn sweep_keeps_upgrade_parked_in_this_run() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("ext");
        write_package(&base.join("wait"), "wait", "1.0.0");
        let parked = sibling(&base, "wait", PENDING_SUFFIX);
        write_package(&parked, "wait", "2.0.0");

        sweep_leftovers_in(&base, std::slice::from_ref(&parked));
        assert_eq!(fs::read_to_string(parked.join("main.js")).unwrap(), "2.0.0");
        assert_eq!(installed_version(&base, "wait"), "1.0.0");
    }

    #[test]
    fn sweep_of_missing_dir_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        sweep_leftovers_in(&tmp.path().join("absent"), &[]);
    }

    #[test]
    fn uninstall_of_missing_item_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("ext");
        fs::create_dir_all(&base).unwrap();
        assert!(uninstall_in(&base, "ghost").is_err());
    }
}
