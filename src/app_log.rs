//! Application-level structured logging system.
//!
//! Provides a ring-buffered, file-backed JSONL log accessible from both Rust
//! and the frontend (via Tauri commands). Each entry carries a timestamp,
//! level, source tag, and message.
//!
//! # Design
//!
//! Two paths keep the hot side cheap:
//!
//! 1. **In-memory ring buffer** (capped at `max_size`) — every `log()` call
//!    pushes here under a short `Mutex` critical section. This is what the
//!    UI viewer and the panic handler read via `get_entries()`.
//!
//! 2. **Bounded channel to a dedicated writer thread** — the same `log()`
//!    call also does a non-blocking `try_send` of the entry. A single
//!    background std thread (`app-log-writer`) drains the channel in batches,
//!    appends to the on-disk JSONL file, and rotates the file by size.
//!
//! No file I/O happens on the caller's thread, so the Tauri async runtime
//! (and any Rust code calling `log()`) never blocks on disk. If the channel
//! is full we drop the on-disk copy silently; the in-memory ring still has
//! the entry for the UI viewer and panic handler.
//!
//! # Rotation
//!
//! When the file exceeds `ROTATE_SIZE_BYTES`, the writer renames it to
//! `<name>.old.jsonl`, overwriting any previous `.old` file, and starts a
//! fresh one. We only keep one rotated file — this is a developer log, not
//! an audit log.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

/// A single structured log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: String,
    pub level: String,
    pub source: String,
    pub msg: String,
}

/// Rotate the file when it exceeds this size. We only keep the current file
/// plus one `.old` sibling, so peak disk usage is 2× this value.
const ROTATE_SIZE_BYTES: u64 = 2 * 1024 * 1024;

/// Writer batch flush cadence. The writer thread wakes up at least this often
/// to flush any pending entries. Batches arriving faster than this are
/// written in a single syscall.
const FLUSH_INTERVAL_MS: u64 = 500;

/// Bound on the writer's mailbox. Enough to absorb bursts of several
/// hundred log entries without dropping. If exceeded, the disk copy of the
/// overflow entry is dropped silently (in-memory ring still has it).
const CHANNEL_CAPACITY: usize = 4096;

/// Messages sent to the writer thread.
enum WriterMsg {
    Entry(LogEntry),
    /// Flush pending entries immediately (drain + write + fsync).
    /// The provided ack-sender receives `()` once the flush is done, giving
    /// the caller a way to block until the data is on disk.
    Flush(mpsc::SyncSender<()>),
    /// Truncate on-disk file as part of a user-initiated clear.
    Clear,
    // NOTE: there's no explicit Shutdown message. The writer loop exits
    // naturally when all senders are dropped (Disconnected from recv_timeout).
    // At process shutdown we call `flush()` which sends a Flush+ack through
    // the existing channel; the OS tears the thread down on exit.
}

/// Global app log instance.
static APP_LOG: std::sync::OnceLock<Mutex<AppLog>> = std::sync::OnceLock::new();

/// Sender handle for the writer thread. Initialized alongside `APP_LOG`.
static WRITER_TX: std::sync::OnceLock<SyncSender<WriterMsg>> = std::sync::OnceLock::new();

struct AppLog {
    buffer: VecDeque<LogEntry>,
    max_size: usize,
}

impl AppLog {
    fn new(max_size: usize, log_path: &std::path::Path) -> Result<Self> {
        // Load existing entries from disk so the UI viewer still shows recent
        // history on restart. Best-effort — corruption shouldn't fail init.
        let mut buffer = VecDeque::with_capacity(max_size.min(8192));
        if log_path.exists() {
            if let Ok(file) = File::open(log_path) {
                let reader = BufReader::new(file);
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                        buffer.push_back(entry);
                    }
                }
                while buffer.len() > max_size {
                    buffer.pop_front();
                }
            }
        }

        Ok(Self { buffer, max_size })
    }

    fn push(&mut self, entry: LogEntry) {
        self.buffer.push_back(entry);
        while self.buffer.len() > self.max_size {
            self.buffer.pop_front();
        }
    }

    fn entries(&self) -> Vec<LogEntry> {
        self.buffer.iter().cloned().collect()
    }

    fn clear_buffer(&mut self) {
        self.buffer.clear();
    }

    fn set_max_size(&mut self, new_max: usize) {
        self.max_size = new_max;
        while self.buffer.len() > self.max_size {
            self.buffer.pop_front();
        }
    }
}

