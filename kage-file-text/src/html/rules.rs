//! Which HTML elements make breaks, which are skipped, and when a skipped
//! element ends without its end tag (EXTENSION_FILE_TEXT §3.7, §5.9).
//!
//! Names are the tokenizer's: ASCII lowercase.

use html5ever::tokenizer::states::RawKind;

/// Elements with no content and no end tag.
pub fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "basefont"
            | "bgsound"
            | "br"
            | "col"
            | "embed"
            | "frame"
            | "hr"
            | "image"
            | "img"
            | "input"
            | "keygen"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// How the tokenizer reads an element's content when it isn't markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    Raw(RawKind),
    /// `plaintext`: the rest of the file is text.
    Plaintext,
}

/// The content mode a start tag switches the tokenizer to, as the HTML
/// tree builder would (scripting on, so `noscript` is raw text too).
pub fn content_of(name: &str) -> Option<Content> {
    Some(match name {
        "script" => Content::Raw(RawKind::ScriptData),
        "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
            Content::Raw(RawKind::Rawtext)
        }
        "title" | "textarea" => Content::Raw(RawKind::Rcdata),
        "plaintext" => Content::Plaintext,
        _ => return None,
    })
}

/// Elements whose content is never shown. `head` isn't here: nothing in
/// it renders except `title`, and anything else in it ends it, as browsers
/// do.
pub fn is_silent(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "noscript"
            | "template"
            | "svg"
            | "iframe"
            | "noembed"
            | "noframes"
            | "datalist"
    )
}

/// Elements that keep their whitespace and line breaks.
pub fn is_preformatted(name: &str) -> bool {
    matches!(name, "pre" | "listing" | "xmp" | "plaintext" | "textarea")
}

/// What a start or end tag does to the text around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Break {
    None,
    Line,
    Paragraph,
}

/// Elements set apart by a blank line.
fn is_paragraph(name: &str) -> bool {
    matches!(
        name,
        "p" | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "pre"
            | "listing"
            | "xmp"
            | "plaintext"
            | "ul"
            | "ol"
            | "menu"
            | "dir"
            | "dl"
            | "table"
            | "figure"
            | "hr"
            | "address"
            | "article"
            | "aside"
            | "details"
            | "dialog"
            | "fieldset"
            | "footer"
            | "form"
            | "header"
            | "hgroup"
            | "main"
            | "nav"
            | "section"
            | "search"
            | "figcaption"
            | "center"
            | "caption"
    )
}

/// Elements on a line of their own.
fn is_line(name: &str) -> bool {
    matches!(
        name,
        "div"
            | "li"
            | "dt"
            | "dd"
            | "tr"
            | "summary"
            | "legend"
            | "option"
            | "optgroup"
            | "select"
            | "textarea"
            | "body"
            | "br"
            | "frameset"
    )
}

/// The break a tag of this element makes, before (start) or after (end).
pub fn break_of(name: &str) -> Break {
    if is_paragraph(name) {
        Break::Paragraph
    } else if is_line(name) {
        Break::Line
    } else {
        Break::None
    }
}

/// The Markdown prefix of a heading element.
pub fn heading_prefix(name: &str) -> Option<&'static str> {
    match name {
        "h1" => Some("# "),
        "h2" => Some("## "),
        "h3" => Some("### "),
        _ => None,
    }
}

/// Start tags that close an open `p`.
fn closes_p(name: &str) -> bool {
    (is_paragraph(name) && name != "caption")
        || matches!(name, "div" | "li" | "dd" | "dt" | "summary")
}

/// Phrasing elements: their end tags don't close an open `p`.
fn is_phrasing(name: &str) -> bool {
    matches!(
        name,
        "a" | "abbr"
            | "audio"
            | "b"
            | "bdi"
            | "bdo"
            | "big"
            | "br"
            | "button"
            | "canvas"
            | "cite"
            | "code"
            | "data"
            | "del"
            | "dfn"
            | "em"
            | "embed"
            | "font"
            | "i"
            | "iframe"
            | "img"
            | "input"
            | "ins"
            | "kbd"
            | "label"
            | "map"
            | "mark"
            | "math"
            | "meter"
            | "nobr"
            | "noscript"
            | "object"
            | "output"
            | "picture"
            | "progress"
            | "q"
            | "rp"
            | "rt"
            | "ruby"
            | "s"
            | "samp"
            | "script"
            | "select"
            | "slot"
            | "small"
            | "span"
            | "strike"
            | "strong"
            | "sub"
            | "sup"
            | "svg"
            | "template"
            | "textarea"
            | "time"
            | "tt"
            | "u"
            | "var"
            | "video"
            | "wbr"
    )
}

/// How an element whose end tag is optional ends without one: a start
/// tag of `starts` or an end tag of `ends` ends it, unless it is inside
/// one of `containers` opened since.
pub struct ImpliedEnd {
    starts: NameTest,
    ends: NameTest,
    pub containers: &'static [&'static str],
}

type NameTest = fn(&str) -> bool;

impl ImpliedEnd {
    pub fn ended_by_start(&self, name: &str) -> bool {
        (self.starts)(name)
    }

    pub fn ended_by_end(&self, name: &str) -> bool {
        (self.ends)(name)
    }
}

/// The implied end of the elements that have one; `None` for elements
/// that end only at their own end tag.
pub fn implied_end(name: &str) -> Option<ImpliedEnd> {
    let (starts, ends, containers): (NameTest, NameTest, &[&str]) = match name {
        "p" => (closes_p, |n| !is_phrasing(n), &[]),
        "li" => (
            |n| n == "li",
            |n| matches!(n, "ul" | "ol" | "menu"),
            &["ul", "ol", "menu"],
        ),
        "dt" | "dd" => (|n| matches!(n, "dt" | "dd"), |n| n == "dl", &["dl"]),
        "tr" => (
            |n| matches!(n, "tr" | "tbody" | "thead" | "tfoot"),
            |n| matches!(n, "table" | "tbody" | "thead" | "tfoot"),
            &["table"],
        ),
        "td" | "th" => (
            |n| matches!(n, "td" | "th" | "tr" | "tbody" | "thead" | "tfoot"),
            |n| matches!(n, "tr" | "table" | "tbody" | "thead" | "tfoot"),
            &["table"],
        ),
        "option" => (
            |n| matches!(n, "option" | "optgroup"),
            |n| matches!(n, "select" | "datalist" | "optgroup"),
            &[],
        ),
        _ => return None,
    };
    Some(ImpliedEnd {
        starts,
        ends,
        containers,
    })
}

/// An inline `style` that hides the element: the last `display`
/// declaration is `none` (an `!important` one beats a later plain one).
pub fn hides(style: &str) -> bool {
    let mut hidden = false;
    let mut important = false;
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        if !property.trim().eq_ignore_ascii_case("display") {
            continue;
        }
        let value = value.trim();
        let (value, is_important) = match value
            .len()
            .checked_sub(10)
            .filter(|&i| value.is_char_boundary(i))
            .map(|i| value.split_at(i))
        {
            Some((v, tail)) if tail.eq_ignore_ascii_case("!important") => (v.trim_end(), true),
            _ => (value, false),
        };
        if important && !is_important {
            continue;
        }
        hidden = value.eq_ignore_ascii_case("none");
        important = is_important;
    }
    hidden
}
