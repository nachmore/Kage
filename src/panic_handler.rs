//! Global panic hook that writes a crash report to `crash.log` before the
//! process exits, capturing the panic payload, location, backtrace, and a
//! snapshot of the recent in-memory app log.
//!
//! This turns silent crashes into actionable diagnostics — when a user
//! reports "it just closed", there's a file we can ask them to attach.

use crate::app_log::LogEntry;
use log::error;
use std::any::Any;
use std::io::Write;
use std::path::PathBuf;

/// Install the global panic hook. Safe to call multiple times; only the first
/// call takes effect.
pub fn install() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Chain with whatever default/existing hook is in place so we don't
        // silently drop stderr output in dev builds.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            write_crash_report(info);
            previous(info);
        }));
    });
}

fn crash_log_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kage")
        .join("logs")
        .join("crash.log")
}

/// Cap on app-log entries appended to the crash report — most recent N
/// entries are kept so the report stays readable.
const MAX_APP_LOG_ENTRIES_IN_REPORT: usize = 200;

/// Inputs needed to build a crash report. Extracted as a struct so the pure
/// `build_crash_report` function below is callable from tests without
/// fabricating a `PanicHookInfo` (which has no public constructor).
pub struct CrashReportInputs<'a> {
    pub timestamp: String,
    pub version: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
    pub payload: &'a (dyn Any + Send),
    pub location: Option<(String, u32, u32)>,
    pub backtrace: String,
    /// `None` means "unable to read app log" — produces a sentinel line in
    /// the report instead of an empty section.
    pub app_log: Option<Vec<LogEntry>>,
}

/// Extract a string panic message from the dyn Any payload, matching the
/// formatting Rust's default panic hook uses.
pub fn extract_panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Build the textual crash report from already-collected inputs. Pure —
/// no I/O, no time, no globals — so it's straightforward to test.
pub fn build_crash_report(inputs: &CrashReportInputs<'_>) -> String {
    let mut report = String::with_capacity(4096);

    report.push_str(&format!(
        "=== Kage crash report @ {} ===\n",
        inputs.timestamp
    ));
    report.push_str(&format!("Version: {}\n", inputs.version));
    report.push_str(&format!("OS: {}\n", inputs.os));
    report.push_str(&format!("Arch: {}\n", inputs.arch));

    let msg = extract_panic_message(inputs.payload);
    report.push_str(&format!("\nPanic message: {}\n", msg));

    if let Some((file, line, column)) = &inputs.location {
        report.push_str(&format!("Panic location: {}:{}:{}\n", file, line, column));
    }

    report.push_str("\n--- Backtrace ---\n");
    report.push_str(&inputs.backtrace);
    if !inputs.backtrace.ends_with('\n') {
        report.push('\n');
    }

    report.push_str("\n--- Recent app log (most recent last) ---\n");
    match &inputs.app_log {
        Some(entries) => {
            let skip = entries.len().saturating_sub(MAX_APP_LOG_ENTRIES_IN_REPORT);
            for entry in entries.iter().skip(skip) {
                report.push_str(&format!(
                    "{} [{}] {}: {}\n",
                    entry.ts, entry.level, entry.source, entry.msg
                ));
            }
        }
        None => {
            report.push_str("<unable to read app log>\n");
        }
    }

    report
}

