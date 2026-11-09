//! The output side of the DOCX reader: what a paragraph is (plain,
//! heading or list item), whether a run is shown, and how paragraphs,
//! table cells and notes reach the normalizer.

use super::Walker;
use crate::ooxml::styles::{outline_level, Level, Styles};
use crate::ooxml::FLUSH_BYTES;

/// Notes numbered in one document; later references get no number.
const MAX_NOTES: usize = 10_000;
/// Characters of the first heading kept as a title, before cleaning.
const RAW_TITLE_CHARS: usize = 1024;

/// A paragraph being read.
#[derive(Default)]
pub(super) struct Para {
    pub(super) text: String,
    pub(super) style: Option<String>,
    /// Direct `w:numPr`: numbered, or explicitly not (`numId` 0).
    pub(super) num: Option<bool>,
    pub(super) ilvl: u8,
    /// Direct `w:outlineLvl`.
    pub(super) outline: Option<u8>,
    /// Some of its text is already out.
    pub(super) started: bool,
    /// The rest of a paragraph after a text box: no heading or list mark.
    pub(super) plain: bool,
}

#[derive(Default)]
pub(super) struct Run {
    pub(super) vanish: Option<bool>,
    pub(super) style: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Heading(u8),
    List(u8),
}

fn kind_of(styles: &Styles, p: &Para) -> Kind {
    if p.plain {
        return Kind::Plain;
    }
    let style = styles.paragraph_style(p.style.as_deref());
    let heading = match p.outline.map(outline_level) {
        Some(Level::Heading(n)) => Some(n),
        Some(Level::Body) => None,
        None => styles.heading(style),
    };
    if let Some(n) = heading {
        return Kind::Heading(n);
    }
    if p.num.unwrap_or_else(|| styles.numbered(style)) {
        Kind::List(p.ilvl)
    } else {
        Kind::Plain
    }
}

impl Walker<'_, '_> {
    /// The number for a note reference, given on first sight.
    pub(super) fn number(&mut self, endnote: bool, id: String) -> Option<u32> {
        let key = (endnote, id.trim().to_string());
        if let Some(&n) = self.refs.get(&key) {
            return Some(n);
        }
        if self.refs.len() >= MAX_NOTES {
            return None;
        }
        let n = self.refs.len() as u32 + 1;
        self.refs.insert(key, n);
        Some(n)
    }

    /// Text here would be shown: not in a field's code, not hidden.
    pub(super) fn shown(&self) -> bool {
        self.in_code == 0 && !self.run_hidden()
    }

    fn run_hidden(&self) -> bool {
        let Some(run) = self.runs.last() else {
            return false;
        };
        let styles = self.styles;
        let para_style = self.paras.last().and_then(|p| p.style.as_deref());
        run.vanish
            .or_else(|| run.style.as_deref().and_then(|s| styles.vanish(s)))
            .or_else(|| {
                styles
                    .paragraph_style(para_style)
                    .and_then(|s| styles.vanish(s))
            })
            .unwrap_or_else(|| styles.default_vanish())
    }

    /// Text from a run, if it is shown.
    pub(super) fn append(&mut self, text: &str) {
        if self.in_code > 0 {
            return;
        }
        if self.run_hidden() {
            self.hidden |= !text.trim().is_empty();
            return;
        }
        let Some(p) = self.paras.last_mut() else {
            return;
        };
        p.text.push_str(text);
        if p.text.len() >= FLUSH_BYTES {
            self.flush();
        }
    }

    pub(super) fn end_paragraph(&mut self) {
        self.flush();
        self.paras.pop();
        // A field's code never spans paragraphs; one left open is damage.
        // Fields whose result goes on are past their code, so forgetting
        // them changes nothing.
        self.fields.clear();
        self.in_code = 0;
    }

    /// Sends the open paragraph's text out, starting the paragraph first.
    pub(super) fn flush(&mut self) {
        let Some(p) = self.paras.last_mut() else {
            return;
        };
        let text = std::mem::take(&mut p.text);
        if p.started {
            self.out.push(&text);
            return;
        }
        if text.trim().is_empty() {
            return;
        }
        p.started = true;
        let kind = kind_of(self.styles, p);
        // The rest of a paragraph after a text box has no indentation.
        let text = if p.plain { text.trim_start() } else { &text };
        self.start(kind, text);
    }

    fn start(&mut self, kind: Kind, text: &str) {
        if self.tables > 0 {
            if self.cell_has_text {
                self.out.push(" ");
            }
            self.cell_has_text = true;
            self.out.push(text.trim_start());
            return;
        }
        if let Some(n) = self.note_label.take() {
            self.notes_heading();
            self.out.paragraph_break();
            self.out.push(&format!("[{n}] "));
            self.out.push(text.trim_start());
            self.last_list = false;
            return;
        }
        match kind {
            Kind::Plain => {
                self.out.paragraph_break();
                self.out.push(text);
            }
            Kind::Heading(level) => {
                let text = text.trim_start();
                self.out.paragraph_break();
                self.out.push(&format!("{} ", "#".repeat(level.into())));
                self.out.push(text);
                if self.title.is_none() && self.notes.is_none() {
                    self.title = Some(text.chars().take(RAW_TITLE_CHARS).collect());
                }
            }
            Kind::List(level) => {
                if self.last_list {
                    self.out.line_break();
                } else {
                    self.out.paragraph_break();
                }
                self.out.push(&format!("{}- ", "  ".repeat(level.into())));
                self.out.push(text.trim_start());
            }
        }
        self.last_list = matches!(kind, Kind::List(_));
    }

    fn notes_heading(&mut self) {
        if !self.notes_heading {
            self.notes_heading = true;
            self.out.paragraph_break();
            self.out.push("Notes");
        }
    }

    /// A note that starts with a table still gets its number.
    pub(super) fn note_mark(&mut self) {
        if let Some(n) = self.note_label.take() {
            self.notes_heading();
            self.out.paragraph_break();
            self.out.push(&format!("[{n}]"));
        }
    }
}
