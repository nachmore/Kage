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
//! [`write`] instead writes a sibling temp file, fsyncs it, renames it
//! over the target and fsyncs the directory. A reader sees either the old
//! contents or the new ones, never a mix:
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
//! - `Ok` means the new contents are durable: the temp's data is synced
//!   before the rename and, on Unix, the directory entry after it. The
//!   directory is opened before anything is written, so one that can't be
//!   synced (no read permission) fails the write without touching the
//!   target. Sync errors fail the write too, except on a filesystem that
//!   can't sync at all, where the step is skipped explicitly
//!   ([`sync_file`]); Windows has no directory fsync (see [`DirSync`]).
//! - A symlinked target (dotfiles setups) is resolved first so the link
//!   survives and the file it points at is the one replaced. A dangling
//!   link is followed the way `fs::write` would: the file is created where
//!   it points, or the write fails; the link is never replaced.
//! - Temp names are `.<file name>.kage-tmp-<16 random hex digits>`, a
//!   namespace of Kage's own (see [`temp_target`]). While a temp exists its
//!   writer holds an exclusive advisory lock on it. A crash can leave one
//!   behind; writes of the same target sweep those up
//!   ([`remove_stale_temps`]), but only temps that are Kage's (that name
//!   shape, a regular file, owned by this user), at least
//!   [`STALE_TEMP_AGE`] old, and not locked by a live writer (where the
//!   filesystem has advisory locks; elsewhere the age decides). On the
//!   failure path a writer removes its own temp (retrying while it is in
//!   use); one that still can't be removed is swept later.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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

/// Marker between the target's name and the random suffix of a temp.
const TMP_MARKER: &str = ".kage-tmp-";
/// Hex digits in a temp's random suffix (64 bits).
const TMP_SUFFIX_LEN: usize = 16;

/// How old a temp must be before a sweep may remove it. A write takes
/// milliseconds, so anything this old was left by a crash; the age check
/// also covers the moments a live writer's temp is unlocked (between
/// creating and locking it, and on Windows between closing and renaming
/// it), and is all there is on filesystems without advisory locks.
pub const STALE_TEMP_AGE: Duration = Duration::from_secs(10 * 60);

/// Atomically replace `path` with `contents`. The parent directory must
/// exist. See the module docs for the guarantees.
pub fn write(path: &Path, contents: impl AsRef<[u8]>, perms: Perms) -> io::Result<()> {
    let target = resolve_symlink(path)?;
    let (dir, name) = split(&target)?;
    // Before anything is written: a directory we can't sync can't make the
    // rename durable, so fail while the target is still untouched.
    let dir_sync = DirSync::open(&dir)?;
    sweep_if_due(&dir, &name);

    let tmp = dir.join(temp_name(&name));
    let result = write_temp(&tmp, &target, contents.as_ref(), perms).and_then(|file| {
        // Windows won't rename a file with an open handle. Elsewhere the
        // handle, and with it the lock, is held until the temp is gone.
        #[cfg(windows)]
        drop(file);
        let renamed = rename_replacing(&tmp, &target);
        #[cfg(not(windows))]
        drop(file);
        renamed
    });
    if result.is_err() {
        // Don't leave our own temp behind for a sweep to find.
        remove_temp(&tmp);
        return result;
    }
    dir_sync.sync()
}

/// Remove temps an interrupted [`write`] of `<dir>/<file_name>` left
/// behind: Kage's own (see [`temp_target`]), at least [`STALE_TEMP_AGE`]
/// old, and not locked by a writer that's still running.
pub fn remove_stale_temps(dir: &Path, file_name: &str) {
    remove_stale_temps_older_than(dir, file_name, STALE_TEMP_AGE);
}

fn remove_stale_temps_older_than(dir: &Path, file_name: &str, min_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_name = entry.file_name();
        if entry_name.to_str().and_then(temp_target) != Some(file_name) {
            continue;
        }
        let path = entry.path();
        match remove_if_stale(&path, min_age) {
            Ok(true) => log::info!("Removed stale temp file {:?}", path),
            Ok(false) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Failed to remove stale temp {:?}: {}", path, e),
        }
    }
}

