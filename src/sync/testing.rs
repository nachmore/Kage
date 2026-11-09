//! Fakes for sync tests (docs/design/SYNC.md §18): an in-memory folder with
//! injected faults, a clock to drive, a memory key store with failures to
//! inject, and cloud files whose placeholders a test controls. Public so
//! integration tests can use them; not part of the app.
//!
//! `FaultyFs` is one computer's view of the folder. A simulated cloud
//! (`tests/sync_support/`) gives each computer its own and copies files
//! between them with [`FaultyFs::put`], [`FaultyFs::get`] and
//! [`FaultyFs::snapshot`], late, partial, out of order or duplicated.

use super::engine::Clock;
use super::fsio::{Entry, SharingViolation, SyncFs};
use super::keystore::{KeyStore, KeyStoreError, KeyStoreKind, MemoryStore, StoredKey};
use crate::os::cloud_files::{CloudFiles, Provider, Residency, SuggestedFolder};
use crate::os::power::PowerState;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// --- FaultyFs ----------------------------------------------------------------

/// The `SyncFs` calls, to aim a fault at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FsOp {
    List,
    Read,
    Write,
    CreateNew,
    Rename,
    Remove,
    CreateDir,
}

/// What an injected fault does to a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// `ENOSPC` / `ERROR_DISK_FULL`: `StorageFull`.
    NoSpace,
    /// `EACCES`, or macOS denying the folder: `PermissionDenied`.
    Denied,
    /// A read-only folder: `ReadOnlyFilesystem`.
    ReadOnly,
    /// A Windows sharing violation (OneDrive, antivirus): `ResourceBusy`
    /// carrying [`SharingViolation`], so `fsio::is_sharing_violation`
    /// sees it.
    Sharing,
    /// The call takes this long. A read slower than its timeout fails with
    /// `TimedOut` at once.
    Slow(Duration),
    /// The call never answers: it fails with `TimedOut` at once, as
    /// `RealFs` does once its timeout fires, so simulated ticks stay fast.
    Hang,
    /// The file or directory isn't there: `NotFound`.
    Missing,
}

impl Fault {
    fn error(&self) -> io::Error {
        match self {
            Self::NoSpace => io::Error::new(io::ErrorKind::StorageFull, "no space left (injected)"),
            Self::Denied => io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied (injected)",
            ),
            Self::ReadOnly => {
                io::Error::new(io::ErrorKind::ReadOnlyFilesystem, "read-only (injected)")
            }
            Self::Sharing => io::Error::new(io::ErrorKind::ResourceBusy, SharingViolation),
            Self::Slow(_) | Self::Hang => io::Error::new(
                io::ErrorKind::TimedOut,
                "the folder didn't answer (injected)",
            ),
            Self::Missing => io::Error::new(io::ErrorKind::NotFound, "not found (injected)"),
        }
    }
}

#[derive(Debug, Clone)]
struct Rule {
    op: Option<FsOp>,
    under: PathBuf,
    fault: Fault,
    /// Calls left before the rule clears; `None` keeps it.
    remaining: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemFile {
    bytes: Vec<u8>,
    modified_ms: i64,
}

#[derive(Debug, Default)]
struct MemState {
    files: BTreeMap<PathBuf, MemFile>,
    dirs: BTreeSet<PathBuf>,
    rules: Vec<Rule>,
    calls: HashMap<FsOp, usize>,
}

/// What a directory held at one moment, to bring back later (a sync app
/// rewinding a directory, a restore from version history).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsSnapshot {
    under: PathBuf,
    files: BTreeMap<PathBuf, MemFile>,
    dirs: BTreeSet<PathBuf>,
}

/// A sync folder in memory, with faults to inject. Directories exist once
/// created (or once a file is put under them).
#[derive(Default)]
pub struct FaultyFs {
    state: Mutex<MemState>,
    clock: Mutex<Option<Arc<dyn Clock>>>,
}

impl std::fmt::Debug for FaultyFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = locked(&self.state);
        f.debug_struct("FaultyFs")
            .field("files", &st.files.len())
            .field("dirs", &st.dirs.len())
            .field("rules", &st.rules.len())
            .finish()
    }
}

