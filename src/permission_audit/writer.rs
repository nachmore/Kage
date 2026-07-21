use super::{default_log_path, AuditEntry};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Append one entry to the given log file. Best-effort: a failure is
/// logged via `log::warn!` and swallowed so a broken disk never blocks
/// a grant operation. Creates the parent directory if missing.
///
/// Each call opens, writes, and closes the file. This is the testing
/// API — production code uses [`append`] which keeps a single
/// long-lived `BufWriter` per process to avoid the open/close round-trip
/// per event. Marked `dead_code`-allowed because the lib's normal call
/// path goes through [`append`]; this entry point exists for tests that
/// need a per-call open/close so they can read what they wrote.
#[allow(dead_code)]
pub fn append_to(path: &Path, entry: &AuditEntry) {
    if !ensure_parent(path) {
        return;
    }
    let Some(serialized) = serialize(entry) else {
        return;
    };

    let mut file = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("audit log: failed to open {}: {}", path.display(), e);
            return;
        }
    };
    // Single writeln = one line, atomic on most OSes for short writes
    // (well under PIPE_BUF). We accept small interleavings if two
    // threads race, which is fine because every line is self-contained.
    if let Err(e) = writeln!(file, "{}", serialized) {
        log::warn!("audit log: failed to append to {}: {}", path.display(), e);
    }
}

pub(super) fn ensure_parent(path: &Path) -> bool {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                log::warn!("audit log: cannot create {}: {}", parent.display(), e);
                return false;
            }
        }
    }
    true
}

pub(super) fn serialize(entry: &AuditEntry) -> Option<String> {
    match serde_json::to_string(entry) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("audit log: failed to serialize entry: {}", e);
            None
        }
    }
}

/// Long-lived writer for the default log path. Audit events arrive
/// during permission-grant flows, which run on the Tauri command
/// thread pool — opening and closing the file every time was wasted
/// I/O, especially during automation runs that grant many tools in
/// quick succession.
///
/// One process-wide handle keeps the FD open and a small BufWriter
/// in front of it. We flush after every record so a crash doesn't
/// lose more than the one entry that was mid-write.
static DEFAULT_WRITER: OnceLock<Mutex<Option<BufWriter<File>>>> = OnceLock::new();

fn default_writer_lock() -> &'static Mutex<Option<BufWriter<File>>> {
    DEFAULT_WRITER.get_or_init(|| Mutex::new(None))
}

fn open_default_writer() -> Option<BufWriter<File>> {
    let path = default_log_path()?;
    if !ensure_parent(&path) {
        return None;
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => Some(BufWriter::with_capacity(4096, f)),
        Err(e) => {
            log::warn!("audit log: failed to open {}: {}", path.display(), e);
            None
        }
    }
}

/// Append to the default path through the cached writer.
pub fn append(entry: &AuditEntry) {
    let Some(serialized) = serialize(entry) else {
        return;
    };

    let lock = default_writer_lock();
    let mut guard = match lock.lock() {
        Ok(g) => g,
        // Another thread panicked while holding the writer — recover
        // the inner state. The writer might be in a half-flushed state
        // but it's better to keep going than refuse all future audits.
        Err(p) => p.into_inner(),
    };

    // Lazy-init on first use so a process that never grants any tools
    // doesn't even open the file.
    if guard.is_none() {
        *guard = open_default_writer();
    }

    let Some(writer) = guard.as_mut() else {
        // Couldn't open — already warned in open_default_writer; no point
        // logging again per entry.
        return;
    };

    if let Err(e) = writeln!(writer, "{}", serialized) {
        log::warn!("audit log: write failed: {}", e);
        // Drop the writer — the FD may be in a bad state. Next append
        // will try to reopen.
        *guard = None;
        return;
    }
    // Flush every record so `read_recent_default` sees recent entries
    // and a crash doesn't drop more than the in-flight line.
    if let Err(e) = writer.flush() {
        log::warn!("audit log: flush failed: {}", e);
        *guard = None;
    }
}

/// Drop the cached writer so the next `append` reopens. Used by `clear`
/// after it truncates the file out from under us — keeping the old FD
/// would either keep writing into a deleted-but-still-open file (Unix)
/// or fail outright (Windows).
fn reset_default_writer() {
    let lock = default_writer_lock();
    if let Ok(mut guard) = lock.lock() {
        *guard = None;
    }
}

/// Truncate the log. Used by the "clear audit log" UI action. Does
/// nothing if the file doesn't exist.
pub fn clear(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    fs::File::create(path)?; // create with truncate
    Ok(())
}

pub fn clear_default() -> std::io::Result<()> {
    let Some(path) = default_log_path() else {
        return Ok(());
    };
    // Drop the cached writer first so the truncate isn't fighting an open
    // append-mode FD. The next `append` will reopen against the fresh file.
    reset_default_writer();
    clear(&path)
}
