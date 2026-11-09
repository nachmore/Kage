//! The token sink: html5ever's tokenizer, no tree builder
//! (EXTENSION_FILE_TEXT §5.9).
//!
//! The sink keeps a few counters instead of a DOM, so memory stays small
//! and time linear in the input, with one exception in html5ever: its
//! duplicate-attribute check costs n² steps for a tag with n attributes.
//! Real tags have a few dozen; a crafted file with one tag of 100,000
//! runs into the worker's deadline and the app reports `timeout`.
//!
//! It tells the tokenizer when an element's content is raw text
//! (`script`, `style`, `title`, …), as the tree builder would, so
//! markup-looking text in a script stays out.
//!
//! Hidden content is one skipped element at a time: the outermost one.
//! It ends at its own end tag (nested elements of the same name are
//! counted), or where its end tag is implied (`rules::implied_end`), or
//! at `</body>` or `</html>`. Markup that is badly nested can only make
//! this skip too little or too much text, never loop.
//!
//! A self-closing tag (`<div hidden/>`, `<script src="…"/>`) is taken as
//! an empty element, as XHTML means it. HTML would ignore the slash, but
//! files that rely on that show broken in browsers too.

use super::rules::{self, Break, Content};
use super::writer::Writer;
use crate::normalize::Normalized;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use std::cell::RefCell;

/// Raw title text kept; the title is cut to 200 characters later.
const TITLE_RAW_BYTES: usize = 16 * 1024;

/// What the sink found.
pub struct Found {
    pub text: Normalized,
    pub title: Option<String>,
    /// Text was left out because it was hidden (`hidden`, `display:none`).
    pub hidden_text: bool,
}

/// Reads decoded HTML.
pub fn read(html: &str, max_chars: u32) -> Found {
    let tokenizer = Tokenizer::new(
        Sink(RefCell::new(Walker::new(max_chars))),
        TokenizerOpts::default(),
    );
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    // The sink never pauses the tokenizer (no scripts, no re-decoding),
    // so one feed reads everything.
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.0.into_inner().finish()
}

struct Sink(RefCell<Walker>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        self.0.borrow_mut().token(token)
    }

    /// CDATA sections are text in SVG and MathML, comments elsewhere.
    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        self.0.borrow().foreign()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Never shown: `script`, `svg`, …
    Silent,
    /// Hidden by the document: `hidden`, `display:none`.
    Hidden,
    /// The document title.
    Title,
}

/// The outermost element being skipped.
struct Skip {
    name: String,
    kind: Kind,
    /// Open elements of the same name, the skipped one included.
    depth: u32,
    /// Open `containers` of its implied end (a list inside a skipped
    /// `li`, …).
    nested: u32,
}

enum Step {
    /// Still inside.
    Inside,
    /// This tag was its end tag.
    Ended,
    /// It ended just before this tag, which is read normally.
    EndedBefore,
}

impl Skip {
    fn new(name: &str, kind: Kind) -> Self {
        Self {
            name: name.to_string(),
            kind,
            depth: 1,
            nested: 0,
        }
    }

    fn step(&mut self, name: &str, start: bool, empty: bool) -> Step {
        if !start && matches!(name, "body" | "html") {
            return Step::EndedBefore;
        }
        if let Some(rule) = rules::implied_end(&self.name) {
            if rule.containers.contains(&name) {
                if start {
                    self.nested += u32::from(!empty);
                    return Step::Inside;
                }
                if self.nested > 0 {
                    self.nested -= 1;
                    return Step::Inside;
                }
            }
            if self.nested > 0 {
                return Step::Inside;
            }
            return if !start && name == self.name {
                Step::Ended
            } else if (start && rule.ended_by_start(name)) || (!start && rule.ended_by_end(name)) {
                Step::EndedBefore
            } else {
                Step::Inside
            };
        }
        if name == self.name {
            if start {
                self.depth += u32::from(!empty);
            } else {
                self.depth -= 1;
                if self.depth == 0 {
                    return Step::Ended;
                }
            }
        }
        Step::Inside
    }
}

struct Walker {
    out: Writer,
    skip: Option<Skip>,
    hidden_text: bool,
    title: String,
    title_done: bool,
    /// Open preformatted elements.
    pre: u32,
    /// Drop a line feed right after `<pre>`, `<listing>`, `<textarea>`.
    drop_lf: bool,
    /// Open `math` elements (foreign content).
    math: u32,
    /// Cells so far in the current table row.
    cells: u32,
    /// Inside a table cell: blocks stay on the row's line.
    in_cell: bool,
}

impl Walker {
    fn new(max_chars: u32) -> Self {
        Self {
            out: Writer::new(max_chars),
            skip: None,
            hidden_text: false,
            title: String::new(),
            title_done: false,
            pre: 0,
            drop_lf: false,
            math: 0,
            cells: 0,
            in_cell: false,
        }
    }

