//! The file text worker's parsers (docs/design/EXTENSION_FILE_TEXT.md
//! §5.5–§5.10).
//!
//! [`extract`] turns the app's request and the file's bytes into the
//! worker's one response: check the request, check the bytes against the
//! extension (`sniff`), run the format's reader, then clean the title and
//! the warnings. Every reader feeds its text through
//! [`normalize::Normalizer`], so the §5.10 rules and the character cap
//! hold for every format.
//!
//! The binary (`main.rs`) adds the process limits; this library has none,
//! so tests and fuzzers can call it directly.

pub mod alloc;
pub mod html;
pub mod limits;
pub mod normalize;
pub mod ooxml;
pub mod pdf;
pub mod plain;
pub mod rtf;
pub mod sniff;

pub use kage_core::file_text_wire::{
    Format, Pages, Warning, WorkerErrorCode, WorkerRequest, WorkerResponse,
};
use kage_core::file_text_wire::{MAX_CHARS, MAX_PAGES, MAX_SLIDES, MAX_TITLE_CHARS, WIRE_VERSION};

/// Why a file couldn't be read. `detail` is English, for the app's log
/// only: it never holds text, names or anything else from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: WorkerErrorCode,
    pub detail: String,
}

impl Failure {
    pub fn new(code: WorkerErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl From<Failure> for WorkerResponse {
    fn from(f: Failure) -> Self {
        WorkerResponse::Err {
            code: f.code,
            detail: f.detail,
        }
    }
}

/// One file to read, after the request was checked.
#[derive(Debug, Clone, Copy)]
pub struct Job<'a> {
    /// The format the extension names.
    pub format: Format,
    pub bytes: &'a [u8],
    /// 1..=`MAX_CHARS`.
    pub max_chars: u32,
    /// PDF pages to read, at most `MAX_PAGES`.
    pub max_pages: u32,
    /// PPTX slides to read, at most `MAX_SLIDES`.
    pub max_slides: u32,
}

impl<'a> Job<'a> {
    /// A job with this build's page and slide limits.
    pub fn new(format: Format, bytes: &'a [u8], max_chars: u32) -> Self {
        Self {
            format,
            bytes,
            max_chars: max_chars.clamp(1, MAX_CHARS),
            max_pages: MAX_PAGES,
            max_slides: MAX_SLIDES,
        }
    }
}

/// What a reader found, before the shared clean-up.
#[derive(Debug, Clone)]
pub struct Extracted {
    pub text: normalize::Normalized,
    /// Raw: [`extract`] cleans it and drops it if empty.
    pub title: Option<String>,
    pub pages: Option<Pages>,
    pub warnings: Vec<Warning>,
}

/// Reads a file for a request. Never panics on its own; the binary still
/// runs it inside `catch_unwind` for the parsers' sake.
pub fn extract(req: &WorkerRequest, bytes: &[u8]) -> WorkerResponse {
    match job_for(req, bytes) {
        Ok(job) => extract_as(&job),
        Err(failure) => failure.into(),
    }
}

/// Checks a request. The app checks the same things first; this guards
/// against a broken install.
pub fn job_for<'a>(req: &WorkerRequest, bytes: &'a [u8]) -> Result<Job<'a>, Failure> {
    if req.v != WIRE_VERSION {
        return Err(Failure::new(
            WorkerErrorCode::Malformed,
            format!("wire version {} not supported", req.v),
        ));
    }
    let unsupported = |detail: &str| Failure::new(WorkerErrorCode::Unsupported, detail);
    let format =
        Format::from_extension(&req.extension).ok_or_else(|| unsupported("unknown extension"))?;
    if !req.formats.contains(&format) {
        return Err(unsupported("format not requested"));
    }
    if !format.is_readable() {
        return Err(unsupported("format not read by this build"));
    }
    Ok(Job {
        format,
        bytes,
        max_chars: req.max_chars.clamp(1, MAX_CHARS),
        max_pages: req.max_pages.clamp(1, MAX_PAGES),
        max_slides: req.max_slides.clamp(1, MAX_SLIDES),
    })
}

/// Reads a file as `job.format`, whether or not this build offers that
/// format. Used by [`extract`], and by tests and fuzzers of formats that
/// are built but not switched on yet.
pub fn extract_as(job: &Job<'_>) -> WorkerResponse {
    let result = sniff::check(job.format, job.bytes).and_then(|()| match job.format {
        Format::Text | Format::Markdown => plain::extract(job),
        Format::Pdf => pdf::extract(job),
        Format::Docx | Format::Pptx => ooxml::extract(job),
        Format::Html => html::extract(job),
        Format::Rtf => rtf::extract(job),
    });
    match result {
        Ok(found) => respond(job, found),
        Err(failure) => failure.into(),
    }
}

fn respond(job: &Job<'_>, found: Extracted) -> WorkerResponse {
    let text = found.text;
    debug_assert!(text.chars <= job.max_chars);
    let title = found
        .title
        .map(|t| normalize::clean_line(&t, MAX_TITLE_CHARS))
        .filter(|t| !t.is_empty());
    let mut warnings: Vec<Warning> = Vec::new();
    let no_text = (text.chars == 0).then_some(Warning::NoText);
    for w in found.warnings.into_iter().chain(no_text) {
        // `possible_secret` is the app's call, never the worker's.
        if w != Warning::PossibleSecret && !warnings.contains(&w) {
            warnings.push(w);
        }
    }
    WorkerResponse::Ok {
        format: job.format,
        text: text.text,
        chars: text.chars,
        truncated: text.truncated,
        total_chars: text.total_chars,
        title,
        pages: found.pages,
        page_starts: text.page_starts,
        warnings,
    }
}