fn not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no such file or directory")
}

fn exists_error() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "a file of that name exists")
}

impl FaultyFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// File times come from `clock` (else 0).
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        let fs = Self::default();
        *locked(&fs.clock) = Some(clock);
        fs
    }

    pub fn set_clock(&self, clock: Arc<dyn Clock>) {
        *locked(&self.clock) = Some(clock);
    }

    fn now(&self) -> i64 {
        locked(&self.clock).as_ref().map_or(0, |c| c.now_ms())
    }

    /// Fail every `op` call (any call for `None`) on a path under `under`.
    pub fn inject(&self, op: impl Into<Option<FsOp>>, under: impl AsRef<Path>, fault: Fault) {
        self.add_rule(op.into(), under.as_ref(), fault, None);
    }

    /// [`Self::inject`] for the next `times` matching calls only: a sharing
    /// violation that clears after N retries, one lost write.
    pub fn inject_times(
        &self,
        op: impl Into<Option<FsOp>>,
        under: impl AsRef<Path>,
        fault: Fault,
        times: u32,
    ) {
        self.add_rule(op.into(), under.as_ref(), fault, Some(times));
    }

    fn add_rule(&self, op: Option<FsOp>, under: &Path, fault: Fault, remaining: Option<u32>) {
        locked(&self.state).rules.push(Rule {
            op,
            under: under.to_path_buf(),
            fault,
            remaining,
        });
    }

    pub fn clear_faults(&self) {
        locked(&self.state).rules.clear();
    }

    /// Calls of `op` made so far, failed ones included.
    pub fn calls(&self, op: FsOp) -> usize {
        locked(&self.state).calls.get(&op).copied().unwrap_or(0)
    }

    /// Count the call, and apply the first fault aimed at it.
    fn enter(&self, op: FsOp, paths: &[&Path], timeout: Option<Duration>) -> io::Result<()> {
        let fault = {
            let mut st = locked(&self.state);
            *st.calls.entry(op).or_default() += 1;
            let hit = st.rules.iter().position(|r| {
                r.op.is_none_or(|o| o == op) && paths.iter().any(|p| p.starts_with(&r.under))
            });
            let Some(i) = hit else {
                return Ok(());
            };
            let fault = st.rules[i].fault.clone();
            if let Some(left) = st.rules[i].remaining.as_mut() {
                *left = left.saturating_sub(1);
                if *left == 0 {
                    st.rules.remove(i);
                }
            }
            fault
        };
        match fault {
            // Slower than its timeout: timed out, without the wait.
            Fault::Slow(d) if timeout.is_some_and(|t| d >= t) => Err(fault.error()),
            Fault::Slow(d) => {
                std::thread::sleep(d);
                Ok(())
            }
            other => Err(other.error()),
        }
    }

    fn add_ancestors(st: &mut MemState, path: &Path) {
        for a in path.ancestors() {
            if a.as_os_str().is_empty() {
                break;
            }
            st.dirs.insert(a.to_path_buf());
        }
    }

    /// Put a file where a sync app would: any name, created or replaced in
    /// place, its directories created. No fault applies.
    pub fn put(&self, path: impl AsRef<Path>, bytes: impl Into<Vec<u8>>) {
        let now = self.now();
        self.put_at(path, bytes, now);
    }

    /// [`Self::put`] with a given modification time.
    pub fn put_at(&self, path: impl AsRef<Path>, bytes: impl Into<Vec<u8>>, modified_ms: i64) {
        let path = path.as_ref();
        let mut st = locked(&self.state);
        if let Some(parent) = path.parent() {
            Self::add_ancestors(&mut st, parent);
        }
        st.files.insert(
            path.to_path_buf(),
            MemFile {
                bytes: bytes.into(),
                modified_ms,
            },
        );
    }

    /// Change a file's modification time.
    pub fn set_modified(&self, path: impl AsRef<Path>, modified_ms: i64) {
        if let Some(f) = locked(&self.state).files.get_mut(path.as_ref()) {
            f.modified_ms = modified_ms;
        }
    }

    /// A file's bytes. No fault applies.
    pub fn get(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        locked(&self.state)
            .files
            .get(path.as_ref())
            .map(|f| f.bytes.clone())
    }

    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        let st = locked(&self.state);
        let p = path.as_ref();
        st.files.contains_key(p) || st.dirs.contains(p)
    }

    /// Remove a file or a directory with everything in it, as a user or a
    /// sync app would. No fault applies.
    pub fn delete(&self, path: impl AsRef<Path>) {
        let p = path.as_ref();
        let mut st = locked(&self.state);
        st.files.retain(|f, _| !f.starts_with(p));
        st.dirs.retain(|d| !d.starts_with(p));
    }

    /// Every file under `under`, with its bytes, by path.
    pub fn files_under(&self, under: impl AsRef<Path>) -> Vec<(PathBuf, Vec<u8>)> {
        let u = under.as_ref();
        locked(&self.state)
            .files
            .iter()
            .filter(|(p, _)| p.starts_with(u))
            .map(|(p, f)| (p.clone(), f.bytes.clone()))
            .collect()
    }

    /// What `under` holds now.
    pub fn snapshot(&self, under: impl AsRef<Path>) -> FsSnapshot {
        let u = under.as_ref();
        let st = locked(&self.state);
        FsSnapshot {
            under: u.to_path_buf(),
            files: st
                .files
                .iter()
                .filter(|(p, _)| p.starts_with(u))
                .map(|(p, f)| (p.clone(), f.clone()))
                .collect(),
            dirs: st
                .dirs
                .iter()
                .filter(|d| d.starts_with(u))
                .cloned()
                .collect(),
        }
    }

    /// Bring `under` back to a snapshot: newer files go, older ones return.
    pub fn restore(&self, snap: &FsSnapshot) {
        let mut st = locked(&self.state);
        st.files.retain(|p, _| !p.starts_with(&snap.under));
        st.dirs.retain(|d| !d.starts_with(&snap.under));
        st.files
            .extend(snap.files.iter().map(|(p, f)| (p.clone(), f.clone())));
        st.dirs.extend(snap.dirs.iter().cloned());
    }

    fn is_dir(st: &MemState, p: &Path) -> bool {
        st.dirs.contains(p)
    }

    fn parent_must_exist(st: &MemState, p: &Path) -> io::Result<()> {
        match p.parent() {
            Some(parent) if Self::is_dir(st, parent) => Ok(()),
            Some(parent) if st.files.contains_key(parent) => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            )),
            _ => Err(not_found()),
        }
    }
}

