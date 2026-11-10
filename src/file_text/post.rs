//! After the worker: the result, the audit entry, telemetry, the log line
//! and the progress events (docs/design/EXTENSION_FILE_TEXT.md §3.6,
//! §3.10, §5.10, §5.12–§5.15).
//!
//! Counts only leave Kage (telemetry). The file name goes to the extension
//! and to the local, clearable audit log, never to `app.jsonl`.

use super::read::{Checked, Named};
use super::worker::Extraction;
use super::{Code, FileText};
use crate::config::Config;
use crate::config_export::sensitive;
use crate::error::AppError;
use crate::lock_ext::LockExt;
use crate::permission_audit::{AuditEntry, AuditEvent};
use kage_core::file_text_wire::{Format, Warning, MAX_TITLE_CHARS};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// --- Cleaning names and titles (§5.10) ------------------------------------------

/// Removed from all text: C0 controls but tab and line feed, DEL, C1
/// controls, U+FEFF, the LRO and RLO overrides, tag characters, and
/// noncharacters. The worker applies the same rule to document text.
fn is_removed(c: char) -> bool {
    let u = c as u32;
    match u {
        0x09 | 0x0A => false,
        0x00..=0x1F | 0x7F..=0x9F => true,
        0xFEFF | 0x202D | 0x202E => true,
        0xE0000..=0xE007F => true,
        0xFDD0..=0xFDEF => true,
        _ => (u & 0xFFFE) == 0xFFFE,
    }
}

/// Every bidi control: none survive in a name or title.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// A name or title on one clean line: NFC, the removed characters and
/// every bidi control dropped, whitespace runs as single spaces, trimmed,
/// at most `max_chars` characters.
pub fn clean_line(raw: &str, max_chars: usize) -> String {
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(raw);
    let mut out = String::new();
    let mut n = 0usize;
    let mut space = false;
    for c in nfc.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if is_removed(c) || is_bidi_control(c) {
            continue;
        }
        let need = if space && n > 0 { 2 } else { 1 };
        if n + need > max_chars {
            break;
        }
        if need == 2 {
            out.push(' ');
        }
        out.push(c);
        n += need;
        space = false;
    }
    out
}

// --- The result (§3.6, §5.12) ----------------------------------------------------

/// The `FileText` for a worker answer. Re-checks the worker's text against
/// `max_chars` and keeps only page offsets that fit the text. Adds
/// `possible_secret` when the text or title looks like it holds a
/// password, token or key; the match itself is never logged or returned.
pub fn build(
    named: &Named,
    checked: &Checked,
    out: Extraction,
    sha256: String,
    file_ref: String,
    max_chars: u32,
) -> Result<FileText, AppError> {
    let counted = out.text.chars().count();
    let chars = u32::try_from(counted)
        .ok()
        .filter(|n| *n <= max_chars)
        .ok_or_else(|| Code::WorkerFailed.error(&[]))?;
    let title = out
        .title
        .map(|t| clean_line(&t, MAX_TITLE_CHARS))
        .filter(|t| !t.is_empty());
    let page_starts = out
        .page_starts
        .filter(|s| s.windows(2).all(|w| w[0] <= w[1]) && s.iter().all(|p| *p <= chars));
    let mut warnings: Vec<Warning> = Vec::new();
    for w in out.warnings {
        if w != Warning::PossibleSecret && !warnings.contains(&w) {
            warnings.push(w);
        }
    }
    // One linear pass: widening every match (`secret_spans`) can take
    // quadratic time on a crafted document, here in Kage's own process.
    let secret = sensitive::has_secret_span(&out.text)
        || title.as_deref().is_some_and(sensitive::has_secret_span);
    if secret {
        warnings.push(Warning::PossibleSecret);
    }
    Ok(FileText {
        name: named.name.clone(),
        extension: named.extension.clone(),
        format: out.format,
        size: checked.size,
        modified_at: checked.modified_at,
        title,
        text: out.text,
        chars,
        truncated: out.truncated,
        total_chars: out.total_chars.filter(|t| *t >= chars),
        pages: out.pages,
        page_starts,
        warnings,
        sha256,
        file_ref,
    })
}

// --- The report: audit, telemetry, log (§5.13–§5.15) -----------------------------

/// What happened to one picked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub extension_id: String,
    /// The cleaned file name.
    pub name: String,
    /// The detected format, else the one the extension names.
    pub format: Option<Format>,
    pub bytes: u64,
    pub chars: u32,
    pub truncated: bool,
    /// `ok`, or the error key's last segment (`too_large`, `cancelled`, …).
    pub outcome: String,
    /// From the dialog closing to the answer.
    pub duration: Duration,
}

