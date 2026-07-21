use super::LogEntry;
use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// Rotate the file when it exceeds this size. We only keep the current file
/// plus one `.old` sibling, so peak disk usage is 2× this value.
pub(super) const ROTATE_SIZE_BYTES: u64 = 2 * 1024 * 1024;

/// Writer batch flush cadence.
const FLUSH_INTERVAL_MS: u64 = 500;

/// Messages sent to the writer thread.
pub(super) enum WriterMsg {
    Entry(LogEntry),
    Flush(mpsc::SyncSender<()>),
    Clear,
}

/// Background writer loop. Owns the file handle; no other code touches it.
pub(super) fn writer_loop(rx: mpsc::Receiver<WriterMsg>, log_path: PathBuf) {
    let mut file: Option<BufWriter<File>> = open_append(&log_path);
    let mut current_size = file_size(&log_path);

    loop {
        let first = match rx.recv_timeout(Duration::from_millis(FLUSH_INTERVAL_MS)) {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(ref mut file) = file {
                    let _ = file.flush();
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(mut file) = file.take() {
                    let _ = file.flush();
                }
                return;
            }
        };

        let mut batch = Vec::new();
        let mut flush_acks = Vec::new();
        let mut clear = false;
        handle_msg(first, &mut batch, &mut flush_acks, &mut clear);
        while let Ok(msg) = rx.try_recv() {
            handle_msg(msg, &mut batch, &mut flush_acks, &mut clear);
        }

        if clear {
            drop(file.take());
            if log_path.exists() {
                let _ = fs::remove_file(&log_path);
            }
            let _ = fs::remove_file(sibling_old_path(&log_path));
            file = open_append(&log_path);
            current_size = 0;
        }

        if !batch.is_empty() {
            if current_size >= ROTATE_SIZE_BYTES {
                if let Some(mut file) = file.take() {
                    let _ = file.flush();
                }
                let old_path = sibling_old_path(&log_path);
                if let Err(error) = rotate(&log_path, &old_path) {
                    log::warn!("app_log: rotation failed: {error}");
                }
                file = open_append(&log_path);
                current_size = file_size(&log_path);
            }

            if let Some(ref mut file) = file {
                current_size = current_size.saturating_add(write_batch(file, &batch));
            }
        }

        if !flush_acks.is_empty() {
            if let Some(ref mut file) = file {
                let _ = file.flush();
            }
            for ack in flush_acks {
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
        WriterMsg::Entry(entry) => batch.push(entry),
        WriterMsg::Flush(ack) => flush_acks.push(ack),
        WriterMsg::Clear => *clear = true,
    }
}

fn write_batch(file: &mut BufWriter<File>, batch: &[LogEntry]) -> u64 {
    let mut bytes: u64 = 0;
    for entry in batch {
        let Ok(line) = serde_json::to_string(entry) else {
            continue;
        };
        if writeln!(file, "{line}").is_ok() {
            bytes = bytes.saturating_add(line.len() as u64 + 1);
        }
    }
    bytes
}

fn open_append(path: &Path) -> Option<BufWriter<File>> {
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => Some(BufWriter::with_capacity(16 * 1024, file)),
        Err(error) => {
            log::warn!("app_log: failed to open {path:?} for append: {error}");
            None
        }
    }
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

pub(super) fn rotate(log_path: &Path, old_path: &Path) -> Result<()> {
    if old_path.exists() {
        fs::remove_file(old_path).context("remove existing .old log")?;
    }
    if log_path.exists() {
        fs::rename(log_path, old_path).context("rotate current log to .old")?;
    }
    Ok(())
}

pub(super) fn sibling_old_path(log_path: &Path) -> PathBuf {
    let ext = log_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let stem = log_path
        .file_stem()
        .and_then(|value| value.to_str())
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