impl SyncFs for FaultyFs {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        self.enter(FsOp::List, &[dir], None)?;
        let st = locked(&self.state);
        if !Self::is_dir(&st, dir) {
            return Err(if st.files.contains_key(dir) {
                io::Error::new(io::ErrorKind::NotADirectory, "not a directory")
            } else {
                not_found()
            });
        }
        let name = |p: &Path| p.file_name().and_then(|n| n.to_str()).map(str::to_string);
        let mut out: Vec<Entry> = st
            .files
            .iter()
            .filter(|(p, _)| p.parent() == Some(dir))
            .filter_map(|(p, f)| {
                Some(Entry {
                    name: name(p)?,
                    len: f.bytes.len() as u64,
                    modified_ms: Some(f.modified_ms),
                    is_dir: false,
                })
            })
            .chain(
                st.dirs
                    .iter()
                    .filter(|d| d.parent() == Some(dir))
                    .filter_map(|d| {
                        Some(Entry {
                            name: name(d)?,
                            len: 0,
                            modified_ms: None,
                            is_dir: true,
                        })
                    }),
            )
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn read(&self, path: &Path, max_len: u64, timeout: Duration) -> io::Result<Vec<u8>> {
        self.enter(FsOp::Read, &[path], Some(timeout))?;
        let st = locked(&self.state);
        let Some(f) = st.files.get(path) else {
            return Err(if Self::is_dir(&st, path) {
                io::Error::new(io::ErrorKind::InvalidInput, "not a regular file")
            } else {
                not_found()
            });
        };
        if f.bytes.len() as u64 > max_len {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "the file is over its size limit",
            ));
        }
        Ok(f.bytes.clone())
    }

    fn write_once(&self, dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
        let target = dir.join(name);
        self.enter(FsOp::Write, &[&target], None)?;
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a file name",
            ));
        }
        let now = self.now();
        let mut st = locked(&self.state);
        if !Self::is_dir(&st, dir) {
            return Err(not_found());
        }
        if st.files.contains_key(&target) || st.dirs.contains(&target) {
            return Err(exists_error());
        }
        st.files.insert(
            target,
            MemFile {
                bytes: bytes.to_vec(),
                modified_ms: now,
            },
        );
        Ok(())
    }

    fn create_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.enter(FsOp::CreateNew, &[path], None)?;
        let now = self.now();
        let mut st = locked(&self.state);
        Self::parent_must_exist(&st, path)?;
        if st.files.contains_key(path) || st.dirs.contains(path) {
            return Err(exists_error());
        }
        st.files.insert(
            path.to_path_buf(),
            MemFile {
                bytes: bytes.to_vec(),
                modified_ms: now,
            },
        );
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.enter(FsOp::Rename, &[from, to], None)?;
        let mut st = locked(&self.state);
        if st.files.contains_key(to) || st.dirs.contains(to) {
            return Err(exists_error());
        }
        Self::parent_must_exist(&st, to)?;
        if let Some(f) = st.files.remove(from) {
            st.files.insert(to.to_path_buf(), f);
            return Ok(());
        }
        if !st.dirs.contains(from) {
            return Err(not_found());
        }
        let moved = |p: &Path| to.join(p.strip_prefix(from).expect("under `from`"));
        let files: Vec<_> = st
            .files
            .iter()
            .filter(|(p, _)| p.starts_with(from))
            .map(|(p, f)| (p.clone(), f.clone()))
            .collect();
        let dirs: Vec<_> = st
            .dirs
            .iter()
            .filter(|d| d.starts_with(from))
            .cloned()
            .collect();
        st.files.retain(|p, _| !p.starts_with(from));
        st.dirs.retain(|d| !d.starts_with(from));
        for (p, f) in files {
            st.files.insert(moved(&p), f);
        }
        for d in dirs {
            st.dirs.insert(moved(&d));
        }
        Ok(())
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        self.enter(FsOp::Remove, &[path], None)?;
        let mut st = locked(&self.state);
        if st.files.remove(path).is_some() {
            return Ok(());
        }
        if !st.dirs.contains(path) {
            return Err(not_found());
        }
        st.files.retain(|p, _| !p.starts_with(path));
        st.dirs.retain(|d| !d.starts_with(path));
        Ok(())
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        self.enter(FsOp::CreateDir, &[dir], None)?;
        let mut st = locked(&self.state);
        if dir.ancestors().any(|a| st.files.contains_key(a)) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "a file is in the way",
            ));
        }
        Self::add_ancestors(&mut st, dir);
        Ok(())
    }
}

