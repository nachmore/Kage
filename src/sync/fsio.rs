//! Folder I/O for sync (docs/design/SYNC.md §5, §14.2). The engine reaches
//! the sync folder only through [`SyncFs`], so tests can run several
//! computers against one simulated cloud and inject faults.
//!
//! [`RealFs`] runs every call on the `kage-sync-io` pool: 2 threads, each
//! call with a timeout. A call stuck in a cloud driver is abandoned and its
//! thread replaced; at most 4 threads are ever abandoned at once. Past
//! that every call fails at once with [`Unresponsive`] until a stuck
//! thread comes back, and the engine pauses sync (§15).
//!
//! Rules every call keeps:
//! - A link (symlink, junction) inside the sync folder is never followed:
//!   a directory below the folder's root that is a link fails the call,
//!   a listed link is left out, and a read refuses anything but a regular
//!   file (§11.7).
//! - `write_once` writes `~kage-<16 hex>.tmp`, syncs it, and renames it to
//!   a name that must not exist yet. Windows sharing violations (OneDrive,
//!   antivirus) are retried for about 5 seconds.

use super::crypto::is_id_text;
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The timeout callers use for one file read (§14.2).
pub const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// The timeout of one directory listing.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// The timeout of a write, rename, remove or directory creation.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
/// Threads that do folder I/O.
pub const IO_THREADS: usize = 2;
/// Stuck threads the pool gives up on before it stops taking calls.
pub const MAX_ABANDONED: usize = 4;
/// How long `write_once` and friends retry a Windows sharing violation.
const SHARING_RETRY_BUDGET: Duration = Duration::from_secs(5);
const SHARING_RETRY_FIRST: Duration = Duration::from_millis(50);

/// One directory entry. File times are hints, never facts (§3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub len: u64,
    pub modified_ms: Option<i64>,
    pub is_dir: bool,
}

/// What the engine may do in the sync folder.
pub trait SyncFs: Send + Sync {
    /// The files and directories in `dir`, by name. Links and names that
    /// aren't UTF-8 are left out.
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>>;
    /// Read a whole file of at most `max_len` bytes, giving up after
    /// `timeout`. A longer file fails with `FileTooLarge`; a timeout with
    /// `TimedOut`. Runs on the I/O pool.
    fn read(&self, path: &Path, max_len: u64, timeout: Duration) -> io::Result<Vec<u8>>;
    /// Write a new file: `~kage-*.tmp`, fsync, then rename to `name`. Fails
    /// with `AlreadyExists` if `name` exists: nothing is ever overwritten.
    fn write_once(&self, dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()>;
    /// Create a file that must not exist yet (the root files).
    fn create_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Move a file or directory to a name that must not exist yet.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Remove a file, or a directory with everything in it. A link is
    /// removed, never followed.
    fn remove(&self, path: &Path) -> io::Result<()>;
    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;
}

/// The error payload when the pool has given up on too many stuck calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unresponsive;

impl std::fmt::Display for Unresponsive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the sync folder isn't responding")
    }
}

impl std::error::Error for Unresponsive {}

/// The error payload for a file that another program holds open: what a
/// Windows sharing violation looks like in [`crate::sync::testing::FaultyFs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharingViolation;

impl std::fmt::Display for SharingViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another program has the file open")
    }
}

impl std::error::Error for SharingViolation {}

/// The error payload for a link found inside the sync folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkInFolder;

impl std::fmt::Display for LinkInFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a link inside the sync folder isn't followed")
    }
}

impl std::error::Error for LinkInFolder {}

fn has_payload<T: std::error::Error + 'static>(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<T>())
}

/// Whether `e` says the pool stopped taking calls (§14.2).
pub fn is_unresponsive(e: &io::Error) -> bool {
    has_payload::<Unresponsive>(e)
}

/// Whether `e` is a Windows sharing violation (or a fake one).
pub fn is_sharing_violation(e: &io::Error) -> bool {
    crate::atomic_write::is_file_in_use(e) || has_payload::<SharingViolation>(e)
}

/// Whether `e` says a link inside the folder was refused.
pub fn is_link_refused(e: &io::Error) -> bool {
    has_payload::<LinkInFolder>(e)
}

/// A fresh temp name: `~kage-<16 hex>.tmp`. Dropbox never uploads `~*.tmp`.
pub fn temp_name() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::rng().fill_bytes(&mut b);
    format!("~kage-{}.tmp", hex::encode(b))
}

