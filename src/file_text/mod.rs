//! Reading a file the user picks, for an extension
//! (docs/design/EXTENSION_FILE_TEXT.md §3.5, §5.1, §5.3–§5.5,
//! §5.11–§5.15).
//!
//! An extension holding `file_text` calls `pick_file_text`. Kage opens the
//! native file dialog; if the user picks a file, Kage checks it, reads it
//! here (so the OS grant for the user's pick applies), and hands only the
//! bytes to the `kage-file-text` worker process, which returns the text.
//! The extension gets the text and a little metadata, never the path.
//!
//! - `mod.rs` (this file): the result types, the errors, the argument and
//!   grant checks, and [`pick`], the request from start to end.
//! - `dialog`: the native dialog, behind [`dialog::DialogSource`] so tests
//!   use a fixed path.
//! - `read`: the checks on the picked file and the read with a timeout.
//! - `worker`: finding, running and killing the worker.
//! - `file_ref`: `sha256` and the keyed, per-extension `file_ref`.
//! - `post`: building the result, the audit entry, telemetry, the log
//!   line, and the Tauri side of progress events.
//! - `request`: the steps after the slot is taken (§5.1 steps 6–12).
//! - `slot`: one request at a time, and the gap between dialogs.
//!
//! Logs never hold the file name, the path, the title or any text.
//! `AppError`'s `Display` fills in `{name}`, so this module logs
//! `err.key`, never the error itself.

pub mod dialog;
pub mod file_ref;
pub mod post;
pub mod read;
pub mod request;
pub mod slot;
pub mod worker;

#[cfg(test)]
mod tests;

use crate::config::Config;
use crate::error::{AppError, ErrorKind};
use crate::lock_ext::LockExt;
use kage_core::file_text_wire::{self as wire, Format, Pages, Warning};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use request::run;
pub use slot::{slot, SlotGuard};

/// The capability that gates the three commands.
pub const CAPABILITY: &str = "file_text";

// --- Results ---------------------------------------------------------------

/// What `pick_file_text` returns for a file it read (§3.6). Never holds a
/// path. Field names stay clear of the host's secret-name heuristic, so an
/// extension can store the object as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileText {
    /// The file name only, NFC, cleaned, at most 255 characters.
    pub name: String,
    /// Lowercase, without the dot.
    pub extension: String,
    /// What the content turned out to be.
    pub format: Format,
    /// Bytes.
    pub size: u64,
    /// Milliseconds since the epoch (UTC), when the OS reports it.
    pub modified_at: Option<i64>,
    /// Document metadata or the first heading, at most 200 characters.
    pub title: Option<String>,
    pub text: String,
    /// Unicode scalar values in `text`.
    pub chars: u32,
    /// The document had more text than `max_chars`.
    pub truncated: bool,
    /// The full length, when the parser knows it.
    pub total_chars: Option<u32>,
    /// PDF pages or PPTX slides.
    pub pages: Option<Pages>,
    /// Char offset in `text` where each read page starts.
    pub page_starts: Option<Vec<u32>>,
    pub warnings: Vec<Warning>,
    /// Lowercase hex SHA-256 of the file's bytes.
    pub sha256: String,
    /// `fr1_` + base64url: the same file at the same place on this
    /// computer, for this extension (§5.11).
    pub file_ref: String,
}

/// What this Kage can read (`file_text_support`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileTextSupport {
    /// Only the formats this build reads.
    pub formats: Vec<FormatInfo>,
    pub max_input_bytes: u64,
    pub max_chars: u32,
    pub default_max_chars: u32,
}

/// One readable format and its file extensions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormatInfo {
    pub id: Format,
    pub extensions: Vec<&'static str>,
}

/// The extension's options for one request (`args` of `pick_file_text`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickFileTextArgs {
    /// 1–7 unique formats. Default: every format this build reads.
    #[serde(default)]
    pub formats: Option<Vec<Format>>,
    /// 1..=1,000,000. Default: 100,000.
    #[serde(default)]
    pub max_chars: Option<u32>,
}

/// `file_text_support`'s answer for this build.
pub fn support() -> FileTextSupport {
    FileTextSupport {
        formats: wire::readable_formats()
            .map(|f| FormatInfo {
                id: f,
                extensions: f.extensions().to_vec(),
            })
            .collect(),
        max_input_bytes: wire::MAX_INPUT_BYTES,
        max_chars: wire::MAX_CHARS,
        default_max_chars: wire::DEFAULT_MAX_CHARS,
    }
}

// --- Errors ----------------------------------------------------------------