// --- FakeClock -----------------------------------------------------------------

/// A clock that only moves when told to.
#[derive(Debug)]
pub struct FakeClock {
    now_ms: AtomicI64,
    power: Mutex<PowerState>,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new(0)
    }
}

impl FakeClock {
    pub fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
            power: Mutex::new(PowerState::AC),
        }
    }

    pub fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }

    pub fn advance(&self, by: Duration) {
        self.now_ms
            .fetch_add(by.as_millis() as i64, Ordering::SeqCst);
    }

    pub fn set_power(&self, power: PowerState) {
        *locked(&self.power) = power;
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn power(&self) -> PowerState {
        *locked(&self.power)
    }
}

// --- MemoryKeyStore ------------------------------------------------------------

/// A key store failure to inject (§18.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFault {
    /// The entry is gone: `get` answers `None`.
    NotFound,
    /// The user or the OS denied access.
    Denied,
    /// The store isn't there or is locked.
    Unavailable,
}

/// A key store that keeps keys in memory, as every isolated run uses, with
/// failures to inject and a count of calls.
pub struct MemoryKeyStore {
    inner: MemoryStore,
    kind: KeyStoreKind,
    fault: Mutex<Option<KeyFault>>,
    calls: AtomicUsize,
}

impl Default for MemoryKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MemoryKeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryKeyStore")
            .field("kind", &self.kind)
            .field("fault", &*locked(&self.fault))
            .finish()
    }
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::with_kind(KeyStoreKind::Memory)
    }

    /// One that reports itself as `kind`, to stand in for a keychain.
    pub fn with_kind(kind: KeyStoreKind) -> Self {
        Self {
            inner: MemoryStore::default(),
            kind,
            fault: Mutex::new(None),
            calls: AtomicUsize::new(0),
        }
    }

    /// Fail every later call this way, or stop failing with `None`.
    pub fn fail(&self, fault: Option<KeyFault>) {
        *locked(&self.fault) = fault;
    }

    /// Calls made so far.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn enter(&self) -> Option<KeyFault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *locked(&self.fault)
    }
}

