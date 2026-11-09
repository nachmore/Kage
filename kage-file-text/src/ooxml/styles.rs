//! Word styles (`word/styles.xml`): which paragraphs are headings or list
//! items, and which text is hidden (EXTENSION_FILE_TEXT §3.7, §5.9).
//!
//! Style ids are localised (a German document's "Heading 1" has the id
//! `berschrift1`), but a built-in style's `w:name` is always English
//! (`heading 1`, `Title`). So a heading is found by, along the `basedOn`
//! chain: the style's outline level (0–2 → `#`–`###`), else its name,
//! else the id itself when the document has no such style.
//!
//! Hidden text can come from a run's own `w:vanish`, its character style,
//! its paragraph's style or the document defaults, in that order.

use super::xml::{self, Node, Xml};
use crate::Failure;
use std::collections::HashMap;

/// Styles kept from one document; the rest are ignored.
const MAX_STYLES: usize = 10_000;
/// `basedOn` steps followed, which also ends a cycle.
const MAX_CHAIN: usize = 16;

#[derive(Debug, Default, Clone)]
struct Style {
    /// `w:name`, lowercase.
    name: Option<String>,
    based_on: Option<String>,
    /// `w:outlineLvl` (0 is the top level, 9 body text).
    outline: Option<u8>,
    /// `w:numPr`: numbered (`numId` other than 0) or explicitly not.
    numbered: Option<bool>,
    /// `w:vanish` in the style's run properties.
    vanish: Option<bool>,
}

/// What a style or paragraph says about headings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// `#`, `##` or `###`.
    Heading(u8),
    /// Explicitly not a level 1–3 heading (body text, or a deeper heading).
    Body,
}

/// From an outline level: 0–2 are headings, the rest are not.
pub fn outline_level(outline: u8) -> Level {
    if outline < 3 {
        Level::Heading(outline + 1)
    } else {
        Level::Body
    }
}

fn level_from_name(name: &str) -> Option<Level> {
    if name == "title" {
        return Some(Level::Heading(1));
    }
    let n: u8 = name.strip_prefix("heading")?.trim().parse().ok()?;
    match n {
        1..=3 => Some(Level::Heading(n)),
        4..=9 => Some(Level::Body),
        _ => None,
    }
}

/// The styles of one document.
#[derive(Debug, Default)]
pub struct Styles {
    map: HashMap<String, Style>,
    default_paragraph: Option<String>,
    default_vanish: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    DocDefaults,
    RPrDefault,
    Style,
    Name,
    BasedOn,
    PPr,
    RPr,
    OutlineLvl,
    NumPr,
    NumId,
    Vanish,
    Other,
}

fn classify(local: &[u8]) -> Tag {
    match local {
        b"docDefaults" => Tag::DocDefaults,
        b"rPrDefault" => Tag::RPrDefault,
        b"style" => Tag::Style,
        b"name" => Tag::Name,
        b"basedOn" => Tag::BasedOn,
        b"pPr" => Tag::PPr,
        b"rPr" => Tag::RPr,
        b"outlineLvl" => Tag::OutlineLvl,
        b"numPr" => Tag::NumPr,
        b"numId" => Tag::NumId,
        b"vanish" => Tag::Vanish,
        _ => Tag::Other,
    }
}

