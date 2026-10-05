use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Generous upper bound on one serialized entry (`msg` is capped at
/// `MAX_MSG_LEN` plus the truncation suffix). Sizes the tail window read at
/// startup so we load `max_size` entries without reading the whole file.
const TAIL_BYTES_PER_ENTRY: u64 = 1024;

/// A single structured log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: String,
    pub level: String,
    pub source: String,
    pub msg: String,
}

pub(super) struct AppLog {
    buffer: VecDeque<LogEntry>,
    max_size: usize,
}

impl AppLog {
    pub(super) fn new(max_size: usize, log_path: &Path) -> Result<Self> {
        // Load existing entries from disk so the UI viewer still shows recent
        // history on restart. Best-effort — corruption shouldn't fail init.
        let buffer = load_tail(max_size, log_path);
        Ok(Self { buffer, max_size })
    }

    pub(super) fn push(&mut self, entry: LogEntry) {
        self.buffer.push_back(entry);
        while self.buffer.len() > self.max_size {
            self.buffer.pop_front();
        }
    }

    pub(super) fn entries(&self) -> Vec<LogEntry> {
        self.buffer.iter().cloned().collect()
    }

    pub(super) fn clear_buffer(&mut self) {
        self.buffer.clear();
    }

    pub(super) fn set_max_size(&mut self, new_max: usize) {
        self.max_size = new_max;
        while self.buffer.len() > self.max_size {
            self.buffer.pop_front();
        }
    }
}

/// Load the newest `max_size` entries from the log file.
///
/// The file can reach the rotation size (~2 MB, ~16k lines) while we keep
/// only `max_size` entries, so read just a tail window and parse lines
/// newest-first, stopping once the buffer is full. Splitting at the byte
/// level matters: a seek can land inside a multi-byte UTF-8 character, which
/// would make a `lines()`-style reader error out on the first line.
fn load_tail(max_size: usize, log_path: &Path) -> VecDeque<LogEntry> {
    let mut buffer = VecDeque::with_capacity(max_size.min(8192));
    let Ok(mut file) = File::open(log_path) else {
        return buffer;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let window = (max_size as u64).saturating_mul(TAIL_BYTES_PER_ENTRY);
    let seeked = len > window;
    if seeked && file.seek(SeekFrom::Start(len - window)).is_err() {
        return buffer;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return buffer;
    }
    // After a seek the first segment is (almost always) a partial line.
    let start = if seeked {
        bytes
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| i + 1)
    } else {
        0
    };
    for line in bytes[start..].rsplit(|&b| b == b'\n') {
        if buffer.len() >= max_size {
            break;
        }
        if let Ok(entry) = serde_json::from_slice::<LogEntry>(line) {
            buffer.push_front(entry);
        }
    }
    buffer
}

/// Maximum message length (in bytes) before truncation.
pub(super) const MAX_MSG_LEN: usize = 500;

pub(super) fn truncate_msg(msg: &str) -> String {
    if msg.len() <= MAX_MSG_LEN {
        msg.to_string()
    } else {
        // Back off to a char boundary — slicing mid-character panics, and
        // this runs on every log call (any long non-ASCII line would hit it).
        let mut end = MAX_MSG_LEN;
        while !msg.is_char_boundary(end) {
            end -= 1;
        }
        let mut truncated = msg[..end].to_string();
        truncated.push_str(&format!("... [truncated, {} total bytes]", msg.len()));
        truncated
    }
}
