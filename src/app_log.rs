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
//! # Pre-init buffering
//!
//! Frontend webviews start running JS several seconds before our `setup()`
//! block reaches `init()` — the Tauri builder constructs and loads them
//! while `setup()` is still loading config and connecting the ACP client.
//! That means `invoke('app_log_write', ...)` calls from the top of a
//! window's `main.js` arrive in `log()` before `WRITER_TX` is initialized.
//! `PREINIT_BUFFER` captures those entries so they're not lost; `init()`
//! drains the buffer into the writer once it's up.
//!
//! # Rotation
//!
//! When the file exceeds `ROTATE_SIZE_BYTES`, the writer renames it to
//! `<name>.old.jsonl`, overwriting any previous `.old` file, and starts a
//! fresh one. We only keep one rotated file — this is a developer log, not
//! an audit log.

mod state;
mod writer;

use anyhow::{Context, Result};
use chrono::Utc;
use state::{truncate_msg, AppLog};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use writer::{writer_loop, WriterMsg};

pub use state::LogEntry;

#[cfg(test)]
use state::MAX_MSG_LEN;
#[cfg(test)]
use std::fs::File;
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use writer::{rotate, sibling_old_path, ROTATE_SIZE_BYTES};

/// Bound on the writer's mailbox. Enough to absorb bursts of several
/// hundred log entries without dropping. If exceeded, the disk copy of the
/// overflow entry is dropped (in-memory ring still has it) and
/// `DROPPED_COUNT` is incremented.
const CHANNEL_CAPACITY: usize = 4096;

/// Global app log instance.
static APP_LOG: std::sync::OnceLock<Mutex<AppLog>> = std::sync::OnceLock::new();

/// Sender handle for the writer thread. Initialized alongside `APP_LOG`.
static WRITER_TX: std::sync::OnceLock<SyncSender<WriterMsg>> = std::sync::OnceLock::new();

/// Count of entries whose DISK copy was dropped because the writer's
/// mailbox was full (the in-memory ring always keeps them). Read by
/// `dropped_count()` so backpressure drops are observable instead of
/// silent.
static DROPPED_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Number of log entries dropped from disk persistence due to writer
/// backpressure since startup.
pub fn dropped_count() -> u64 {
    DROPPED_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Pre-init buffer for entries that arrive before `init()` has run.
/// Frontend webviews start loading their JS as soon as the Tauri builder
/// constructs them — that's well before `setup()` runs and calls our
/// `init()`. So a beacon like `invoke('app_log_write', ...)` at the top
/// of `main.js` reaches the backend command handler while `WRITER_TX`
/// is still `None`, and without this buffer the entry would silently
/// vanish. Drained the first time `log()` runs after `init()`.
///
/// `Some(Vec)` means "pre-init, still buffering"; `None` means "drained,
/// don't buffer anymore — `init()` is up".
static PREINIT_BUFFER: Mutex<Option<Vec<LogEntry>>> = Mutex::new(Some(Vec::new()));

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

    drain_preinit_buffer();

    Ok(())
}

/// Drain the pre-init buffer into the now-running writer. Marks the buffer
/// as drained (`None`) so subsequent calls to `log_with_ts` skip the
/// pre-init path and go straight to the ring + writer. Idempotent — safe
/// to call more than once, but `init()` already calls it once.
fn drain_preinit_buffer() {
    let buffered = match PREINIT_BUFFER.lock() {
        Ok(mut guard) => guard.take(),
        Err(p) => p.into_inner().take(),
    };
    let Some(entries) = buffered else { return };
    for entry in entries {
        // Replay through the same path as a fresh log call, minus the
        // pre-init detour we just disabled by taking the buffer above.
        if let Some(lock) = APP_LOG.get() {
            if let Ok(mut log) = lock.lock() {
                log.push(entry.clone());
            }
        }
        if let Some(tx) = WRITER_TX.get() {
            if tx.try_send(WriterMsg::Entry(entry)).is_err() {
                DROPPED_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
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

    // Pre-init path: webviews can call `app_log_write` before `init()` has
    // run (the Tauri builder loads HTML/JS several seconds before our
    // `setup()` block reaches `app_log::init`). Buffer here so the beacon
    // at the top of main.js isn't silently discarded.
    if WRITER_TX.get().is_none() {
        if let Ok(mut guard) = PREINIT_BUFFER.lock() {
            if let Some(buf) = guard.as_mut() {
                buf.push(entry);
                return;
            }
        }
    }

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
                // Full: writer is backed up — drop the disk copy.
                // Disconnected: writer is gone; shouldn't happen while the
                // app is running. Either way, count it so backpressure is
                // observable via dropped_count().
                DROPPED_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// Flush pending entries to disk. Blocks briefly (up to ~1s) waiting for the
/// writer to acknowledge. Safe to call from any thread, including during
/// shutdown.
pub fn flush() {
    let Some(tx) = WRITER_TX.get() else { return };

    // Surface accumulated backpressure drops before the final flush so
    // the count lands in the on-disk log at least once per run.
    let dropped = dropped_count();
    if dropped > 0 {
        log(
            "warn",
            "app_log",
            &format!(
                "{} entries dropped from disk log (writer backpressure)",
                dropped
            ),
        );
    }

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
        assert_eq!(
            msgs,
            vec!["7".to_string(), "8".to_string(), "9".to_string()]
        );
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
            tx.send(WriterMsg::Entry(make_entry(
                "info",
                "test",
                &format!("m{i}"),
            )))
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
        assert_eq!(
            sibling_old_path(&base),
            PathBuf::from("/tmp/logs/app.old.jsonl")
        );

        let base = PathBuf::from("/tmp/logs/foo.log");
        assert_eq!(
            sibling_old_path(&base),
            PathBuf::from("/tmp/logs/foo.old.log")
        );

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
        assert_eq!(
            parsed.ts, entry_ts,
            "writer must preserve entry ts verbatim"
        );

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

    #[test]
    fn preinit_buffer_drain_replays_in_order_and_disables_buffering() {
        // Frontend webviews can call `app_log_write` while the Tauri builder
        // is still constructing the rest of the app — that lands in
        // `app_log::log` before `init()` has set up `WRITER_TX`. Without the
        // pre-init buffer, those entries would silently vanish. Verify here
        // that `drain_preinit_buffer` replays them in order and that the
        // buffer is marked drained so subsequent log calls don't recurse
        // back into the pre-init path.
        //
        // We can't call the public `log_with_ts` because it touches the
        // process-global `APP_LOG`/`WRITER_TX` that other tests already
        // populate. Instead drive `PREINIT_BUFFER` directly. Take/restore
        // the buffer state so we don't poison sibling tests.
        let saved = PREINIT_BUFFER.lock().unwrap().take();

        // Seed the buffer as if two early frontend invokes had landed.
        *PREINIT_BUFFER.lock().unwrap() = Some(vec![
            make_entry("info", "chat", "early-1"),
            make_entry("warn", "floating", "early-2"),
        ]);

        drain_preinit_buffer();

        // Buffer should be `None` now — the "drained" marker. A subsequent
        // log_with_ts must NOT re-buffer; it should go straight to the
        // ring/writer path.
        assert!(
            PREINIT_BUFFER.lock().unwrap().is_none(),
            "drain should leave buffer in the drained (None) state"
        );

        // Restore so other tests aren't affected.
        *PREINIT_BUFFER.lock().unwrap() = saved;
    }
}
