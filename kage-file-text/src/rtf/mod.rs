//! RTF (EXTENSION_FILE_TEXT §3.7, §5.9). A second-release format: built
//! here, offered once `READS_P1_FORMATS` is on.
//!
//! An in-house reader: `lexer` splits the bytes into groups, control
//! words and text, and this module keeps a stack of group states (where
//! text goes, character and paragraph properties) and routes text.
//!
//! - Body text, as plain text with light Markdown: `\par` ends a
//!   paragraph (a blank line follows), `\line` a line, `\cell` a table
//!   cell (a tab) and `\row` a row. Paragraphs with a heading style
//!   (`\outlinelevel` 0–2, or a style named "heading 1"–"heading 3" or
//!   "Title") start with `#`–`###`; list paragraphs (`\ls`, or a
//!   `\listtext`/`\pntext` number) with `- `.
//! - Footnotes and endnotes go at the end, under a `Notes` line, as in
//!   DOCX.
//! - Left out: the font, colour, style and list tables, pictures,
//!   objects, headers, footers, field instructions (field results stay),
//!   comments, bookmarks, index entries, the `\info` block (but `\title`,
//!   which is the title), starred destinations this reader doesn't know,
//!   deleted revisions, and hidden text (`\v`, `hidden_text_skipped`).
//! - `\'hh` bytes and any 8-bit text are decoded with the font's
//!   `\fcharset` (ANSI is Windows-1252) or `\cpg`, else the document's
//!   `\ansicpg` (Windows-1252 by default). Bytes that aren't UTF-8 under
//!   `\ansicpg65001` are read as Windows-1252, as older Cocoa writers
//!   meant them. `\uN` is a UTF-16 unit, with surrogate pairs joined; the
//!   `\ucN` fallback after it (a byte, an escape or a control word each
//!   count as one) is skipped. `\upr` keeps only its `\ud` part.
//! - More than 256 nested groups is `too_complex`. `\binN` data is
//!   skipped in the lexer. Text after the document's group closes is
//!   ignored; a document that ends early is read as far as it goes.

mod lexer;
mod out;
mod route;
mod words;

use crate::{Extracted, Failure, Job, WorkerErrorCode};
use encoding_rs::{Encoding, MACINTOSH, WINDOWS_1252};
use kage_core::file_text_wire::MAX_NESTING_DEPTH;
use lexer::{Lexer, Token};
use out::Out;
use std::collections::HashMap;

/// Most `\ucN` fallback units.
const MAX_FALLBACK: i32 = 16;

/// Reads an RTF file.
pub fn extract(job: &Job<'_>) -> Result<Extracted, Failure> {
    let mut reader = Reader::new(job.max_chars);
    for token in Lexer::new(job.bytes) {
        if !reader.token(token)? {
            break;
        }
    }
    Ok(reader.finish())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Body,
    Note,
    Title,
}

/// Where a group's text goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dest {
    Text(Target),
    Skip,
    Fonts,
    Styles,
    /// One style of the style sheet.
    Style,
    Info,
    /// `\upr`: only its `\ud` part is read, into the target.
    Upr(Target),
}

#[derive(Debug, Clone, Copy)]
struct Group {
    dest: Dest,
    uc: u8,
    hidden: bool,
    deleted: bool,
    /// `None`: the default font (`\deffN`).
    font: Option<i32>,
    heading: u8,
    list: bool,
}

impl Group {
    const ROOT: Group = Group {
        dest: Dest::Text(Target::Body),
        uc: 1,
        hidden: false,
        deleted: false,
        font: None,
        heading: 0,
        list: false,
    };

    fn child(self) -> Group {
        let dest = match self.dest {
            Dest::Styles => Dest::Style,
            Dest::Style => Dest::Skip,
            other => other,
        };
        Group { dest, ..self }
    }
}

#[derive(Default)]
struct StyleDef {
    number: i32,
    character: bool,
    outline: Option<u8>,
    name: String,
}

