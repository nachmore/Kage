//! Durable state of an import in progress, so one interrupted midway (Kage
//! killed, power lost) is finished or undone the next time Kage starts or
//! imports.
//!
//! An import that writes files first stages each one to a temp file next
//! to its target, then writes `import-journal.json` (state `applying`) to
//! Kage's config folder listing every target, its staged file and the name
//! its current copy will be moved aside to, plus a copy of `config.json`.
//! Only then are files renamed into place. The caller saves `config.json`
//! and calls [`Transaction::finish`], which first records `committed` (the
//! commit marker), then deletes the moved-aside copies and the journal.
//!
//! [`recover`] reads a journal left behind: `committed` → delete what's
//! left of the old copies; `applying` → put every old file back and restore
//! `config.json`, so files and config always match. A moved-aside copy is
//! deleted only once its transaction committed.
//!
//! Undoing never deletes or replaces a file this transaction didn't write:
//! the journal records what each write puts at its target (a SHA-256 of
//! the content, its length and, on Unix, the file's device and inode, which
//! a rename keeps), and a target that no longer is that file (an extension
//! wrote it again, or a new one is there) is left as it is, its earlier
//! copy kept beside it. Each step of an undo (a file put back, config.json
//! restored) is appended to the journal, flushed, before the next one
//! starts, so a retry after a partial undo skips what's already done and
//! never touches a file created since.
//!
//! Imports hold `.kage-import.lock` (an OS file lock, released if the
//! process dies) while they run, so a second Kage process never "recovers"
//! an import that is still going.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const JOURNAL_NAME: &str = "import-journal.json";
const LOCK_NAME: &str = ".kage-import.lock";

/// One import at a time in this process (the file lock covers others).
static IN_PROCESS: Mutex<()> = Mutex::new(());

/// Kage's own folder under the config dir, where config.json lives.
fn kage_dir() -> Result<PathBuf> {
    Ok(crate::paths::config_dir()
        .context("No config folder")?
        .join("kage"))
}

/// The import lock: this process's turn, and no other process mid-import.
pub struct ImportLock {
    _file: File,
    _turn: MutexGuard<'static, ()>,
}

impl ImportLock {
    /// Wait for this process's turn, then take the file lock. `Ok(None)`:
    /// another Kage process is importing right now.
    pub fn acquire() -> Result<Option<Self>> {
        let turn = IN_PROCESS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = kage_dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {:?}", dir))?;
        let mut opts = OpenOptions::new();
        opts.create(true).truncate(false).write(true);
        no_follow_owner_only(&mut opts);
        let file = opts
            .open(dir.join(LOCK_NAME))
            .context("Failed to open the import lock")?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self {
                _file: file,
                _turn: turn,
            })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => {
                Err(e).context("Failed to take the import lock")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Applying,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileEntry {
    target: PathBuf,
    /// The new content, until it's renamed into place.
    staged: PathBuf,
    /// Where the current file is moved aside to (`None`: there is none).
    backup: Option<PathBuf>,
    /// The file this transaction puts at `target`. `None` in a journal an
    /// earlier build wrote: nothing at the target is then provably this
    /// transaction's, so an undo never removes or replaces it.
    #[serde(default)]
    written: Option<Written>,
    /// This entry is undone (recorded before the next step starts).
    #[serde(default)]
    undone: bool,
}

/// What a transaction wrote at a target, to tell it from a file written
/// there since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Written {
    /// SHA-256 of the content, hex.
    sha256: String,
    len: u64,
    /// `(device, inode)` on Unix: kept by the rename into place, and new
    /// for any file created afterwards, whatever it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inode: Option<(u64, u64)>,
}

impl Written {
    /// What the file at `path` is, read without following a link.
    fn of(path: &Path) -> Result<Self> {
        use sha2::Digest;
        let md = std::fs::symlink_metadata(path)
            .with_context(|| format!("Failed to inspect {:?}", path))?;
        if !md.is_file() {
            anyhow::bail!("{:?} isn't a regular file", path);
        }
        let mut file = File::open(path).with_context(|| format!("Failed to read {:?}", path))?;
        let mut hasher = sha2::Sha256::new();
        let len = std::io::copy(&mut file, &mut hasher)
            .with_context(|| format!("Failed to read {:?}", path))?;
        Ok(Self {
            sha256: hex::encode(hasher.finalize()),
            len,
            inode: inode_of(&md),
        })
    }

