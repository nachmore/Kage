//! What one page draws (an addition to EXTENSION_FILE_TEXT §6.2).
//!
//! `pdf-extract` draws a Form XObject by recursing into its content, with
//! no depth limit and no memory of what it drew. A form that draws itself
//! overflows the stack; ten forms that each draw the next one twice draw
//! the last one a thousand times; a page can list one content stream a
//! thousand times. Before a page is drawn, [`Guard::check`] works out
//! what drawing it would cost:
//!
//! - A form that would draw one of the forms it is inside of, or that
//!   sits more than `MAX_FORM_DEPTH` forms deep, is emptied.
//! - A form whose drawing (with the forms inside it) would cost more than
//!   `MAX_WORK` is emptied.
//! - A page that would still cost more than `MAX_WORK` is skipped.
//!
//! What a form draws comes from `lopdf`'s content parser, the one
//! `pdf-extract` uses, so the two can't disagree about a `Do`. For a
//! page, the light lexer in `scan.rs` counts the calls, and every XObject
//! in the page's resources counts as drawn at least once, so a call the
//! lexer misses still gets its form checked.
//!
//! Cost is content bytes plus `DRAW_COST` per stream drawn. Emptied forms
//! stay empty for the rest of the document. All of this happens only in
//! hostile or broken files.
//!
//! [`Guard::codes`] walks the same page and forms to find the codes each
//! of some fonts shows on the page (for the CJK maps in `fonts.rs`).

use super::scan::{self, Scan};
use super::{caught, streams};
use kage_core::file_text_wire::MAX_PDF_STREAM_BYTES;
use pdf_extract::content::Content as Operations;
use pdf_extract::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Forms inside forms that `pdf-extract` may draw.
const MAX_FORM_DEPTH: usize = 32;
/// Content bytes one page (or one form, with what it draws) may cost.
const MAX_WORK: u64 = 2 * MAX_PDF_STREAM_BYTES;
/// What drawing a stream costs on top of its bytes.
const DRAW_COST: u64 = 256;

/// Where a form finds the names it draws: a resource dictionary that is
/// its own object, or one inside a page or form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Resources {
    Object(ObjectId),
    InsideOf(ObjectId),
}

type State = (ObjectId, Resources);
type Calls = Vec<(Vec<u8>, u64)>;

/// A stream's size and what it draws and shows.
struct Content {
    len: u64,
    /// The lexer's view: calls and codes.
    scan: Scan,
    /// What `lopdf` sees it draw, once asked for.
    drawn: Option<Calls>,
}

/// What the guard knows across the pages of one document.
#[derive(Default)]
pub struct Guard {
    /// Record the codes each font shows (for [`Guard::codes`]).
    with_codes: bool,
    contents: HashMap<ObjectId, Content>,
    costs: HashMap<State, u64>,
    emptied: HashSet<ObjectId>,
    pending: Vec<ObjectId>,
}

impl Guard {
    /// A guard; `with_codes` if [`Guard::codes`] will be asked.
    pub fn new(with_codes: bool) -> Self {
        Self {
            with_codes,
            ..Self::default()
        }
    }

    /// Whether `page` can be drawn. Forms that need emptying are emptied.
    pub fn check(&mut self, doc: &mut Document, page: ObjectId) -> bool {
        let fits = self.page_cost(doc, page) <= MAX_WORK;
        for form in std::mem::take(&mut self.pending) {
            if let Ok(Object::Stream(stream)) = doc.get_object_mut(form) {
                streams::replace(stream, Vec::new());
            }
        }
        fits
    }

    fn page_cost(&mut self, doc: &Document, page: ObjectId) -> u64 {
        let Some(resources) = page_resources(doc, page) else {
            return 0;
        };
        let mut cost = 0u64;
        for id in doc.get_page_contents(page) {
            cost = cost.saturating_add(DRAW_COST + self.content(doc, id).len);
        }
        let mut stack = Vec::new();
        for (name, n) in self.page_calls(doc, page, resources) {
            let Some(child) = self.child(doc, resources, &name) else {
                continue;
            };
            let child_cost = self.form_cost(doc, child, &mut stack);
            cost = cost.saturating_add(n.saturating_mul(child_cost));
        }
        cost
    }

