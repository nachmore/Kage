//! Where the RTF reader's text goes: decoding the pending bytes, `\uN`,
//! and routing text and breaks to the body, the notes, the title or a
//! style name.

use super::{words, Break, Dest, Reader, Target};
use crate::Extracted;
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
use kage_core::file_text_wire::Warning;

/// Raw title text kept; the title is cut to 200 characters later.
const TITLE_RAW_BYTES: usize = 16 * 1024;
/// Style names kept, for heading detection.
const STYLE_NAME_BYTES: usize = 64;
/// Fonts and styles remembered; more are ignored.
const MAX_TABLE_ENTRIES: usize = 4096;

impl Reader {
    pub(super) fn set_font_encoding(&mut self, encoding: Option<&'static Encoding>) {
        let (Some(font), Some(encoding)) = (self.font_entry, encoding) else {
            return;
        };
        if self.fonts.len() < MAX_TABLE_ENTRIES || self.fonts.contains_key(&font) {
            self.fonts.insert(font, encoding);
        }
    }

    pub(super) fn commit_style(&mut self) {
        let style = std::mem::take(&mut self.style);
        if style.character || self.headings.len() >= MAX_TABLE_ENTRIES {
            return;
        }
        let level = style
            .outline
            .unwrap_or_else(|| words::heading_of_style_name(&style.name));
        if level > 0 {
            self.headings.insert(style.number, level);
        }
    }

    pub(super) fn unicode(&mut self, n: i32) {
        let unit = (if n < 0 { n + 0x1_0000 } else { n }) as u32;
        let c = match unit {
            0xD800..=0xDBFF => {
                self.replaced |= self.high.replace(unit).is_some();
                return;
            }
            0xDC00..=0xDFFF => match self.high.take() {
                Some(high) => char::from_u32(0x1_0000 + ((high - 0xD800) << 10) + (unit - 0xDC00)),
                None => None,
            },
            _ => {
                self.replaced |= self.high.take().is_some();
                char::from_u32(unit)
            }
        };
        match c {
            Some(c) => self.emit_char(c),
            None => self.replaced = true,
        }
    }

    fn encoding(&self) -> &'static Encoding {
        let font = self
            .stack
            .last()
            .and_then(|g| g.font)
            .unwrap_or(self.default_font);
        self.fonts.get(&font).copied().unwrap_or(self.code_page)
    }

    /// Decodes the pending text bytes and routes them.
    pub(super) fn flush(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.bytes);
        let encoding = self.encoding();
        let (mut text, mut replaced) = encoding.decode_without_bom_handling(&bytes);
        if replaced && encoding == UTF_8 {
            // Older Cocoa writers declare `\ansicpg65001` and still escape
            // Windows-1252 bytes; that code page maps every byte.
            (text, replaced) = WINDOWS_1252.decode_without_bom_handling(&bytes);
        }
        // A high surrogate waiting for its pair is lost.
        let lost_high = self.high.take().is_some();
        self.replaced |= replaced || lost_high;
        self.emit(&text);
        drop(text);
        self.bytes = bytes;
        self.bytes.clear();
    }

    pub(super) fn emit_char(&mut self, c: char) {
        self.emit(c.encode_utf8(&mut [0; 4]));
    }

    fn emit(&mut self, text: &str) {
        let Some(&g) = self.stack.last() else {
            return;
        };
        let target = match g.dest {
            Dest::Text(target) => target,
            Dest::Style => {
                let room = STYLE_NAME_BYTES.saturating_sub(self.style.name.len());
                push_capped(&mut self.style.name, text, room);
                return;
            }
            _ => return,
        };
        if g.deleted {
            return;
        }
        if g.hidden {
            self.hidden_text |= !text.trim().is_empty();
            return;
        }
        match target {
            Target::Body => {
                let prefix = match g.heading {
                    1 => Some("# "),
                    2 => Some("## "),
                    3 => Some("### "),
                    _ => (g.list || self.list_number).then_some("- "),
                };
                self.out.text(text, prefix);
            }
            Target::Note => {
                // A note's number is left out, so the space after it goes
                // too.
                let starts = self.notes.is_empty() || self.notes.ends_with('\n');
                let text = if starts {
                    text.trim_start_matches([' ', '\t'])
                } else {
                    text
                };
                self.notes.push_str(text);
            }
            Target::Title => {
                let room = TITLE_RAW_BYTES.saturating_sub(self.title.len());
                push_capped(&mut self.title, text, room);
            }
        }
    }

    pub(super) fn brk(&mut self, kind: Break) {
        let Some(Dest::Text(target)) = self.stack.last().map(|g| g.dest) else {
            return;
        };
        match (target, kind) {
            (Target::Body, Break::Paragraph) => {
                self.out.paragraph_break();
                self.list_number = false;
            }
            (Target::Body, Break::Line) => self.out.line_break(false),
            (Target::Body, Break::Row) => {
                self.out.line_break(true);
                self.list_number = false;
            }
            (Target::Body, Break::Cell) => self.out.tab(),
            (Target::Note, Break::Paragraph) => self.notes.push_str("\n\n"),
            (Target::Note, Break::Line | Break::Row) => self.notes.push('\n'),
            (Target::Note, Break::Cell) => self.notes.push('\t'),
            (Target::Title, _) => {
                let room = TITLE_RAW_BYTES.saturating_sub(self.title.len());
                push_capped(&mut self.title, " ", room);
            }
        }
    }

    pub(super) fn finish(mut self) -> Extracted {
        self.flush();
        if !self.notes.trim().is_empty() {
            self.out.paragraph_break();
            self.out.text("Notes", None);
            self.out.paragraph_break();
            self.out.text(&self.notes, None);
        }
        let mut warnings = Vec::new();
        if self.replaced {
            warnings.push(Warning::ReplacedCharacters);
        }
        if self.hidden_text {
            warnings.push(Warning::HiddenTextSkipped);
        }
        let title = self.title.trim();
        Extracted {
            title: (!title.is_empty()).then(|| title.to_string()),
            text: self.out.finish(),
            pages: None,
            warnings,
        }
    }
}

/// Appends at most `room` bytes of `text`, cut at a character boundary.
fn push_capped(out: &mut String, text: &str, room: usize) {
    let mut cut = text.len().min(room);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    out.push_str(&text[..cut]);
}
