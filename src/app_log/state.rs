use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

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

/// Maximum message length before truncation.
pub(super) const MAX_MSG_LEN: usize = 500;

pub(super) fn truncate_msg(msg: &str) -> String {
    if msg.len() <= MAX_MSG_LEN {
        msg.to_string()
    } else {
        let mut truncated = msg[..MAX_MSG_LEN].to_string();
        truncated.push_str(&format!("... [truncated, {} total chars]", msg.len()));
        truncated
    }
}