fn key_error(f: KeyFault) -> anyhow::Error {
    match f {
        KeyFault::Denied => KeyStoreError::Denied.into(),
        KeyFault::Unavailable | KeyFault::NotFound => KeyStoreError::Unavailable.into(),
    }
}

impl KeyStore for MemoryKeyStore {
    fn kind(&self) -> KeyStoreKind {
        self.kind
    }

    fn get(&self, folder_id: &str) -> anyhow::Result<Option<StoredKey>> {
        match self.enter() {
            None => self.inner.get(folder_id),
            Some(KeyFault::NotFound) => Ok(None),
            Some(f) => Err(key_error(f)),
        }
    }

    fn set(&self, folder_id: &str, key: &StoredKey) -> anyhow::Result<()> {
        match self.enter() {
            None | Some(KeyFault::NotFound) => self.inner.set(folder_id, key),
            Some(f) => Err(key_error(f)),
        }
    }

    fn delete(&self, folder_id: &str) -> anyhow::Result<()> {
        match self.enter() {
            None | Some(KeyFault::NotFound) => self.inner.delete(folder_id),
            Some(f) => Err(key_error(f)),
        }
    }
}

// --- FakeCloudFiles ------------------------------------------------------------

#[derive(Debug)]
struct CloudState {
    residency: HashMap<PathBuf, Residency>,
    default: Residency,
    downloads: Vec<PathBuf>,
    pinned: Vec<PathBuf>,
    pin_fails: bool,
    provider: Provider,
    suggestions: Vec<SuggestedFolder>,
}

impl Default for CloudState {
    fn default() -> Self {
        Self {
            residency: HashMap::new(),
            default: Residency::Local,
            downloads: Vec::new(),
            pinned: Vec::new(),
            pin_fails: false,
            provider: Provider::Other,
            suggestions: Vec::new(),
        }
    }
}

/// Cloud files whose placeholders a test controls. Every file is local
/// until marked otherwise.
#[derive(Debug, Default)]
pub struct FakeCloudFiles {
    state: Mutex<CloudState>,
}

impl FakeCloudFiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// Report `path` as `r` from now on.
    pub fn set_residency(&self, path: impl AsRef<Path>, r: Residency) {
        locked(&self.state)
            .residency
            .insert(path.as_ref().to_path_buf(), r);
    }

    /// What files not marked otherwise report.
    pub fn set_default_residency(&self, r: Residency) {
        locked(&self.state).default = r;
    }

    /// Paths whose download was requested, in order, repeats included.
    pub fn download_requests(&self) -> Vec<PathBuf> {
        locked(&self.state).downloads.clone()
    }

    /// The download requests so far, cleared.
    pub fn take_download_requests(&self) -> Vec<PathBuf> {
        std::mem::take(&mut locked(&self.state).downloads)
    }

    /// Every requested file is local now.
    pub fn complete_downloads(&self) {
        let mut st = locked(&self.state);
        let done: Vec<PathBuf> = st.downloads.clone();
        for p in done {
            st.residency.insert(p, Residency::Local);
        }
    }

    pub fn pinned(&self) -> Vec<PathBuf> {
        locked(&self.state).pinned.clone()
    }

    /// Make `pin_folder` fail (pinning is best effort).
    pub fn fail_pinning(&self, fails: bool) {
        locked(&self.state).pin_fails = fails;
    }

    pub fn set_provider(&self, p: Provider) {
        locked(&self.state).provider = p;
    }

    pub fn set_suggestions(&self, s: Vec<SuggestedFolder>) {
        locked(&self.state).suggestions = s;
    }
}