    fn foreign(&self) -> bool {
        self.math > 0 || self.skip.as_ref().is_some_and(|s| s.name == "svg")
    }

    fn token(&mut self, token: Token) -> TokenSinkResult<()> {
        match token {
            Token::TagToken(tag) => return self.tag(&tag),
            Token::CharacterTokens(text) => self.characters(&text),
            Token::ParseError(_) => {}
            _ => self.drop_lf = false,
        }
        TokenSinkResult::Continue
    }

    fn characters(&mut self, text: &str) {
        let mut text = text;
        if std::mem::take(&mut self.drop_lf) {
            text = text.strip_prefix('\n').unwrap_or(text);
        }
        if let Some(skip) = &self.skip {
            match skip.kind {
                Kind::Title if !self.title_done => {
                    let room = TITLE_RAW_BYTES.saturating_sub(self.title.len());
                    let mut cut = text.len().min(room);
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    self.title.push_str(&text[..cut]);
                }
                Kind::Hidden if !text.trim().is_empty() => self.hidden_text = true,
                _ => {}
            }
            return;
        }
        if self.pre > 0 {
            self.out.preformatted(text);
        } else {
            self.out.text(text);
        }
    }

    fn tag(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        let name: &str = &tag.name;
        let start = tag.kind == TagKind::StartTag;
        let empty = tag.self_closing || rules::is_void(name);
        self.drop_lf = false;
        // Raw text has to be switched on inside skipped elements too, or
        // a `</div>` in a script would end a hidden `div`.
        let content = (start && !empty && !self.foreign())
            .then(|| rules::content_of(name))
            .flatten();
        let result = match content {
            Some(Content::Raw(kind)) => TokenSinkResult::RawData(kind),
            Some(Content::Plaintext) => TokenSinkResult::Plaintext,
            None => TokenSinkResult::Continue,
        };
        if let Some(skip) = &mut self.skip {
            match skip.step(name, start, empty) {
                Step::Inside => return result,
                Step::Ended => {
                    self.end_skip();
                    return result;
                }
                Step::EndedBefore => self.end_skip(),
            }
        }
        if start && !empty {
            if let Some(kind) = self.skip_kind(tag) {
                self.skip = Some(Skip::new(name, kind));
                return result;
            }
        }
        if name == "math" && !tag.self_closing {
            if start {
                self.math += 1;
            } else {
                self.math = self.math.saturating_sub(1);
            }
        }
        self.layout(name, start);
        result
    }

    fn end_skip(&mut self) {
        if self.skip.take().is_some_and(|s| s.kind == Kind::Title) {
            self.title_done = true;
        }
    }

    fn skip_kind(&self, tag: &Tag) -> Option<Kind> {
        let name: &str = &tag.name;
        if name == "title" && !self.foreign() {
            return Some(Kind::Title);
        }
        if rules::is_silent(name) {
            return Some(Kind::Silent);
        }
        let hidden = tag.attrs.iter().any(|a| match &*a.name.local {
            "hidden" => true,
            "style" => rules::hides(&a.value),
            _ => false,
        });
        hidden.then_some(Kind::Hidden)
    }

    /// Breaks, prefixes, cells and preformatted text.
    fn layout(&mut self, name: &str, start: bool) {
        if rules::is_preformatted(name) {
            if start {
                self.pre += 1;
                self.drop_lf = name != "xmp" && name != "plaintext";
            } else {
                self.pre = self.pre.saturating_sub(1);
            }
        }
        match name {
            "td" | "th" => {
                if start {
                    if self.cells > 0 {
                        self.out.tab();
                    }
                    self.cells += 1;
                }
                self.in_cell = start;
                return;
            }
            "tr" | "table" => {
                self.cells = 0;
                self.in_cell = false;
            }
            _ => {}
        }
        match rules::break_of(name) {
            Break::None => return,
            // Blocks inside a cell stay on the row's line.
            _ if self.in_cell => return self.out.space(),
            _ if name == "br" => self.out.hard_break(),
            Break::Line => self.out.line_break(),
            Break::Paragraph => self.out.paragraph_break(),
        }
        let prefix = match name {
            "li" => Some("- "),
            _ => rules::heading_prefix(name),
        };
        if let Some(prefix) = prefix {
            if start {
                self.out.set_prefix(prefix);
            } else {
                self.out.clear_prefix();
            }
        } else if !start && matches!(name, "ul" | "ol" | "menu") {
            self.out.clear_prefix();
        }
    }

    fn finish(self) -> Found {
        let title = self.title.trim();
        Found {
            text: self.out.finish(),
            title: (!title.is_empty()).then(|| title.to_string()),
            hidden_text: self.hidden_text,
        }
    }
}
