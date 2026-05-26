//! Detect a crash from the previous session and surface it to the user.
//!
//! The panic hook in `panic_handler.rs` writes a crash report to
//! `<data_local>/kage/logs/crash.log`. The file is *append* mode — a
//! crashy day can stack many reports. On launch we want to:
//!
//!   1. Find the most recent report (the last `=== Kage crash report
//!      @ <ts> ===` block in the file).
//!   2. Pull the user-readable bits out (timestamp, version, panic
//!      message, location).
//!   3. Suppress if we've already shown the recovery dialog for that
//!      timestamp — `config.system.last_seen_crash_timestamp` records
//!      what we've offered the user. String-equality on the literal
//!      header works; we don't need to parse the date.
//!
//! Surfacing happens through the existing floating banner system —
//! the frontend calls `get_recent_crash` after the window mounts and
//! decides what to render. This module owns the parse + the
//! "consumed?" mark; the UI owns the banner copy + actions.
//!
//! Pure parser is exercised by unit tests so we can pin the report
//! format invariant.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where the panic hook writes its crash log. Mirrors
/// `panic_handler::crash_log_path`. Pulled here so the recovery
/// reader doesn't have to reach into the panic module's internals.
pub fn crash_log_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kage")
        .join("logs")
        .join("crash.log")
}

/// Header marker every report opens with. Kept as a constant so the
/// parser and the writer stay in lock-step — if you change the
/// writer, change this and the unit tests will tell you about it.
const REPORT_HEADER_PREFIX: &str = "=== Kage crash report @ ";

/// Maximum bytes of the crash file we'll read at startup. The file
/// can in principle grow large (one report ≈ 2-50 KB depending on
/// app-log content); we want to find the LAST report which lives at
/// the tail. Reading the whole file would be wasteful for users with
/// long crash histories. We seek backward up to this many bytes from
/// EOF, which is enough for one well-formed report.
const TAIL_READ_BYTES: u64 = 256 * 1024;

/// Compact summary of the most recent crash. Returned to the UI so
/// it can build a banner / open the file / pre-fill a feedback URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrashSummary {
    /// The literal timestamp string from the report header. Used as
    /// the dedupe key against `last_seen_crash_timestamp`. Format
    /// matches whatever the writer uses (`%Y-%m-%d %H:%M:%S%.3f %z`),
    /// but parsing isn't required — string equality is enough.
    pub timestamp: String,
    /// App version that crashed. May differ from the running version
    /// if the user has since updated.
    pub version: String,
    /// First line of the panic payload. Truncated to a reasonable
    /// length so the banner doesn't try to render a 4 KB string.
    pub panic_message: String,
    /// `<file>:<line>:<col>` if the panic carried a location. None
    /// for foreign panics or `panic!()` without `location`.
    pub location: Option<String>,
    /// Absolute path of the crash log file, so the UI can offer a
    /// "View log" affordance that opens it in the OS default editor.
    pub log_path: String,
}

/// Read the tail of the crash file and parse out the most recent
/// report. Returns `None` if the file doesn't exist or the tail
/// doesn't contain a recognisable header.
///
/// Pure I/O: reads but does not modify. Marking a report as "seen"
/// is the caller's responsibility (they need the live config for
/// that, which we don't reach into here).
pub fn read_recent_crash() -> Option<CrashSummary> {
    let path = crash_log_path();
    if !path.exists() {
        return None;
    }
    let body = read_tail(&path, TAIL_READ_BYTES).ok()?;
    let summary = parse_last_report(&body)?;
    Some(CrashSummary {
        log_path: path.to_string_lossy().to_string(),
        ..summary
    })
}

fn read_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    if start > 0 {
        f.seek(SeekFrom::Start(start))?;
    }
    let mut buf = Vec::with_capacity(max_bytes.min(len) as usize);
    f.read_to_end(&mut buf)?;
    // The file is plain UTF-8 we wrote ourselves; if it isn't, we'd
    // rather skip the dialog than blow up on launch.
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Pull the most recent `=== Kage crash report @ ... ===` block out
/// of the given body and return its summary fields. Returns `None`
/// if no header is found or the block is too malformed to extract a
/// timestamp + version.
pub fn parse_last_report(body: &str) -> Option<CrashSummary> {
    let header_idx = body.rfind(REPORT_HEADER_PREFIX)?;
    let block = &body[header_idx..];

    // The header line is `=== Kage crash report @ <ts> ===`. We pull
    // the timestamp out by trimming the static prefix + suffix.
    let header_line = block.lines().next()?;
    let after_prefix = header_line.strip_prefix(REPORT_HEADER_PREFIX)?;
    let timestamp = after_prefix.trim_end_matches(" ===").trim().to_string();
    if timestamp.is_empty() {
        return None;
    }

    let mut version = String::new();
    let mut panic_message = String::new();
    let mut location: Option<String> = None;

    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("Version: ") {
            version = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Panic message: ") {
            panic_message = truncate_message(rest.trim());
        } else if let Some(rest) = line.strip_prefix("Panic location: ") {
            location = Some(rest.trim().to_string());
        } else if line.starts_with("--- Backtrace ---") {
            // Past the metadata block; stop scanning so a later
            // backtrace line that happens to start with "Version:"
            // can't trump the real one.
            break;
        }
    }
    if version.is_empty() && panic_message.is_empty() {
        // Nothing useful to surface — bail rather than pop a banner
        // with empty content.
        return None;
    }
    Some(CrashSummary {
        timestamp,
        version,
        panic_message,
        location,
        log_path: String::new(), // filled in by `read_recent_crash`
    })
}