impl Styles {
    /// Reads `styles.xml`.
    pub fn parse(text: &str) -> Result<Self, Failure> {
        let mut styles = Styles::default();
        let mut xml = Xml::new(text, classify);
        let mut current: Option<(String, Style)> = None;
        loop {
            match xml.next()? {
                Node::Open(tag, e) => match tag {
                    Tag::Style if xml.depth() == 2 => {
                        let kind = xml::attr(&e, b"type");
                        let paragraph = matches!(kind.as_deref(), None | Some("paragraph"));
                        let wanted = paragraph || kind.as_deref() == Some("character");
                        let id = xml::attr(&e, b"styleId");
                        let default =
                            xml::attr(&e, b"default").is_some_and(|v| xml::on_off(Some(&v)));
                        if paragraph && default && styles.default_paragraph.is_none() {
                            styles.default_paragraph.clone_from(&id);
                        }
                        current = id
                            .filter(|_| wanted && styles.map.len() < MAX_STYLES)
                            .map(|id| (id, Style::default()));
                    }
                    Tag::Name if xml.at(&[Tag::Style, Tag::Name]) => {
                        if let Some((_, s)) = current.as_mut() {
                            s.name = xml::attr(&e, b"val").map(|v| v.trim().to_ascii_lowercase());
                        }
                    }
                    Tag::BasedOn if xml.at(&[Tag::Style, Tag::BasedOn]) => {
                        if let Some((_, s)) = current.as_mut() {
                            s.based_on = xml::attr(&e, b"val");
                        }
                    }
                    Tag::OutlineLvl if xml.at(&[Tag::Style, Tag::PPr, Tag::OutlineLvl]) => {
                        if let Some((_, s)) = current.as_mut() {
                            s.outline = xml::attr(&e, b"val").and_then(|v| v.trim().parse().ok());
                        }
                    }
                    Tag::NumId if xml.at(&[Tag::Style, Tag::PPr, Tag::NumPr, Tag::NumId]) => {
                        if let Some((_, s)) = current.as_mut() {
                            s.numbered = xml::attr(&e, b"val").map(|v| v.trim() != "0");
                        }
                    }
                    Tag::Vanish if xml.at(&[Tag::Style, Tag::RPr, Tag::Vanish]) => {
                        if let Some((_, s)) = current.as_mut() {
                            s.vanish = Some(xml::val_on(&e));
                        }
                    }
                    Tag::Vanish
                        if xml.at(&[Tag::DocDefaults, Tag::RPrDefault, Tag::RPr, Tag::Vanish]) =>
                    {
                        styles.default_vanish = xml::val_on(&e);
                    }
                    _ => {}
                },
                Node::Close(Tag::Style) if xml.depth() == 1 => {
                    if let Some((id, style)) = current.take() {
                        styles.map.entry(id).or_insert(style);
                    }
                }
                Node::Eof => return Ok(styles),
                _ => {}
            }
        }
    }

    /// The style a paragraph uses: its own, else the default one.
    pub fn paragraph_style<'s>(&'s self, own: Option<&'s str>) -> Option<&'s str> {
        own.or(self.default_paragraph.as_deref())
    }

    /// Follows `basedOn` from `id` and returns the first answer `get`
    /// gives.
    fn along<T>(&self, id: &str, get: impl Fn(&Style) -> Option<T>) -> Option<T> {
        let mut id = id;
        for _ in 0..MAX_CHAIN {
            let style = self.map.get(id)?;
            if let Some(found) = get(style) {
                return Some(found);
            }
            id = style.based_on.as_deref()?;
        }
        None
    }

    /// Whether a paragraph style makes a heading.
    pub fn heading(&self, id: Option<&str>) -> Option<u8> {
        let id = id?;
        let level = if self.map.contains_key(id) {
            self.along(id, |s| {
                s.outline
                    .map(outline_level)
                    .or_else(|| s.name.as_deref().and_then(level_from_name))
            })
        } else {
            // No styles part, or a style it doesn't define: the id is all
            // there is (`Heading1`, `Title`).
            level_from_name(&id.to_ascii_lowercase().replace("heading", "heading "))
        };
        match level {
            Some(Level::Heading(n)) => Some(n),
            _ => None,
        }
    }

    /// Whether a paragraph style numbers its paragraphs.
    pub fn numbered(&self, id: Option<&str>) -> bool {
        id.and_then(|id| self.along(id, |s| s.numbered))
            .unwrap_or(false)
    }