fn write_crash_report(info: &std::panic::PanicHookInfo<'_>) {
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    let app_log =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(crate::app_log::get_entries)).ok();

    let inputs = CrashReportInputs {
        timestamp: chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f %z")
            .to_string(),
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        payload: info.payload(),
        location: info
            .location()
            .map(|l| (l.file().to_string(), l.line(), l.column())),
        backtrace: bt,
        app_log,
    };

    let report = build_crash_report(&inputs);

    // Write it out. Create the dir if missing. If writing fails there's
    // nowhere useful to go — best effort is all we can do.
    let path = crash_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Append so repeated crashes during a single session don't clobber each other.
    let write_result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(report.as_bytes()));

    match write_result {
        Ok(()) => {
            // Can't rely on the logger being healthy inside a panic — write to
            // stderr as a last-ditch signal too.
            eprintln!("Kage panicked — crash report written to {}", path.display());
            // Best-effort log entry (may be dropped if logger itself panicked)
            error!("Kage panicked — crash report written to {}", path.display());
        }
        Err(e) => {
            eprintln!("Kage panicked and also failed to write crash report: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: &str, level: &str, source: &str, msg: &str) -> LogEntry {
        LogEntry {
            ts: ts.to_string(),
            level: level.to_string(),
            source: source.to_string(),
            msg: msg.to_string(),
        }
    }

    fn base_inputs<'a>(payload: &'a (dyn Any + Send)) -> CrashReportInputs<'a> {
        CrashReportInputs {
            timestamp: "2026-05-07 12:00:00.000 +0000".to_string(),
            version: "9.9.9",
            os: "test-os",
            arch: "test-arch",
            payload,
            location: Some(("src/foo.rs".to_string(), 42, 7)),
            backtrace: "stack frame 1\nstack frame 2\n".to_string(),
            app_log: Some(vec![entry("t1", "info", "src", "first message")]),
        }
    }

    #[test]
    fn extract_message_from_str_payload() {
        let payload: Box<dyn Any + Send> = Box::new("static panic msg");
        assert_eq!(extract_panic_message(&*payload), "static panic msg");
    }

    #[test]
    fn extract_message_from_string_payload() {
        let payload: Box<dyn Any + Send> = Box::new(String::from("owned panic msg"));
        assert_eq!(extract_panic_message(&*payload), "owned panic msg");
    }

    #[test]
    fn extract_message_from_unknown_payload_uses_sentinel() {
        // Payload that isn't a string at all — e.g. panic!(42).
        let payload: Box<dyn Any + Send> = Box::new(42_u32);
        assert_eq!(
            extract_panic_message(&*payload),
            "<non-string panic payload>"
        );
    }

    #[test]
    fn report_includes_required_sections_and_metadata() {
        let payload: Box<dyn Any + Send> = Box::new("boom");
        let report = build_crash_report(&base_inputs(&*payload));

        assert!(report.contains("=== Kage crash report @ 2026-05-07 12:00:00.000 +0000 ==="));
        assert!(report.contains("Version: 9.9.9"));
        assert!(report.contains("OS: test-os"));
        assert!(report.contains("Arch: test-arch"));
        assert!(report.contains("Panic message: boom"));
        assert!(report.contains("Panic location: src/foo.rs:42:7"));
        assert!(report.contains("--- Backtrace ---"));
        assert!(report.contains("stack frame 1"));
        assert!(report.contains("--- Recent app log"));
        assert!(report.contains("first message"));
    }

    #[test]
    fn report_omits_location_line_when_unavailable() {
        let payload: Box<dyn Any + Send> = Box::new("boom");
        let mut inputs = base_inputs(&*payload);
        inputs.location = None;
        let report = build_crash_report(&inputs);
        assert!(!report.contains("Panic location:"));
        // The other sections still render — important so an unknown location
        // doesn't truncate the rest of the report.
        assert!(report.contains("Panic message: boom"));
        assert!(report.contains("--- Backtrace ---"));
    }

    #[test]
    fn report_caps_app_log_at_recent_window() {
        // Build a log of 250 entries; only the last 200 should show up.
        let payload: Box<dyn Any + Send> = Box::new("boom");
        let mut entries = Vec::with_capacity(250);
        for i in 0..250 {
            entries.push(entry(
                &format!("ts-{:03}", i),
                "info",
                "test",
                &format!("msg-{:03}", i),
            ));
        }
        let mut inputs = base_inputs(&*payload);
        inputs.app_log = Some(entries);
        let report = build_crash_report(&inputs);

        // First 50 entries (msg-000 through msg-049) must be dropped.
        assert!(!report.contains("msg-000"));
        assert!(!report.contains("msg-049"));
        // msg-050 is the first kept entry; msg-249 is the most recent.
        assert!(report.contains("msg-050"));
        assert!(report.contains("msg-249"));
    }

    #[test]
    fn report_handles_app_log_unavailable_with_sentinel() {
        let payload: Box<dyn Any + Send> = Box::new("boom");
        let mut inputs = base_inputs(&*payload);
        inputs.app_log = None;
        let report = build_crash_report(&inputs);
        assert!(report.contains("<unable to read app log>"));
        // Other sections still present.
        assert!(report.contains("Panic message: boom"));
    }

    #[test]
    fn report_handles_empty_app_log_without_panicking() {
        let payload: Box<dyn Any + Send> = Box::new("boom");
        let mut inputs = base_inputs(&*payload);
        inputs.app_log = Some(vec![]);
        let report = build_crash_report(&inputs);
        // Header is present, no entries below it.
        assert!(report.contains("--- Recent app log"));
        // No sentinel — empty log is different from "couldn't read".
        assert!(!report.contains("<unable to read app log>"));
    }

    #[test]
    fn report_normalizes_backtrace_trailing_newline() {
        // A backtrace without a trailing \n should still produce a clean section.
        let payload: Box<dyn Any + Send> = Box::new("boom");
        let mut inputs = base_inputs(&*payload);
        inputs.backtrace = "single line".to_string();
        let report = build_crash_report(&inputs);
        // After "single line" we want a newline before the next section header.
        assert!(report.contains("single line\n\n--- Recent app log"));
    }
}