impl CloudFiles for FakeCloudFiles {
    fn residency(&self, path: &Path) -> Residency {
        let st = locked(&self.state);
        st.residency.get(path).copied().unwrap_or(st.default)
    }

    fn request_download(&self, path: &Path) {
        locked(&self.state).downloads.push(path.to_path_buf());
    }

    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()> {
        let mut st = locked(&self.state);
        if st.pin_fails {
            anyhow::bail!("pinning isn't supported here (injected)");
        }
        st.pinned.push(dir.to_path_buf());
        Ok(())
    }

    fn detect_provider(&self, path: &Path) -> Provider {
        let _ = path;
        locked(&self.state).provider
    }

    fn suggest_folders(&self) -> Vec<SuggestedFolder> {
        locked(&self.state).suggestions.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::fsio::is_sharing_violation;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn faulty_fs_behaves_like_a_folder() {
        let fs = FaultyFs::with_clock(Arc::new(FakeClock::new(1000)));
        let dir = p("/cloud/Kage Sync/devices/k7m2p9xqa3rt5vwe");
        assert_eq!(fs.list(&dir).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(
            fs.write_once(&dir, "head-000000000001.kage", b"x")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        fs.create_dir_all(&dir.join("shards")).unwrap();
        fs.write_once(&dir, "head-000000000001.kage", b"h1")
            .unwrap();
        assert_eq!(
            fs.write_once(&dir, "head-000000000001.kage", b"h2")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        let entries = fs.list(&dir).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "head-000000000001.kage");
        assert_eq!(entries[0].modified_ms, Some(1000));
        assert!(entries[1].is_dir);
        assert_eq!(
            fs.read(
                &dir.join("head-000000000001.kage"),
                10,
                Duration::from_secs(1)
            )
            .unwrap(),
            b"h1"
        );
        assert_eq!(
            fs.read(
                &dir.join("head-000000000001.kage"),
                1,
                Duration::from_secs(1)
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::FileTooLarge
        );
        let root = p("/cloud/Kage Sync");
        fs.create_new(&root.join("kage-sync.json"), b"{}").unwrap();
        assert_eq!(
            fs.create_new(&root.join("kage-sync.json"), b"{}")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        // Moving a tree into the archive.
        fs.create_dir_all(&root.join("archive/20261109-100000"))
            .unwrap();
        fs.rename(
            &root.join("devices"),
            &root.join("archive/20261109-100000/devices"),
        )
        .unwrap();
        assert!(!fs.exists(&dir));
        assert!(fs.exists(
            root.join("archive/20261109-100000/devices/k7m2p9xqa3rt5vwe/head-000000000001.kage")
        ));
        fs.remove(&root.join("archive")).unwrap();
        assert!(fs.files_under(root.join("archive")).is_empty());
        assert_eq!(
            fs.remove(&root.join("archive")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn faults_hit_their_calls_and_clear() {
        let fs = FaultyFs::new();
        let d = p("/f/devices/a");
        fs.create_dir_all(&d).unwrap();
        fs.inject(FsOp::Write, &d, Fault::NoSpace);
        assert_eq!(
            fs.write_once(&d, "x", b"1").unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        fs.clear_faults();
        fs.write_once(&d, "x", b"1").unwrap();

        fs.inject_times(FsOp::Write, &d, Fault::Sharing, 2);
        for _ in 0..2 {
            assert!(is_sharing_violation(
                &fs.write_once(&d, "y", b"1").unwrap_err()
            ));
        }
        fs.write_once(&d, "y", b"1").unwrap();

        fs.inject(None, "/f", Fault::Denied);
        assert_eq!(
            fs.list(&d).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        fs.clear_faults();

        fs.inject(FsOp::Read, d.join("x"), Fault::Hang);
        let err = fs
            .read(&d.join("x"), 10, Duration::from_secs(20))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        fs.clear_faults();
        fs.inject(FsOp::Read, &d, Fault::Slow(Duration::from_millis(30)));
        assert_eq!(
            fs.read(&d.join("x"), 10, Duration::from_millis(10))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            fs.read(&d.join("x"), 10, Duration::from_secs(1)).unwrap(),
            b"1"
        );
        fs.inject(FsOp::CreateNew, "/f", Fault::ReadOnly);
        assert_eq!(
            fs.create_new(&d.join("z"), b"").unwrap_err().kind(),
            io::ErrorKind::ReadOnlyFilesystem
        );
        assert!(fs.calls(FsOp::Write) >= 4);
        assert_eq!(fs.calls(FsOp::Rename), 0);
    }

    #[test]
    fn sync_apps_can_deliver_rewind_and_delete() {
        let fs = FaultyFs::new();
        let d = p("/f/devices/a");
        fs.put(d.join("head-000000000001.kage"), b"one".to_vec());
        let snap = fs.snapshot("/f/devices");
        // A partial file written in place, then completed.
        fs.put(d.join("head-000000000002.kage"), b"tw".to_vec());
        fs.put(d.join("head-000000000002.kage"), b"two".to_vec());
        fs.put(d.join("head-000000000002 (1).kage"), b"two".to_vec());
        assert_eq!(fs.list(&d).unwrap().len(), 3);
        fs.restore(&snap);
        let names: Vec<_> = fs.list(&d).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["head-000000000001.kage"]);
        fs.delete("/f/devices");
        assert!(!fs.exists(&d));
        assert_eq!(fs.get(d.join("head-000000000001.kage")), None);
    }

    #[test]
    fn memory_key_store_injects_failures() {
        let ks = MemoryKeyStore::with_kind(KeyStoreKind::Keychain);
        assert_eq!(ks.kind(), KeyStoreKind::Keychain);
        ks.set("f", &StoredKey::new(1, &[1; 32])).unwrap();
        assert!(ks.get("f").unwrap().is_some());
        ks.fail(Some(KeyFault::NotFound));
        assert!(ks.get("f").unwrap().is_none());
        ks.fail(Some(KeyFault::Denied));
        let err = ks.get("f").unwrap_err();
        assert_eq!(
            err.downcast_ref::<KeyStoreError>(),
            Some(&KeyStoreError::Denied)
        );
        ks.fail(Some(KeyFault::Unavailable));
        assert!(ks.set("f", &StoredKey::new(2, &[2; 32])).is_err());
        ks.fail(None);
        assert_eq!(ks.get("f").unwrap().unwrap().epoch, 1);
        assert_eq!(ks.calls(), 6);
    }

    #[test]
    fn fake_cloud_files_records_requests() {
        let cf = FakeCloudFiles::new();
        let f = p("/f/a.kage");
        assert_eq!(cf.residency(&f), Residency::Local);
        cf.set_residency(&f, Residency::Placeholder);
        assert_eq!(cf.residency(&f), Residency::Placeholder);
        cf.request_download(&f);
        assert_eq!(cf.download_requests(), std::slice::from_ref(&f));
        cf.complete_downloads();
        assert_eq!(cf.residency(&f), Residency::Local);
        assert_eq!(cf.take_download_requests().len(), 1);
        assert!(cf.download_requests().is_empty());
        cf.fail_pinning(true);
        assert!(cf.pin_folder(Path::new("/f")).is_err());
        cf.fail_pinning(false);
        cf.pin_folder(Path::new("/f")).unwrap();
        assert_eq!(cf.pinned(), [p("/f")]);
        cf.set_provider(Provider::Dropbox);
        assert_eq!(cf.detect_provider(&f), Provider::Dropbox);
    }

    #[test]
    fn fake_clock_moves_when_told() {
        let c = FakeClock::new(5);
        c.advance(Duration::from_secs(1));
        assert_eq!(c.now_ms(), 1005);
        c.set_power(PowerState::Battery);
        assert_eq!(c.power(), PowerState::Battery);
    }
}