    /// Whether the file at `path` is still this one.
    fn is_at(&self, path: &Path) -> bool {
        let Ok(md) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if !md.is_file() || md.len() != self.len {
            return false;
        }
        if self.inode.is_some() && inode_of(&md) != self.inode {
            return false;
        }
        Self::of(path).is_ok_and(|now| now.sha256 == self.sha256)
    }
}

#[cfg(unix)]
fn inode_of(md: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((md.dev(), md.ino()))
}

#[cfg(not(unix))]
fn inode_of(_md: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// One step of an undo, appended to the journal once it's done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    /// `files[i]` is undone.
    File(usize),
    /// config.json is back as it was.
    Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    format: u32,
    state: State,
    config: PathBuf,
    /// A copy of `config` as it was (`None`: there was none).
    config_backup: Option<PathBuf>,
    files: Vec<FileEntry>,
    /// Folders created for new files, in creation order.
    created_dirs: Vec<PathBuf>,
    /// Whether recovery puts `config.json` back. Cleared when the running
    /// Kage that made the import undid it and keeps the right config in
    /// memory: a later start must not restore a snapshot older than what
    /// it saved since.
    #[serde(default = "default_true")]
    restore_config: bool,
    /// Failed recovery attempts so far.
    #[serde(default)]
    attempts: u32,
    /// config.json was put back by an earlier try at undoing.
    #[serde(default)]
    config_undone: bool,
}

fn default_true() -> bool {
    true
}

/// Recovery attempts after which an import that can't be undone (a
/// folder that never comes back) is set aside — its journal renamed, every
/// rollback copy kept — so it can't block imports for good.
const MAX_ATTEMPTS: u32 = 5;

/// A temp or rollback file this module may delete or rename: one of the
/// import's own (`.kage-import.…`), next to its target.
fn is_own_sibling(path: &Path, target: &Path) -> bool {
    path.parent() == target.parent()
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(super::TEMP_PREFIX))
}

impl Journal {
    fn is_well_formed(&self) -> bool {
        self.format == 1
            && self
                .config_backup
                .as_ref()
                .is_none_or(|b| is_own_sibling(b, &self.config))
            && self.files.iter().all(|f| {
                is_own_sibling(&f.staged, &f.target)
                    && f.backup
                        .as_ref()
                        .is_none_or(|b| is_own_sibling(b, &f.target))
            })
    }
}

fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// On Unix, `opts` never opens through a symlink at the path, and a file
/// it creates is owner-only (like everything else an import writes).
fn no_follow_owner_only(opts: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = opts;
}

/// Write `journal` to `path` atomically and durably, owner-only
/// ([`crate::atomic_write`]; a link at `path` is replaced, never written
/// through).
fn write_journal(path: &Path, journal: &Journal) -> Result<()> {
    let body = serde_json::to_vec_pretty(journal)?;
    crate::atomic_write::write_no_follow(path, body, crate::atomic_write::Perms::Private)
        .context("Failed to write the import journal")
}

/// Append a finished undo step to the journal at `path`, flushed to disk
/// before returning. One record per line after the journal itself; a
/// record cut short by a crash doesn't parse and is ignored, so its step
/// is redone (and finds nothing of its own to undo).
fn record_step(path: &Path, step: Step) -> Result<()> {
    let mut line = b"\n".to_vec();
    line.extend(serde_json::to_vec(&step)?);
    let mut opts = OpenOptions::new();
    opts.append(true);
    no_follow_owner_only(&mut opts);
    let mut f = opts
        .open(path)
        .context("Failed to open the import journal")?;
    f.write_all(&line)
        .context("Failed to record progress in the import journal")?;
    f.sync_all()
        .context("Failed to record progress in the import journal")
}

