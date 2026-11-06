//! Crash-safe replacement of small files (config, JSON sidecars, steering
//! docs).
//!
//! `std::fs::write` truncates the target and then writes it, so a crash,
//! a force quit, or the updater's deliberate `process::exit` between the
//! two leaves an empty or half-written file. Readers then fail on it for
//! good (an extension's stored API key becomes unparseable, a torn
//! `mcp.json` reads as `{}` and the next registration wipes the user's
//! other servers).
//!
//! [`write`] instead writes a sibling temp file, fsyncs it, and renames
//! it over the target. A reader sees either the old contents or the new
//! ones, never a mix:
//!
//! - The temp file lives in the target's directory, so the rename never
//!   crosses volumes (a cross-volume "rename" is a copy, which isn't
//!   atomic).
//! - `std::fs::rename` replaces an existing target: `rename(2)` on Unix,
//!   a replace-existing rename (`MoveFileExW(MOVEFILE_REPLACE_EXISTING)`
//!   or its POSIX-semantics successor) on Windows. On Windows the rename
//!   fails while another process (antivirus, the indexer, a sync client)
//!   has the fresh temp or the target open without delete sharing, which
//!   usually clears quickly, so it is retried for up to about a second.
//! - A symlinked target (dotfiles setups) is resolved first so the link
//!   survives and the file it points at is the one replaced. A dangling
//!   link is followed the way `fs::write` would: the file is created where
//!   it points, or the write fails; the link is never replaced.
//! - Temp names are `<file name>.tmp.<pid>.<seq>`. A crash can leave one
//!   behind; the first write of each target in a process removes any left
//!   by other processes ([`remove_stale_temps`]). On the failure path this
//!   process removes its own temp (retrying while it is in use); one that
//!   still can't be removed is swept by the next launch.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Permissions given to the replaced file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perms {
    /// Owner read/write only (`0600`) on Unix, whatever the file had
    /// before. For files that can hold secrets (extension data, config).
    Private,
    /// Keep the existing file's permissions; a new file gets the default
    /// for `File::create`. For files that belong to the user or to another
    /// tool (`mcp.json`, steering docs).
    Inherit,
}

/// Infix between the target's file name and the `<pid>.<seq>` suffix.
const TMP_INFIX: &str = ".tmp.";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Atomically replace `path` with `contents`. The parent directory must
/// exist. See the module docs for the guarantees.
pub fn write(path: &Path, contents: impl AsRef<[u8]>, perms: Perms) -> io::Result<()> {
    let target = resolve_symlink(path)?;
    let (dir, name) = split(&target)?;
    sweep_once(&dir, &name);

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{name}{TMP_INFIX}{}.{seq}", std::process::id()));
    let result = write_temp(&tmp, &target, contents.as_ref(), perms)
        .and_then(|()| rename_replacing(&tmp, &target));
    if result.is_err() {
        // Don't leave our own temp behind: only a later process sweeps it.
        remove_temp(&tmp);
        return result;
    }
    sync_dir(&dir);
    Ok(())
}

/// Remove temp files that an interrupted [`write`] by another process left
/// next to `<dir>/<file_name>`. Temps of the current process are left
/// alone: one may belong to a write in flight on another thread.
pub fn remove_stale_temps(dir: &Path, file_name: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let me = std::process::id();
    for entry in entries.flatten() {
        let entry_name = entry.file_name();
        let Some(entry_name) = entry_name.to_str() else {
            continue;
        };
        if temp_owner_pid(entry_name, file_name).is_none_or(|pid| pid == me) {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => log::info!("Removed stale temp file {:?}", entry.path()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Failed to remove stale temp {:?}: {}", entry.path(), e),
        }
    }
}

/// Whether `file_name` is a temp file [`write`] creates (for any target).
/// Directory walkers (backup export) use this to skip leftovers.
pub fn is_temp_file_name(file_name: &str) -> bool {
    // `<target>.tmp.<pid>.<seq>`: split at the last TMP_INFIX so a target
    // name that itself contains ".tmp." still parses.
    file_name
        .rfind(TMP_INFIX)
        .is_some_and(|i| i > 0 && parse_suffix(&file_name[i + TMP_INFIX.len()..]).is_some())
}

/// The pid embedded in `entry_name` if it is a temp file of `target_name`.
fn temp_owner_pid(entry_name: &str, target_name: &str) -> Option<u32> {
    let suffix = entry_name
        .strip_prefix(target_name)?
        .strip_prefix(TMP_INFIX)?;
    parse_suffix(suffix)
}

/// Parse `<pid>.<seq>` (both all-digit). Returns the pid.
fn parse_suffix(suffix: &str) -> Option<u32> {
    let (pid, seq) = suffix.split_once('.')?;
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(pid) || !all_digits(seq) {
        return None;
    }
    pid.parse().ok()
}

/// Sweep each target's directory once per process. Later writes can't
/// meet a stale temp from another process: Kage is single-instance, and
/// this process cleans up its own temps on every failure path.
fn sweep_once(dir: &Path, name: &str) {
    static SWEPT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let key = dir.join(name);
    let first = SWEPT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key);
    if first {
        remove_stale_temps(dir, name);
    }
}

