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
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
            AuditEvent::Granted {
                tool, grant_type, ..
            } => {
                format!("Granted '{}' ({})", tool, grant_type)
            }
            AuditEvent::Denied { tool, .. } => format!("Denied '{}'", tool),
            AuditEvent::Revoked {
                tool, prior_policy, ..
            } => {
                format!("Revoked '{}' (was {})", tool, prior_policy)
            }
            AuditEvent::Expired {
                tool,
                prior_grant_type,
            } => {
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
        Self {
            at: at.into(),
            event,
        }
    }
}

/// The on-disk path for the audit log. Returns `None` if the config
/// directory itself is unavailable (very rare).
pub fn default_log_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("kage")
            .join("permission-audit.jsonl"),
    )
}

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

fn ensure_parent(path: &Path) -> bool {
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

fn serialize(entry: &AuditEntry) -> Option<String> {
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

/// Read the last `limit` entries, most-recent-first. Tolerates
/// corrupt/partial JSON lines by logging and skipping. A missing log
/// file is NOT an error — the return is just an empty Vec.
///
/// Reads the file backwards in chunks so the caller pays for ~`limit`
/// entries regardless of how large the log has grown. JSONL is
/// well-suited to this: every line is self-contained, so a chunk-aligned
/// suffix can be parsed in isolation as long as we discard the partial
/// first line (which gets re-read as part of the next chunk further back).
pub fn read_recent(path: &std::path::Path, limit: usize) -> Vec<AuditEntry> {
    if limit == 0 || !path.exists() {
        return Vec::new();
    }
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("audit log: failed to open {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let total_len = match file.seek(SeekFrom::End(0)) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("audit log: seek failed for {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    if total_len == 0 {
        return Vec::new();
    }

    // 32 KB chunks — typical entry is well under 512 B (tool name +
    // timestamp + a small args preview), so one chunk usually covers
    // 60+ entries. We grow it in subsequent reads if a single chunk
    // didn't cover `limit` lines.
    const CHUNK: u64 = 32 * 1024;

    // Buffer holds the suffix of the file we've read so far. We always
    // hold the bytes for at least one *complete* line at the start,
    // which lets us hand the rest off to the line iterator below
    // confident that no entry is split across our parse boundary.
    let mut tail: Vec<u8> = Vec::new();
    let mut cursor = total_len;
    let mut hit_bof = false;

    let mut entries: Vec<AuditEntry> = Vec::new();

    while !hit_bof && entries.len() < limit {
        let read_size = CHUNK.min(cursor);
        cursor -= read_size;
        if cursor == 0 {
            hit_bof = true;
        }

        if let Err(e) = file.seek(SeekFrom::Start(cursor)) {
            log::warn!("audit log: seek failed for {}: {}", path.display(), e);
            return Vec::new();
        }
        let mut chunk = vec![0u8; read_size as usize];
        if let Err(e) = file.read_exact(&mut chunk) {
            log::warn!("audit log: read failed for {}: {}", path.display(), e);
            return Vec::new();
        }

        // Prepend the chunk we just read to whatever tail we already
        // had. Together they form a contiguous suffix of the file.
        chunk.extend_from_slice(&tail);
        tail = chunk;

        // If we haven't reached BOF yet, the first line in `tail` is
        // probably partial (the previous read split through the middle
        // of a line). Drop it — we'll pick it up on the next chunk.
        // When we *have* reached BOF, the first line is whole.
        let parse_start = if hit_bof {
            0
        } else {
            match memchr_newline(&tail) {
                Some(i) => i + 1, // skip past the newline
                None => {
                    // No newline yet — the entire chunk is a single
                    // partial line. Loop and read more.
                    continue;
                }
            }
        };

        // Parse all complete lines in tail[parse_start..], collect
        // entries newest-last (file order), then reverse only the
        // batch we just gathered to push them newest-first into
        // `entries`.
        let mut batch: Vec<AuditEntry> = Vec::new();
        for line in tail[parse_start..].split(|&b| b == b'\n') {
            // Trim a trailing \r so Windows-style line endings parse cleanly.
            let line = match line.split_last() {
                Some((&b'\r', rest)) => rest,
                _ => line,
            };
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<AuditEntry>(line) {
                Ok(entry) => batch.push(entry),
                Err(e) => {
                    log::warn!(
                        "audit log: skipping malformed entry in {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }
        // batch is in file order — newest-last. Push reversed so
        // `entries` is newest-first overall.
        for e in batch.into_iter().rev() {
            entries.push(e);
            if entries.len() >= limit {
                break;
            }
        }

        // We've consumed everything up to the last newline boundary in
        // `tail`. Reset for the next iteration: keep only the partial
        // prefix (bytes before parse_start) so the next read can stitch
        // onto it.
        tail.truncate(parse_start);
    }

    entries
}

/// Find the index of the first `\n` in `bytes`, if any.
/// Tiny helper — kept inline rather than pulling in the `memchr` crate.
fn memchr_newline(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|&b| b == b'\n')
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
    // Drop the cached writer first so the truncate isn't fighting an open
    // append-mode FD. The next `append` will reopen against the fresh file.
    reset_default_writer();
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
            append_to(
                &path,
                &AuditEntry::at_time(
                    format!("2026-04-28T12:00:0{}.000Z", i),
                    AuditEvent::Granted {
                        tool: format!("tool{}", i),
                        grant_type: "once".to_string(),
                        session_id: None,
                        args_preview: None,
                    },
                ),
            );
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
            append_to(
                &path,
                &AuditEntry::at_time(
                    format!("2026-04-28T12:00:{:02}.000Z", i),
                    AuditEvent::Denied {
                        tool: format!("t{}", i),
                        session_id: None,
                    },
                ),
            );
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
            AuditEvent::Expired {
                tool: "x".into(),
                prior_grant_type: "always".into(),
            },
        ))
        .unwrap();
        let mut content = String::new();
        content.push_str("not json at all\n");
        content.push_str("{malformed\n");
        content.push_str(&good);
        content.push('\n');
        content.push('\n'); // empty line
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
        append_to(
            &path,
            &AuditEntry::now(AuditEvent::Revoked {
                tool: "x".into(),
                prior_policy: "allow".into(),
                prior_grant_type: Some("24h".into()),
            }),
        );
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
        let events = [
            AuditEvent::Granted {
                tool: "a".into(),
                grant_type: "once".into(),
                session_id: None,
                args_preview: None,
            },
            AuditEvent::Denied {
                tool: "b".into(),
                session_id: Some("s".into()),
            },
            AuditEvent::Revoked {
                tool: "c".into(),
                prior_policy: "allow".into(),
                prior_grant_type: Some("always".into()),
            },
            AuditEvent::Expired {
                tool: "d".into(),
                prior_grant_type: "24h".into(),
            },
            AuditEvent::TerminatorModeChanged { enabled: true },
        ];
        for (i, e) in events.iter().enumerate() {
            append_to(
                &path,
                &AuditEntry::at_time(format!("2026-04-28T12:00:0{}.000Z", i), e.clone()),
            );
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
                tool: "shell_exec".into(),
                grant_type: "always".into(),
                session_id: None,
                args_preview: None
            }
            .summary(),
            "Granted 'shell_exec' (always)"
        );
        assert_eq!(
            AuditEvent::TerminatorModeChanged { enabled: true }.summary(),
            "Terminator mode enabled"
        );
    }

    #[test]
    fn read_recent_with_zero_limit_returns_empty() {
        let dir = tempdir();
        let path = logpath(&dir);
        for i in 0..3 {
            append_to(
                &path,
                &AuditEntry::at_time(
                    format!("2026-04-28T12:00:0{}.000Z", i),
                    AuditEvent::Denied {
                        tool: format!("t{}", i),
                        session_id: None,
                    },
                ),
            );
        }
        assert!(read_recent(&path, 0).is_empty());
    }

    /// Reads must be O(limit) regardless of file size — the chunk-walking
    /// reader has to handle entries that cross 32 KB boundaries cleanly.
    /// Build a log large enough to span several chunks, then verify the
    /// last few entries come back in the right order.
    #[test]
    fn read_recent_handles_chunk_boundaries() {
        let dir = tempdir();
        let path = logpath(&dir);
        // Append enough entries that the file grows past several 32 KB
        // chunks. Each entry serializes to ~150 bytes; 1500 entries is
        // ~225 KB, comfortably more than 7 chunks.
        const N: usize = 1500;
        for i in 0..N {
            append_to(
                &path,
                &AuditEntry::at_time(
                    // Pad the tool name so each line is a different length
                    // and chunk boundaries land in different intra-line
                    // offsets — exercises the partial-line discard path.
                    format!(
                        "2026-04-28T12:{:02}:{:02}.{:03}Z",
                        i / 3600,
                        (i / 60) % 60,
                        i % 1000
                    ),
                    AuditEvent::Granted {
                        tool: format!("tool_{:04}_{}", i, "x".repeat(i % 50)),
                        grant_type: "once".into(),
                        session_id: None,
                        args_preview: None,
                    },
                ),
            );
        }

        // Asking for the last 10 should return entries N-1 .. N-10 newest-first.
        let got = read_recent(&path, 10);
        assert_eq!(got.len(), 10);
        for (i, entry) in got.iter().enumerate() {
            let expected_idx = N - 1 - i;
            if let AuditEvent::Granted { tool, .. } = &entry.event {
                let expected_prefix = format!("tool_{:04}_", expected_idx);
                assert!(
                    tool.starts_with(&expected_prefix),
                    "got tool {:?} at position {}, expected prefix {:?}",
                    tool,
                    i,
                    expected_prefix
                );
            } else {
                panic!("unexpected event variant");
            }
        }

        // Asking for more than the file holds returns everything in order.
        let got_all = read_recent(&path, N + 100);
        assert_eq!(got_all.len(), N);
        if let AuditEvent::Granted { tool, .. } = &got_all[0].event {
            assert!(tool.starts_with(&format!("tool_{:04}_", N - 1)));
        }
        if let AuditEvent::Granted { tool, .. } = &got_all[N - 1].event {
            assert!(tool.starts_with("tool_0000_"));
        }
    }

    /// Files written with CRLF line endings (e.g. on Windows via a text
    /// editor) must still parse — the byte-slice reader trims the \r.
    #[test]
    fn read_recent_tolerates_crlf_line_endings() {
        let dir = tempdir();
        let path = logpath(&dir);
        let entry = AuditEntry::at_time(
            "2026-04-28T12:00:00.000Z",
            AuditEvent::Denied {
                tool: "x".into(),
                session_id: None,
            },
        );
        let mut content = serde_json::to_string(&entry).unwrap();
        content.push_str("\r\n");
        std::fs::write(&path, content).unwrap();

        let got = read_recent(&path, 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].at, "2026-04-28T12:00:00.000Z");
    }

    /// File without a trailing newline (atomic write race, OS crash mid-write)
    /// must still parse the last line.
    #[test]
    fn read_recent_handles_missing_trailing_newline() {
        let dir = tempdir();
        let path = logpath(&dir);
        let e1 = serde_json::to_string(&AuditEntry::at_time(
            "2026-04-28T12:00:00.000Z",
            AuditEvent::Denied {
                tool: "first".into(),
                session_id: None,
            },
        ))
        .unwrap();
        let e2 = serde_json::to_string(&AuditEntry::at_time(
            "2026-04-28T12:00:01.000Z",
            AuditEvent::Denied {
                tool: "second".into(),
                session_id: None,
            },
        ))
        .unwrap();
        // Note: no trailing newline after e2.
        std::fs::write(&path, format!("{}\n{}", e1, e2)).unwrap();

        let got = read_recent(&path, 10);
        assert_eq!(got.len(), 2);
        if let AuditEvent::Denied { tool, .. } = &got[0].event {
            assert_eq!(tool, "second");
        } else {
            panic!("wrong event");
        }
    }

    // ---- Helper-level tests added with the BufWriter refactor (B.2) -------

    #[test]
    fn ensure_parent_creates_missing_dir_and_returns_true() {
        let dir = tempdir();
        let nested = dir.join("a").join("b").join("c");
        let path = nested.join("audit.jsonl");
        assert!(!nested.exists());
        assert!(ensure_parent(&path));
        assert!(nested.is_dir());
    }

    #[test]
    fn ensure_parent_succeeds_when_dir_already_exists() {
        let dir = tempdir();
        // Path under an existing dir; ensure_parent should be a no-op.
        let path = dir.join("audit.jsonl");
        assert!(ensure_parent(&path));
        assert!(dir.is_dir());
    }

    #[test]
    fn ensure_parent_handles_root_path_without_panicking() {
        // A path with no parent (e.g. `audit.jsonl` with no directory
        // component) must still return true — `Path::parent` is None,
        // which means "nothing to create."
        let path = PathBuf::from("audit.jsonl");
        assert!(ensure_parent(&path));
    }

    #[test]
    fn serialize_returns_some_for_valid_entry() {
        let entry = AuditEntry::at_time(
            "2026-04-28T12:00:00.000Z",
            AuditEvent::Denied {
                tool: "x".into(),
                session_id: None,
            },
        );
        let s = serialize(&entry).expect("serialize must succeed");
        // Round-trip back through serde to confirm the output is well-formed.
        let parsed: AuditEntry = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn append_to_then_append_to_appends_new_line_each_time() {
        // The path-explicit API used in tests doesn't go through the cached
        // writer — verify it still appends cleanly multiple times so the
        // refactor didn't accidentally break read-back behaviour.
        let dir = tempdir();
        let path = logpath(&dir);

        for i in 0..5 {
            append_to(
                &path,
                &AuditEntry::at_time(
                    format!("2026-04-28T12:00:0{}.000Z", i),
                    AuditEvent::Denied {
                        tool: format!("t{}", i),
                        session_id: None,
                    },
                ),
            );
        }
        let got = read_recent(&path, 100);
        assert_eq!(got.len(), 5);
    }

    #[test]
    fn args_preview_optional_is_omitted_from_json_when_none() {
        // Keeps the JSONL file tight when the caller doesn't have args.
        let entry = AuditEntry::at_time(
            "2026-04-28T12:00:00.000Z",
            AuditEvent::Granted {
                tool: "x".into(),
                grant_type: "once".into(),
                session_id: None,
                args_preview: None,
            },
        );
        let s = serde_json::to_string(&entry).unwrap();
        assert!(
            !s.contains("session_id"),
            "session_id should be omitted, got: {}",
            s
        );
        assert!(
            !s.contains("args_preview"),
            "args_preview should be omitted, got: {}",
            s
        );
    }
}
