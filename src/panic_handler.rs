//! Global panic hook that writes a crash report to `crash.log` before the
//! process exits, capturing the panic payload, location, backtrace, and a
//! snapshot of the recent in-memory app log.
//!
//! This turns silent crashes into actionable diagnostics — when a user
//! reports "it just closed", there's a file we can ask them to attach.

use log::error;
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

fn write_crash_report(info: &std::panic::PanicHookInfo<'_>) {
    // Build the report in memory first so partial failures don't corrupt the
    // eventual on-disk report.
    let mut report = String::with_capacity(4096);

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f %z");
    report.push_str(&format!("=== Kage crash report @ {} ===\n", timestamp));
    report.push_str(&format!("Version: {}\n", env!("CARGO_PKG_VERSION")));
    report.push_str(&format!("OS: {}\n", std::env::consts::OS));
    report.push_str(&format!("Arch: {}\n", std::env::consts::ARCH));

    // Panic payload
    let payload = info.payload();
    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    };
    report.push_str(&format!("\nPanic message: {}\n", msg));

    if let Some(location) = info.location() {
        report.push_str(&format!(
            "Panic location: {}:{}:{}\n",
            location.file(),
            location.line(),
            location.column()
        ));
    }

    // Backtrace (only if RUST_BACKTRACE is enabled — otherwise Backtrace::capture() is ~free)
    report.push_str("\n--- Backtrace ---\n");
    let bt = std::backtrace::Backtrace::force_capture();
    report.push_str(&format!("{}\n", bt));

    // Recent in-memory app log (best effort — don't fail the crash report if
    // this is unavailable for any reason).
    // Cap at the most recent 200 entries so the crash report stays readable.
    report.push_str("\n--- Recent app log (most recent last) ---\n");
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        crate::app_log::get_entries,
    )) {
        Ok(entries) => {
            let skip = entries.len().saturating_sub(200);
            for entry in entries.into_iter().skip(skip) {
                report.push_str(&format!(
                    "{} [{}] {}: {}\n",
                    entry.ts, entry.level, entry.source, entry.msg
                ));
            }
        }
        Err(_) => {
            report.push_str("<unable to read app log>\n");
        }
    }

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
