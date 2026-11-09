//! The page tree, one page at a time (EXTENSION_FILE_TEXT §6.2 steps 2
//! and 3).
//!
//! `pdf-extract`'s `output_doc_page` walks the whole page tree for every
//! page, so 2,000 pages of a 20,000-page file would take seconds. Instead
//! the tree is walked once ([`collect`]); before each page the root
//! `/Pages` node is pointed at that page alone, and `output_doc_page`
//! reads page 1 ([`render`]).
//!
//! Each page is settled first: its `/Resources` and `/MediaBox` are
//! looked up through `/Parent` (with a visited set, at most
//! `MAX_PARENT_STEPS` steps) and set on the page itself. `pdf-extract`
//! then never walks `/Parent` (which loops on a cycle), and a page with
//! no media box, or a broken one, gets US Letter instead of a panic. A
//! broken `/ArtBox` is dropped for the same reason.
//!
//! `render` runs `pdf-extract` inside `catch_unwind`: a panic loses only
//! the rest of that page.

use super::text::PageText;
use crate::{Failure, WorkerErrorCode};
use pdf_extract::{Dictionary, Document, Object, ObjectId, PlainTextOutput};
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;

const MAX_PARENT_STEPS: usize = 64;
const US_LETTER: [f32; 4] = [0.0, 0.0, 612.0, 792.0];

/// The pages to read, in order.
pub struct PageTree {
    root: ObjectId,
    pub pages: Vec<ObjectId>,
}

/// Walks the page tree once and settles every page.
pub fn collect(doc: &mut Document) -> Result<PageTree, Failure> {
    let malformed = |detail: &str| Failure::new(WorkerErrorCode::Malformed, detail);
    let root = doc
        .catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|_| malformed("no page tree"))?;
    let mut seen = HashSet::new();
    let pages: Vec<ObjectId> = doc
        .page_iter()
        .filter(|&id| id != root && seen.insert(id))
        .collect();
    if pages.is_empty() {
        return Err(malformed("no pages"));
    }
    for &page in &pages {
        settle(doc, page);
    }
    Ok(PageTree { root, pages })
}

fn settle(doc: &mut Document, page: ObjectId) {
    let (resources, media_box) = inherited(doc, page);
    let art_box = doc
        .get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"ArtBox").ok())
        .map(|a| rectangle(doc, a));
    let Ok(dict) = doc.get_dictionary_mut(page) else {
        return;
    };
    dict.set(
        "Resources",
        resources.unwrap_or_else(|| Object::Dictionary(Dictionary::new())),
    );
    dict.set(
        "MediaBox",
        media_box.unwrap_or_else(|| box_object(US_LETTER)),
    );
    match art_box {
        Some(Some(art)) => dict.set("ArtBox", art),
        Some(None) => {
            dict.remove(b"ArtBox");
        }
        None => {}
    }
}

/// The page's resources and media box, from the page or its ancestors.
fn inherited(doc: &Document, page: ObjectId) -> (Option<Object>, Option<Object>) {
    let mut resources = None;
    let mut media_box = None;
    let mut seen = HashSet::new();
    let mut node = page;
    for _ in 0..MAX_PARENT_STEPS {
        if !seen.insert(node) {
            break;
        }
        let Ok(dict) = doc.get_dictionary(node) else {
            break;
        };
        if resources.is_none() {
            resources = dict
                .get(b"Resources")
                .ok()
                .filter(|r| is_dictionary(doc, r))
                .cloned();
        }
        if media_box.is_none() {
            media_box = dict.get(b"MediaBox").ok().and_then(|m| rectangle(doc, m));
        }
        if resources.is_some() && media_box.is_some() {
            break;
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => node = parent,
            Err(_) => break,
        }
    }
    (resources, media_box)
}

/// What `pdf-extract` takes for a dictionary: one, or a reference to one.
pub fn is_dictionary(doc: &Document, object: &Object) -> bool {
    match object {
        Object::Dictionary(_) => true,
        Object::Reference(id) => doc.get_dictionary(*id).is_ok(),
        _ => false,
    }
}

/// A clean four-number rectangle, if `object` is (or names) an array
/// that starts with four finite numbers.
fn rectangle(doc: &Document, object: &Object) -> Option<Object> {
    let array = match object {
        Object::Reference(id) => doc.get_object(*id).ok()?.as_array().ok()?,
        other => other.as_array().ok()?,
    };
    let mut numbers = [0f32; 4];
    for (slot, value) in numbers.iter_mut().zip(array.iter()) {
        *slot = match value {
            Object::Integer(i) => *i as f32,
            Object::Real(r) => *r,
            _ => return None,
        };
        if !slot.is_finite() {
            return None;
        }
    }
    (array.len() >= 4).then(|| box_object(numbers))
}

fn box_object(numbers: [f32; 4]) -> Object {
    Object::Array(numbers.into_iter().map(Object::Real).collect())
}

/// How drawing one page went.
pub struct Rendered {
    pub text: PageText,
    /// `pdf-extract` panicked; the text up to then is kept.
    pub panicked: bool,
    /// It failed in some other way, before or during the page.
    pub failed: bool,
}

/// Draws one page into a writer that wants `wanted` more characters.
pub fn render(doc: &mut Document, tree: &PageTree, page: ObjectId, wanted: usize) -> Rendered {
    let mut text = PageText::new(wanted);
    let Ok(root) = doc.get_dictionary_mut(tree.root) else {
        return Rendered {
            text,
            panicked: false,
            failed: true,
        };
    };
    root.set("Kids", vec![Object::Reference(page)]);
    root.set("Count", 1);
    let doc: &Document = doc;
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut output = PlainTextOutput::new(&mut text);
        pdf_extract::output_doc_page(doc, &mut output, 1).is_ok()
    }));
    let panicked = outcome.is_err();
    let failed = matches!(outcome, Ok(false)) && !text.refused();
    Rendered {
        text,
        panicked,
        failed,
    }
}
