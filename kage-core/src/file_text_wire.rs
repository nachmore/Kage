//! Wire types for reading a file the user picks
//! (`docs/design/EXTENSION_FILE_TEXT.md` §5.6).
//!
//! The app and the `kage-file-text` worker both link this module, so the
//! request, the response, the format table and the limits exist once.
//! The two binaries always ship together; `WorkerRequest::v` only guards
//! against a broken install.
//!
//! A request on the worker's stdin is one frame: a `u32` LE header length,
//! the header (`WorkerRequest`) as JSON, a `u64` LE payload length, then
//! the file's bytes. The worker answers with one `WorkerResponse` as JSON
//! on stdout.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::time::Duration;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

// --- Protocol -------------------------------------------------------

/// The header version this build writes and reads.
pub const WIRE_VERSION: u32 = 1;
/// The worker binary's name, without `.exe`. It is looked up next to the
/// running executable only, never on `PATH`.
pub const WORKER_BIN_NAME: &str = "kage-file-text";
/// The only argument the app passes to the worker.
pub const WORKER_FLAG: &str = "--v1";

// --- Limits (§5.7) --------------------------------------------------

/// Largest file Kage reads. Larger files are refused before reading.
/// Also the largest frame payload.
pub const MAX_INPUT_BYTES: u64 = 32 * MIB;
/// Largest frame header (the request JSON).
pub const MAX_HEADER_BYTES: u32 = 4 * KIB as u32;
/// Most characters an extension may ask for.
pub const MAX_CHARS: u32 = 1_000_000;
/// Characters returned when the extension doesn't ask for a number.
pub const DEFAULT_MAX_CHARS: u32 = 100_000;
/// PDF pages read; the rest are skipped (`pages_skipped`).
pub const MAX_PAGES: u32 = 2_000;
/// PPTX slides read; the rest are skipped (`pages_skipped`).
pub const MAX_SLIDES: u32 = 1_000;
/// ZIP entries allowed in a DOCX or PPTX central directory.
pub const MAX_ZIP_ENTRIES: usize = 20_000;
/// One XML part, counted while inflating.
pub const MAX_PART_INFLATED_BYTES: u64 = 64 * MIB;
/// All parts of one document, counted while inflating.
pub const MAX_TOTAL_INFLATED_BYTES: u64 = 256 * MIB;
/// One PDF stream, where the PDF crate lets us cap it.
pub const MAX_PDF_STREAM_BYTES: u64 = 32 * MIB;
/// XML and RTF nesting depth.
pub const MAX_NESTING_DEPTH: usize = 256;
/// Characters kept of a document title.
pub const MAX_TITLE_CHARS: usize = 200;
/// Characters kept of a file name.
pub const MAX_NAME_CHARS: usize = 255;
/// Characters kept of the extension display name the host injects.
pub const MAX_DISPLAY_NAME_CHARS: usize = 64;
/// The worker's counting allocator refuses allocations past this total.
pub const WORKER_ALLOC_CAP_BYTES: u64 = 768 * MIB;
/// Windows: the worker's Job Object process memory limit.
pub const WORKER_JOB_MEMORY_BYTES: u64 = GIB;
/// Linux: the worker's `RLIMIT_AS`.
pub const WORKER_ADDRESS_SPACE_BYTES: u64 = 2 * GIB;
/// Stack of the worker thread that runs the parser.
pub const WORKER_STACK_BYTES: u64 = 64 * MIB;
/// Wall-clock time before the app kills the worker.
pub const WORKER_DEADLINE: Duration = Duration::from_secs(30);
/// Unix: the worker's `RLIMIT_CPU`.
pub const WORKER_CPU_LIMIT: Duration = Duration::from_secs(30);
/// Worker stderr the app keeps (an English debug log, never shown).
pub const WORKER_STDERR_CAP_BYTES: u64 = 16 * KIB;
/// Room on top of `max_chars * 4` in the worker's stdout cap.
pub const WORKER_STDOUT_SLACK_BYTES: u64 = MIB;
/// Time allowed for reading the picked file, for files still downloading
/// from a cloud drive.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Least time between one extension's dialogs.
pub const DIALOG_GAP: Duration = Duration::from_secs(3);

