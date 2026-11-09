//! DOCX (EXTENSION_FILE_TEXT §3.7, §5.9).
//!
//! The body of the main part (`word/document.xml`), then the footnotes and
//! endnotes it refers to, under `Notes`:
//! - `w:p` is a paragraph, with a blank line after it. Title and
//!   Heading 1–3 (by style, `styles.rs`) start with `#`–`###`. Numbered
//!   and bulleted paragraphs start with `- ` (two spaces more per level),
//!   one line each.
//! - A table is a block of lines: one per `w:tr`, cells split by a tab.
//!   Paragraphs inside a cell, and nested tables, join with spaces.
//! - `w:t` is text, `w:tab` a tab, `w:br` and `w:cr` line breaks. A text
//!   box (`w:txbxContent`) is read where it stands, as paragraphs of its
//!   own. A footnote or endnote reference becomes `[n]`, numbered in the
//!   order they appear; the note follows at the end as `[n] text`.
//! - Left out: deleted and moved-away text (`w:del`, `w:moveFrom`), field
//!   codes (`w:instrText`, and runs between a field's begin and separate),
//!   hidden runs (`w:vanish`, also through styles: `hidden_text_skipped`),
//!   comments, headers and footers (other parts, never read), and every
//!   `mc:AlternateContent` branch but the first, so text isn't doubled.
//!
//! Title: `dc:title`, else the first heading.

mod paragraphs;

use super::package::{secondary, target_of, Package, Rel};
use super::styles::Styles;
use super::xml::{self, Node, Xml};
use crate::normalize::Normalizer;
use crate::sniff::xml_text;
use crate::{Extracted, Failure, Job, Warning, WorkerErrorCode};
use paragraphs::{Para, Run};
use quick_xml::events::BytesStart;
use std::collections::HashMap;

/// Deepest list level that still adds indentation.
const MAX_LIST_INDENT: u8 = 8;

/// Reads a DOCX file.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    let mut pkg = Package::open(job.bytes)?;
    let package_rels = secondary(pkg.rels(""))?.unwrap_or_default();
    let main = Package::main_part(&package_rels, "word/document.xml");
    let rels = secondary(pkg.rels(&main))?.unwrap_or_default();
    let styles = read_styles(&mut pkg, &rels)?;
    let body = pkg
        .read(&main)?
        .ok_or_else(|| Failure::new(WorkerErrorCode::Malformed, "no main document part"))?;

    let mut out = Normalizer::new(job.max_chars);
    let mut walker = Walker::new(&mut out, &styles);
    walker.walk(&xml_text(&body)?, None)?;
    drop(body);
    for (kind, endnote) in [("footnotes", false), ("endnotes", true)] {
        if walker.out.is_full() || !walker.refs.keys().any(|(e, _)| *e == endnote) {
            continue;
        }
        let Some(part) = target_of(&rels, kind) else {
            continue;
        };
        let Some(Some(bytes)) = secondary(pkg.read(part))? else {
            continue;
        };
        if let Some(text) = secondary(xml_text(&bytes))? {
            secondary(walker.walk(&text, Some(endnote)))?;
        }
    }
    let (heading, hidden) = (walker.title.take(), walker.hidden);
    drop(walker);
    let title = pkg.core_title(&package_rels)?.or(heading);
    Ok(Extracted {
        text: out.finish(true),
        title,
        pages: None,
        warnings: hidden
            .then_some(Warning::HiddenTextSkipped)
            .into_iter()
            .collect(),
    })
}