enum Break {
    Paragraph,
    Line,
    Row,
    Cell,
}

struct Reader {
    out: Out,
    stack: Vec<Group>,
    /// `\*` came just before.
    star: bool,
    /// `\uN` fallback units still to skip.
    fallback: i32,
    /// Text bytes not decoded yet.
    bytes: Vec<u8>,
    /// A `\u` high surrogate waiting for its pair.
    high: Option<u32>,
    code_page: &'static Encoding,
    default_font: i32,
    fonts: HashMap<i32, &'static Encoding>,
    font_entry: Option<i32>,
    headings: HashMap<i32, u8>,
    style: StyleDef,
    /// The paragraph had a `\listtext` or `\pntext` number.
    list_number: bool,
    notes: String,
    title: String,
    title_done: bool,
    hidden_text: bool,
    replaced: bool,
}

impl Reader {
    fn new(max_chars: u32) -> Self {
        Self {
            out: Out::new(max_chars),
            stack: Vec::new(),
            star: false,
            fallback: 0,
            bytes: Vec::new(),
            high: None,
            code_page: WINDOWS_1252,
            default_font: 0,
            fonts: HashMap::new(),
            font_entry: None,
            headings: HashMap::new(),
            style: StyleDef::default(),
            list_number: false,
            notes: String::new(),
            title: String::new(),
            title_done: false,
            hidden_text: false,
            replaced: false,
        }
    }

    /// Handles a token. `Ok(false)`: the document's group has closed.
    fn token(&mut self, token: Token<'_>) -> Result<bool, Failure> {
        if self.stack.is_empty() && token != Token::Open {
            return Ok(true);
        }
        match token {
            Token::Open => {
                self.end_run();
                if self.stack.len() >= MAX_NESTING_DEPTH {
                    return Err(Failure::new(
                        WorkerErrorCode::TooComplex,
                        "RTF groups nested too deep",
                    ));
                }
                let group = self.stack.last().map_or(Group::ROOT, |g| g.child());
                if group.dest == Dest::Style {
                    self.style = StyleDef::default();
                }
                self.stack.push(group);
            }
            Token::Close => {
                self.end_run();
                if let Some(group) = self.stack.pop() {
                    let title = Dest::Text(Target::Title);
                    if group.dest == Dest::Style {
                        self.commit_style();
                    } else if group.dest == title
                        && self.stack.last().map(|g| g.dest) != Some(title)
                    {
                        self.title_done = true;
                    }
                }
                return Ok(!self.stack.is_empty());
            }
            Token::Word(name, param) => self.word(name, param),
            Token::Symbol(b) => self.symbol(b),
            Token::Byte(b) => {
                self.star = false;
                if self.fallback > 0 {
                    self.fallback -= 1;
                } else {
                    self.bytes.push(b);
                }
            }
            Token::Text(text) => {
                self.star = false;
                let skip = text.len().min(self.fallback as usize);
                self.fallback -= skip as i32;
                self.bytes.extend_from_slice(&text[skip..]);
            }
        }
        Ok(true)
    }

    /// A group boundary ends the pending text and any `\uN` fallback.
    fn end_run(&mut self) {
        self.flush();
        self.star = false;
        self.fallback = 0;
    }

    fn top(&mut self) -> &mut Group {
        self.stack.last_mut().expect("a group is open")
    }