/// The most worker stdout the app reads for a request: four bytes per
/// character (UTF-8 or a two-byte JSON escape) plus room for the rest of
/// the response.
pub const fn worker_stdout_cap(max_chars: u32) -> u64 {
    max_chars as u64 * 4 + WORKER_STDOUT_SLACK_BYTES
}

// --- Formats --------------------------------------------------------

/// Whether this build reads the second-release formats (HTML, PPTX,
/// RTF). WP-FT8 sets it once their parsers have landed.
pub const READS_P1_FORMATS: bool = false;

/// A file format Kage can read. Serialises lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Text,
    Markdown,
    Pdf,
    Docx,
    Html,
    Pptx,
    Rtf,
}

impl Format {
    /// Every format, in table order.
    pub const ALL: [Format; 7] = [
        Format::Text,
        Format::Markdown,
        Format::Pdf,
        Format::Docx,
        Format::Html,
        Format::Pptx,
        Format::Rtf,
    ];

    /// The wire id (`"pdf"`, …).
    pub const fn id(self) -> &'static str {
        match self {
            Format::Text => "text",
            Format::Markdown => "markdown",
            Format::Pdf => "pdf",
            Format::Docx => "docx",
            Format::Html => "html",
            Format::Pptx => "pptx",
            Format::Rtf => "rtf",
        }
    }

    /// The format with this wire id. Exact match only.
    pub fn from_id(id: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.id() == id)
    }

    /// File extensions of this format: lowercase, without the dot.
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Text => &["txt", "text", "log", "csv", "tsv"],
            Format::Markdown => &["md", "markdown", "mdown", "mkd"],
            Format::Pdf => &["pdf"],
            Format::Docx => &["docx"],
            Format::Html => &["html", "htm", "xhtml"],
            Format::Pptx => &["pptx"],
            Format::Rtf => &["rtf"],
        }
    }

    /// The format a file extension (without the dot) belongs to. Ignores
    /// ASCII case. Doesn't check `is_readable`.
    pub fn from_extension(extension: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| {
            f.extensions()
                .iter()
                .any(|e| e.eq_ignore_ascii_case(extension))
        })
    }

    /// A second-release format (HTML, PPTX, RTF).
    pub const fn is_p1(self) -> bool {
        matches!(self, Format::Html | Format::Pptx | Format::Rtf)
    }

    /// Whether this build reads the format.
    pub const fn is_readable(self) -> bool {
        !self.is_p1() || READS_P1_FORMATS
    }
}

/// The formats this build reads, in table order.
pub fn readable_formats() -> impl Iterator<Item = Format> {
    Format::ALL.into_iter().filter(|f| f.is_readable())
}

// --- Result parts ---------------------------------------------------

/// Something the caller should know about the text. Serialises
/// snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    /// More than `MAX_PAGES` pages or `MAX_SLIDES` slides; the rest were
    /// not read.
    PagesSkipped,
    /// Almost no text found (for a PDF: likely a scan).
    NoText,
    /// Some bytes couldn't be decoded and became U+FFFD.
    ReplacedCharacters,
    /// Plain text wasn't UTF-8 or UTF-16; the encoding was guessed.
    LegacyEncoding,
    /// The PDF asks readers not to copy its text.
    Restricted,
    /// Text marked hidden in the document was left out.
    HiddenTextSkipped,
    /// The text looks like it holds a password, token or key. Added by
    /// the app, never by the worker.
    PossibleSecret,
}

/// PDF pages or PPTX slides: how many the document has and how many
/// were read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pages {
    pub count: u32,
    pub read: u32,
}

// --- Request and response -------------------------------------------

/// The frame header the app sends the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRequest {
    /// `WIRE_VERSION`.
    pub v: u32,
    /// The formats the caller accepts.
    pub formats: Vec<Format>,
    /// The file's extension, lowercase without the dot, for detection.
    pub extension: String,
    pub max_chars: u32,
    pub max_pages: u32,
    pub max_slides: u32,
}

