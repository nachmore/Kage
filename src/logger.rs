//! `log` crate adapter that routes every `info!`/`warn!`/`error!`/`debug!`
//! call into [`app_log`].
//!
//! Pre-2026-05 this module had its own `FileLogger` that opened a file under
//! a `Mutex<File>`, called `metadata()` and `write_all()` + `flush()` on every
//! single log line, and re-checked rotation each call. That meant a hot
//! logging burst could stall any caller on disk I/O while holding the mutex.
//! `app_log` already has the right pattern — bounded channel + dedicated
//! `app-log-writer` thread + size-based rotation — so the cleaner fix is to
//! make this module a thin adapter that funnels into `app_log::log` instead
//! of duplicating the rotation / format / file-handle plumbing.
//!
//! Init ordering note: `app_log::init` needs `Config` (for `log_buffer_size`),
//! so it can only run after config load. `init_logger` is called earlier in
//! `main.rs` for crash visibility — so the adapter buffers any pre-init
//! entries in memory and drains them through `app_log::log` on first post-
//! init call. That way the "Kage Starting" banner survives.

use anyhow::{Context, Result};
use chrono::Local;
use log::{Level, LevelFilter, Metadata, Record};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static CONSOLE_LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Pre-init buffer for log records that arrive before `app_log::init` has
/// run. The shim queues them here and drains on the first post-init call,
/// so early startup messages (single-instance check, panic handler install,
/// etc.) still make it onto disk once the writer thread is up.
static PREINIT_BUFFER: Mutex<Option<Vec<BufferedRecord>>> = Mutex::new(Some(Vec::new()));

struct BufferedRecord {
    /// Timestamp captured at log-emit time (RFC3339 with millis), so a
    /// replay-on-drain doesn't squash the entire pre-init burst into the
    /// single moment `app_log::init` happens to finish.
    ts: String,
    level: &'static str,
    target: String,
    message: String,
}

fn level_to_str(level: Level) -> &'static str {
    match level {
        Level::Error => "error",
        Level::Warn => "warn",
        Level::Info => "info",
        Level::Debug => "debug",
        Level::Trace => "trace",
    }
}

/// Drain any entries we buffered before `app_log` was initialized. Called
/// by the shim's `log` and `flush` methods; no-ops once the buffer's been
/// drained (the slot is set to `None`).
fn drain_preinit_buffer_if_ready() {
    // Only drain once `app_log` is up — otherwise we'd push into the buffer
    // we're trying to drain.
    if !crate::app_log::is_initialized() {
        return;
    }
    let mut guard = match PREINIT_BUFFER.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(buffered) = guard.take() {
        for rec in buffered {
            crate::app_log::log_with_ts(&rec.ts, rec.level, &rec.target, &rec.message);
        }
    }
}

struct LogShim;

static LOG_SHIM: LogShim = LogShim;

impl log::Log for LogShim {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level = level_to_str(record.level());
        let target = record.target();
        let message = record.args().to_string();

        // Try to drain any pre-init buffer first so this entry isn't observed
        // out-of-order relative to the buffered ones.
        drain_preinit_buffer_if_ready();

        if crate::app_log::is_initialized() {
            crate::app_log::log(level, target, &message);
        } else {
            // Pre-init: buffer for later drain. If `app_log` never initialises
            // (init failed) the buffer just grows bounded by startup duration —
            // not a correctness concern at that point. Capture the timestamp
            // now so the drained entries reflect when each was actually
            // emitted, not the moment app_log finally came up.
            let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            if let Ok(mut guard) = PREINIT_BUFFER.lock() {
                if let Some(ref mut buf) = *guard {
                    buf.push(BufferedRecord {
                        ts,
                        level,
                        target: target.to_string(),
                        message: message.clone(),
                    });
                }
            }
        }

        // Console mirroring — independent of `app_log`. Errors/warnings always
        // get an stderr line so they're visible when the app is launched from
        // a terminal even without the `/debug` flag.
        if record.level() <= Level::Warn {
            eprintln!("[{}] {}", record.level(), record.args());
        }
        if CONSOLE_LOGGING_ENABLED.load(Ordering::Relaxed) {
            let timestamp = Local::now().format("%H:%M:%S%.3f");
            println!("[{}] {} {}", timestamp, record.level(), record.args());
        }
    }

    fn flush(&self) {
        drain_preinit_buffer_if_ready();
        crate::app_log::flush();
    }
}

/// Install the global `log` crate adapter. After this returns, every
/// `log::info!`/`warn!`/`error!`/`debug!` call routes through [`app_log`].
pub fn init_logger() -> Result<()> {
    log::set_logger(&LOG_SHIM).context("Failed to set logger")?;
    log::set_max_level(LevelFilter::Info);
    Ok(())
}

/// Toggle full stdout mirroring (used by the `/debug` flag). Errors and
/// warnings already go to stderr unconditionally.
pub fn enable_console_logging() {
    CONSOLE_LOGGING_ENABLED.store(true, Ordering::Relaxed);
}