/// Remove the temp at `path` if no live writer can still own it. Returns
/// whether it was removed.
fn remove_if_stale(path: &Path, min_age: Duration) -> io::Result<bool> {
    // Not followed: a link planted at a temp's name is never ours.
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || !owned_by_current_user(&meta) {
        return Ok(false);
    }
    // A clock that went backwards reads as brand new: keep it.
    let age = meta.modified()?.elapsed().unwrap_or(Duration::ZERO);
    if age < min_age {
        return Ok(false);
    }
    // Read-write where allowed: NFS emulates flock with fcntl locks, which
    // need a writable descriptor for an exclusive lock.
    let file = match open_no_follow(path, true).or_else(|_| open_no_follow(path, false)) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            // Can't open it at all (an Inherit temp of a target with no
            // owner access): its lock can't be checked, the age has to do.
            fs::remove_file(path)?;
            return Ok(true);
        }
        Err(e) => return Err(e),
    };
    match file.try_lock() {
        Ok(()) => {}
        // A writer is still in the middle of it.
        Err(fs::TryLockError::WouldBlock) => return Ok(false),
        // No advisory locks here: the age alone has to rule a writer out.
        Err(fs::TryLockError::Error(e)) => {
            log::debug!("Can't lock temp {:?} ({}); going by its age", path, e);
        }
    }
    // Removed while still locked, so no writer can take it over meanwhile.
    fs::remove_file(path)?;
    Ok(true)
}

#[cfg(unix)]
fn owned_by_current_user(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and can't fail.
    meta.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn owned_by_current_user(_meta: &fs::Metadata) -> bool {
    // The directories Kage writes are per user; ACLs keep others out.
    true
}

fn open_no_follow(path: &Path, write: bool) -> io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true).write(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
}

/// Whether `file_name` is a temp file [`write`] creates (for any target).
/// Directory walkers (backup export) use this to skip leftovers.
pub fn is_temp_file_name(file_name: &str) -> bool {
    temp_target(file_name).is_some()
}

/// The target a temp named `entry_name` belongs to: `Some("k.json")` for
/// `.k.json.kage-tmp-0123456789abcdef`. Only that exact shape matches
/// (leading dot, the marker, 16 lowercase hex digits, nothing after), so
/// a file a user named isn't mistaken for one.
fn temp_target(entry_name: &str) -> Option<&str> {
    let rest = entry_name.strip_prefix('.')?;
    let (target, suffix) = rest.rsplit_once(TMP_MARKER)?;
    let is_suffix = suffix.len() == TMP_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (!target.is_empty() && is_suffix).then_some(target)
}

fn temp_name(target_name: &str) -> String {
    format!(".{target_name}{TMP_MARKER}{:016x}", rand::random::<u64>())
}

/// Sweep a target's directory for its stale temps at most once per
/// [`STALE_TEMP_AGE`] per process: often enough that a temp left by a
/// crash goes soon after it is old enough to remove, rarely enough that
/// a busy file doesn't pay a directory scan on every write.
fn sweep_if_due(dir: &Path, name: &str) {
    static LAST_SWEEP: OnceLock<Mutex<HashMap<PathBuf, Instant>>> = OnceLock::new();
    let key = dir.join(name);
    let due = {
        let mut last = LAST_SWEEP
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match last.get(&key) {
            Some(at) if now.duration_since(*at) < STALE_TEMP_AGE => false,
            _ => {
                last.insert(key, now);
                true
            }
        }
    };
    if due {
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

/// Create the temp, lock it, and write and sync `contents`. Returns the
/// still-open (still-locked) handle.
fn write_temp(tmp: &Path, target: &Path, contents: &[u8], perms: Perms) -> io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    // create_new: never write through a file someone else put at our name.
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if perms == Perms::Private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(tmp)?;
    // Tells a sweep in another process that this temp is live. A fresh
    // file nobody else has opened: the only failure is a filesystem
    // without advisory locks, where the sweep's age check still protects
    // it.
    if let Err(e) = f.try_lock() {
        log::debug!("Can't lock temp {:?}: {}", tmp, e);
    }
    #[cfg(unix)]
    if perms == Perms::Inherit {
        if let Ok(meta) = fs::metadata(target) {
            f.set_permissions(meta.permissions())?;
        }
    }
    #[cfg(not(unix))]
    let _ = (target, perms);
    f.write_all(contents)?;
    sync_file(&f)?;
    Ok(f)
}

/// `File::sync_all`, except on a filesystem that can't sync. On macOS
/// `sync_all` issues `F_FULLFSYNC` (a flush through the drive's cache),
/// which some filesystems refuse (SMB, some FUSE mounts); a plain `fsync`
/// is tried then. Where the filesystem can't sync at all (EINVAL /
/// ENOTSUP / EOPNOTSUPP / ENOTTY) the step is skipped: there is nothing
/// more this process can do, and failing would leave such a volume
/// unwritable (`fs::write`, which never syncs, worked there). Any other
/// error is returned.
#[cfg(unix)]
fn sync_file(f: &fs::File) -> io::Result<()> {
    let err = match f.sync_all() {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if !sync_unsupported(&err) {
        return Err(err);
    }
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `f` owns this open descriptor for the whole call.
        if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let plain = io::Error::last_os_error();
        if !sync_unsupported(&plain) {
            return Err(plain);
        }
    }
    log::debug!("Filesystem can't sync ({}); skipped", err);
    Ok(())
}

#[cfg(not(unix))]
fn sync_file(f: &fs::File) -> io::Result<()> {
    f.sync_all()
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
            "Failed to remove temp file {:?} ({}); a later write removes it",
            tmp,
            e
        ),
    }
}