/// Get the log directory path.
pub fn get_log_dir() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("Failed to get local data directory")?
        .join("kage")
        .join("logs");
    Ok(dir)
}

fn log_file_path() -> Result<PathBuf> {
    Ok(get_log_dir()?.join("app.jsonl"))
}

/// Initialize the global app log. Call once at startup.
pub fn init(max_size: usize) -> Result<()> {
    let log_path = log_file_path()?;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let log = AppLog::new(max_size, &log_path)?;
    APP_LOG
        .set(Mutex::new(log))
        .map_err(|_| anyhow::anyhow!("App log already initialized"))?;

    // Start the writer thread. Use a bounded channel so senders never block
    // indefinitely on a slow disk; `try_send` drops entries if we can't keep
    // up and increments DROPPED_COUNT.
    let (tx, rx) = mpsc::sync_channel::<WriterMsg>(CHANNEL_CAPACITY);
    WRITER_TX
        .set(tx)
        .map_err(|_| anyhow::anyhow!("App log writer already started"))?;

    thread::Builder::new()
        .name("app-log-writer".into())
        .spawn(move || writer_loop(rx, log_path))
        .context("Failed to spawn app-log-writer thread")?;

    Ok(())
}

/// Background writer loop. Owns the file handle; no other code touches it.
///
/// Drains the channel in batches, appending JSONL entries. Rotates the file
/// when it gets too big by renaming to `.old.jsonl`, overwriting any prior
/// rotated copy (we only keep one).
fn writer_loop(rx: mpsc::Receiver<WriterMsg>, log_path: PathBuf) {
    // Open file in append mode. If the open fails, we still drain the channel
    // so senders don't block — they just lose their disk persistence.
    let mut file: Option<BufWriter<File>> = open_append(&log_path);
    let mut current_size: u64 = file_size(&log_path);

    loop {
        // Wait up to FLUSH_INTERVAL_MS for a message. On timeout we just
        // loop back and check if we should flush.
        let first = match rx.recv_timeout(Duration::from_millis(FLUSH_INTERVAL_MS)) {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Periodic flush of any buffered bytes. BufWriter::flush is
                // cheap when the inner buffer is empty.
                if let Some(ref mut f) = file {
                    let _ = f.flush();
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // All senders dropped — we're done.
                if let Some(mut f) = file.take() {
                    let _ = f.flush();
                }
                return;
            }
        };

        // Collect a batch: the first message plus everything else already
        // pending. This is the single-syscall win.
        let mut batch: Vec<LogEntry> = Vec::new();
        let mut flush_acks: Vec<mpsc::SyncSender<()>> = Vec::new();
        let mut clear = false;

        handle_msg(first, &mut batch, &mut flush_acks, &mut clear);
        while let Ok(msg) = rx.try_recv() {
            handle_msg(msg, &mut batch, &mut flush_acks, &mut clear);
        }

        // Honour clear before the batch write — a clear means the user wants
        // a clean slate. Anything we just buffered from this cycle still gets
        // written (the entries they're clearing are the old ones on disk).
        if clear {
            // Drop current handle, truncate, reopen.
            if let Some(f) = file.take() {
                drop(f);
            }
            if log_path.exists() {
                let _ = fs::remove_file(&log_path);
            }
            let _ = fs::remove_file(sibling_old_path(&log_path));
            file = open_append(&log_path);
            current_size = 0;
        }

        if !batch.is_empty() {
            // Rotate BEFORE appending if the current file is already over
            // threshold. This way the oversized file gets rotated cleanly
            // and the fresh entries land in the new (small) current file.
            if current_size >= ROTATE_SIZE_BYTES {
                if let Some(mut f) = file.take() {
                    let _ = f.flush();
                    drop(f);
                }
                let old_path = sibling_old_path(&log_path);
                if let Err(e) = rotate(&log_path, &old_path) {
                    log::warn!("app_log: rotation failed: {e}");
                }
                file = open_append(&log_path);
                current_size = file_size(&log_path);
            }

            if let Some(ref mut f) = file {
                let written = write_batch(f, &batch);
                current_size = current_size.saturating_add(written);
            }
        }

        // Ack any flush requests after the batch is on disk.
        if !flush_acks.is_empty() {
            if let Some(ref mut f) = file {
                let _ = f.flush();
            }
            for ack in flush_acks {
                // Receiver may have gone away — not our problem.
                let _ = ack.try_send(());
            }
        }
    }
}

