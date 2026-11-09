//! One walk over a slide or notes slide (EXTENSION_FILE_TEXT §3.7).
//!
//! A slide is walked twice: the `Title` pass writes the heading (the
//! first visible title placeholder, which may be drawn after other
//! shapes), the `Body` pass every other shape. Both passes meet the text
//! bodies in the same order, so they agree on which one is the heading.
//! A notes slide gets one `Notes` pass; a hidden slide one `Hidden` pass,
//! which writes nothing and only looks for text.

use crate::normalize::Normalizer;
use crate::ooxml::xml::{self, Node, Xml};
use crate::ooxml::FLUSH_BYTES;
use crate::Failure;
use quick_xml::events::BytesStart;

/// Characters of a slide title kept as the document title, before cleaning.
const RAW_TITLE_CHARS: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tag {
    Shape,
    CNvPr,
    Ph,
    TxBody,
    P,
    Run,
    T,
    Br,
    Tbl,
    Tr,
    Tc,
    AlternateContent,
    Branch,
    Other,
}

fn classify(local: &[u8]) -> Tag {
    match local {
        b"sp" | b"grpSp" | b"graphicFrame" | b"cxnSp" | b"pic" | b"contentPart" => Tag::Shape,
        b"cNvPr" => Tag::CNvPr,
        b"ph" => Tag::Ph,
        b"txBody" => Tag::TxBody,
        b"p" => Tag::P,
        b"r" | b"fld" => Tag::Run,
        b"t" => Tag::T,
        b"br" => Tag::Br,
        b"tbl" => Tag::Tbl,
        b"tr" => Tag::Tr,
        b"tc" => Tag::Tc,
        b"AlternateContent" => Tag::AlternateContent,
        b"Choice" | b"Fallback" => Tag::Branch,
        _ => Tag::Other,
    }
}

/// What one walk over a slide part writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Pass {
    /// The heading: the first visible title placeholder only.
    Title,
    /// Every other shape.
    Body,
    /// A notes slide's body placeholder.
    Notes,
    /// A hidden slide: nothing is written; only looks for text.
    Hidden,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ph {
    Title,
    Body,
    /// Slide number, date, header, footer.
    Furniture,
    Other,
}

fn placeholder(kind: Option<&str>) -> Ph {
    match kind.map(str::trim) {
        Some("title" | "ctrTitle") => Ph::Title,
        Some("body") => Ph::Body,
        Some("sldNum" | "dt" | "ftr" | "hdr") => Ph::Furniture,
        _ => Ph::Other,
    }
}

#[derive(Clone, Copy, Default)]
struct Shape {
    hidden: bool,
    ph: Option<Ph>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Take {
    Write,
    /// Hidden: read, but only to notice it held text.
    Discard,
}

/// What a walk found besides the text it wrote.
pub(super) struct Found {
    /// The heading (Title pass) or the `Notes:` label (Notes pass) was
    /// written.
    pub(super) started: bool,
    /// Title pass: the heading's first characters.
    pub(super) title: String,
    pub(super) hidden: bool,
}

pub(super) struct Slide<'o> {
    out: &'o mut Normalizer,
    pass: Pass,
    shapes: Vec<Shape>,
    /// The first visible title placeholder has been met.
    title_seen: bool,
    /// The text body or table being read, and its depth.
    take: Option<(Take, usize)>,
    para: String,
    /// The open `a:p` already has text out.
    para_open: bool,
    /// The current shape already has text out (Body pass).
    block_open: bool,
    tables: usize,
    cells: usize,
    cell_has_text: bool,
    alt: Vec<bool>,
    found: Found,
}

impl<'o> Slide<'o> {
    pub(super) fn new(out: &'o mut Normalizer, pass: Pass) -> Self {
        Self {
            out,
            pass,
            shapes: Vec::new(),
            title_seen: false,
            take: None,
            para: String::new(),
            para_open: false,
            block_open: false,
            tables: 0,
            cells: 0,
            cell_has_text: false,
            alt: Vec::new(),
            found: Found {
                started: false,
                title: String::new(),
                hidden: false,
            },
        }
    }