fn read_styles(pkg: &mut Package<'_>, rels: &[Rel]) -> Result<Styles, Failure> {
    let Some(part) = target_of(rels, "styles") else {
        return Ok(Styles::default());
    };
    let Some(Some(bytes)) = secondary(pkg.read(part))? else {
        return Ok(Styles::default());
    };
    let Some(text) = secondary(xml_text(&bytes))? else {
        return Ok(Styles::default());
    };
    Ok(secondary(Styles::parse(&text))?.unwrap_or_default())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tag {
    P,
    PPr,
    PStyle,
    NumPr,
    NumId,
    Ilvl,
    OutlineLvl,
    R,
    RPr,
    RStyle,
    Vanish,
    T,
    Tab,
    Br,
    NoBreakHyphen,
    FldChar,
    Tbl,
    Tr,
    Tc,
    Removed,
    AlternateContent,
    Branch,
    TxbxContent,
    FootnoteReference,
    EndnoteReference,
    Note,
    Other,
}

fn classify(local: &[u8]) -> Tag {
    match local {
        b"p" => Tag::P,
        b"pPr" => Tag::PPr,
        b"pStyle" => Tag::PStyle,
        b"numPr" => Tag::NumPr,
        b"numId" => Tag::NumId,
        b"ilvl" => Tag::Ilvl,
        b"outlineLvl" => Tag::OutlineLvl,
        b"r" => Tag::R,
        b"rPr" => Tag::RPr,
        b"rStyle" => Tag::RStyle,
        b"vanish" => Tag::Vanish,
        b"t" => Tag::T,
        b"tab" | b"ptab" => Tag::Tab,
        b"br" | b"cr" => Tag::Br,
        b"noBreakHyphen" => Tag::NoBreakHyphen,
        b"fldChar" => Tag::FldChar,
        b"tbl" => Tag::Tbl,
        b"tr" => Tag::Tr,
        b"tc" => Tag::Tc,
        b"del" | b"moveFrom" => Tag::Removed,
        b"AlternateContent" => Tag::AlternateContent,
        b"Choice" | b"Fallback" => Tag::Branch,
        b"txbxContent" => Tag::TxbxContent,
        b"footnoteReference" => Tag::FootnoteReference,
        b"endnoteReference" => Tag::EndnoteReference,
        b"footnote" | b"endnote" => Tag::Note,
        _ => Tag::Other,
    }
}

struct Walker<'o, 's> {
    out: &'o mut Normalizer,
    styles: &'s Styles,
    paras: Vec<Para>,
    runs: Vec<Run>,
    /// Complex fields begun in this paragraph: whether each is still in
    /// its code part. A field's code never spans paragraphs, so the stack
    /// is cleared at each paragraph's end.
    fields: Vec<bool>,
    /// How many of `fields` are in their code part.
    in_code: usize,
    /// Open `mc:AlternateContent`s: whether a branch was taken.
    alt: Vec<bool>,
    tables: usize,
    cells: usize,
    cell_has_text: bool,
    last_list: bool,
    hidden: bool,
    title: Option<String>,
    /// (endnote, id) → the number shown for it.
    refs: HashMap<(bool, String), u32>,
    /// Reading a notes part: `Some(endnote)`.
    notes: Option<bool>,
    /// The number of the note being read, until its first paragraph.
    note_label: Option<u32>,
    notes_heading: bool,
}

impl<'o, 's> Walker<'o, 's> {
    fn new(out: &'o mut Normalizer, styles: &'s Styles) -> Self {
        Self {
            out,
            styles,
            paras: Vec::new(),
            runs: Vec::new(),
            fields: Vec::new(),
            in_code: 0,
            alt: Vec::new(),
            tables: 0,
            cells: 0,
            cell_has_text: false,
            last_list: false,
            hidden: false,
            title: None,
            refs: HashMap::new(),
            notes: None,
            note_label: None,
            notes_heading: false,
        }
    }

    /// Reads one part: the body (`notes` is `None`) or a notes part.
    fn walk(&mut self, text: &str, notes: Option<bool>) -> Result<(), Failure> {
        self.paras.clear();
        self.runs.clear();
        self.fields.clear();
        self.in_code = 0;
        self.alt.clear();
        self.tables = 0;
        self.notes = notes;
        self.note_label = None;
        let mut xml = Xml::new(text, classify);
        while !self.out.is_full() {
            match xml.next()? {
                Node::Open(tag, e) => self.open(&mut xml, tag, &e)?,
                Node::Close(tag) => self.close(tag),
                Node::Text(t) => {
                    if xml.up(0) == Some(Tag::T) && xml.up(1) == Some(Tag::R) {
                        let tab = if self.tables > 0 { ' ' } else { '\t' };
                        let text: String = t
                            .chars()
                            .map(|c| match c {
                                '\n' | '\r' => ' ',
                                '\t' => tab,
                                c => c,
                            })
                            .collect();
                        self.append(&text);
                    }
                }
                Node::Eof => break,
            }
        }
        // A part cut off mid-paragraph still gives its text.
        while !self.paras.is_empty() {
            self.end_paragraph();
        }
        Ok(())
    }

    fn open(
        &mut self,
        xml: &mut Xml<'_, Tag>,
        tag: Tag,
        e: &BytesStart<'_>,
    ) -> Result<(), Failure> {
        use Tag::*;
        let in_run = xml.up(1) == Some(R);
        match tag {
            Removed => return xml.skip(),
            AlternateContent => self.alt.push(false),
            Branch if xml.up(1) == Some(AlternateContent) => match self.alt.last_mut() {
                Some(taken) if !*taken => *taken = true,
                _ => return xml.skip(),
            },
            P => self.paras.push(Para::default()),
            PStyle if xml.at(&[P, PPr, PStyle]) => {
                if let Some(p) = self.paras.last_mut() {
                    p.style = xml::attr(e, b"val");
                }
            }
            NumId if xml.at(&[P, PPr, NumPr, NumId]) => {
                if let Some(p) = self.paras.last_mut() {
                    p.num = xml::attr(e, b"val").map(|v| v.trim() != "0");
                }
            }
            Ilvl if xml.at(&[P, PPr, NumPr, Ilvl]) => {
                if let Some(p) = self.paras.last_mut() {
                    let level = xml::attr(e, b"val").and_then(|v| v.trim().parse().ok());
                    p.ilvl = level.unwrap_or(0).min(MAX_LIST_INDENT);
                }
            }
            OutlineLvl if xml.at(&[P, PPr, OutlineLvl]) => {
                if let Some(p) = self.paras.last_mut() {
                    p.outline = xml::attr(e, b"val").and_then(|v| v.trim().parse().ok());
                }
            }
            R => self.runs.push(Run::default()),
            RStyle if xml.at(&[R, RPr, RStyle]) => {
                if let Some(r) = self.runs.last_mut() {
                    r.style = xml::attr(e, b"val");
                }
            }
            Vanish if xml.at(&[R, RPr, Vanish]) => {
                if let Some(r) = self.runs.last_mut() {
                    r.vanish = Some(xml::val_on(e));
                }
            }
            Tab if in_run => self.append(if self.tables > 0 { " " } else { "\t" }),
            Br if in_run => self.append(if self.tables > 0 { " " } else { "\n" }),
            NoBreakHyphen if in_run => self.append("-"),
            FldChar if in_run => match xml::attr(e, b"fldCharType").as_deref() {
                Some("begin") => {
                    self.fields.push(true);
                    self.in_code += 1;
                }
                Some("separate") => {
                    if let Some(code) = self.fields.last_mut() {
                        self.in_code -= usize::from(*code);
                        *code = false;
                    }
                }
                Some("end") => {
                    let was_code = self.fields.pop() == Some(true);
                    self.in_code -= usize::from(was_code);
                }
                _ => {}
            },
            FootnoteReference | EndnoteReference
                if in_run && self.notes.is_none() && self.shown() =>
            {
                if let Some(n) =
                    xml::attr(e, b"id").and_then(|id| self.number(tag == EndnoteReference, id))
                {
                    self.append(&format!("[{n}]"));
                }
            }
            Tbl => {
                if self.tables == 0 {
                    self.note_mark();
                    self.out.paragraph_break();
                    self.last_list = false;
                }
                self.tables += 1;
            }
            Tr if self.tables == 1 => {
                self.out.line_break();
                self.cells = 0;
            }
            Tc if self.tables == 1 => {
                if self.cells > 0 {
                    self.out.push("\t");
                }
                self.cell_has_text = false;
            }
            TxbxContent => {
                self.flush();
                if let Some(p) = self.paras.last_mut() {
                    p.started = false;
                    p.plain = true;
                }
            }
            Note if xml.depth() == 2 => {
                let Some(endnote) = self.notes else {
                    return Ok(());
                };
                let separator = matches!(
                    xml::attr(e, b"type").as_deref(),
                    Some("separator" | "continuationSeparator" | "continuationNotice")
                );
                let id = xml::attr(e, b"id").unwrap_or_default();
                match self.refs.get(&(endnote, id.trim().to_string())) {
                    Some(&n) if !separator => self.note_label = Some(n),
                    _ => return xml.skip(),
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn close(&mut self, tag: Tag) {
        match tag {
            Tag::P => self.end_paragraph(),
            Tag::R => {
                self.runs.pop();
            }
            Tag::Tbl => {
                self.tables = self.tables.saturating_sub(1);
                if self.tables == 0 {
                    self.out.paragraph_break();
                    self.last_list = false;
                }
            }
            Tag::Tc if self.tables == 1 => self.cells += 1,
            Tag::AlternateContent => {
                self.alt.pop();
            }
            Tag::Note => self.note_label = None,
            _ => {}
        }
    }
}
