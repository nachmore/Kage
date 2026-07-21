//! Append-only audit log for tool permission events.
//!
//! Stored as JSONL (one JSON object per line) at
//! `<config_dir>/kage/permission-audit.jsonl`. Writes are best-effort
//! — a failing append never blocks the grant/deny flow. Reads tolerate
//! corrupt lines (logged and skipped) so one bad entry doesn't brick
//! the viewer.
//!
//! Intentionally NOT tamper-evident. The file lives under the user's
//! config directory, has user-writable permissions, and could be
//! trivially edited. This is a "what did I do recently" tool, not a
//! forensic audit trail — that's documented honestly in SECURITY_MODEL.md.

mod reader;
mod types;
mod writer;

pub use reader::read_recent_default;
pub use types::{default_log_path, AuditEntry, AuditEvent};
pub use writer::{append, clear_default};

#[cfg(test)]
mod tests;