/// Retry `op` while it fails with a sharing violation: 50 ms, doubling,
/// for about [`SHARING_RETRY_BUDGET`] in all. `sleep` is a parameter so
/// the policy can be tested.
fn retry_sharing<T>(
    mut op: impl FnMut() -> io::Result<T>,
    sleep: &dyn Fn(Duration),
) -> io::Result<T> {
    let mut waited = Duration::ZERO;
    let mut delay = SHARING_RETRY_FIRST;
    loop {
        match op() {
            Err(e) if is_sharing_violation(&e) && waited < SHARING_RETRY_BUDGET => {
                let d = delay.min(SHARING_RETRY_BUDGET - waited);
                sleep(d);
                waited += d;
                delay *= 2;
            }
            other => return other,
        }
    }
}

fn real_sleep(d: Duration) {
    std::thread::sleep(d);
}

// --- The I/O pool ------------------------------------------------------------

const QUEUED: u8 = 0;
const RUNNING: u8 = 1;
const DONE: u8 = 2;
const CANCELLED: u8 = 3;
const ABANDONED: u8 = 4;

type Job = Box<dyn FnOnce() + Send>;

struct Task {
    /// Changes only under the pool's lock.
    status: AtomicU8,
    job: Mutex<Option<Job>>,
}

#[derive(Default)]
struct PoolState {
    queue: VecDeque<Arc<Task>>,
    /// Threads alive, abandoned ones included.
    threads: usize,
    /// Threads still running a call that timed out.
    abandoned: usize,
}

struct Shared {
    state: Mutex<PoolState>,
    work: Condvar,
}

/// How the I/O pool is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Threads alive, abandoned ones included. Never above
    /// `IO_THREADS + MAX_ABANDONED`.
    pub threads: usize,
    pub abandoned: usize,
}

impl PoolStats {
    /// Whether the pool still takes calls.
    pub fn responding(&self) -> bool {
        self.abandoned < MAX_ABANDONED
    }
}

/// Threads that run blocking calls with a timeout. Threads start on first
/// use, never at app start.
struct IoPool {
    shared: Arc<Shared>,
}

fn lock(m: &Mutex<PoolState>) -> MutexGuard<'_, PoolState> {
    // A job's panic is caught before it can poison this lock; recover
    // anyway rather than wedge folder I/O for the rest of the session.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl IoPool {
    fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(PoolState::default()),
                work: Condvar::new(),
            }),
        }
    }

    fn stats(&self) -> PoolStats {
        let st = lock(&self.shared.state);
        PoolStats {
            threads: st.threads,
            abandoned: st.abandoned,
        }
    }

    /// Start a thread if fewer than [`IO_THREADS`] are free to work and the
    /// cap allows one more.
    fn top_up(&self, st: &mut PoolState) -> io::Result<()> {
        while st.threads - st.abandoned < IO_THREADS && st.threads < IO_THREADS + MAX_ABANDONED {
            let shared = self.shared.clone();
            std::thread::Builder::new()
                .name("kage-sync-io".into())
                .spawn(move || worker(shared))?;
            st.threads += 1;
        }
        Ok(())
    }

    /// Run `f` on a pool thread and wait at most `timeout` for it.
    fn run<T: Send + 'static>(
        &self,
        timeout: Duration,
        f: impl FnOnce() -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let (tx, rx) = mpsc::sync_channel::<io::Result<T>>(1);
        let task = Arc::new(Task {
            status: AtomicU8::new(QUEUED),
            job: Mutex::new(Some(Box::new(move || {
                let _ = tx.send(f());
            }))),
        });
        {
            let mut st = lock(&self.shared.state);
            if st.abandoned >= MAX_ABANDONED {
                return Err(io::Error::new(io::ErrorKind::TimedOut, Unresponsive));
            }
            self.top_up(&mut st)?;
            st.queue.push_back(task.clone());
        }
        self.shared.work.notify_one();
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(io::Error::other("the folder I/O call stopped unexpectedly"))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let mut st = lock(&self.shared.state);
                // It may have finished while we waited for the lock.
                if let Ok(r) = rx.try_recv() {
                    return r;
                }
                match task.status.load(Ordering::SeqCst) {
                    QUEUED => task.status.store(CANCELLED, Ordering::SeqCst),
                    RUNNING => {
                        task.status.store(ABANDONED, Ordering::SeqCst);
                        st.abandoned += 1;
                        log::warn!(
                            "Sync folder I/O timed out after {:?}; abandoned a thread ({} of {})",
                            timeout,
                            st.abandoned,
                            MAX_ABANDONED
                        );
                        if let Err(e) = self.top_up(&mut st) {
                            log::warn!("Couldn't start a folder I/O thread: {}", e);
                        }
                    }
                    _ => {}
                }
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the sync folder didn't answer in time",
                ))
            }
        }
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let task = {
            let mut st = lock(&shared.state);
            loop {
                if let Some(t) = st.queue.pop_front() {
                    if t.status.load(Ordering::SeqCst) == CANCELLED {
                        continue;
                    }
                    t.status.store(RUNNING, Ordering::SeqCst);
                    break t;
                }
                st = shared.work.wait(st).unwrap_or_else(|p| p.into_inner());
            }
        };
        let job = task.job.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(job) = job {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                log::error!("A sync folder I/O call panicked");
            }
        }
        let mut st = lock(&shared.state);
        if task.status.load(Ordering::SeqCst) == ABANDONED {
            // A replacement already took this thread's place.
            st.abandoned -= 1;
            st.threads -= 1;
            log::info!("A stuck sync folder I/O call finished; its thread exits");
            return;
        }
        task.status.store(DONE, Ordering::SeqCst);
    }
}

