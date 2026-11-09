//! A streaming reader for OOXML parts (EXTENSION_FILE_TEXT §5.9).
//!
//! A thin layer over `quick_xml`:
//! - Elements are matched by local name, so the Transitional and Strict
//!   namespaces read the same. Each reader maps names to its own small
//!   tag type and the stack of open tags is kept here, so a reader can ask
//!   where it is (`up`, `inside`).
//! - More than `MAX_NESTING_DEPTH` open elements is `too_complex`.
//! - An empty element (`<w:tab/>`) comes as `Open` then `Close`.
//! - Text, CDATA and character references come as `Text`. Of the named
//!   entities only the five XML ones are resolved; `quick_xml` never loads
//!   a DTD, so there is no external entity and no entity expansion.
//! - Any XML error is `malformed`.

use crate::{Failure, WorkerErrorCode};
use kage_core::file_text_wire::MAX_NESTING_DEPTH;
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::borrow::Cow;

/// One step through a part.
pub enum Node<'x, T> {
    /// An element starts. It stays on top of the stack until its `Close`.
    Open(T, BytesStart<'x>),
    /// The element on top of the stack ends; it has been popped.
    Close(T),
    Text(Cow<'x, str>),
    Eof,
}

pub fn malformed() -> Failure {
    Failure::new(WorkerErrorCode::Malformed, "XML part doesn't parse")
}

fn too_deep() -> Failure {
    Failure::new(WorkerErrorCode::TooComplex, "XML nested too deep")
}

/// A part being read. `T` is the reader's tag type.
pub struct Xml<'x, T> {
    reader: Reader<&'x [u8]>,
    stack: Vec<T>,
    classify: fn(&[u8]) -> T,
    close_next: bool,
}

impl<'x, T: Copy + PartialEq> Xml<'x, T> {
    pub fn new(text: &'x str, classify: fn(&[u8]) -> T) -> Self {
        Self {
            reader: Reader::from_str(text),
            stack: Vec::new(),
            classify,
            close_next: false,
        }
    }