impl Report {
    /// The audit entry. The name is left out if it looks like a secret.
    pub fn audit_event(&self) -> AuditEvent {
        AuditEvent::ExtensionFileRead {
            extension_id: self.extension_id.clone(),
            file_name: Some(self.name.clone())
                .filter(|n| !n.is_empty() && !sensitive::has_secret_span(n)),
            format: self.format.map(|f| f.id().to_string()),
            bytes: self.bytes,
            chars: self.chars,
            truncated: self.truncated,
            outcome: self.outcome.clone(),
            duration_ms: millis(self.duration),
        }
    }

    /// `extension_file_text` props: counts in buckets, no names or text.
    pub fn telemetry_props(&self) -> serde_json::Value {
        serde_json::json!({
            "extension_id": self.extension_id,
            "format": self.format.map_or("unknown", |f| f.id()),
            "outcome": self.outcome,
            "size_bucket": size_bucket(self.bytes),
            "chars_bucket": chars_bucket(self.chars),
            "ms_bucket": ms_bucket(self.duration),
        })
    }

    /// The one log line for the request. Never the name.
    pub fn log_line(&self) -> String {
        format!(
            "File text: {} read a {} file ({}, {} chars) in {} ms: {}",
            self.extension_id,
            self.format.map_or("unknown", |f| f.id()),
            size_bucket(self.bytes),
            self.chars,
            millis(self.duration),
            self.outcome
        )
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// `<100KB`, `<1MB`, `<10MB`, `≤32MB`; `>32MB` for files refused as too
/// large.
pub fn size_bucket(bytes: u64) -> &'static str {
    const KB: u64 = 1024;
    match bytes {
        b if b < 100 * KB => "<100KB",
        b if b < KB * KB => "<1MB",
        b if b < 10 * KB * KB => "<10MB",
        b if b <= kage_core::file_text_wire::MAX_INPUT_BYTES => "≤32MB",
        _ => ">32MB",
    }
}

/// `<10k`, `<100k`, `<1M`, `1M`.
pub fn chars_bucket(chars: u32) -> &'static str {
    match chars {
        c if c < 10_000 => "<10k",
        c if c < 100_000 => "<100k",
        c if c < 1_000_000 => "<1M",
        _ => "1M",
    }
}

/// `<500`, `<2000`, `<10000`, `10000+`.
pub fn ms_bucket(d: Duration) -> &'static str {
    match millis(d) {
        m if m < 500 => "<500",
        m if m < 2_000 => "<2000",
        m if m < 10_000 => "<10000",
        _ => "10000+",
    }
}

// --- Progress events (§3.10) ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    DialogClosed,
    Reading,
    Downloading,
    Extracting,
    Done,
}

/// `file_text_progress`'s payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub extension_id: String,
    pub phase: Phase,
    /// The cleaned file name, once there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// With `dialog_closed`: whether a file will be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picked: Option<bool>,
}

/// Where a request reports to: the calling window, the audit log and
/// telemetry in the app; a recorder in tests.
pub trait Events: Send + Sync {
    fn progress(&self, progress: &Progress);
    /// A picked file was handled (any outcome). Not called when no file
    /// was picked.
    fn finished(&self, report: &Report);
    /// The extension still holds `file_text` and is on, checked after the
    /// dialog closes (it may have been turned off while it was open).
    fn still_allowed(&self) -> bool;
}

/// The app's [`Events`].
pub struct TauriEvents<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
    config: Arc<Mutex<Config>>,
    extension_id: String,
}

impl<R: tauri::Runtime> TauriEvents<R> {
    pub fn new(
        app: tauri::AppHandle<R>,
        window: tauri::WebviewWindow<R>,
        config: Arc<Mutex<Config>>,
        extension_id: &str,
    ) -> Self {
        Self {
            app,
            window,
            config,
            extension_id: extension_id.to_string(),
        }
    }
}

impl<R: tauri::Runtime> Events for TauriEvents<R> {
    fn progress(&self, progress: &Progress) {
        crate::event_targets::emit_to_self(
            &self.window,
            crate::events::FILE_TEXT_PROGRESS,
            progress,
        );
    }

    fn finished(&self, report: &Report) {
        crate::permission_audit::append(&AuditEntry::now(report.audit_event()));
        crate::telemetry::track(
            &self.app,
            "extension_file_text",
            Some(report.telemetry_props()),
        );
    }

    fn still_allowed(&self) -> bool {
        let config = self.config.lock_or_recover();
        super::check_access(&config, &self.extension_id, &self.extension_id).is_ok()
    }
}