// --- The real file system ----------------------------------------------------

/// The real file system, behind the `kage-sync-io` pool.
pub struct RealFs {
    pool: IoPool,
}

impl std::fmt::Debug for RealFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealFs")
            .field("pool", &self.pool.stats())
            .finish()
    }
}

impl Default for RealFs {
    fn default() -> Self {
        Self::new()
    }
}

impl RealFs {
    /// No thread starts until the first call.
    pub fn new() -> Self {
        Self {
            pool: IoPool::new(),
        }
    }

    pub fn stats(&self) -> PoolStats {
        self.pool.stats()
    }
}

impl SyncFs for RealFs {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let dir = dir.to_path_buf();
        self.pool.run(LIST_TIMEOUT, move || list_dir(&dir))
    }

    fn read(&self, path: &Path, max_len: u64, timeout: Duration) -> io::Result<Vec<u8>> {
        let path = path.to_path_buf();
        self.pool.run(timeout, move || read_file(&path, max_len))
    }

    fn write_once(&self, dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
        check_name(name)?;
        let (dir, name, bytes) = (dir.to_path_buf(), name.to_string(), bytes.to_vec());
        self.pool.run(WRITE_TIMEOUT, move || {
            write_once_now(&dir, &name, &bytes, &real_sleep)
        })
    }

    fn create_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let (path, bytes) = (path.to_path_buf(), bytes.to_vec());
        self.pool
            .run(WRITE_TIMEOUT, move || create_new_now(&path, &bytes))
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        self.pool.run(WRITE_TIMEOUT, move || rename_now(&from, &to))
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        let path = path.to_path_buf();
        self.pool.run(WRITE_TIMEOUT, move || remove_now(&path))
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        let dir = dir.to_path_buf();
        self.pool.run(WRITE_TIMEOUT, move || {
            check_no_links(&dir)?;
            fs::create_dir_all(&dir)?;
            check_no_links(&dir)
        })
    }
}

/// A name to write in a directory: one component, not `.` or `..`.
fn check_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.contains('\0')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a file name",
        ));
    }
    Ok(())
}

/// The directory names Kage makes inside the sync folder (§5.1): `keys`,
/// `devices`, a device id, `shards`, `blobs`, `archive` and an archive
/// generation.
fn is_structural(name: &str) -> bool {
    matches!(name, "keys" | "devices" | "shards" | "blobs" | "archive")
        || is_id_text(name)
        || is_archive_generation(name)
}

/// `YYYYMMDD-HHMMSS` (§13.6).
fn is_archive_generation(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 15
        && b[8] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
}

/// Refuse a link at `dir` or at any of its ancestors that has one of the
/// names Kage makes inside the folder. The walk stops at the first other
/// name: the folder's root and the user's folders above it may be links.
fn check_no_links(dir: &Path) -> io::Result<()> {
    let mut cur = Some(dir);
    while let Some(p) = cur {
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            break;
        };
        if !is_structural(name) {
            break;
        }
        match fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, LinkInFolder));
            }
            Ok(m) if !m.is_dir() => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "a sync folder directory is a file",
                ));
            }
            Ok(_) => {}
            // The call itself reports what's missing.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        cur = p.parent();
    }
    Ok(())
}

