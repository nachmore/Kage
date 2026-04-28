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

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// Kinds of events we track. Each variant carries the data needed to
/// reconstruct what happened without cross-referencing the config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AuditEvent {
    /// User approved a tool request. `grant_type` is the scope they
    /// picked at the prompt ("once", "24h", "always").
    Granted {
        tool: String,
        grant_type: String,
        /// Optional: the session id the request belonged to, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// Optional: arguments the agent was asking to use, if the
        /// caller surfaced them. We store up to 2 KB per entry to
        /// keep the log navigable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args_preview: Option<String>,
    },
    /// User denied a single request. Not the same as revoke — the
    /// existing policy stays in place, this was a one-time "no".
    Denied {
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// User revoked a standing grant (removed it from settings, or
    /// changed its policy to "ask" / "deny").
    Revoked {
        tool: String,
        /// What the policy was before the revoke happened.
        prior_policy: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior_grant_type: Option<String>,
    },
    /// A grant expired on its own (24h or 30-day staleness).
    Expired {
        tool: String,
        prior_grant_type: String,
    },
    /// User turned terminator mode on or off. We track this because
    /// during terminator mode every request is auto-approved and
    /// doesn't get its own `Granted` entry.
    TerminatorModeChanged { enabled: bool },
}

impl AuditEvent {
    /// Human-readable summary used by UI and logs.
    #[allow(dead_code)] // Kept for log formatting and future Rust-side UIs.
    pub fn summary(&self) -> String {
        match self {
            AuditEvent::Granted { tool, grant_type, .. } => {
                format!("Granted '{}' ({})", tool, grant_type)
            }
            AuditEvent::Denied { tool, .. } => format!("Denied '{}'", tool),
            AuditEvent::Revoked { tool, prior_policy, .. } => {
                format!("Revoked '{}' (was {})", tool, prior_policy)
            }
            AuditEvent::Expired { tool, prior_grant_type } => {
                format!("Expired '{}' ({})", tool, prior_grant_type)
            }
            AuditEvent::TerminatorModeChanged { enabled } => {
                if *enabled {
                    "Terminator mode enabled".to_string()
                } else {
                    "Terminator mode disabled".to_string()
                }
            }
        }
    }
}

/// One row in the audit log. Flat so serde_json produces a single
/// ordered-key JSON object that's easy to eyeball in the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    /// ISO 8601 UTC timestamp, e.g. "2026-04-28T14:23:00.123Z".
    pub at: String,
    #[serde(flatten)]
    pub event: AuditEvent,
}

impl AuditEntry {
    /// Construct an entry timestamped now (UTC). Tests use
    /// `AuditEntry::at_time` to control the timestamp.
    pub fn now(event: AuditEvent) -> Self {
        Self {
            at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            event,
        }
    }

    #[allow(dead_code)] // used by tests
    pub fn at_time(at: impl Into<String>, event: AuditEvent) -> Self {
        Self { at: at.into(), event }
    }
}

/// The on-disk path for the audit log. Returns `None` if the config
/// directory itself is unavailable (very rare).
pub fn default_log_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("kage").join("permission-audit.jsonl"))
}

/// Append one entry to the given log file. Best-effort: a failure is
/// logged via `log::warn!` and swallowed so a broken disk never blocks
/// a grant operation. Creates the parent directory if missing.
pub fn append_to(path: &std::path::Path, entry: &AuditEntry) {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                log::warn!("audit log: cannot create {}: {}", parent.display(), e);
                return;
            }
        }
    }

    let serialized = match serde_json::to_string(entry) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("audit log: failed to serialize entry: {}", e);
            return;
        }
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

/// Convenience: append to the default path. Use this from command
/// handlers; tests should call `append_to` with a tempdir path.
pub fn append(entry: &AuditEntry) {
    let Some(path) = default_log_path() else {
        log::warn!("audit log: config_dir unavailable, skipping append");
        return;
    };
    append_to(&path, entry);
}

/// Read the last `limit` entries, most-recent-first. Tolerates
/// corrupt/partial JSON lines by logging and skipping. A missing log
/// file is NOT an error — the return is just an empty Vec.
pub fn read_recent(path: &std::path::Path, limit: usize) -> Vec<AuditEntry> {
    if !path.exists() {
        return Vec::new();
    }
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("audit log: failed to open {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let reader = BufReader::new(file);
    let mut entries: Vec<AuditEntry> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log::warn!("audit log: read error at line {}: {}", lineno + 1, e);
                continue;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<AuditEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                log::warn!(
                    "audit log: skipping malformed line {} in {}: {}",
                    lineno + 1,
                    path.display(),
                    e
                );
            }
        }
    }
    // Return most-recent-first, capped at `limit`.
    entries.reverse();
    if entries.len() > limit {
        entries.truncate(limit);
    }
    entries
}

/// Convenience: read from the default path.
pub fn read_recent_default(limit: usize) -> Vec<AuditEntry> {
    let Some(path) = default_log_path() else {
        return Vec::new();
    };
    read_recent(&path, limit)
}