fn handle_msg(
    msg: WriterMsg,
    batch: &mut Vec<LogEntry>,
    flush_acks: &mut Vec<mpsc::SyncSender<()>>,
    clear: &mut bool,
) {
    match msg {
        WriterMsg::Entry(e) => batch.push(e),
        WriterMsg::Flush(ack) => flush_acks.push(ack),
        WriterMsg::Clear => *clear = true,
    }
}

fn write_batch(file: &mut BufWriter<File>, batch: &[LogEntry]) -> u64 {
    let mut bytes: u64 = 0;
    for entry in batch {
        let line = match serde_json::to_string(entry) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Single writeln per entry goes to the BufWriter's buffer; one
        // underlying syscall per buffer-full (or on flush).
        if writeln!(file, "{line}").is_ok() {
            bytes = bytes.saturating_add(line.len() as u64 + 1); // +1 for '\n'
        }
    }
    bytes
}

fn open_append(path: &std::path::Path) -> Option<BufWriter<File>> {
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(f) => Some(BufWriter::with_capacity(16 * 1024, f)),
        Err(e) => {
            log::warn!("app_log: failed to open {path:?} for append: {e}");
            None
        }
    }
}

fn file_size(path: &std::path::Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn rotate(log_path: &std::path::Path, old_path: &std::path::Path) -> Result<()> {
    // Overwrite any existing .old file — we only keep one rotation.
    if old_path.exists() {
        fs::remove_file(old_path).context("remove existing .old log")?;
    }
    if log_path.exists() {
        fs::rename(log_path, old_path).context("rotate current log to .old")?;
    }
    Ok(())
}

/// Compute the `.old.jsonl` sibling path for a given current log path.
/// `app.jsonl` → `app.old.jsonl`, `foo.log` → `foo.old.log`, etc.
fn sibling_old_path(log_path: &std::path::Path) -> PathBuf {
    let ext = log_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let stem = log_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("app");
    let new_name = if ext.is_empty() {
        format!("{stem}.old")
    } else {
        format!("{stem}.old.{ext}")
    };
    match log_path.parent() {
        Some(dir) => dir.join(new_name),
        None => PathBuf::from(new_name),
    }
}

/// Maximum message length before truncation.
const MAX_MSG_LEN: usize = 500;

/// Truncate a message if it exceeds MAX_MSG_LEN, appending an indicator.
fn truncate_msg(msg: &str) -> String {
    if msg.len() <= MAX_MSG_LEN {
        msg.to_string()
    } else {
        let mut truncated = msg[..MAX_MSG_LEN].to_string();
        truncated.push_str(&format!("... [truncated, {} total chars]", msg.len()));
        truncated
    }
}

/// Write a log entry.
///
/// Pushes into the in-memory ring buffer immediately, then does a non-blocking
/// `try_send` to the writer thread for disk persistence. If the writer is
/// backed up, the disk copy is dropped (in-memory ring still has it) and
/// `DROPPED_COUNT` is incremented.
pub fn log(level: &str, source: &str, msg: &str) {
    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    log_with_ts(&ts, level, source, msg);
}

/// Like `log`, but uses a caller-supplied timestamp instead of "now". Used
/// by the `log` crate adapter to drain its pre-init buffer with the
/// timestamps the entries originally carried, so replay doesn't squash
/// boot-time events into one moment.
pub fn log_with_ts(ts: &str, level: &str, source: &str, msg: &str) {
    let entry = LogEntry {
        ts: ts.to_string(),
        level: level.to_string(),
        source: source.to_string(),
        msg: truncate_msg(msg),
    };

    // In-memory ring first. Short critical section — only a VecDeque push.
    if let Some(lock) = APP_LOG.get() {
        if let Ok(mut log) = lock.lock() {
            log.push(entry.clone());
        }
    }

    // Then try to hand the entry to the writer. Non-blocking — if the mailbox
    // is full we drop the disk copy and move on rather than stalling the
    // caller. The in-memory ring still has the entry, so the UI viewer and
    // panic handler are unaffected.
    if let Some(tx) = WRITER_TX.get() {
        match tx.try_send(WriterMsg::Entry(entry)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                // Full: writer is backed up — drop silently.
                // Disconnected: writer is gone; shouldn't happen while the
                // app is running.
            }
        }
    }
}