fn parent_of(path: &Path) -> io::Result<&Path> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a path without a folder"))
}

fn exists_no_follow(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn to_ms(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).ok(),
        Err(e) => i64::try_from(e.duration().as_millis()).ok().map(|ms| -ms),
    }
}

fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    check_no_links(dir)?;
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // Doesn't follow a link.
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if ft.is_symlink() || !(ft.is_dir() || ft.is_file()) {
            continue;
        }
        let (len, modified_ms) = match entry.metadata() {
            Ok(m) => (
                if ft.is_dir() { 0 } else { m.len() },
                m.modified().ok().and_then(to_ms),
            ),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => (0, None),
        };
        out.push(Entry {
            name,
            len,
            modified_ms,
            is_dir: ft.is_dir(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "the file is over its size limit",
    )
}

fn read_file(path: &Path, max_len: u64) -> io::Result<Vec<u8>> {
    #[cfg(test)]
    tests::hang_point(path);
    check_no_links(parent_of(path)?)?;
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    if meta.len() > max_len {
        return Err(too_large());
    }
    let file = fs::File::open(path)?;
    // Whatever was opened must still be a regular file.
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.take(max_len + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max_len {
        return Err(too_large());
    }
    Ok(buf)
}

/// `sync_all`, except on a filesystem that can't sync at all (some network
/// and FUSE mounts), where the step is skipped: nothing more can be done
/// there, and failing would leave the folder unwritable.
fn sync_file(f: &fs::File) -> io::Result<()> {
    match f.sync_all() {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
            ) =>
        {
            log::debug!(
                "The sync folder's filesystem can't sync files ({}); skipped",
                e
            );
            Ok(())
        }
        other => other,
    }
}

/// Make a rename durable where the OS lets a directory be synced. Best
/// effort: the file itself is already synced.
fn sync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

fn write_once_now(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    sleep: &dyn Fn(Duration),
) -> io::Result<()> {
    check_no_links(dir)?;
    let target = dir.join(name);
    if exists_no_follow(&target)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a file of that name exists",
        ));
    }
    let tmp = dir.join(temp_name());
    let result = (|| {
        // `create_new` never writes through a file or link already there.
        // A new name has no other holder, so a refusal here is real (on
        // Windows a sharing violation and "access denied" look alike):
        // only the rename and the cleanup are retried.
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        sync_file(&f)?;
        // Windows won't rename a file that is still open.
        drop(f);
        retry_sharing(|| rename_no_replace(&tmp, &target), sleep)
    })();
    if result.is_err() {
        match retry_sharing(|| fs::remove_file(&tmp), sleep) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Couldn't remove a sync temp file: {}", e),
        }
    }
    result?;
    sync_dir(dir);
    Ok(())
}

/// A rename that refuses to replace. Only this computer writes in its own
/// directory, so the check before the rename has no writer to race.
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    if exists_no_follow(to)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a file of that name exists",
        ));
    }
    fs::rename(from, to)
}

fn create_new_now(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = parent_of(path)?;
    check_no_links(dir)?;
    // No retry: a new name has no other holder (see `write_once_now`).
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let written = f.write_all(bytes).and_then(|()| sync_file(&f));
    if let Err(e) = written {
        drop(f);
        // Ours, and only half written: don't leave it for a reader.
        let _ = fs::remove_file(path);
        return Err(e);
    }
    drop(f);
    sync_dir(dir);
    Ok(())
}

fn rename_now(from: &Path, to: &Path) -> io::Result<()> {
    check_no_links(parent_of(from)?)?;
    check_no_links(parent_of(to)?)?;
    retry_sharing(|| rename_no_replace(from, to), &real_sleep)?;
    sync_dir(parent_of(to)?);
    Ok(())
}