    /// What a page draws: the lexer's counts, and at least once every
    /// XObject its resources name.
    ///
    /// The names the page draws come first, sorted, and then the other
    /// names in the resources. The order decides where a loop between
    /// forms is cut: walking from a form the page draws empties a form
    /// further in, so the page keeps the text of the form it draws. A
    /// hash map's order would make that a coin toss.
    fn page_calls(&mut self, doc: &Document, page: ObjectId, resources: Resources) -> Calls {
        let mut counts: HashMap<Vec<u8>, u64> = HashMap::new();
        for id in doc.get_page_contents(page) {
            for (name, n) in &self.content(doc, id).scan.calls {
                *counts.entry(name.clone()).or_default() += n;
            }
        }
        let mut calls: Calls = counts.into_iter().collect();
        calls.sort();
        if let Some(xobjects) = resource_dict(doc, resources)
            .and_then(|d| d.get(b"XObject").ok())
            .and_then(|x| deref_dict(doc, x))
        {
            let drawn: HashSet<Vec<u8>> = calls.iter().map(|(name, _)| name.clone()).collect();
            for (name, _) in xobjects.iter() {
                if !drawn.contains(name) {
                    calls.push((name.clone(), 1));
                }
            }
        }
        calls
    }

    /// What drawing a form costs. Empties it if it can't be drawn safely.
    fn form_cost(&mut self, doc: &Document, state: State, stack: &mut Vec<State>) -> u64 {
        let (form, resources) = state;
        if self.emptied.contains(&form) {
            return DRAW_COST;
        }
        if let Some(&cost) = self.costs.get(&state) {
            return cost;
        }
        let mut cost = DRAW_COST + self.content(doc, form).len;
        let calls = self.drawn(doc, form);
        stack.push(state);
        for (name, n) in calls {
            let Some(child) = self.child(doc, resources, &name) else {
                continue;
            };
            if stack.contains(&child) || stack.len() >= MAX_FORM_DEPTH {
                cost = self.empty(form);
                break;
            }
            let child_cost = self.form_cost(doc, child, stack);
            cost = cost.saturating_add(n.saturating_mul(child_cost));
            if cost > MAX_WORK {
                cost = self.empty(form);
                break;
            }
        }
        stack.pop();
        self.costs.insert(state, cost);
        cost
    }

    fn empty(&mut self, form: ObjectId) -> u64 {
        if self.emptied.insert(form) {
            self.pending.push(form);
        }
        DRAW_COST
    }

    /// The codes each of `fonts` shows on `page`, in its content and the
    /// forms it draws. A font name counts for every font it names on the
    /// page: `pdf-extract` keeps the first font it meets under a name for
    /// the whole page.
    pub fn codes(
        &mut self,
        doc: &Document,
        page: ObjectId,
        fonts: &HashSet<ObjectId>,
    ) -> HashMap<ObjectId, BTreeSet<u16>> {
        let Some(resources) = page_resources(doc, page) else {
            return HashMap::new();
        };
        let mut by_name: HashMap<Vec<u8>, BTreeSet<u16>> = HashMap::new();
        let mut named: HashMap<Vec<u8>, HashSet<ObjectId>> = HashMap::new();
        let mut seen: HashSet<State> = HashSet::new();
        let page_calls = self.page_calls(doc, page, resources);
        let mut todo: Vec<(Vec<ObjectId>, Calls, Resources)> =
            vec![(doc.get_page_contents(page), page_calls, resources)];
        while let Some((ids, calls, resources)) = todo.pop() {
            for id in ids {
                let shown = self.content(doc, id).scan.shown.clone();
                for (name, codes) in shown {
                    if let Some(font) = font_named(doc, resources, &name, fonts) {
                        named.entry(name.clone()).or_default().insert(font);
                    }
                    by_name.entry(name).or_default().extend(codes);
                }
            }
            for (name, _) in calls {
                let Some(child) = self.child(doc, resources, &name) else {
                    continue;
                };
                if !self.emptied.contains(&child.0) && seen.insert(child) {
                    let drawn = self.drawn(doc, child.0);
                    todo.push((vec![child.0], drawn, child.1));
                }
            }
        }
        let mut out: HashMap<ObjectId, BTreeSet<u16>> = HashMap::new();
        for (name, fonts) in named {
            for font in fonts {
                out.entry(font)
                    .or_default()
                    .extend(by_name.get(&name).into_iter().flatten());
            }
        }
        out
    }

