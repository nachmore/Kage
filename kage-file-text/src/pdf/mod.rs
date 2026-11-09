//! PDF (EXTENSION_FILE_TEXT §3.7, §5.9, §6.2).
//!
//! `pdf-extract` 0.12.1 on `lopdf` 0.42, set up as WP-FT0 chose:
//!
//! 1. Load with a stream cap; repair through `lopdf` 0.47 if 0.42 can't
//!    load the file; refuse a file that needs a user password
//!    (`load.rs`, `streams.rs`).
//! 2. Walk the page tree once; settle each page's inherited resources and
//!    media box; read one page at a time through the root `/Pages` node,
//!    inside `catch_unwind` (`pages.rs`).
//! 3. Before a page, check what it draws: forms that draw themselves,
//!    nest too deep or cost too much are emptied (`guard.rs`).
//! 4. Fonts: CJK CID fonts without a ToUnicode map get one from Adobe's
//!    tables, filled before each page with the CIDs it shows; font
//!    streams the parsers can't take are dropped (`fonts.rs`).
//! 5. Text goes through a capped writer, then the ligature, presentation
//!    form, Kangxi and right-to-left fixes, then the shared normaliser,
//!    one page per `push` (`text.rs`).
//!
//! Pages are read in order until `max_pages` or until the text is full.
//! A blank line separates pages; `page_starts` records where each starts.
//! Warnings: `pages_skipped` past `max_pages`, `restricted` when the
//! file forbids copying, `no_text` below 16 characters a page. The title
//! is the document information's `/Title`.

mod cid_tables;
mod fonts;
mod guard;
mod load;
mod pages;
mod scan;
mod streams;
mod text;

use crate::normalize::Normalizer;
use crate::{Extracted, Failure, Job, WorkerErrorCode};
use kage_core::file_text_wire::{Pages, Warning};
use pdf_extract::{Document, Object};

/// Fewer characters than this per page read, on average: `no_text`.
const MIN_CHARS_PER_PAGE: u64 = 16;

/// Reads a PDF file.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    let loaded = load::load(job.bytes)?;
    let mut doc = loaded.doc;
    let title = info_title(&doc);
    let tree = pages::collect(&mut doc)?;
    fonts::drop_unusable(&mut doc);
    let cjk = fonts::add_cjk_maps(&mut doc);
    let cjk_fonts = cjk.fonts();

    let count = tree.pages.len();
    let max_chars = job.max_chars.max(1) as usize;
    let mut normalizer = Normalizer::new(job.max_chars);
    let mut guard = guard::Guard::new(!cjk.is_empty());
    let (mut read, mut lost) = (0usize, 0usize);
    let mut cut = false;
    for &page in tree.pages.iter().take(job.max_pages as usize) {
        if normalizer.is_full() {
            break;
        }
        read += 1;
        normalizer.start_page();
        if !guard.check(&mut doc, page) {
            lost += 1;
            continue;
        }
        if !cjk.is_empty() {
            let shown = guard.codes(&doc, page, &cjk_fonts);
            cjk.fit(&mut doc, &shown);
        }
        let wanted = max_chars.saturating_sub(normalizer.chars());
        let rendered = pages::render(&mut doc, &tree, page, wanted);
        lost += usize::from(rendered.panicked || rendered.failed || rendered.text.overflowed);
        let full = rendered.text.full;
        normalizer.push(&text::fix_page(&rendered.text.into_text()));
        if full && !normalizer.is_full() {
            // Can't happen (see text.rs); if it did, the page was cut.
            cut = true;
            break;
        }
    }

    if normalizer.chars() == 0 && lost == read {
        return Err(Failure::new(
            WorkerErrorCode::Malformed,
            "every page failed to draw",
        ));
    }
    let complete = read == count && lost == 0;
    let mut text = normalizer.finish(complete);
    if cut {
        text.truncated = true;
        text.total_chars = None;
    }

    let mut warnings = Vec::new();
    let max_pages = job.max_pages as usize;
    if count > max_pages && read >= max_pages {
        warnings.push(Warning::PagesSkipped);
    }
    if loaded.restricted {
        warnings.push(Warning::Restricted);
    }
    if !text.truncated && (text.chars as u64) < MIN_CHARS_PER_PAGE * read.max(1) as u64 {
        warnings.push(Warning::NoText);
    }
    Ok(Extracted {
        text,
        title,
        pages: Some(Pages {
            count: saturate(count),
            read: saturate(read),
        }),
        warnings,
    })
}

fn saturate(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Runs a `lopdf` or `pdf-extract` call that may panic on a broken file.
/// `None` if it did.
fn caught<T>(f: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok()
}

/// The document information's `/Title`, raw (the caller cleans it).
fn info_title(doc: &Document) -> Option<String> {
    fn resolve<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
        doc.dereference(o).ok().map(|(_, o)| o)
    }
    let info = resolve(doc, doc.trailer.get(b"Info").ok()?)?
        .as_dict()
        .ok()?;
    let title = resolve(doc, info.get(b"Title").ok()?)?;
    let bytes = title.as_str().ok()?;
    match bytes {
        [0xFE, 0xFF, rest @ ..] => {
            let units: Vec<u16> = rest
                .chunks(2)
                .map(|c| u16::from_be_bytes([c[0], c.get(1).copied().unwrap_or(0)]))
                .collect();
            Some(String::from_utf16_lossy(&units))
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => Some(String::from_utf8_lossy(rest).into_owned()),
        _ => {
            // PDFDocEncoding: `lopdf`'s table drops tabs and line breaks,
            // which separate words in a title.
            let spaced: Vec<u8> = bytes
                .iter()
                .map(|&b| if b"\t\n\x0C\r".contains(&b) { b' ' } else { b })
                .collect();
            let spaced = Object::String(spaced, pdf_extract::StringFormat::Literal);
            pdf_extract::decode_text_string(&spaced).ok()
        }
    }
}
