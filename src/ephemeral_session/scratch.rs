//! The empty working folder strict sessions run in:
//! `<data_local_dir>/kage/agent-scratch/`.
//!
//! An agent reads project files and project instructions from a session's
//! working folder. A background call must not see the user's, so its
//! session starts in this Kage-owned folder, which is owner-only on Unix
//! and always empty. Anything in it after a call is a sign a tool ran: the
//! host logs a warning and empties it.

use log::warn;
use std::io;
use std::path::{Path, PathBuf};

/// Folder name under `<data_local_dir>/kage/`.
pub const DIR_NAME: &str = "agent-scratch";

/// Where the scratch folder lives for this user. Doesn't create it.
pub fn scratch_dir() -> Option<PathBuf> {
    crate::paths::data_local_dir().map(|d| d.join("kage").join(DIR_NAME))
}

/// Create the scratch folder if needed (owner-only on Unix) and return it.
pub fn prepare_scratch_dir() -> io::Result<PathBuf> {
    let dir = scratch_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no local data folder"))?;
    ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Make `dir` a real, owner-only folder. Something else at that path (a
/// file, or a link that could point into the user's files) is removed
/// first, so the agent never starts somewhere Kage didn't choose.
pub(super) fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(md) if md.is_dir() => {}
        Ok(_) => {
            warn!("The agent scratch path is not a folder; replacing it");
            remove_non_dir(dir)?;
            create_private(dir)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_private(dir)?,
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn create_private(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

/// Remove a file or a link. A link to a folder is a directory entry on
/// Windows, removed with `remove_dir`; neither call follows the link.
fn remove_non_dir(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path).or_else(|e| std::fs::remove_dir(path).map_err(|_| e))
}

/// Remove everything inside `dir` and return how many entries were there.
/// Links are removed, never followed. A missing folder holds nothing.
pub(super) fn empty_dir(dir: &Path) -> io::Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut count = 0;
    let mut first_error = None;
    for entry in entries {
        let entry = entry?;
        count += 1;
        let path = entry.path();
        let removed = match std::fs::symlink_metadata(&path) {
            Ok(md) if md.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => remove_non_dir(&path),
            Err(e) => Err(e),
        };
        if let Err(e) = removed {
            first_error.get_or_insert(e);
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(count),
    }
}

/// Empty `dir`, logging a warning if anything was in it. `when` says which
/// side of the call this is, for the log line. Never logs file names: they
/// could carry the user's content.
pub(super) fn check_and_empty(dir: &Path, session_id: &str, when: &str) {
    match empty_dir(dir) {
        Ok(0) => {}
        Ok(n) => warn!(
            "The agent scratch folder held {} entries {} strict session {}; a tool may have run. Emptied it.",
            n, when, session_id
        ),
        Err(e) => warn!(
            "Failed to empty the agent scratch folder {} strict session {}: {}",
            when, session_id, e
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scratch_dir_is_under_kage_local_data() {
        let dir = scratch_dir().unwrap();
        assert!(dir.ends_with(Path::new("kage").join(DIR_NAME)));
        assert!(crate::paths::is_under_isolation_root(&dir));
    }

    #[test]
    fn empty_dir_removes_files_folders_and_counts_them() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scratch");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(empty_dir(&dir).unwrap(), 0);

        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("sub").join("deeper")).unwrap();
        std::fs::write(dir.join("sub").join("deeper").join("f"), b"x").unwrap();
        assert_eq!(empty_dir(&dir).unwrap(), 2);
        assert!(dir.is_dir(), "the folder itself stays");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn empty_dir_of_a_missing_folder_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(empty_dir(&tmp.path().join("missing")).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_removed_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scratch");
        let outside = tmp.path().join("users-files");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
        ensure_private_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();

        assert_eq!(empty_dir(&dir).unwrap(), 1);
        assert!(outside.join("keep.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_scratch_dir_is_owner_only_and_never_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("kage").join(DIR_NAME);
        ensure_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        // A looser folder is tightened.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        // A link planted in its place is replaced by a real folder.
        let outside = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, &dir).unwrap();
        ensure_private_dir(&dir).unwrap();
        let md = std::fs::symlink_metadata(&dir).unwrap();
        assert!(md.is_dir() && !md.file_type().is_symlink());
        assert!(outside.is_dir(), "the link's target is left alone");
    }

    #[test]
    fn a_file_in_place_of_the_folder_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scratch");
        std::fs::write(&dir, b"not a folder").unwrap();
        ensure_private_dir(&dir).unwrap();
        assert!(dir.is_dir());
    }
}