/// Every `errors.file_text.*` key Rust raises (§3.8). `declined` and
/// `no_window` are the JS host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    NotGranted,
    ExtensionDisabled,
    InvalidFormats,
    InvalidMaxChars,
    Busy,
    TooSoon,
    Unavailable,
    DialogFailed,
    NotAFile,
    Unsupported,
    WrongContent,
    TooLarge,
    ReadFailed,
    NotDownloaded,
    Encrypted,
    LegacyFormat,
    NotText,
    Malformed,
    TooComplex,
    Timeout,
    Cancelled,
    WorkerFailed,
}

impl Code {
    /// The key's last segment, which is also the audit and telemetry
    /// outcome.
    pub const fn as_str(self) -> &'static str {
        match self {
            Code::NotGranted => "not_granted",
            Code::ExtensionDisabled => "extension_disabled",
            Code::InvalidFormats => "invalid_formats",
            Code::InvalidMaxChars => "invalid_max_chars",
            Code::Busy => "busy",
            Code::TooSoon => "too_soon",
            Code::Unavailable => "unavailable",
            Code::DialogFailed => "dialog_failed",
            Code::NotAFile => "not_a_file",
            Code::Unsupported => "unsupported",
            Code::WrongContent => "wrong_content",
            Code::TooLarge => "too_large",
            Code::ReadFailed => "read_failed",
            Code::NotDownloaded => "not_downloaded",
            Code::Encrypted => "encrypted",
            Code::LegacyFormat => "legacy_format",
            Code::NotText => "not_text",
            Code::Malformed => "malformed",
            Code::TooComplex => "too_complex",
            Code::Timeout => "timeout",
            Code::Cancelled => "cancelled",
            Code::WorkerFailed => "worker_failed",
        }
    }

    pub const fn kind(self) -> ErrorKind {
        match self {
            Code::NotGranted | Code::ExtensionDisabled => ErrorKind::NotPermitted,
            Code::Busy | Code::TooSoon => ErrorKind::RateLimited,
            Code::Unavailable | Code::DialogFailed | Code::ReadFailed | Code::WorkerFailed => {
                ErrorKind::Internal
            }
            Code::NotDownloaded | Code::Timeout => ErrorKind::Timeout,
            Code::Cancelled => ErrorKind::Cancelled,
            Code::InvalidFormats
            | Code::InvalidMaxChars
            | Code::NotAFile
            | Code::Unsupported
            | Code::WrongContent
            | Code::TooLarge
            | Code::Encrypted
            | Code::LegacyFormat
            | Code::NotText
            | Code::Malformed
            | Code::TooComplex => ErrorKind::InvalidInput,
        }
    }

    pub fn key(self) -> String {
        format!("errors.file_text.{}", self.as_str())
    }

    pub fn error(self, params: &[(&str, &str)]) -> AppError {
        AppError::keyed(self.kind(), self.key(), params)
    }

    /// The error for a reason that only needs the file name.
    pub fn named(self, name: &str) -> AppError {
        self.error(&[("name", name)])
    }

    /// The code the worker's answer maps to.
    pub const fn from_worker(code: wire::WorkerErrorCode) -> Code {
        use wire::WorkerErrorCode as W;
        match code {
            W::Unsupported => Code::Unsupported,
            W::WrongContent => Code::WrongContent,
            W::Encrypted => Code::Encrypted,
            W::LegacyFormat => Code::LegacyFormat,
            W::NotText => Code::NotText,
            W::Malformed => Code::Malformed,
            W::TooComplex => Code::TooComplex,
        }
    }
}

/// The audit, telemetry and log outcome of an error: its key's last
/// segment (`too_large`, …).
pub fn outcome_of(err: &AppError) -> &str {
    err.key.rsplit('.').next().unwrap_or(&err.key)
}