/// Read the journal at `path`, with the undo steps appended to it.
fn read_journal(body: &[u8]) -> Result<Journal> {
    let mut values = serde_json::Deserializer::from_slice(body).into_iter::<serde_json::Value>();
    let first = values
        .next()
        .context("The import journal is empty")?
        .context("The import journal is damaged")?;
    let mut journal: Journal =
        serde_json::from_value(first).context("The import journal is damaged")?;
    for value in values {
        let Ok(step) = value.and_then(serde_json::from_value::<Step>) else {
            // A torn last record: that step wasn't recorded.
            break;
        };
        match step {
            Step::File(i) => {
                if let Some(f) = journal.files.get_mut(i) {
                    f.undone = true;
                }
            }
            Step::Config => journal.config_undone = true,
        }
    }
    Ok(journal)
}

/// Copy `from` to a new owner-only file `to`, flushed.
fn durable_copy(from: &Path, to: &Path) -> Result<()> {
    let body = std::fs::read(from).with_context(|| format!("Failed to read {:?}", from))?;
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(to)
        .with_context(|| format!("Failed to create {:?}", to))?;
    f.write_all(&body)?;
    f.sync_all()?;
    Ok(())
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// An import's file writes, from staging until the caller keeps them
/// ([`Self::finish`]) or puts everything back ([`Self::rollback`]).
/// Dropped without either, it puts everything back.
pub struct Transaction {
    lock: Option<ImportLock>,
    path: PathBuf,
    journal: Journal,
    /// Entries (by index) whose files were renamed into place.
    done: usize,
    finished: bool,
}

impl Transaction {
    /// Record a transaction for `staged` (target, staged temp) pairs and
    /// back up `config.json`. Nothing is replaced yet.
    pub fn begin(
        lock: ImportLock,
        staged: Vec<(PathBuf, PathBuf)>,
        created_dirs: Vec<PathBuf>,
    ) -> Result<Self> {
        let dir = kage_dir()?;
        if exists(&dir.join(JOURNAL_NAME)) {
            // Never written over: it's the only record of an import that
            // hasn't been finished or undone.
            anyhow::bail!("An earlier import is still unresolved");
        }
        let config = crate::config::Config::get_config_path()?;
        let config_backup = if exists(&config) {
            let b = super::sibling_temp(&config, "bak")?;
            durable_copy(&config, &b)?;
            Some(b)
        } else {
            None
        };
        let discard_config_backup = |b: &Option<PathBuf>| {
            if let Some(b) = b {
                let _ = std::fs::remove_file(b);
            }
        };
        let entries = staged
            .into_iter()
            .map(|(target, staged)| {
                let backup = if exists(&target) {
                    Some(super::sibling_temp(&target, "bak")?)
                } else {
                    None
                };
                // What this transaction will put at `target`: the staged
                // file itself, renamed into place.
                let written = Written::of(&staged)?;
                Ok(FileEntry {
                    target,
                    staged,
                    backup,
                    written: Some(written),
                    undone: false,
                })
            })
            .collect::<Result<Vec<_>>>();
        let files = match entries {
            Ok(files) => files,
            Err(e) => {
                discard_config_backup(&config_backup);
                return Err(e);
            }
        };
        let journal = Journal {
            format: 1,
            state: State::Applying,
            config,
            config_backup,
            files,
            created_dirs,
            restore_config: true,
            attempts: 0,
            config_undone: false,
        };
        let path = dir.join(JOURNAL_NAME);
        if let Err(e) = write_journal(&path, &journal) {
            discard_config_backup(&journal.config_backup);
            return Err(e);
        }
        Ok(Self {
            lock: Some(lock),
            path,
            journal,
            done: 0,
            finished: false,
        })
    }

    /// Put every staged file in place: the current one is moved aside
    /// (kept whole for a rollback), then the new one renamed in. A rename
    /// replaces a link instead of writing through it. On failure everything
    /// is put back.
    pub fn commit(&mut self) -> Result<()> {
        while self.done < self.journal.files.len() {
            let f = &self.journal.files[self.done];
            let result = (|| -> Result<()> {
                if f.backup.is_none() && exists(&f.target) {
                    // Created since the journal was written (an extension
                    // saving): it has no rollback copy, so it isn't
                    // replaced; the import is undone instead.
                    anyhow::bail!("{:?} appeared while importing", f.target);
                }
                if let Some(b) = &f.backup {
                    std::fs::rename(&f.target, b)
                        .with_context(|| format!("Failed to replace {:?}", f.target))?;
                }
                if let Err(e) = std::fs::rename(&f.staged, &f.target) {
                    if let Some(b) = &f.backup {
                        let _ = std::fs::rename(b, &f.target);
                    }
                    return Err(e).with_context(|| format!("Failed to write {:?}", f.target));
                }
                Ok(())
            })();
            if let Err(e) = result {
                self.undo();
                return Err(e);
            }
            self.done += 1;
        }
        for d in self.journal.files.iter().filter_map(|f| f.target.parent()) {
            sync_dir(d);
        }
        Ok(())
    }

    /// Keep the new files (the caller saved the config that goes with
    /// them): record the commit, then delete what they replaced.
    pub fn finish(mut self) -> Result<()> {
        self.journal.state = State::Committed;
        if let Err(e) = write_journal(&self.path, &self.journal) {
            // Without the marker a later recovery would undo this import;
            // undo it now instead, so files and config.json still match.
            log::error!("Import: couldn't record the commit ({}); undoing it", e);
            self.journal.state = State::Applying;
            self.undo();
            return Err(e);
        }
        complete(&self.journal);
        let _ = std::fs::remove_file(&self.path);
        self.finished = true;
        self.lock = None;
        Ok(())
    }

    /// Put every replaced file back and remove the new ones (the caller
    /// couldn't save the config that goes with them).
    pub fn rollback(mut self) {
        self.undo();
    }

    /// Test helper: stop as a crash would, journal and all left behind.
    #[cfg(test)]
    pub fn abandon(mut self) {
        self.finished = true;
        self.lock = None;
    }

    /// Undo in the running Kage. The caller keeps the right config in
    /// memory (and saves it), so config.json is put back here if it can be
    /// and is never left for a later start to restore.
    fn undo(&mut self) {
        // Recorded first: if this undo is cut short, a later start must not
        // restore (or delete) a config.json the running Kage saved since.
        self.journal.restore_config = false;
        if let Err(e) = write_journal(&self.path, &self.journal) {
            log::error!("Import rollback: couldn't update the journal: {:#}", e);
        }
        if let Some(b) = self.journal.config_backup.clone() {
            if exists(&b) && std::fs::rename(&b, &self.journal.config).is_err() {
                log::warn!("Import rollback: couldn't put config.json back; the running config will be saved over it");
                let _ = std::fs::remove_file(&b);
            }
        }
        if roll_back(&self.path, &mut self.journal, Unrecorded::KeepGoing) {
            let _ = std::fs::remove_file(&self.path);
        } else {
            let _ = write_journal(&self.path, &self.journal);
            log::error!(
                "Import rollback incomplete; the journal stays at {:?} for the next start",
                self.path
            );
        }
        self.finished = true;
        self.lock = None;
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.finished {
            self.undo();
        }
    }
}

/// Delete what a committed transaction replaced.
fn complete(j: &Journal) {
    for f in &j.files {
        if let Some(b) = &f.backup {
            let _ = std::fs::remove_file(b);
        }
        let _ = std::fs::remove_file(&f.staged);
    }
    if let Some(b) = &j.config_backup {
        let _ = std::fs::remove_file(b);
    }
}

/// Remove `path`; `false` if it's still there (an error other than "not
/// found").
fn remove_new(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            log::error!("Import rollback: couldn't remove {:?}: {}", path, e);
            false
        }
    }
}