    fn content(&mut self, doc: &Document, id: ObjectId) -> &mut Content {
        let with_codes = self.with_codes;
        self.contents.entry(id).or_insert_with(|| {
            let (len, scan) = match decoded(doc, id) {
                Some(Some(bytes)) => (bytes.len() as u64, scan::content(&bytes, with_codes)),
                // Over the cap (a filter the cap can't count): it costs
                // the most a stream may.
                Some(None) => (MAX_PDF_STREAM_BYTES, Scan::default()),
                None => (0, Scan::default()),
            };
            Content {
                len,
                scan,
                drawn: None,
            }
        })
    }

    /// The `Do` calls `lopdf` parses in a form, with counts.
    fn drawn(&mut self, doc: &Document, form: ObjectId) -> Calls {
        if let Some(calls) = &self.content(doc, form).drawn {
            return calls.clone();
        }
        let mut counts: HashMap<Vec<u8>, u64> = HashMap::new();
        let operations = decoded(doc, form)
            .flatten()
            .and_then(|bytes| caught(|| Operations::decode(&bytes)))
            .and_then(Result::ok);
        for op in operations.iter().flat_map(|c| c.operations.iter()) {
            if op.operator == "Do" {
                if let Some(Ok(name)) = op.operands.first().map(Object::as_name) {
                    *counts.entry(name.to_vec()).or_default() += 1;
                }
            }
        }
        // Sorted, so a loop below this form is cut in the same place on
        // every run (see `page_calls`).
        let mut calls: Calls = counts.into_iter().collect();
        calls.sort();
        self.content(doc, form).drawn = Some(calls.clone());
        calls
    }

    /// The form `name` names in `resources`, and where it finds names.
    fn child(&self, doc: &Document, resources: Resources, name: &[u8]) -> Option<State> {
        let dict = resource_dict(doc, resources)?;
        let xobjects = deref_dict(doc, dict.get(b"XObject").ok()?)?;
        let (id, object) = doc.dereference(xobjects.get(name).ok()?).ok()?;
        let id = id?;
        let stream = object.as_stream().ok()?;
        let own = match stream.dict.get(b"Resources") {
            Ok(Object::Reference(r)) if doc.get_dictionary(*r).is_ok() => Resources::Object(*r),
            Ok(Object::Dictionary(_)) => Resources::InsideOf(id),
            _ => resources,
        };
        Some((id, own))
    }
}

/// A stream's decoded data: `None` if it isn't a stream, `Some(None)` if
/// it decodes past the cap.
fn decoded(doc: &Document, id: ObjectId) -> Option<Option<Vec<u8>>> {
    let stream = doc.get_object(id).and_then(Object::as_stream).ok()?;
    Some(streams::decoded(stream, MAX_PDF_STREAM_BYTES as usize))
}

/// The font `name` names in `resources`, if it is one of `fonts`.
fn font_named(
    doc: &Document,
    resources: Resources,
    name: &[u8],
    fonts: &HashSet<ObjectId>,
) -> Option<ObjectId> {
    let dict = resource_dict(doc, resources)?;
    let font_dict = deref_dict(doc, dict.get(b"Font").ok()?)?;
    let (id, _) = doc.dereference(font_dict.get(name).ok()?).ok()?;
    id.filter(|id| fonts.contains(id))
}

/// A page's resources (`pages.rs` has put them on the page itself).
fn page_resources(doc: &Document, page: ObjectId) -> Option<Resources> {
    match doc.get_dictionary(page).ok()?.get(b"Resources").ok()? {
        Object::Reference(r) => Some(Resources::Object(*r)),
        Object::Dictionary(_) => Some(Resources::InsideOf(page)),
        _ => None,
    }
}

fn resource_dict(doc: &Document, resources: Resources) -> Option<&Dictionary> {
    match resources {
        Resources::Object(id) => doc.get_dictionary(id).ok(),
        Resources::InsideOf(owner) => {
            let dict = match doc.get_object(owner).ok()? {
                Object::Stream(s) => &s.dict,
                Object::Dictionary(d) => d,
                _ => return None,
            };
            dict.get(b"Resources").ok()?.as_dict().ok()
        }
    }
}

fn deref_dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    match object {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        other => other.as_dict().ok(),
    }
}