fn remove_now(path: &Path) -> io::Result<()> {
    check_no_links(parent_of(path)?)?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        // A directory link on Windows needs `remove_dir`.
        return fs::remove_file(path).or_else(|_| fs::remove_dir(path));
    }
    if meta.is_dir() {
        // Never follows links inside it.
        return fs::remove_dir_all(path);
    }
    retry_sharing(|| fs::remove_file(path), &real_sleep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// Paths whose read blocks until released, to stand in for a cloud
    /// driver that never answers.
    static HANGING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    static RELEASED: Condvar = Condvar::new();

    pub(super) fn hang_point(path: &Path) {
        let mut hanging = HANGING.lock().unwrap();
        while hanging.iter().any(|p| p == path) {
            hanging = RELEASED.wait(hanging).unwrap();
        }
    }

    fn hang(path: &Path) {
        HANGING.lock().unwrap().push(path.to_path_buf());
    }

    fn release(prefix: &Path) {
        HANGING.lock().unwrap().retain(|p| !p.starts_with(prefix));
        RELEASED.notify_all();
    }

    fn wait_for(mut cond: impl FnMut() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out waiting"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn write_once_then_read_and_list() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = RealFs::new();
        let dir = tmp.path().join("devices/k7m2p9xqa3rt5vwe/shards");
        assert_eq!(fs.stats().threads, 0, "no thread before the first call");
        fs.create_dir_all(&dir).unwrap();
        fs.write_once(&dir, "a.kage", b"hello").unwrap();
        assert_eq!(
            fs.read(&dir.join("a.kage"), 100, READ_TIMEOUT).unwrap(),
            b"hello"
        );
        let entries = fs.list(&dir).unwrap();
        assert_eq!(entries.len(), 1, "no temp left behind: {:?}", entries);
        assert_eq!(entries[0].name, "a.kage");
        assert_eq!(entries[0].len, 5);
        assert!(!entries[0].is_dir);
        assert!(entries[0].modified_ms.is_some());
        let up = fs.list(dir.parent().unwrap()).unwrap();
        assert_eq!(up.len(), 1);
        assert!(up[0].is_dir);
        assert!(fs.stats().threads <= IO_THREADS);
    }

    #[test]
    fn write_once_never_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = RealFs::new();
        fs.write_once(tmp.path(), "x.kage", b"first").unwrap();
        let err = fs.write_once(tmp.path(), "x.kage", b"second").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(tmp.path().join("x.kage")).unwrap(), b"first");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert_eq!(
                fs.write_once(tmp.path(), bad, b"x").unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn temp_names_have_the_classified_shape() {
        let n = temp_name();
        assert!(n.starts_with("~kage-") && n.ends_with(".tmp"));
        assert_eq!(n.len(), "~kage-".len() + 16 + ".tmp".len());
        assert_eq!(
            crate::sync::folder::classify(crate::sync::folder::DirKind::Shards, &n),
            crate::sync::folder::FileClass::OwnTemp
        );
        assert_ne!(n, temp_name());
    }

    #[test]
    fn create_new_is_exclusive() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = RealFs::new();
        let p = tmp.path().join("kage-sync.json");
        fs.create_new(&p, b"{}").unwrap();
        assert_eq!(
            fs.create_new(&p, b"{\"x\":1}").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"{}");
    }

    #[test]
    fn rename_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = RealFs::new();
        let devices = tmp.path().join("devices");
        let a = devices.join("k7m2p9xqa3rt5vwe");
        fs.create_dir_all(&a.join("shards")).unwrap();
        fs.write_once(&a.join("shards"), "s.kage", b"1").unwrap();
        fs.write_once(&a, "head-000000000001.kage", b"h").unwrap();
        let archive = tmp.path().join("archive/20261109-100000");
        fs.create_dir_all(&archive).unwrap();
        fs.rename(&devices, &archive.join("devices")).unwrap();
        assert!(!devices.exists());
        assert!(archive
            .join("devices/k7m2p9xqa3rt5vwe/shards/s.kage")
            .exists());
        // No replacing on rename either.
        fs.create_dir_all(&devices).unwrap();
        assert_eq!(
            fs.rename(&devices, &archive.join("devices"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        fs.remove(&archive).unwrap();
        assert!(!archive.exists());
        assert_eq!(
            fs.remove(&archive).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn read_refuses_large_files_and_non_files() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = RealFs::new();
        std::fs::write(tmp.path().join("big.kage"), vec![1u8; 100]).unwrap();
        let err = fs
            .read(&tmp.path().join("big.kage"), 99, READ_TIMEOUT)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(
            fs.read(&tmp.path().join("big.kage"), 100, READ_TIMEOUT)
                .unwrap()
                .len(),
            100
        );
        assert_eq!(
            fs.read(tmp.path(), 100, READ_TIMEOUT).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            fs.read(&tmp.path().join("missing"), 100, READ_TIMEOUT)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_inside_the_folder_are_never_followed() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let fs = RealFs::new();
        let root = tmp.path().join("Kage Sync");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"not for sync").unwrap();
        let me = root.join("devices/k7m2p9xqa3rt5vwe");
        std::fs::create_dir_all(&me).unwrap();
        // A planted link where Kage's shards directory goes.
        symlink(&outside, me.join("shards")).unwrap();
        let shards = me.join("shards");
        let err = fs.write_once(&shards, "a.kage", b"x").unwrap_err();
        assert!(is_link_refused(&err), "{:?}", err);
        assert!(is_link_refused(&fs.list(&shards).unwrap_err()));
        assert!(is_link_refused(
            &fs.read(&shards.join("secret.txt"), 100, READ_TIMEOUT)
                .unwrap_err()
        ));
        assert!(!outside.join("a.kage").exists());
        // A linked file isn't read or listed; removing it removes the link.
        symlink(
            outside.join("secret.txt"),
            me.join("head-000000000001.kage"),
        )
        .unwrap();
        assert_eq!(
            fs.read(&me.join("head-000000000001.kage"), 100, READ_TIMEOUT)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let names: Vec<String> = fs.list(&me).unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.is_empty(), "{:?}", names);
        fs.remove(&me.join("head-000000000001.kage")).unwrap();
        fs.remove(&shards).unwrap();
        assert!(outside.join("secret.txt").exists());
        // A link at the target name blocks the write.
        std::fs::create_dir_all(&shards).unwrap();
        symlink(outside.join("secret.txt"), shards.join("b.kage")).unwrap();
        assert_eq!(
            fs.write_once(&shards, "b.kage", b"x").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            std::fs::read(outside.join("secret.txt")).unwrap(),
            b"not for sync"
        );
        // The user's folders above the root may be links.
        let linked_parent = tmp.path().join("linked");
        symlink(tmp.path(), &linked_parent).unwrap();
        let via_link = linked_parent.join("Kage Sync/devices/k7m2p9xqa3rt5vwe");
        fs.write_once(&via_link, "head-000000000002.kage", b"ok")
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("head-000000000001.kage");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        let fs = RealFs::new();
        let start = Instant::now();
        let err = fs.read(&fifo, 100, READ_TIMEOUT).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(fs.list(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn a_hanging_read_returns_after_its_timeout_and_the_pool_stays_capped() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = Arc::new(RealFs::new());
        let mut paths = Vec::new();
        for i in 0..8 {
            let p = tmp.path().join(format!("{:032x}.kage", i));
            std::fs::write(&p, b"data").unwrap();
            hang(&p);
            paths.push(p);
        }
        let timeout = Duration::from_millis(50);
        for p in &paths[..MAX_ABANDONED] {
            let start = Instant::now();
            let err = fs.read(p, 100, timeout).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::TimedOut);
            assert!(!is_unresponsive(&err));
            assert!(start.elapsed() < Duration::from_secs(5));
            let s = fs.stats();
            assert!(s.threads <= IO_THREADS + MAX_ABANDONED, "{:?}", s);
        }
        let s = fs.stats();
        assert_eq!(s.abandoned, MAX_ABANDONED);
        assert!(!s.responding());
        assert!(s.threads <= IO_THREADS + MAX_ABANDONED);
        // Now every call fails at once.
        for p in &paths[MAX_ABANDONED..] {
            let start = Instant::now();
            let err = fs.read(p, 100, Duration::from_secs(5)).unwrap_err();
            assert!(is_unresponsive(&err), "{:?}", err);
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        assert!(fs.stats().threads <= IO_THREADS + MAX_ABANDONED);
        // The driver comes back: stuck threads finish and leave.
        release(tmp.path());
        wait_for(|| fs.stats().abandoned == 0);
        assert!(fs.stats().threads <= IO_THREADS + MAX_ABANDONED);
        assert_eq!(fs.read(&paths[0], 100, READ_TIMEOUT).unwrap(), b"data");
        wait_for(|| fs.stats().threads <= IO_THREADS + MAX_ABANDONED);
    }

    /// A gate jobs wait at until a test opens it.
    struct Gate(Mutex<bool>, Condvar);

    impl Gate {
        fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(false), Condvar::new()))
        }
        fn wait(&self) {
            let mut open = self.0.lock().unwrap();
            while !*open {
                open = self.1.wait(open).unwrap();
            }
        }
        fn open(&self) {
            *self.0.lock().unwrap() = true;
            self.1.notify_all();
        }
    }

    #[test]
    fn a_queued_call_that_times_out_is_never_run() {
        let pool = Arc::new(IoPool::new());
        let gate = Gate::new();
        let started = Arc::new(AtomicUsize::new(0));
        // Both threads busy.
        let busy: Vec<_> = (0..IO_THREADS)
            .map(|_| {
                let (pool, gate, started) = (pool.clone(), gate.clone(), started.clone());
                std::thread::spawn(move || {
                    pool.run(Duration::from_secs(30), move || {
                        started.fetch_add(1, Ordering::SeqCst);
                        gate.wait();
                        Ok(())
                    })
                })
            })
            .collect();
        wait_for(|| started.load(Ordering::SeqCst) == IO_THREADS);
        // A third call waits in the queue and times out there.
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ran.clone();
        let err = pool
            .run(Duration::from_millis(50), move || {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            pool.stats().abandoned,
            0,
            "a queued call abandons no thread"
        );
        gate.open();
        for t in busy {
            t.join().unwrap().unwrap();
        }
        // The queue has drained past the cancelled call.
        pool.run(Duration::from_secs(10), || Ok(())).unwrap();
        pool.run(Duration::from_secs(10), || Ok(())).unwrap();
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(pool.stats().threads, IO_THREADS);
    }

    #[test]
    fn a_panicking_call_fails_without_killing_its_thread() {
        let pool = IoPool::new();
        let err = pool
            .run(Duration::from_secs(10), || -> io::Result<()> {
                panic!("boom");
            })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(pool.run(Duration::from_secs(10), || Ok(5)).unwrap(), 5);
        assert_eq!(pool.stats().threads, IO_THREADS);
    }

    #[test]
    fn sharing_violations_are_retried_for_about_five_seconds() {
        let slept = Mutex::new(Duration::ZERO);
        let sleep = |d: Duration| *slept.lock().unwrap() += d;
        let calls = AtomicUsize::new(0);
        let sharing = || io::Error::new(io::ErrorKind::PermissionDenied, SharingViolation);
        // Clears after 3 tries.
        let r = retry_sharing(
            || {
                if calls.fetch_add(1, Ordering::SeqCst) < 3 {
                    Err(sharing())
                } else {
                    Ok(7)
                }
            },
            &sleep,
        );
        assert_eq!(r.unwrap(), 7);
        assert_eq!(
            *slept.lock().unwrap(),
            Duration::from_millis(50 + 100 + 200)
        );
        // Never clears: gives up after the budget.
        *slept.lock().unwrap() = Duration::ZERO;
        calls.store(0, Ordering::SeqCst);
        let r: io::Result<()> = retry_sharing(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(sharing())
            },
            &sleep,
        );
        assert!(is_sharing_violation(&r.unwrap_err()));
        assert_eq!(*slept.lock().unwrap(), SHARING_RETRY_BUDGET);
        assert!(calls.load(Ordering::SeqCst) < 12);
        // Other errors aren't retried.
        calls.store(0, Ordering::SeqCst);
        let r: io::Result<()> = retry_sharing(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::new(io::ErrorKind::StorageFull, "full"))
            },
            &sleep,
        );
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::StorageFull);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_write_leaves_no_temp() {
        let tmp = tempfile::tempdir().unwrap();
        // The rename fails: the target appears between the check and the
        // rename, which `rename_no_replace` reports.
        let dir = tmp.path();
        std::fs::write(dir.join("taken"), b"x").unwrap();
        let err = write_once_now(dir, "taken", b"y", &real_sleep).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["taken"]);
    }

    #[test]
    fn structural_names() {
        for n in [
            "keys",
            "devices",
            "shards",
            "blobs",
            "archive",
            "k7m2p9xqa3rt5vwe",
            "20261109-100000",
        ] {
            assert!(is_structural(n), "{}", n);
        }
        for n in [
            "Kage Sync",
            "Dropbox",
            "2026110-1000000",
            "K7M2P9XQA3RT5VWE",
            "Shards",
        ] {
            assert!(!is_structural(n), "{}", n);
        }
    }
}