/// Whether the folder `path` lives in is there to look at (a steering doc
/// can be on a volume that isn't mounted right now).
fn folder_reachable(path: &Path) -> bool {
    path.parent().is_some_and(Path::is_dir)
}

/// Undo one file of an uncommitted transaction. `true` when it's done:
/// the old file is back, or the new one gone, or what's there now isn't
/// this transaction's to touch. `false`: try again later.
fn undo_file(f: &FileEntry, created_dirs: &[PathBuf]) -> bool {
    if !folder_reachable(&f.target) {
        // A new file in a folder this import created, already removed
        // with it: nothing left to undo.
        let ours = f.backup.is_none()
            && f.target
                .parent()
                .is_some_and(|d| created_dirs.iter().any(|c| c == d));
        if !ours {
            log::error!("Import rollback: {:?} isn't reachable now", f.target);
        }
        return ours;
    }
    if exists(&f.staged) {
        // Never renamed into place: the target is untouched, unless the
        // first rename (moving it aside) happened and the second didn't.
        if !remove_new(&f.staged) {
            return false;
        }
        if let Some(b) = f.backup.as_ref().filter(|b| exists(b)) {
            if exists(&f.target) {
                log::warn!(
                    "Import rollback: {:?} was created after the import moved it aside; keeping it, the earlier copy stays at {:?}",
                    f.target,
                    b
                );
            } else if let Err(e) = std::fs::rename(b, &f.target) {
                log::error!("Import rollback: couldn't restore {:?}: {}", f.target, e);
                return false;
            }
        }
        return true;
    }
    // Renamed into place. Only this transaction's own file there is
    // removed or replaced.
    let ours = exists(&f.target) && f.written.as_ref().is_some_and(|w| w.is_at(&f.target));
    if exists(&f.target) && !ours {
        log::warn!(
            "Import rollback: {:?} changed since the import wrote it; keeping it{}",
            f.target,
            match f.backup.as_ref().filter(|b| exists(b)) {
                Some(b) => format!(", the earlier copy stays at {:?}", b),
                None => String::new(),
            }
        );
        return true;
    }
    match &f.backup {
        Some(b) if exists(b) => match std::fs::rename(b, &f.target) {
            Ok(()) => true,
            Err(e) => {
                log::error!("Import rollback: couldn't restore {:?}: {}", f.target, e);
                false
            }
        },
        // Already put back on an earlier try.
        Some(_) => true,
        None if ours => remove_new(&f.target),
        None => true,
    }
}