/// Truncate a panic message to something a banner can render. We
/// take only the first line (panics with multi-line payloads tend
/// to have the actionable bit on line 1 and a long debug-format
/// dump after) and cap at a reasonable column count.
fn truncate_message(s: &str) -> String {
    const MAX: usize = 240;
    let first_line = s.lines().next().unwrap_or("");
    if first_line.len() <= MAX {
        return first_line.to_string();
    }
    // UTF-8 boundary safe truncate.
    let mut end = MAX;
    while end > 0 && !first_line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &first_line[..end])
}

/// Should we show the recovery dialog for this crash? `false` if
/// already seen.
pub fn is_unseen(summary: &CrashSummary, last_seen: Option<&str>) -> bool {
    match last_seen {
        Some(prev) => prev != summary.timestamp,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(ts: &str, version: &str, message: &str, location: Option<&str>) -> String {
        let mut s = format!("=== Kage crash report @ {} ===\n", ts);
        s.push_str(&format!("Version: {}\n", version));
        s.push_str("OS: linux\nArch: x86_64\n\n");
        s.push_str(&format!("Panic message: {}\n", message));
        if let Some(loc) = location {
            s.push_str(&format!("Panic location: {}\n", loc));
        }
        s.push_str("\n--- Backtrace ---\nstack frame 1\n\n");
        s.push_str("--- Recent app log (most recent last) ---\n");
        s.push_str("ts [info] src: prior log line\n");
        s
    }

    #[test]
    fn parse_pulls_fields_from_a_well_formed_report() {
        let body = report(
            "2026-05-26 10:00:00.123 +0000",
            "1.2.3",
            "boom",
            Some("src/foo.rs:10:5"),
        );
        let s = parse_last_report(&body).unwrap();
        assert_eq!(s.timestamp, "2026-05-26 10:00:00.123 +0000");
        assert_eq!(s.version, "1.2.3");
        assert_eq!(s.panic_message, "boom");
        assert_eq!(s.location.as_deref(), Some("src/foo.rs:10:5"));
    }

    #[test]
    fn parse_picks_the_last_report_when_multiple_appended() {
        let mut body = report("2026-01-01 00:00:00.000 +0000", "1.0.0", "old crash", None);
        body.push('\n');
        body.push_str(&report(
            "2026-05-26 10:00:00.000 +0000",
            "1.2.3",
            "fresh crash",
            None,
        ));
        let s = parse_last_report(&body).unwrap();
        assert_eq!(s.timestamp, "2026-05-26 10:00:00.000 +0000");
        assert_eq!(s.panic_message, "fresh crash");
    }

    #[test]
    fn parse_returns_none_when_no_header() {
        assert!(parse_last_report("just some logs without a header\n").is_none());
        assert!(parse_last_report("").is_none());
    }

    #[test]
    fn parse_omits_location_when_absent() {
        let body = report("2026-05-26 10:00:00.000 +0000", "1.2.3", "boom", None);
        let s = parse_last_report(&body).unwrap();
        assert!(s.location.is_none());
    }

    #[test]
    fn parse_truncates_extremely_long_panic_messages() {
        let long = "x".repeat(500);
        let body = report("ts", "v", &long, None);
        let s = parse_last_report(&body).unwrap();
        // Cap is 240 chars + the 3-byte UTF-8 ellipsis. Use chars(),
        // not len(), since byte counts include the ellipsis bytes.
        assert!(s.panic_message.chars().count() <= 241);
        assert!(s.panic_message.ends_with('…'));
    }

    #[test]
    fn parse_takes_only_first_line_of_message() {
        let body = report("ts", "v", "first\nsecond line should be dropped", None);
        let s = parse_last_report(&body).unwrap();
        assert_eq!(s.panic_message, "first");
    }

    #[test]
    fn parse_ignores_lines_after_backtrace_header() {
        // A backtrace frame that happened to look like "Version: ..."
        // must not overwrite the real Version line.
        let mut body = String::from("=== Kage crash report @ ts ===\n");
        body.push_str("Version: real-version\n");
        body.push_str("Panic message: boom\n");
        body.push_str("\n--- Backtrace ---\n");
        body.push_str("Version: NOT-the-version\n");
        let s = parse_last_report(&body).unwrap();
        assert_eq!(s.version, "real-version");
    }

    #[test]
    fn parse_returns_none_when_block_has_no_useful_metadata() {
        // Header but no Version + no Panic message — nothing to show.
        let body = "=== Kage crash report @ ts ===\nOS: linux\nArch: x86\n";
        assert!(parse_last_report(body).is_none());
    }

    #[test]
    fn is_unseen_handles_first_run_and_repeat() {
        let s = CrashSummary {
            timestamp: "ts".into(),
            version: "v".into(),
            panic_message: "m".into(),
            location: None,
            log_path: String::new(),
        };
        assert!(is_unseen(&s, None));
        assert!(is_unseen(&s, Some("different-ts")));
        assert!(!is_unseen(&s, Some("ts")));
    }
}