/// Follow a symlinked target to the file it names, so the rename replaces
/// that file instead of the link. A dangling link resolves to where it
/// points, as `fs::write` would create the file there; if that directory
/// doesn't exist the write fails, again like `fs::write`, rather than
/// replacing the link with a regular file.
fn resolve_symlink(path: &Path) -> io::Result<PathBuf> {
    // Same bound as the kernel's ELOOP limit.
    const MAX_LINKS: usize = 40;
    let mut cur = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                if let Ok(real) = fs::canonicalize(&cur) {
                    return Ok(real);
                }
                // Dangling (or not resolvable): step through it by hand.
                let dest = fs::read_link(&cur)?;
                cur = match cur.parent() {
                    Some(parent) if dest.is_relative() => parent.join(dest),
                    _ => dest,
                };
            }
            Ok(_) => return Ok(cur),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // `cur` doesn't exist: write it, if its directory does (a
                // plain path with a missing directory fails on its own).
                return match cur.parent() {
                    _ if cur == path => Ok(cur),
                    Some(p) if p.as_os_str().is_empty() || p.is_dir() => Ok(cur),
                    _ => Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "{} is a symlink to {}, whose directory doesn't exist",
                            path.display(),
                            cur.display()
                        ),
                    )),
                };
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "too many levels of symbolic links: {}",
        path.display()
    )))
}

fn split(target: &Path) -> io::Result<(PathBuf, String)> {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a writable file path: {}", target.display()),
            )
        })?
        .to_string();
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    Ok((dir, name))
}

fn write_temp(tmp: &Path, target: &Path, contents: &[u8], perms: Perms) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    // create_new: never write through a file someone else put at our name.
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if perms == Perms::Private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(tmp)?;
    #[cfg(unix)]
    if perms == Perms::Inherit {
        if let Ok(meta) = fs::metadata(target) {
            f.set_permissions(meta.permissions())?;
        }
    }
    #[cfg(not(unix))]
    let _ = (target, perms);
    f.write_all(contents)?;
    f.sync_all()?;
    // `f` drops (closes) here: Windows won't rename a file with an open
    // handle.
    Ok(())
}

/// Windows' "the file is open elsewhere" family: ACCESS_DENIED (mapped to
/// PermissionDenied), ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION
/// (33). std maps the last two to no specific `ErrorKind`. Always false
/// off Windows.
pub(crate) fn is_file_in_use(e: &io::Error) -> bool {
    cfg!(windows)
        && (e.kind() == io::ErrorKind::PermissionDenied
            || matches!(e.raw_os_error(), Some(32 | 33)))
}

/// Retry `op` while it fails with [`is_file_in_use`], backing off to about
/// a second in total. Other errors (and every error off Windows) return at
/// once.
fn retry_in_use(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    const ATTEMPTS: u64 = 8;
    for attempt in 1..ATTEMPTS {
        match op() {
            Err(e) if is_file_in_use(&e) => {
                std::thread::sleep(std::time::Duration::from_millis(25 * attempt));
            }
            other => return other,
        }
    }
    op()
}

fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    retry_in_use(|| fs::rename(from, to))
}

fn remove_temp(tmp: &Path) {
    match retry_in_use(|| fs::remove_file(tmp)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(
            "Failed to remove temp file {:?} ({}); the next launch removes it",
            tmp,
            e
        ),
    }
}