/// What [`roll_back`] does when a finished step can't be recorded.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unrecorded {
    /// Stop: the next try starts from what the journal says.
    Stop,
    /// Carry on (the running Kage undoing its own import restores its
    /// config at once, and the files must match it), keeping the journal so
    /// the next try checks again. A step redone then finds nothing of its
    /// own to undo ([`Written::is_at`]).
    KeepGoing,
}

/// Undo an uncommitted transaction: every file back as it was, then
/// config.json. Each finished step is recorded in the journal at `path`
/// before the next one starts, and steps an earlier try recorded are
/// skipped. Returns whether everything was put back and recorded (and so
/// the journal may go); anything it couldn't reach, undo or record leaves
/// the journal, and every rollback copy, for the next try.
fn roll_back(path: &Path, j: &mut Journal, unrecorded: Unrecorded) -> bool {
    let mut ok = true;
    let record = |step: Step, ok: &mut bool| -> bool {
        match record_step(path, step) {
            Ok(()) => true,
            Err(e) => {
                log::error!("Import rollback: {:#}", e);
                *ok = false;
                unrecorded == Unrecorded::KeepGoing
            }
        }
    };
    for i in (0..j.files.len()).rev() {
        if j.files[i].undone {
            continue;
        }
        if !undo_file(&j.files[i], &j.created_dirs) {
            ok = false;
            continue;
        }
        j.files[i].undone = true;
        if !record(Step::File(i), &mut ok) {
            return false;
        }
    }
    if j.restore_config && !j.config_undone {
        match &j.config_backup {
            Some(b) if exists(b) => match std::fs::rename(b, &j.config) {
                Ok(()) => {
                    j.config_undone = true;
                    if !record(Step::Config, &mut ok) {
                        return false;
                    }
                }
                Err(e) => {
                    log::error!("Import rollback: couldn't restore config.json: {}", e);
                    ok = false;
                }
            },
            // Put back on an earlier try (its record was lost).
            Some(_) => {
                j.config_undone = true;
                if !record(Step::Config, &mut ok) {
                    return false;
                }
            }
            // There was none, so the one there now was saved by this import
            // (nothing else writes config.json while it holds the lock).
            // Recorded *before* it's removed: a later try must never remove
            // one saved since, and a removal that fails here leaves only
            // this import's config behind.
            None => match record_step(path, Step::Config) {
                Ok(()) => {
                    j.config_undone = true;
                    if !remove_new(&j.config) {
                        log::error!(
                            "Import rollback: config.json from the undone import is still there"
                        );
                    }
                }
                Err(e) => {
                    log::error!("Import rollback: {:#}", e);
                    return false;
                }
            },
        }
    }
    // Only once everything is back (a retry may need them), and only if
    // still empty: never removes anything else.
    if ok {
        for d in j.created_dirs.iter().rev() {
            let _ = std::fs::remove_dir(d);
        }
    }
    ok
}

