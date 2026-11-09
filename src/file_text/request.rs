//! One `pick_file_text` request after its checks passed and it holds the
//! slot (docs/design/EXTENSION_FILE_TEXT.md §5.1 steps 6–12). Blocking:
//! the dialog, the read and the worker all wait on the calling thread.
//!
//! The cancel flag is checked when the dialog closes (an open native
//! dialog can't be closed from code), after the read (its bytes are
//! dropped) and during extraction (the worker is killed).

use super::dialog::{DialogRequest, DialogSource};
use super::post::{self, Events, Phase, Progress, Report};
use super::read::{self, Named};
use super::worker::Extractor;
use super::{file_ref, outcome_of, Code, FileText, Request, SlotGuard};
use crate::error::AppError;
use kage_core::file_text_wire::{WorkerRequest, READ_TIMEOUT};
use std::path::Path;
use std::time::Instant;

/// What a request talks to.
pub struct Env<'a> {
    pub dialog: &'a dyn DialogSource,
    pub extractor: &'a dyn Extractor,
    pub events: &'a dyn Events,
}

impl Env<'_> {
    fn progress(&self, request: &Request, phase: Phase, file: Option<&str>, picked: Option<bool>) {
        self.events.progress(&Progress {
            extension_id: request.extension_id.clone(),
            phase,
            file: file.map(str::to_string),
            picked,
        });
    }
}

/// Opens the dialog, reads the picked file and extracts its text. Frees
/// the slot when it returns. `Ok(None)`: no file, or the request was
/// cancelled (or the extension turned off) while the dialog was open.
pub fn run(
    env: &Env<'_>,
    slot: SlotGuard,
    request: &Request,
) -> Result<Option<FileText>, AppError> {
    let id = &request.extension_id;
    let picked = env
        .dialog
        .pick(&DialogRequest::new(&request.display_name, &request.formats));
    slot.dialog_closed(Instant::now());
    let path = match picked {
        Ok(Some(path)) if !slot.cancelled() && env.events.still_allowed() => path,
        Ok(Some(_)) => {
            env.progress(request, Phase::DialogClosed, None, Some(false));
            log::info!("File text: {id}'s request ended while its dialog was open");
            return Ok(None);
        }
        Ok(None) => {
            env.progress(request, Phase::DialogClosed, None, Some(false));
            log::info!("File text: {id}'s dialog closed without a file");
            return Ok(None);
        }
        Err(err) => {
            env.progress(request, Phase::DialogClosed, None, Some(false));
            log::warn!("File text: {id}'s dialog failed: {}", outcome_of(&err));
            return Err(err);
        }
    };
    env.progress(request, Phase::DialogClosed, None, Some(true));

    let started = Instant::now();
    let named = read::name_of(&path);
    let mut report = Report {
        extension_id: id.clone(),
        name: named.name.clone(),
        format: named.format,
        bytes: 0,
        chars: 0,
        truncated: false,
        outcome: String::new(),
        duration: Default::default(),
    };
    let result = read_and_extract(env, &slot, request, &path, &named, &mut report);
    report.duration = started.elapsed();
    match &result {
        Ok(text) => {
            report.format = Some(text.format);
            report.chars = text.chars;
            report.truncated = text.truncated;
            report.outcome = "ok".into();
        }
        Err(err) => report.outcome = outcome_of(err).to_string(),
    }
    env.events.finished(&report);
    log::info!("{}", report.log_line());
    env.progress(request, Phase::Done, Some(&named.name), None);
    drop(slot);
    result.map(Some)
}

/// §5.1 steps 7–11 for the picked `path`.
fn read_and_extract(
    env: &Env<'_>,
    slot: &SlotGuard,
    request: &Request,
    path: &Path,
    named: &Named,
    report: &mut Report,
) -> Result<FileText, AppError> {
    if let Ok(meta) = std::fs::metadata(path) {
        report.bytes = meta.len();
    }
    let checked = read::check(path, named, &request.formats)?;
    report.bytes = checked.size;
    let phase = if checked.placeholder {
        Phase::Downloading
    } else {
        Phase::Reading
    };
    env.progress(request, phase, Some(&named.name), None);
    let bytes = read::read_bytes(path, &named.name, READ_TIMEOUT, slot.flag())?;
    if slot.cancelled() {
        return Err(Code::Cancelled.named(&named.name));
    }
    let sha256 = file_ref::sha256_hex(&bytes);
    let file_ref = file_ref::file_ref(&request.extension_id, path);

    env.progress(request, Phase::Extracting, Some(&named.name), None);
    let worker_request = WorkerRequest::new(
        request.formats.clone(),
        named.extension.clone(),
        request.max_chars,
    );
    let out = env
        .extractor
        .extract(&worker_request, &bytes, slot.flag())
        .map_err(|failure| {
            log::info!(
                "File text: the worker didn't read {}'s file: {}",
                request.extension_id,
                failure.detail()
            );
            failure.to_error(&named.name, &named.extension, &request.formats)
        })?;
    if slot.cancelled() {
        return Err(Code::Cancelled.named(&named.name));
    }
    post::build(named, &checked, out, sha256, file_ref, request.max_chars)
}