/// Persist the rename itself (the directory entry) on Unix. Best-effort:
/// the data is already safe in the temp file's inode either way.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn writes_new_and_replaces_existing() {
        let dir = tempdir();
        let path = dir.path().join("data.json");
        write(&path, b"{\"v\":1}", Perms::Inherit).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"v\":1}");
        write(&path, b"{\"v\":2}", Perms::Inherit).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"v\":2}");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("data.json")]);
    }

    #[test]
    fn failed_replace_keeps_the_old_target_and_leaves_no_temp() {
        let dir = tempdir();
        // A non-empty directory where the write wants a file: the temp is
        // written in full, then the rename fails.
        let target = dir.path().join("blocked");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("x"), b"x").unwrap();
        assert!(write(&target, b"new", Perms::Inherit).is_err());
        assert_eq!(fs::read_to_string(target.join("x")).unwrap(), "x");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("blocked")],
            "temp left behind"
        );
    }

    #[test]
    fn stale_temps_from_other_processes_are_removed() {
        let dir = tempdir();
        let path = dir.path().join("k.json");
        fs::write(&path, b"{\"intact\":true}").unwrap();
        // An interrupted write by an earlier process: a truncated temp.
        let other_pid = std::process::id().wrapping_add(1);
        let stale = dir.path().join(format!("k.json.tmp.{other_pid}.7"));
        fs::write(&stale, b"{\"inta").unwrap();
        // Our own in-flight temp and look-alikes must survive the sweep.
        let mine = dir
            .path()
            .join(format!("k.json.tmp.{}.3", std::process::id()));
        fs::write(&mine, b"x").unwrap();
        let not_a_temp = dir.path().join("k.json.tmp.backup");
        fs::write(&not_a_temp, b"x").unwrap();
        let other_target = dir.path().join(format!("j.json.tmp.{other_pid}.1"));
        fs::write(&other_target, b"x").unwrap();

        // The reader never sees the torn temp.
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"intact\":true}");

        remove_stale_temps(dir.path(), "k.json");
        assert!(!stale.exists());
        assert!(mine.exists());
        assert!(not_a_temp.exists());
        assert!(other_target.exists(), "only k.json's temps are swept");
    }

    #[test]
    fn first_write_of_a_target_sweeps_its_stale_temps() {
        let dir = tempdir();
        let path = dir.path().join("s.json");
        let stale = dir.path().join(format!(
            "s.json.tmp.{}.0",
            std::process::id().wrapping_add(9)
        ));
        fs::write(&stale, b"").unwrap();
        write(&path, b"{}", Perms::Inherit).unwrap();
        assert!(!stale.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
    }

    #[test]
    fn temp_name_parsing() {
        assert!(is_temp_file_name("config.json.tmp.123.0"));
        assert!(is_temp_file_name("a.tmp.b.json.tmp.1.2"));
        assert!(!is_temp_file_name("config.json"));
        assert!(!is_temp_file_name("config.json.tmp"));
        assert!(!is_temp_file_name("config.json.tmp.1"));
        assert!(!is_temp_file_name("config.json.tmp.1.json"));
        assert!(!is_temp_file_name(".tmp.1.2"));
        assert_eq!(temp_owner_pid("x.json.tmp.42.9", "x.json"), Some(42));
        assert_eq!(temp_owner_pid("x.json.tmp.42.9", "y.json"), None);
        assert_eq!(temp_owner_pid("x.json.tmp.4a.9", "x.json"), None);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only_and_inherit_keeps_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir();
        let secret = dir.path().join("secret.json");
        fs::write(&secret, b"old").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
        write(&secret, b"new", Perms::Private).unwrap();
        let mode = fs::metadata(&secret).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let shared = dir.path().join("shared.json");
        fs::write(&shared, b"old").unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o640)).unwrap();
        write(&shared, b"new", Perms::Inherit).unwrap();
        let mode = fs::metadata(&shared).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_written_through_not_replaced() {
        let dir = tempdir();
        let real = dir.path().join("dotfiles").join("mcp.json");
        fs::create_dir(real.parent().unwrap()).unwrap();
        let link = dir.path().join("mcp.json");
        std::os::unix::fs::symlink("dotfiles/mcp.json", &link).unwrap();
        write(&link, b"{}", Perms::Inherit).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "{}");

        // Pointing into a directory that's gone: fail, keep the link.
        let gone = dir.path().join("gone.json");
        std::os::unix::fs::symlink(dir.path().join("nope").join("x.json"), &gone).unwrap();
        assert!(write(&gone, b"{}", Perms::Inherit).is_err());
        assert!(fs::symlink_metadata(&gone)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn in_use_errors_are_windows_only() {
        let sharing = io::Error::from_raw_os_error(32);
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(is_file_in_use(&sharing), cfg!(windows));
        assert_eq!(is_file_in_use(&denied), cfg!(windows));
        assert!(!is_file_in_use(&io::Error::from(io::ErrorKind::NotFound)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_target_keeps_the_link() {
        let dir = tempdir();
        let real = dir.path().join("real.json");
        fs::write(&real, b"old").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write(&link, b"new", Perms::Inherit).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    }
}