/// What [`recover`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Recovery {
    /// No import was interrupted.
    Nothing,
    /// One had committed: its leftovers were deleted.
    Completed,
    /// One hadn't: everything was put back as it was before it.
    RolledBack,
    /// Another Kage process is importing right now: left alone.
    Busy,
    /// One couldn't be undone after repeated tries: its journal was set
    /// aside (renamed) with every rollback copy kept, so imports go on.
    SetAside,
}

/// Finish or undo an import a previous run left (see the module docs).
pub fn recover() -> Result<Recovery> {
    let Some(lock) = ImportLock::acquire()? else {
        return Ok(Recovery::Busy);
    };
    recover_locked(&lock)
}

/// [`recover`], for a caller already holding the lock. An error leaves the
/// journal (and every rollback copy) in place.
pub fn recover_locked(_lock: &ImportLock) -> Result<Recovery> {
    let path = kage_dir()?.join(JOURNAL_NAME);
    let body = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Recovery::Nothing),
        Err(e) => return Err(e).context("Failed to read the import journal"),
    };
    let mut journal = read_journal(&body)?;
    if !journal.is_well_formed() {
        anyhow::bail!("The import journal names files that aren't an import's own");
    }
    match journal.state {
        State::Committed => {
            complete(&journal);
            std::fs::remove_file(&path).context("Failed to remove the import journal")?;
            log::info!("Finished cleaning up an import that completed before Kage stopped");
            Ok(Recovery::Completed)
        }
        State::Applying => {
            if !roll_back(&path, &mut journal, Unrecorded::Stop) {
                journal.attempts += 1;
                if journal.attempts >= MAX_ATTEMPTS {
                    let aside = path.with_file_name(format!(
                        "import-journal.unresolved-{}.json",
                        chrono::Utc::now().format("%Y%m%dT%H%M%S")
                    ));
                    let _ = write_journal(&path, &journal);
                    std::fs::rename(&path, &aside)
                        .context("Failed to set the import journal aside")?;
                    log::error!(
                        "Gave up undoing an interrupted import after {} tries; its journal is at {:?} and every rollback copy was kept",
                        journal.attempts,
                        aside
                    );
                    return Ok(Recovery::SetAside);
                }
                let _ = write_journal(&path, &journal);
                anyhow::bail!("Couldn't put back every file of an interrupted import");
            }
            std::fs::remove_file(&path).context("Failed to remove the import journal")?;
            log::warn!("Undid an import that was interrupted before it finished");
            Ok(Recovery::RolledBack)
        }
    }
}