    pub fn next(&mut self) -> Result<Node<'x, T>, Failure> {
        if self.close_next {
            self.close_next = false;
            return self.stack.pop().map(Node::Close).ok_or_else(malformed);
        }
        loop {
            let event = self.reader.read_event().map_err(|_| malformed())?;
            match event {
                Event::Start(e) => return self.enter(e),
                Event::Empty(e) => {
                    self.close_next = true;
                    return self.enter(e);
                }
                Event::End(_) => return self.stack.pop().map(Node::Close).ok_or_else(malformed),
                Event::Text(e) => {
                    let text = e.xml10_content().map_err(|_| malformed())?;
                    if !text.is_empty() {
                        return Ok(Node::Text(text));
                    }
                }
                Event::CData(e) => {
                    let text = e.decode().map_err(|_| malformed())?;
                    if !text.is_empty() {
                        return Ok(Node::Text(text));
                    }
                }
                Event::GeneralRef(e) => {
                    if let Ok(Some(c)) = e.resolve_char_ref() {
                        return Ok(Node::Text(Cow::Owned(c.to_string())));
                    }
                    let name = e.decode().map_err(|_| malformed())?;
                    if let Some(text) = resolve_xml_entity(&name) {
                        return Ok(Node::Text(Cow::Borrowed(text)));
                    }
                    // An unknown entity: nothing to show.
                }
                Event::Eof => return Ok(Node::Eof),
                // Declarations, comments, processing instructions, DOCTYPE.
                _ => {}
            }
        }
    }

    fn enter(&mut self, e: BytesStart<'x>) -> Result<Node<'x, T>, Failure> {
        if self.stack.len() >= MAX_NESTING_DEPTH {
            return Err(too_deep());
        }
        let tag = (self.classify)(e.local_name().as_ref());
        self.stack.push(tag);
        Ok(Node::Open(tag, e))
    }

    /// Skips the element on top of the stack, through its `Close`. Call
    /// it right after its `Open`.
    pub fn skip(&mut self) -> Result<(), Failure> {
        let Some(depth) = self.stack.len().checked_sub(1) else {
            return Ok(());
        };
        loop {
            match self.next()? {
                Node::Close(_) if self.stack.len() == depth => return Ok(()),
                Node::Eof => return Ok(()),
                _ => {}
            }
        }
    }

    /// The open element `n` levels up: 0 is the innermost.
    pub fn up(&self, n: usize) -> Option<T> {
        let len = self.stack.len();
        len.checked_sub(n + 1).map(|i| self.stack[i])
    }

    /// The innermost open elements are `path` (outermost first).
    pub fn at(&self, path: &[T]) -> bool {
        self.stack.ends_with(path)
    }

    /// How many elements are open.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}

/// The value of the attribute with this local name, whatever its prefix
/// (`w:val`, `val`).
pub fn attr(e: &BytesStart<'_>, local: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == local)
        .and_then(|a| a.unescape_value().ok().map(Cow::into_owned))
}

/// The value of a prefixed attribute with this local name: `r:id`, not
/// the plain `id` on the same element.
pub fn prefixed_attr(e: &BytesStart<'_>, local: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.prefix().is_some() && a.key.local_name().as_ref() == local)
        .and_then(|a| a.unescape_value().ok().map(Cow::into_owned))
}

/// An OOXML on/off value (`ST_OnOff`). A missing value means on.
pub fn on_off(value: Option<&str>) -> bool {
    !matches!(
        value.map(str::trim),
        Some("0" | "false" | "off" | "False" | "Off" | "FALSE" | "OFF")
    )
}

/// An element's on/off `val` attribute (`<w:vanish/>`, `<w:vanish w:val="0"/>`).
pub fn val_on(e: &BytesStart<'_>) -> bool {
    on_off(attr(e, b"val").as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Tag {
        A,
        B,
        Other,
    }

    fn classify(local: &[u8]) -> Tag {
        match local {
            b"a" => Tag::A,
            b"b" => Tag::B,
            _ => Tag::Other,
        }
    }

    fn texts(xml: &str) -> Result<Vec<String>, Failure> {
        let mut x = Xml::new(xml, classify);
        let mut out = Vec::new();
        loop {
            match x.next()? {
                Node::Text(t) => out.push(t.into_owned()),
                Node::Eof => return Ok(out),
                _ => {}
            }
        }
    }

    #[test]
    fn names_match_by_local_name_and_empty_elements_close() {
        let xml = r#"<w:a xmlns:w="x"><s:b/><w:a>t</w:a></w:a>"#;
        let mut x = Xml::new(xml, classify);
        let mut seen = Vec::new();
        loop {
            match x.next().unwrap() {
                Node::Open(t, _) => seen.push(format!("+{t:?}@{}", x.depth())),
                Node::Close(t) => seen.push(format!("-{t:?}")),
                Node::Text(t) => {
                    assert!(x.at(&[Tag::A, Tag::A]));
                    seen.push(t.into_owned());
                }
                Node::Eof => break,
            }
        }
        assert_eq!(seen, ["+A@1", "+B@2", "-B", "+A@2", "t", "-A", "-A"]);
    }

    #[test]
    fn references_resolve_but_custom_entities_do_not_expand() {
        let xml = "<a>x &amp; &#x41;&#66; &lt;&custom;</a>";
        assert_eq!(texts(xml).unwrap().concat(), "x & AB <");
        let bomb = r#"<!DOCTYPE a [<!ENTITY e "lol"><!ENTITY f "&e;&e;&e;">]><a>&f;</a>"#;
        assert_eq!(texts(bomb).unwrap().concat(), "");
        let external = r#"<!DOCTYPE a [<!ENTITY x SYSTEM "file:///etc/passwd">]><a>&x;</a>"#;
        assert_eq!(texts(external).unwrap().concat(), "");
    }

    #[test]
    fn depth_past_the_cap_is_too_complex() {
        let ok = format!(
            "{}{}",
            "<a>".repeat(MAX_NESTING_DEPTH),
            "</a>".repeat(MAX_NESTING_DEPTH)
        );
        assert!(texts(&ok).is_ok());
        let deep = format!("{}{}", "<a>".repeat(300), "</a>".repeat(300));
        assert_eq!(texts(&deep).unwrap_err().code, WorkerErrorCode::TooComplex);
    }

    #[test]
    fn bad_xml_is_malformed() {
        for xml in ["<a><b></a>", "<a></b>", "</a>", "<a x='1' x='2'>"] {
            let err = texts(xml).err();
            assert!(
                err.is_none() || err.unwrap().code == WorkerErrorCode::Malformed,
                "{xml}"
            );
        }
        assert_eq!(
            texts("<a><b></a>").unwrap_err().code,
            WorkerErrorCode::Malformed
        );
    }

    #[test]
    fn skip_passes_the_whole_element() {
        let mut x = Xml::new("<a><b><a>x</a></b>y</a>", classify);
        assert!(matches!(x.next().unwrap(), Node::Open(Tag::A, _)));
        assert!(matches!(x.next().unwrap(), Node::Open(Tag::B, _)));
        x.skip().unwrap();
        assert!(matches!(x.next().unwrap(), Node::Text(t) if t == "y"));
        // Skipping an empty element.
        let mut x = Xml::new("<a><b/>z</a>", classify);
        x.next().unwrap();
        x.next().unwrap();
        x.skip().unwrap();
        assert!(matches!(x.next().unwrap(), Node::Text(t) if t == "z"));
    }

    #[test]
    fn attributes_by_local_name_and_prefix() {
        let mut x = Xml::new(r#"<a id="256" r:id="rId2" w:val="0"/>"#, classify);
        let Node::Open(_, e) = x.next().unwrap() else {
            panic!()
        };
        assert_eq!(attr(&e, b"id").as_deref(), Some("256"));
        assert_eq!(prefixed_attr(&e, b"id").as_deref(), Some("rId2"));
        assert!(!val_on(&e));
        assert!(on_off(None) && on_off(Some("1")) && on_off(Some("true")));
        assert!(!on_off(Some("off")) && !on_off(Some("false")));
    }
}