/// Flush pending entries to disk. Blocks briefly (up to ~1s) waiting for the
/// writer to acknowledge. Safe to call from any thread, including during
/// shutdown.
pub fn flush() {
    let Some(tx) = WRITER_TX.get() else { return };

    let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
    // Use blocking send — we're about to block anyway waiting for the ack,
    // and a missed flush at shutdown is worse than a 500ms stall.
    if tx.send(WriterMsg::Flush(ack_tx)).is_err() {
        return;
    }
    let _ = ack_rx.recv_timeout(Duration::from_millis(1000));
}

/// Get a snapshot of all in-memory log entries.
pub fn get_entries() -> Vec<LogEntry> {
    APP_LOG
        .get()
        .and_then(|lock| lock.lock().ok())
        .map(|log| log.entries())
        .unwrap_or_default()
}

/// Clear both in-memory ring and on-disk log files.
pub fn clear() -> Result<()> {
    if let Some(lock) = APP_LOG.get() {
        if let Ok(mut log) = lock.lock() {
            log.clear_buffer();
        }
    }
    // Ask the writer to truncate on its own thread. Keeps file ownership
    // in one place.
    if let Some(tx) = WRITER_TX.get() {
        let _ = tx.send(WriterMsg::Clear);
    }
    Ok(())
}

/// True once `init` has run. Used by the `log` crate adapter to decide
/// whether it can call `log` directly or needs to buffer the entry until
/// the writer thread is up.
pub fn is_initialized() -> bool {
    APP_LOG.get().is_some() && WRITER_TX.get().is_some()
}

/// Update the max buffer size (e.g. when config changes).
pub fn set_max_size(new_max: usize) {
    if let Some(lock) = APP_LOG.get() {
        if let Ok(mut log) = lock.lock() {
            log.set_max_size(new_max);
        }
    }
}