/// `.txt, .md, .pdf, .docx`: the main extension of each format, for
/// messages. Needs no translation.
pub fn extension_list(formats: &[Format]) -> String {
    formats
        .iter()
        .filter_map(|f| f.extensions().first())
        .map(|e| format!(".{e}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `48.2 MB`: a size for messages, in binary units.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let text = format!("{value:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text);
    format!("{text} {}", UNITS[unit])
}

// --- Checks before the dialog (§5.1 steps 1–3) --------------------------------

/// A checked request: who asks, what for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub extension_id: String,
    /// Cleaned, at most 64 characters, never empty.
    pub display_name: String,
    /// Requested and readable by this build, in the caller's order.
    pub formats: Vec<Format>,
    pub max_chars: u32,
}

/// Steps 1–3: the id, the display name, the grant, the arguments.
pub fn prepare(
    config: &Config,
    extension_id: &str,
    display_name: &str,
    args: Option<PickFileTextArgs>,
) -> Result<Request, AppError> {
    crate::extensions::validate_extension_id(extension_id).map_err(|e| {
        AppError::keyed(
            ErrorKind::InvalidInput,
            "errors.extension.invalid_id",
            &[("reason", &e.to_string())],
        )
    })?;
    let display_name = clean_display_name(display_name, extension_id);
    check_access(config, extension_id, &display_name)?;
    let (formats, max_chars) = validate_args(args)?;
    Ok(Request {
        extension_id: extension_id.to_string(),
        display_name,
        formats,
        max_chars,
    })
}

/// The injected display name, cleaned like a file name (§5.10). Empty
/// means the id.
pub fn clean_display_name(raw: &str, extension_id: &str) -> String {
    let name = post::clean_line(raw, wire::MAX_DISPLAY_NAME_CHARS);
    if name.is_empty() {
        extension_id.to_string()
    } else {
        name
    }
}

/// The grant holds `file_text` and the extension is on. The JS host
/// already refused otherwise; this is defence in depth.
pub fn check_access(config: &Config, extension_id: &str, name: &str) -> Result<(), AppError> {
    let granted = config.extension_grants.get(extension_id).is_some_and(|g| {
        g.granted
            .iter()
            .any(|c| c.trim().eq_ignore_ascii_case(CAPABILITY))
    });
    if !granted {
        return Err(Code::NotGranted.error(&[("extension", name)]));
    }
    let enabled = config
        .extension_states
        .get(extension_id)
        .copied()
        .unwrap_or(true);
    if !enabled {
        return Err(Code::ExtensionDisabled.error(&[("extension", name)]));
    }
    Ok(())
}

/// The formats and the character limit. Formats this build doesn't read
/// yet (the second-release ones on a first-release build) are dropped,
/// so an extension written for a newer Kage still works here; if none
/// is left, the request is refused.
pub fn validate_args(args: Option<PickFileTextArgs>) -> Result<(Vec<Format>, u32), AppError> {
    let args = args.unwrap_or_default();
    let invalid_formats = || {
        let readable: Vec<Format> = wire::readable_formats().collect();
        Code::InvalidFormats.error(&[("formats", &extension_list(&readable))])
    };
    let formats = match args.formats {
        None => wire::readable_formats().collect(),
        Some(list) => {
            let mut seen = std::collections::HashSet::new();
            if list.is_empty()
                || list.len() > Format::ALL.len()
                || !list.iter().all(|f| seen.insert(*f))
            {
                return Err(invalid_formats());
            }
            let readable: Vec<Format> = list.into_iter().filter(|f| f.is_readable()).collect();
            if readable.is_empty() {
                return Err(invalid_formats());
            }
            readable
        }
    };
    let max_chars = match args.max_chars {
        None => wire::DEFAULT_MAX_CHARS,
        Some(n) if (1..=wire::MAX_CHARS).contains(&n) => n,
        Some(_) => {
            return Err(Code::InvalidMaxChars.error(&[("max", &wire::MAX_CHARS.to_string())]));
        }
    };
    Ok((formats, max_chars))
}

// --- The request -------------------------------------------------------------------

/// `pick_file_text` (§5.1): the checks, then the dialog, the read and the
/// worker on one blocking thread. `Ok(None)` when the user picked no
/// file or the request was cancelled while the dialog was open.
pub async fn pick<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
    config: Arc<Mutex<Config>>,
    extension_id: String,
    display_name: String,
    args: Option<PickFileTextArgs>,
) -> Result<Option<FileText>, AppError> {
    let started = (|| {
        let request = prepare(
            &config.lock_or_recover(),
            &extension_id,
            &display_name,
            args,
        )?;
        let worker = worker::Worker::locate().ok_or_else(|| Code::Unavailable.error(&[]))?;
        let guard = slot()
            .take(&request.extension_id, Instant::now())
            .map_err(|c| c.error(&[]))?;
        Ok::<_, AppError>((request, worker, guard))
    })();
    let (request, worker, guard) = match started {
        Ok(parts) => parts,
        Err(err) => {
            log::info!(
                "File text: a request from {} was refused: {}",
                log_id(&extension_id),
                outcome_of(&err)
            );
            return Err(err);
        }
    };
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = dialog::NativeDialog::new(app.clone(), window.clone());
        let events = post::TauriEvents::new(app, window, config, &request.extension_id);
        let env = request::Env {
            dialog: &dialog,
            extractor: &worker,
            events: &events,
        };
        run(&env, guard, &request)
    })
    .await
    .map_err(|_| Code::WorkerFailed.error(&[]))?
}

/// `cancel_file_text`: cancels the extension's request if it is the one
/// running.
pub fn cancel(extension_id: &str) -> bool {
    let cancelled = slot().cancel(extension_id);
    if cancelled {
        log::info!("File text: {} cancelled its request", log_id(extension_id));
    }
    cancelled
}

/// An extension id for a log line: valid ids as they are, anything else
/// replaced (it came from the caller and may not be an id at all).
pub fn log_id(extension_id: &str) -> &str {
    if crate::extensions::validate_extension_id(extension_id).is_ok() {
        extension_id
    } else {
        "<invalid id>"
    }
}