    pub(super) fn walk(mut self, text: &str) -> Result<Found, Failure> {
        let mut xml = Xml::new(text, classify);
        while !self.out.is_full() {
            match xml.next()? {
                Node::Open(tag, e) => {
                    if self.open(&mut xml, tag, &e)? {
                        break;
                    }
                }
                Node::Close(tag) => {
                    if self.close(&xml, tag) {
                        break;
                    }
                }
                Node::Text(t) => {
                    let in_run = xml.up(1) == Some(Tag::Run);
                    if xml.up(0) == Some(Tag::T) && in_run {
                        let text: String = t
                            .chars()
                            .map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c })
                            .collect();
                        self.append(&text);
                    }
                    if self.pass == Pass::Hidden && self.found.hidden {
                        break;
                    }
                }
                Node::Eof => break,
            }
        }
        self.flush();
        Ok(self.found)
    }

    /// Returns true when the walk is done.
    fn open(
        &mut self,
        xml: &mut Xml<'_, Tag>,
        tag: Tag,
        e: &BytesStart<'_>,
    ) -> Result<bool, Failure> {
        match tag {
            Tag::AlternateContent => self.alt.push(false),
            Tag::Branch if xml.up(1) == Some(Tag::AlternateContent) => match self.alt.last_mut() {
                Some(taken) if !*taken => *taken = true,
                _ => xml.skip()?,
            },
            Tag::Shape => {
                let hidden = self.shapes.last().is_some_and(|s| s.hidden);
                self.shapes.push(Shape { hidden, ph: None });
            }
            Tag::CNvPr if xml.up(2) == Some(Tag::Shape) => {
                if let Some(shape) = self.shapes.last_mut() {
                    shape.hidden |= xml::attr(e, b"hidden").is_some_and(|v| xml::on_off(Some(&v)));
                }
            }
            Tag::Ph if self.take.is_none() => {
                if let Some(shape) = self.shapes.last_mut() {
                    shape.ph = Some(placeholder(xml::attr(e, b"type").as_deref()));
                }
            }
            Tag::TxBody | Tag::Tbl if self.take.is_none() => match self.decide() {
                Some(take) => {
                    self.take = Some((take, xml.depth()));
                    self.block_open = false;
                    if tag == Tag::Tbl {
                        self.tables = 1;
                        self.write_break(true);
                    }
                }
                None => xml.skip()?,
            },
            Tag::Tr if self.tables > 0 => {
                self.write_break(false);
                self.cells = 0;
            }
            Tag::Tc if self.tables > 0 => {
                if self.cells > 0 && self.writing() {
                    self.out.push("\t");
                }
                self.cell_has_text = false;
            }
            Tag::Br if xml.up(1) == Some(Tag::P) => {
                let line = self.tables == 0 && matches!(self.pass, Pass::Body | Pass::Notes);
                self.append(if line { "\n" } else { " " });
            }
            _ => {}
        }
        Ok(false)
    }

    /// Returns true when the walk is done.
    fn close(&mut self, xml: &Xml<'_, Tag>, tag: Tag) -> bool {
        match tag {
            Tag::P if self.take.is_some() => {
                self.flush();
                self.para_open = false;
            }
            Tag::Tc if self.tables > 0 => self.cells += 1,
            Tag::Shape => {
                self.shapes.pop();
            }
            Tag::AlternateContent => {
                self.alt.pop();
            }
            Tag::TxBody | Tag::Tbl if self.take.is_some_and(|(_, d)| d == xml.depth() + 1) => {
                self.take = None;
                if tag == Tag::Tbl {
                    self.tables = 0;
                    self.write_break(true);
                }
                // The heading is one placeholder.
                return self.pass == Pass::Title;
            }
            _ => {}
        }
        false
    }

    /// Whether this pass reads the text body or table that starts here.
    fn decide(&mut self) -> Option<Take> {
        let shape = self.shapes.last().copied().unwrap_or_default();
        let heading = !self.title_seen && !shape.hidden && shape.ph == Some(Ph::Title);
        self.title_seen |= heading;
        let furniture = shape.ph == Some(Ph::Furniture);
        match self.pass {
            Pass::Hidden => (!furniture).then_some(Take::Discard),
            Pass::Title => heading.then_some(Take::Write),
            Pass::Body if heading || furniture => None,
            Pass::Body if shape.hidden => Some(Take::Discard),
            Pass::Body => Some(Take::Write),
            Pass::Notes => (!shape.hidden && shape.ph == Some(Ph::Body)).then_some(Take::Write),
        }
    }

    fn writing(&self) -> bool {
        matches!(self.take, Some((Take::Write, _)))
    }

    /// A paragraph or line break around a table, in the Body pass.
    fn write_break(&mut self, paragraph: bool) {
        if self.pass == Pass::Body && self.writing() {
            if paragraph {
                self.out.paragraph_break();
            } else {
                self.out.line_break();
            }
        }
    }

    fn append(&mut self, text: &str) {
        match self.take {
            Some((Take::Write, _)) => {
                self.para.push_str(text);
                if self.para.len() >= FLUSH_BYTES {
                    self.flush();
                }
            }
            Some((Take::Discard, _)) => self.found.hidden |= !text.trim().is_empty(),
            None => {}
        }
    }

    /// Sends the open `a:p`'s text out, starting its line or block first.
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.para);
        if self.para_open {
            self.out.push(&text);
            return;
        }
        let text = text.trim_start();
        if text.is_empty() {
            return;
        }
        self.para_open = true;
        match self.pass {
            Pass::Title => {
                self.out.push(if self.found.started { " " } else { "## " });
                self.found.started = true;
                let room = RAW_TITLE_CHARS.saturating_sub(self.found.title.chars().count());
                if room > 0 && !self.found.title.is_empty() {
                    self.found.title.push(' ');
                }
                self.found.title.extend(text.chars().take(room));
            }
            Pass::Notes => {
                if !self.found.started {
                    self.found.started = true;
                    self.out.paragraph_break();
                    self.out.push("Notes:");
                }
                self.out.line_break();
            }
            Pass::Body if self.tables > 0 => {
                if self.cell_has_text {
                    self.out.push(" ");
                }
                self.cell_has_text = true;
            }
            Pass::Body => {
                if self.block_open {
                    self.out.line_break();
                } else {
                    self.block_open = true;
                    self.out.paragraph_break();
                }
            }
            Pass::Hidden => return,
        }
        self.out.push(text);
    }
}