    fn word(&mut self, name: &[u8], param: Option<i32>) {
        let star = std::mem::take(&mut self.star);
        if self.fallback > 0 {
            self.fallback -= 1;
            return;
        }
        self.flush();
        let n = param.unwrap_or(0);
        let dest = self.top().dest;
        if dest == Dest::Skip {
            return;
        }
        match name {
            b"par" | b"sect" | b"page" => self.brk(Break::Paragraph),
            b"line" | b"column" => self.brk(Break::Line),
            b"row" | b"nestrow" => self.brk(Break::Row),
            b"cell" | b"nestcell" => self.brk(Break::Cell),
            b"u" => {
                self.unicode(n);
                self.fallback = i32::from(self.top().uc);
            }
            b"uc" => self.top().uc = param.unwrap_or(1).clamp(0, MAX_FALLBACK) as u8,
            b"v" => self.top().hidden = n != 0 || param.is_none(),
            b"deleted" => self.top().deleted = n != 0 || param.is_none(),
            b"plain" => {
                let g = self.top();
                g.hidden = false;
                g.deleted = false;
                g.font = None;
            }
            b"pard" => {
                let g = self.top();
                g.heading = 0;
                g.list = false;
            }
            b"f" if dest == Dest::Fonts => self.font_entry = param,
            b"f" if matches!(dest, Dest::Text(_)) => self.top().font = param,
            b"fcharset" if dest == Dest::Fonts => self.set_font_encoding(words::charset(n)),
            b"cpg" if dest == Dest::Fonts => self.set_font_encoding(words::code_page(n)),
            b"ansicpg" => self.code_page = words::code_page(n).unwrap_or(self.code_page),
            b"mac" => self.code_page = MACINTOSH,
            b"deff" => self.default_font = n,
            b"s" if dest == Dest::Style => self.style.number = n,
            b"s" if matches!(dest, Dest::Text(_)) => {
                self.top().heading = self.headings.get(&n).copied().unwrap_or(0);
            }
            b"cs" | b"ds" | b"ts" if dest == Dest::Style => self.style.character = true,
            b"outlinelevel" if dest == Dest::Style => {
                self.style.outline = Some(words::heading_of_outline_level(n));
            }
            b"outlinelevel" if matches!(dest, Dest::Text(_)) => {
                self.top().heading = words::heading_of_outline_level(n);
            }
            b"ls" if matches!(dest, Dest::Text(_)) => self.top().list = n > 0,
            b"listtext" | b"pntext" => {
                self.list_number |= dest == Dest::Text(Target::Body);
                self.top().dest = Dest::Skip;
            }
            b"fonttbl" => self.top().dest = Dest::Fonts,
            b"stylesheet" => self.top().dest = Dest::Styles,
            b"info" => self.top().dest = Dest::Info,
            b"title" if dest == Dest::Info => {
                self.top().dest = if self.title_done {
                    Dest::Skip
                } else {
                    Dest::Text(Target::Title)
                };
            }
            b"footnote" if matches!(dest, Dest::Text(Target::Body | Target::Note)) => {
                if !self.notes.is_empty() {
                    self.notes.push_str("\n\n");
                }
                self.top().dest = Dest::Text(Target::Note);
            }
            b"upr" => {
                if let Dest::Text(target) = dest {
                    self.top().dest = Dest::Upr(target);
                }
            }
            b"ud" => {
                if let Dest::Upr(target) = dest {
                    self.top().dest = Dest::Text(target);
                }
            }
            // Containers whose text is read.
            b"field" | b"fldrslt" | b"shp" | b"shpinst" | b"shptxt" => {}
            _ if words::is_skipped_destination(name) => self.top().dest = Dest::Skip,
            _ => {
                if let Some(c) = words::char_word(name) {
                    self.emit_char(c);
                } else if star {
                    self.top().dest = Dest::Skip;
                }
            }
        }
    }

    fn symbol(&mut self, b: u8) {
        if b == b'*' {
            self.star = true;
            return;
        }
        self.star = false;
        if self.fallback > 0 {
            self.fallback -= 1;
            return;
        }
        match b {
            b'\\' | b'{' | b'}' => self.bytes.push(b),
            b'~' => {
                self.flush();
                self.emit_char('\u{A0}');
            }
            b'_' => {
                self.flush();
                self.emit_char('\u{2011}');
            }
            // `\-` optional hyphen, `\:` index subentry, `\|` formula,
            // and broken escapes: nothing to show.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