#[cfg(test)]
pub(super) fn journal_path() -> PathBuf {
    kage_dir().unwrap().join(JOURNAL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &Path, target: &str, backup: Option<&str>) -> FileEntry {
        FileEntry {
            target: dir.join(target),
            staged: dir.join(format!("{}1-0.tmp", super::super::TEMP_PREFIX)),
            backup: backup.map(|b| dir.join(b)),
            written: None,
            undone: false,
        }
    }

    #[test]
    fn a_target_is_this_transactions_only_while_it_is_the_file_written() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.json");
        std::fs::write(&target, b"BUNDLE").unwrap();
        let written = Written::of(&target).unwrap();
        assert!(written.is_at(&target));
        // Rewritten in place, or replaced by a new file with the same bytes.
        std::fs::write(&target, b"NEWER!").unwrap();
        assert!(!written.is_at(&target));
        // (Made while the old one still exists, so it can't reuse its inode.)
        let other = dir.path().join("other.json");
        std::fs::write(&other, b"BUNDLE").unwrap();
        std::fs::rename(&other, &target).unwrap();
        assert_eq!(
            written.is_at(&target),
            cfg!(not(unix)),
            "a new file is never the one written (Unix: by inode)"
        );
        assert!(!written.is_at(&dir.path().join("missing.json")));
    }

    #[test]
    fn an_undo_never_removes_or_replaces_what_it_cant_prove_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        // A journal from an earlier build records nothing about the write:
        // the file there stays.
        std::fs::write(p.join("new.json"), b"someone's").unwrap();
        assert!(undo_file(&entry(p, "new.json", None), &[]));
        assert_eq!(std::fs::read(p.join("new.json")).unwrap(), b"someone's");
        // A file written there since the import: kept, and the earlier copy
        // with it.
        std::fs::write(p.join("old.json"), b"NEWER").unwrap();
        let bak = format!("{}1-1~old.json.bak", super::super::TEMP_PREFIX);
        std::fs::write(p.join(&bak), b"LOCAL").unwrap();
        let mut f = entry(p, "old.json", Some(&bak));
        std::fs::write(p.join("probe"), b"BUNDLE").unwrap();
        f.written = Some(Written::of(&p.join("probe")).unwrap());
        assert!(undo_file(&f, &[]));
        assert_eq!(std::fs::read(p.join("old.json")).unwrap(), b"NEWER");
        assert_eq!(std::fs::read(p.join(&bak)).unwrap(), b"LOCAL");
        // The import's own file: the earlier copy goes back.
        std::fs::write(p.join("old.json"), b"BUNDLE").unwrap();
        f.written = Some(Written::of(&p.join("old.json")).unwrap());
        assert!(undo_file(&f, &[]));
        assert_eq!(std::fs::read(p.join("old.json")).unwrap(), b"LOCAL");
        assert!(!p.join(&bak).exists());
    }

    #[test]
    fn recorded_steps_are_read_back_and_a_torn_one_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal {
            format: 1,
            state: State::Applying,
            config: dir.path().join("config.json"),
            config_backup: None,
            files: vec![
                entry(dir.path(), "a.json", None),
                entry(dir.path(), "b.json", None),
                entry(dir.path(), "c.json", None),
            ],
            created_dirs: Vec::new(),
            restore_config: true,
            attempts: 0,
            config_undone: false,
        };
        let path = dir.path().join(JOURNAL_NAME);
        write_journal(&path, &journal).unwrap();
        record_step(&path, Step::File(2)).unwrap();
        record_step(&path, Step::Config).unwrap();
        // Cut short by a crash.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"\n{\"file\":").unwrap();
        drop(f);
        let read = read_journal(&std::fs::read(&path).unwrap()).unwrap();
        let undone: Vec<bool> = read.files.iter().map(|f| f.undone).collect();
        assert_eq!(undone, vec![false, false, true]);
        assert!(read.config_undone);
        // Rewritten whole (a failed try counted), the steps stay.
        write_journal(&path, &read).unwrap();
        let again = read_journal(&std::fs::read(&path).unwrap()).unwrap();
        assert!(again.files[2].undone);
        assert!(again.config_undone);
    }
}