/// Get the path to the log directory (for "Open Logs Folder").
pub fn log_dir_string() -> String {
    get_log_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Convenience macros for Rust-side logging.
#[macro_export]
macro_rules! app_log_info {
    ($source:expr, $($arg:tt)*) => {
        $crate::app_log::log("info", $source, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! app_log_warn {
    ($source:expr, $($arg:tt)*) => {
        $crate::app_log::log("warn", $source, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! app_log_error {
    ($source:expr, $($arg:tt)*) => {
        $crate::app_log::log("error", $source, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! app_log_debug {
    ($source:expr, $($arg:tt)*) => {
        $crate::app_log::log("debug", $source, &format!($($arg)*))
    };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Tests hit the writer thread via a temporary log file path. Because
    //! `APP_LOG` / `WRITER_TX` are process-global `OnceLock`s, we drive the
    //! internal `AppLog` / writer loop directly rather than through `init`.

    use super::*;
    use std::sync::mpsc;
    use tempfile::TempDir;

    fn make_entry(level: &str, source: &str, msg: &str) -> LogEntry {
        LogEntry {
            ts: "2026-05-04T10:00:00.000Z".to_string(),
            level: level.to_string(),
            source: source.to_string(),
            msg: msg.to_string(),
        }
    }

    #[test]
    fn in_memory_ring_respects_max_size() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("app.jsonl");
        let mut log = AppLog::new(3, &path).unwrap();

        log.push(make_entry("info", "t", "a"));
        log.push(make_entry("info", "t", "b"));
        log.push(make_entry("info", "t", "c"));
        log.push(make_entry("info", "t", "d"));

        let entries = log.entries();
        let msgs: Vec<_> = entries.iter().map(|e| e.msg.as_str()).collect();
        assert_eq!(msgs, vec!["b", "c", "d"]);
    }

    #[test]
    fn in_memory_ring_shrinks_on_set_max_size() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("app.jsonl");
        let mut log = AppLog::new(5, &path).unwrap();
        for i in 0..5 {
            log.push(make_entry("info", "t", &format!("{i}")));
        }
        log.set_max_size(2);
        let msgs: Vec<_> = log.entries().into_iter().map(|e| e.msg).collect();
        assert_eq!(msgs, vec!["3".to_string(), "4".to_string()]);
    }

    #[test]
    fn load_restores_recent_entries_up_to_max() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("app.jsonl");
        {
            let mut f = File::create(&path).unwrap();
            for i in 0..10 {
                let entry = make_entry("info", "t", &format!("{i}"));
                writeln!(f, "{}", serde_json::to_string(&entry).unwrap()).unwrap();
            }
        }
        let log = AppLog::new(3, &path).unwrap();
        let msgs: Vec<_> = log.entries().into_iter().map(|e| e.msg).collect();
        assert_eq!(msgs, vec!["7".to_string(), "8".to_string(), "9".to_string()]);
    }

    #[test]
    fn truncate_msg_caps_oversized_messages() {
        let short = "x".repeat(10);
        assert_eq!(truncate_msg(&short), short);

        let long = "y".repeat(MAX_MSG_LEN + 50);
        let truncated = truncate_msg(&long);
        assert!(truncated.len() < long.len());
        assert!(truncated.starts_with(&"y".repeat(MAX_MSG_LEN)));
        assert!(truncated.contains("truncated"));
    }

    #[test]
    fn writer_loop_appends_batch_to_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("app.jsonl");

        let (tx, rx) = mpsc::sync_channel::<WriterMsg>(16);
        let path_clone = path.clone();
        let handle = thread::spawn(move || writer_loop(rx, path_clone));

        for i in 0..5 {
            tx.send(WriterMsg::Entry(make_entry("info", "test", &format!("m{i}"))))
                .unwrap();
        }

        // Ask for an explicit flush so we don't have to sleep on the timer.
        let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
        tx.send(WriterMsg::Flush(ack_tx)).unwrap();
        ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = contents.lines().collect();
        assert_eq!(lines.len(), 5);
        for (i, line) in lines.iter().enumerate() {
            let parsed: LogEntry = serde_json::from_str(line).unwrap();
            assert_eq!(parsed.msg, format!("m{i}"));
        }

        // Dropping the sender disconnects the channel, which makes the writer
        // loop return naturally.
        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn sibling_old_path_adds_old_infix_before_extension() {
        let base = PathBuf::from("/tmp/logs/app.jsonl");
        assert_eq!(sibling_old_path(&base), PathBuf::from("/tmp/logs/app.old.jsonl"));

        let base = PathBuf::from("/tmp/logs/foo.log");
        assert_eq!(sibling_old_path(&base), PathBuf::from("/tmp/logs/foo.old.log"));

        let base = PathBuf::from("app");
        assert_eq!(sibling_old_path(&base), PathBuf::from("app.old"));
    }

    #[test]
    fn rotate_overwrites_any_existing_old_file() {
        let tmp = TempDir::new().unwrap();
        let current = tmp.path().join("app.jsonl");
        let old = sibling_old_path(&current);

        fs::write(&current, b"new contents").unwrap();
        fs::write(&old, b"stale").unwrap();

        rotate(&current, &old).unwrap();

        assert!(!current.exists(), "current should be renamed away");
        assert!(old.exists(), "old should exist after rotate");
        assert_eq!(fs::read_to_string(&old).unwrap(), "new contents");
    }

    #[test]
    fn writer_loop_rotates_when_size_threshold_crossed() {
        // We can't easily shrink ROTATE_SIZE_BYTES for a test without adding
        // a test-only constant, so seed the file at > threshold and confirm
        // the next batch triggers a rename to `.old`.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("app.jsonl");
        let old = sibling_old_path(&path);

        let filler = "z".repeat(200);
        let seed_line = format!(
            r#"{{"ts":"2026-05-04T10:00:00.000Z","level":"info","source":"seed","msg":"{filler}"}}"#
        );
        {
            let mut f = File::create(&path).unwrap();
            let per_line = seed_line.len() as u64 + 1;
            let needed = ROTATE_SIZE_BYTES + 1024;
            let mut written: u64 = 0;
            while written < needed {
                writeln!(f, "{seed_line}").unwrap();
                written += per_line;
            }
        }
        // Pre-existing .old should get overwritten when we rotate.
        fs::write(&old, b"stale").unwrap();

        let (tx, rx) = mpsc::sync_channel::<WriterMsg>(16);
        let path_clone = path.clone();
        let handle = thread::spawn(move || writer_loop(rx, path_clone));

        // Any new entry should tip us over and trigger rotation on the writer.
        tx.send(WriterMsg::Entry(make_entry("info", "trigger", "rotate-me")))
            .unwrap();

        let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
        tx.send(WriterMsg::Flush(ack_tx)).unwrap();
        ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        assert!(old.exists(), "expected rotated file at {:?}", old);
        let old_contents = fs::read_to_string(&old).unwrap();
        assert!(
            old_contents.contains("seed"),
            "rotated file should carry the seeded entries"
        );
        assert!(
            !old_contents.contains("rotate-me"),
            "the triggering entry should land in the NEW file, not .old"
        );

        // Current file exists and is much smaller than the rotated one.
        assert!(path.exists());
        let cur = fs::read_to_string(&path).unwrap();
        assert!(cur.contains("rotate-me"));

        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn log_with_ts_preserves_caller_supplied_timestamp() {
        // The `log` crate adapter buffers pre-init entries and drains them
        // through log_with_ts so the on-disk timestamps match when each
        // entry was actually emitted. Regress against future refactors that
        // might "helpfully" overwrite the supplied ts with Utc::now().
        let entry_ts = "2025-01-15T08:30:45.123Z";

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("app.jsonl");

        let (tx, rx) = mpsc::sync_channel::<WriterMsg>(16);
        let path_clone = path.clone();
        let handle = thread::spawn(move || writer_loop(rx, path_clone));

        // Drive the writer through the same WriterMsg::Entry path log_with_ts
        // uses. We can't call log_with_ts directly because it routes through
        // the global APP_LOG/WRITER_TX, which other tests already populate.
        let entry = LogEntry {
            ts: entry_ts.to_string(),
            level: "info".to_string(),
            source: "test".to_string(),
            msg: "boot-time event".to_string(),
        };
        tx.send(WriterMsg::Entry(entry)).unwrap();

        let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
        tx.send(WriterMsg::Flush(ack_tx)).unwrap();
        ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        let parsed: LogEntry = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(parsed.ts, entry_ts, "writer must preserve entry ts verbatim");

        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn writer_loop_clears_both_current_and_old_files() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("app.jsonl");

        let (tx, rx) = mpsc::sync_channel::<WriterMsg>(16);
        let path_clone = path.clone();
        let handle = thread::spawn(move || writer_loop(rx, path_clone));

        tx.send(WriterMsg::Entry(make_entry("info", "t", "before")))
            .unwrap();
        let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
        tx.send(WriterMsg::Flush(ack_tx)).unwrap();
        ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        assert!(fs::metadata(&path).unwrap().len() > 0);

        tx.send(WriterMsg::Clear).unwrap();
        // Nudge with another flush round to let the Clear message be picked
        // up and processed.
        let (ack_tx2, ack_rx2) = mpsc::sync_channel::<()>(1);
        tx.send(WriterMsg::Flush(ack_tx2)).unwrap();
        ack_rx2.recv_timeout(Duration::from_secs(2)).unwrap();

        // After clear, the current file exists but is empty.
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX);
        assert_eq!(size, 0, "expected cleared file to be empty, got {size}");

        drop(tx);
        handle.join().unwrap();
    }
}