/// Makes the rename durable by syncing the directory that holds it.
///
/// Unix: an `fsync` of the directory ([`sync_file`]). Opening or syncing
/// it fails the write, except on a filesystem that has no directory sync,
/// which says so with EINVAL / ENOTSUP / EOPNOTSUPP (/ ENOTTY for
/// `F_FULLFSYNC`): that one step is skipped, because there is nothing more
/// this process can do.
///
/// Windows: skipped. A directory can't be opened for a flush through std,
/// and NTFS records the rename in its metadata journal, so there is no
/// directory step to take.
struct DirSync {
    #[cfg(unix)]
    dir: fs::File,
}

impl DirSync {
    #[cfg(unix)]
    fn open(dir: &Path) -> io::Result<Self> {
        let dir = fs::File::open(dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("can't open {} to sync it: {}", dir.display(), e),
            )
        })?;
        Ok(Self { dir })
    }

    #[cfg(not(unix))]
    fn open(_dir: &Path) -> io::Result<Self> {
        Ok(Self {})
    }

    #[cfg(unix)]
    fn sync(&self) -> io::Result<()> {
        sync_file(&self.dir)
    }

    #[cfg(not(unix))]
    fn sync(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_unsupported(e: &io::Error) -> bool {
    // Compared, not matched: ENOTSUP and EOPNOTSUPP are one value on Linux.
    e.kind() == io::ErrorKind::Unsupported
        || e.raw_os_error().is_some_and(|c| {
            [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOTTY].contains(&c)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    /// Create `path` with an mtime an hour ago: past the stale age.
    fn old_file(path: &Path, body: &[u8]) {
        fs::write(path, body).unwrap();
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
    }

    const SUFFIX: &str = "0123456789abcdef";

    #[test]
    fn writes_new_and_replaces_existing() {
        let dir = tempdir();
        let path = dir.path().join("data.json");
        write(&path, b"{\"v\":1}", Perms::Inherit).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"v\":1}");
        write(&path, b"{\"v\":2}", Perms::Inherit).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"v\":2}");
        assert_eq!(names(dir.path()), vec!["data.json"]);
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
        assert_eq!(names(dir.path()), vec!["blocked"], "temp left behind");
    }

    #[test]
    fn stale_unlocked_temps_are_removed_and_nothing_else() {
        let dir = tempdir();
        let path = dir.path().join("k.json");
        fs::write(&path, b"{\"intact\":true}").unwrap();
        // Left by a crashed writer an hour ago: a truncated temp.
        let stale = dir.path().join(format!(".k.json{TMP_MARKER}{SUFFIX}"));
        old_file(&stale, b"{\"inta");
        // A crash a minute ago, or a writer between creating and locking
        // its temp: too young to be sure, kept.
        let fresh = dir
            .path()
            .join(format!(".k.json{TMP_MARKER}fedcba9876543210"));
        fs::write(&fresh, b"x").unwrap();
        // User files that look a bit like temps, all old enough.
        let lookalikes = [
            "k.json.tmp.123.4".to_string(), // the shape temps used to have
            "k.json.tmp.backup".to_string(),
            format!("k.json{TMP_MARKER}{SUFFIX}"), // no leading dot
            format!(".k.json{TMP_MARKER}notes"),
            // Uppercase hex. (Not SUFFIX uppercased: on a case-insensitive
            // volume that would be the stale temp itself.)
            format!(".k.json{TMP_MARKER}ABCDEF0123456789"),
            format!(".k.json{TMP_MARKER}{SUFFIX}.bak"),
            format!(".k.json{TMP_MARKER}{SUFFIX}0"),
            format!(".j.json{TMP_MARKER}{SUFFIX}"), // another target's temp
        ];
        for n in &lookalikes {
            old_file(&dir.path().join(n), b"mine");
        }
        // A directory at a temp's name is never ours either.
        let dir_at_name = dir
            .path()
            .join(format!(".k.json{TMP_MARKER}aaaaaaaaaaaaaaaa"));
        fs::create_dir(&dir_at_name).unwrap();

        // The reader never sees the torn temp.
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"intact\":true}");

        remove_stale_temps(dir.path(), "k.json");
        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(dir_at_name.is_dir());
        for n in &lookalikes {
            assert_eq!(
                fs::read(dir.path().join(n)).unwrap(),
                b"mine",
                "{n} removed"
            );
        }
    }

    #[test]
    fn a_live_writers_locked_temp_survives_a_sweep() {
        // The regression: a second writer's sweep deleted a live writer's
        // fsynced temp, whose rename then failed with ENOENT.
        let dir = tempdir();
        let path = dir.path().join("live.json");
        fs::write(&path, b"old").unwrap();
        let tmp = dir.path().join(format!(".live.json{TMP_MARKER}{SUFFIX}"));
        old_file(&tmp, b"new");
        // The first writer holds its lock while the temp exists. Old mtime
        // on purpose: the lock alone must protect it (a slow disk, a
        // clock change).
        let writer = fs::File::options().write(true).open(&tmp).unwrap();
        writer.try_lock().unwrap();

        // A second writer of the same target sweeps on its first write.
        write(&path, b"second", Perms::Private).unwrap();
        assert!(tmp.exists(), "the live writer's temp was removed");

        // The first writer finishes: its rename still works.
        rename_replacing(&tmp, &path).unwrap();
        drop(writer);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(names(dir.path()), vec!["live.json"]);
    }

    #[test]
    fn writes_sweep_their_targets_stale_temps() {
        let dir = tempdir();
        let path = dir.path().join("s.json");
        let stale = dir.path().join(format!(".s.json{TMP_MARKER}{SUFFIX}"));
        old_file(&stale, b"");
        write(&path, b"{}", Perms::Inherit).unwrap();
        assert!(!stale.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
    }

    #[test]
    fn temp_names_are_in_kages_namespace() {
        let name = temp_name("config.json");
        assert_eq!(temp_target(&name), Some("config.json"));
        assert!(name.starts_with(".config.json.kage-tmp-"));
        assert_ne!(name, temp_name("config.json"), "random suffix");

        assert!(is_temp_file_name(&format!(
            ".a.kage-tmp-x.json{TMP_MARKER}{SUFFIX}"
        )));
        assert_eq!(
            temp_target(&format!("..hidden{TMP_MARKER}{SUFFIX}")),
            Some(".hidden")
        );
        assert!(!is_temp_file_name("config.json"));
        assert!(!is_temp_file_name("config.json.tmp.123.0"));
        assert!(!is_temp_file_name(&format!(".{TMP_MARKER}{SUFFIX}")));
        assert!(!is_temp_file_name(&format!(
            ".x{TMP_MARKER}{}",
            &SUFFIX[1..]
        )));
        assert!(!is_temp_file_name(&format!(
            ".x{TMP_MARKER}{}g",
            &SUFFIX[1..]
        )));
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
    fn a_directory_that_cannot_be_synced_fails_the_write_untouched() {
        // The regression: in a write-only (0300) directory the rename went
        // through, the directory couldn't be opened for its fsync, and the
        // write still reported success.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir();
        let sub = dir.path().join("wx-only");
        fs::create_dir(&sub).unwrap();
        let path = sub.join("c.json");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o300)).unwrap();
        let readable = fs::File::open(&sub).is_ok(); // root reads anything
        let result = write(&path, b"new", Perms::Private);
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o700)).unwrap();
        if readable {
            return;
        }
        let err = result.expect_err("an unsyncable directory must fail the write");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "old", "target touched");
        assert_eq!(names(&sub), vec!["c.json"], "temp left behind");
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_directory_sync_is_told_apart_from_real_errors() {
        for code in [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOTTY] {
            assert!(sync_unsupported(&io::Error::from_raw_os_error(code)));
        }
        for code in [libc::EIO, libc::EACCES, libc::ENOSPC, libc::EBADF] {
            assert!(!sync_unsupported(&io::Error::from_raw_os_error(code)));
        }
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

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_a_temps_name_is_never_removed() {
        let dir = tempdir();
        let victim = dir.path().join("victim.txt");
        fs::write(&victim, b"keep").unwrap();
        let link = dir.path().join(format!(".k.json{TMP_MARKER}{SUFFIX}"));
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        remove_stale_temps_older_than(dir.path(), "k.json", Duration::ZERO);
        assert!(fs::symlink_metadata(&link).is_ok());
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
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