impl WorkerRequest {
    /// A request at this build's version and page limits.
    pub fn new(formats: Vec<Format>, extension: impl Into<String>, max_chars: u32) -> Self {
        Self {
            v: WIRE_VERSION,
            formats,
            extension: extension.into(),
            max_chars,
            max_pages: MAX_PAGES,
            max_slides: MAX_SLIDES,
        }
    }
}

/// The worker's one answer, as JSON on stdout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkerResponse {
    Ok {
        /// What the content turned out to be.
        format: Format,
        text: String,
        /// Unicode scalar values in `text`.
        chars: u32,
        /// The document had more text than `max_chars`.
        truncated: bool,
        /// The full length, when the parser knows it.
        #[serde(default)]
        total_chars: Option<u32>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        pages: Option<Pages>,
        /// Char offset in `text` where each read page starts.
        #[serde(default)]
        page_starts: Option<Vec<u32>>,
        #[serde(default)]
        warnings: Vec<Warning>,
    },
    Err {
        code: WorkerErrorCode,
        /// English, for the log only. Never shown or returned.
        detail: String,
    },
}

/// Why the worker couldn't read the bytes. Each maps to the
/// `errors.file_text.<code>` key with the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorCode {
    Unsupported,
    WrongContent,
    Encrypted,
    LegacyFormat,
    NotText,
    Malformed,
    TooComplex,
}

impl WorkerErrorCode {
    /// The wire name, which is also the last segment of the i18n key and
    /// the audit and telemetry outcome.
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkerErrorCode::Unsupported => "unsupported",
            WorkerErrorCode::WrongContent => "wrong_content",
            WorkerErrorCode::Encrypted => "encrypted",
            WorkerErrorCode::LegacyFormat => "legacy_format",
            WorkerErrorCode::NotText => "not_text",
            WorkerErrorCode::Malformed => "malformed",
            WorkerErrorCode::TooComplex => "too_complex",
        }
    }
}

// --- Framing --------------------------------------------------------

fn invalid_data(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// Writes one request frame and flushes. Refuses a header over
/// `MAX_HEADER_BYTES` or a payload over `MAX_INPUT_BYTES`.
pub fn write_request<W: Write>(w: &mut W, req: &WorkerRequest, payload: &[u8]) -> io::Result<()> {
    let header = serde_json::to_vec(req).map_err(io::Error::other)?;
    let header_len = u32::try_from(header.len())
        .ok()
        .filter(|n| *n <= MAX_HEADER_BYTES)
        .ok_or_else(|| invalid_data("file text request header too large"))?;
    let payload_len = payload.len() as u64;
    if payload_len > MAX_INPUT_BYTES {
        return Err(invalid_data("file text payload too large"));
    }
    w.write_all(&header_len.to_le_bytes())?;
    w.write_all(&header)?;
    w.write_all(&payload_len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// Reads one request frame. Checks both lengths before allocating.
/// A header that isn't a `WorkerRequest` is `InvalidData`; the caller
/// checks `v`.
pub fn read_request<R: Read>(r: &mut R) -> io::Result<(WorkerRequest, Vec<u8>)> {
    let mut len4 = [0u8; 4];
    r.read_exact(&mut len4)?;
    let header_len = u32::from_le_bytes(len4);
    if header_len > MAX_HEADER_BYTES {
        return Err(invalid_data("file text request header too large"));
    }
    let mut header = vec![0u8; header_len as usize];
    r.read_exact(&mut header)?;
    let req: WorkerRequest = serde_json::from_slice(&header)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut len8 = [0u8; 8];
    r.read_exact(&mut len8)?;
    let payload_len = u64::from_le_bytes(len8);
    if payload_len > MAX_INPUT_BYTES {
        return Err(invalid_data("file text payload too large"));
    }
    let mut payload = vec![0u8; payload_len as usize];
    r.read_exact(&mut payload)?;
    Ok((req, payload))
}

#[cfg(test)]
mod tests;