    /// `w:vanish` from a style, following `basedOn`.
    pub fn vanish(&self, id: &str) -> Option<bool> {
        self.along(id, |s| s.vanish)
    }

    /// `w:vanish` in the document's default run properties.
    pub fn default_vanish(&self) -> bool {
        self.default_vanish
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<w:styles xmlns:w="w">
  <w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
  <w:style w:type="paragraph" w:styleId="Titel"><w:name w:val="Title"/></w:style>
  <w:style w:type="paragraph" w:styleId="Mine"><w:name w:val="My heading"/><w:basedOn w:val="Titel"/></w:style>
  <w:style w:type="paragraph" w:styleId="H4"><w:name w:val="heading 4"/><w:basedOn w:val="berschrift1"/></w:style>
  <w:style w:type="paragraph" w:styleId="Outline2"><w:name w:val="x"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
  <w:style w:type="paragraph" w:styleId="ListBullet"><w:name w:val="List Bullet"/><w:pPr><w:numPr><w:numId w:val="3"/></w:numPr></w:pPr></w:style>
  <w:style w:type="paragraph" w:styleId="NoList"><w:basedOn w:val="ListBullet"/><w:pPr><w:numPr><w:numId w:val="0"/></w:numPr></w:pPr></w:style>
  <w:style w:type="character" w:styleId="Secret"><w:name w:val="Secret"/><w:rPr><w:vanish/></w:rPr></w:style>
  <w:style w:type="character" w:styleId="Shown"><w:basedOn w:val="Secret"/><w:rPr><w:vanish w:val="0"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="LoopA"><w:basedOn w:val="LoopB"/></w:style>
  <w:style w:type="paragraph" w:styleId="LoopB"><w:basedOn w:val="LoopA"/></w:style>
  <w:style w:type="table" w:styleId="Grid"><w:name w:val="heading 1"/></w:style>
</w:styles>"#;

    #[test]
    fn headings_by_outline_level_name_and_chain() {
        let s = Styles::parse(STYLES).unwrap();
        assert_eq!(s.heading(Some("berschrift1")), Some(1));
        assert_eq!(s.heading(Some("Titel")), Some(1));
        assert_eq!(s.heading(Some("Mine")), Some(1));
        assert_eq!(s.heading(Some("Outline2")), Some(2));
        // A deeper heading stops the chain: not `#`.
        assert_eq!(s.heading(Some("H4")), None);
        assert_eq!(s.heading(Some("Normal")), None);
        assert_eq!(s.heading(Some("LoopA")), None);
        // Table styles are not paragraph styles.
        assert_eq!(s.heading(Some("Grid")), None);
        assert_eq!(s.heading(None), None);
    }

    #[test]
    fn ids_alone_when_there_is_no_style() {
        let s = Styles::default();
        assert_eq!(s.heading(Some("Heading1")), Some(1));
        assert_eq!(s.heading(Some("heading3")), Some(3));
        assert_eq!(s.heading(Some("Title")), Some(1));
        assert_eq!(s.heading(Some("Heading4")), None);
        assert_eq!(s.heading(Some("Body")), None);
    }

    #[test]
    fn numbering_and_hidden_styles() {
        let s = Styles::parse(STYLES).unwrap();
        assert!(s.numbered(Some("ListBullet")));
        assert!(!s.numbered(Some("NoList")));
        assert!(!s.numbered(Some("Normal")));
        assert_eq!(s.vanish("Secret"), Some(true));
        assert_eq!(s.vanish("Shown"), Some(false));
        assert_eq!(s.vanish("Normal"), None);
        assert!(!s.default_vanish());
        assert_eq!(s.paragraph_style(None), Some("Normal"));
        let hidden = Styles::parse(
            r#"<w:styles><w:docDefaults><w:rPrDefault><w:rPr><w:vanish/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#,
        )
        .unwrap();
        assert!(hidden.default_vanish());
    }
}
