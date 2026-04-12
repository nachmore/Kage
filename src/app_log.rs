//! Application-level structured logging system.
//!
//! Provides a ring-buffer backed JSONL log that is accessible from both Rust
//! and the frontend (via Tauri commands). Each entry carries a timestamp,
//! level, source tag, and message.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

/// A single structured log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: String,
    pub level: String,
    pub source: String,
    pub msg: String,
}

/// Global app log instance.
static APP_LOG: std::sync::OnceLock<Mutex<AppLog>> = std::sync::OnceLock::new();

struct AppLog {
    buffer: VecDeque<LogEntry>,
    max_size: usize,
    log_path: PathBuf,
    dirty_count: usize,
}

impl AppLog {
    fn new(max_size: usize) -> Result<Self> {
        let log_path = get_log_dir()?.join("app.jsonl");
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Load existing entries from disk
        let mut buffer = VecDeque::new();
        if log_path.exists() {
            if let Ok(file) = File::open(&log_path) {
                let reader = BufReader::new(file);
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                        buffer.push_back(entry);
                    }
                }
                // Trim to max_size (keep newest)
                while buffer.len() > max_size {
                    buffer.pop_front();
                }
            }
        }

        Ok(Self {
            buffer,
            max_size,
            log_path,
            dirty_count: 0,
        })
    }

    fn push(&mut self, entry: LogEntry) {
        self.buffer.push_back(entry);
        while self.buffer.len() > self.max_size {
            self.buffer.pop_front();
        }
        self.dirty_count += 1;
        // Flush to disk every 10 writes or if buffer is small
        if self.dirty_count >= 10 {
            let _ = self.flush();
        }
    }

    fn flush(&mut self) -> Result<()> {
        if self.dirty_count == 0 {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.log_path)
            .context("Failed to open app log file")?;
        for entry in &self.buffer {
            if let Ok(line) = serde_json::to_string(entry) {
                let _ = writeln!(file, "{}", line);
            }
        }
        self.dirty_count = 0;
        Ok(())
    }

    fn entries(&self) -> Vec<LogEntry> {
        self.buffer.iter().cloned().collect()
    }

    fn clear(&mut self) -> Result<()> {
        self.buffer.clear();
        self.dirty_count = 0;
        if self.log_path.exists() {
            fs::remove_file(&self.log_path)?;
        }
        Ok(())
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

/// Initialize the global app log. Call once at startup.
pub fn init(max_size: usize) -> Result<()> {
    let log = AppLog::new(max_size)?;
    APP_LOG
        .set(Mutex::new(log))
        .map_err(|_| anyhow::anyhow!("App log already initialized"))?;
    Ok(())
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

/// Write a log entry from Rust code.
pub fn log(level: &str, source: &str, msg: &str) {
    if let Some(lock) = APP_LOG.get() {
        if let Ok(mut log) = lock.lock() {
            log.push(LogEntry {
                ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                level: level.to_string(),
                source: source.to_string(),
                msg: truncate_msg(msg),
            });
        }
    }
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

/// Flush pending entries to disk. Call on shutdown.
pub fn flush() {
    if let Some(lock) = APP_LOG.get() {
        if let Ok(mut log) = lock.lock() {
            let _ = log.flush();
        }
    }
}

/// Get all log entries.
pub fn get_entries() -> Vec<LogEntry> {
    APP_LOG
        .get()
        .and_then(|lock| lock.lock().ok())
        .map(|log| log.entries())
        .unwrap_or_default()
}

/// Clear all log entries.
pub fn clear() -> Result<()> {
    APP_LOG
        .get()
        .and_then(|lock| lock.lock().ok())
        .map(|mut log| log.clear())
        .unwrap_or(Ok(()))
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