/// Truncate the log. Used by the "clear audit log" UI action. Does
/// nothing if the file doesn't exist.
pub fn clear(path: &std::path::Path) -> std::io::Result<()> {
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
    clear(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("kage-audit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn logpath(base: &std::path::Path) -> PathBuf {
        base.join("permission-audit.jsonl")
    }

    #[test]
    fn append_then_read_roundtrip() {
        let dir = tempdir();
        let path = logpath(&dir);
        let entry = AuditEntry::at_time(
            "2026-04-28T12:00:00.000Z",
            AuditEvent::Granted {
                tool: "shell_exec".to_string(),
                grant_type: "24h".to_string(),
                session_id: Some("s-1".to_string()),
                args_preview: Some("git status".to_string()),
            },
        );
        append_to(&path, &entry);
        let got = read_recent(&path, 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], entry);
    }

    #[test]
    fn read_returns_most_recent_first() {
        let dir = tempdir();
        let path = logpath(&dir);
        for i in 0..5 {
            append_to(&path, &AuditEntry::at_time(
                format!("2026-04-28T12:00:0{}.000Z", i),
                AuditEvent::Granted {
                    tool: format!("tool{}", i),
                    grant_type: "once".to_string(),
                    session_id: None,
                    args_preview: None,
                },
            ));
        }
        let got = read_recent(&path, 10);
        assert_eq!(got.len(), 5);
        // Last appended should be first in the returned slice.
        if let AuditEvent::Granted { tool, .. } = &got[0].event {
            assert_eq!(tool, "tool4");
        } else {
            panic!("unexpected event type");
        }
    }

    #[test]
    fn read_respects_limit() {
        let dir = tempdir();
        let path = logpath(&dir);
        for i in 0..10 {
            append_to(&path, &AuditEntry::at_time(
                format!("2026-04-28T12:00:{:02}.000Z", i),
                AuditEvent::Denied { tool: format!("t{}", i), session_id: None },
            ));
        }
        let got = read_recent(&path, 3);
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let dir = tempdir();
        let path = logpath(&dir); // never created
        assert!(read_recent(&path, 100).is_empty());
    }

    #[test]
    fn read_tolerates_malformed_lines() {
        let dir = tempdir();
        let path = logpath(&dir);
        // Mix of valid and malformed lines.
        let good = serde_json::to_string(&AuditEntry::at_time(
            "2026-04-28T12:00:01.000Z",
            AuditEvent::Expired { tool: "x".into(), prior_grant_type: "always".into() },
        )).unwrap();
        let mut content = String::new();
        content.push_str("not json at all\n");
        content.push_str("{malformed\n");
        content.push_str(&good);
        content.push('\n');
        content.push_str("\n"); // empty line
        content.push_str("{\"at\":\"2026\",\"event\":\"unknown_event\"}\n");
        std::fs::write(&path, content).unwrap();

        let got = read_recent(&path, 10);
        assert_eq!(got.len(), 1, "only the one well-formed line should parse");
        if let AuditEvent::Expired { tool, .. } = &got[0].event {
            assert_eq!(tool, "x");
        } else {
            panic!("wrong event");
        }
    }

    #[test]
    fn clear_empties_existing_log() {
        let dir = tempdir();
        let path = logpath(&dir);
        append_to(&path, &AuditEntry::now(AuditEvent::Revoked {
            tool: "x".into(),
            prior_policy: "allow".into(),
            prior_grant_type: Some("24h".into()),
        }));
        assert_eq!(read_recent(&path, 10).len(), 1);
        clear(&path).unwrap();
        assert!(read_recent(&path, 10).is_empty());
    }

    #[test]
    fn clear_nonexistent_file_is_noop() {
        let dir = tempdir();
        let path = logpath(&dir);
        assert!(clear(&path).is_ok());
    }

    #[test]
    fn all_event_kinds_roundtrip() {
        let dir = tempdir();
        let path = logpath(&dir);
        let events = vec![
            AuditEvent::Granted {
                tool: "a".into(), grant_type: "once".into(),
                session_id: None, args_preview: None,
            },
            AuditEvent::Denied { tool: "b".into(), session_id: Some("s".into()) },
            AuditEvent::Revoked {
                tool: "c".into(), prior_policy: "allow".into(),
                prior_grant_type: Some("always".into()),
            },
            AuditEvent::Expired { tool: "d".into(), prior_grant_type: "24h".into() },
            AuditEvent::TerminatorModeChanged { enabled: true },
        ];
        for (i, e) in events.iter().enumerate() {
            append_to(&path, &AuditEntry::at_time(
                format!("2026-04-28T12:00:0{}.000Z", i),
                e.clone(),
            ));
        }
        let got = read_recent(&path, 100);
        assert_eq!(got.len(), 5);
        // Reverse-order check: got[0].event == events[4], etc.
        for (i, got_entry) in got.iter().enumerate() {
            assert_eq!(got_entry.event, events[events.len() - 1 - i]);
        }
    }

    #[test]
    fn summary_is_human_readable() {
        assert_eq!(
            AuditEvent::Granted {
                tool: "shell_exec".into(), grant_type: "always".into(),
                session_id: None, args_preview: None
            }.summary(),
            "Granted 'shell_exec' (always)"
        );
        assert_eq!(
            AuditEvent::TerminatorModeChanged { enabled: true }.summary(),
            "Terminator mode enabled"
        );
    }

    #[test]
    fn args_preview_optional_is_omitted_from_json_when_none() {
        // Keeps the JSONL file tight when the caller doesn't have args.
        let entry = AuditEntry::at_time(
            "2026-04-28T12:00:00.000Z",
            AuditEvent::Granted {
                tool: "x".into(), grant_type: "once".into(),
                session_id: None, args_preview: None,
            },
        );
        let s = serde_json::to_string(&entry).unwrap();
        assert!(!s.contains("session_id"), "session_id should be omitted, got: {}", s);
        assert!(!s.contains("args_preview"), "args_preview should be omitted, got: {}", s);
    }
}
