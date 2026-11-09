//! The OOXML package: parts, relationships and the inflate budget
//! (EXTENSION_FILE_TEXT §5.9).
//!
//! Only the parts a reader asks for are inflated, each through
//! `sniff::read_entry`'s capped reader: at most `MAX_PART_INFLATED_BYTES`
//! for one part and `MAX_TOTAL_INFLATED_BYTES` for all of them. Past either
//! cap: `too_complex`. Media and every other part stay compressed.
//!
//! Parts are found the OPC way: the package relationships
//! (`_rels/.rels`) name the main part, and each part's relationships
//! (`<folder>/_rels/<name>.rels`) name the parts it uses. Relationship
//! types are matched by their last segment, so Transitional
//! (`schemas.openxmlformats.org`) and Strict (`purl.oclc.org`) read the
//! same. Part names match without regard to ASCII case.

use super::xml::{self, Node, Xml};
use crate::sniff::{open_package, read_entry, xml_text};
use crate::{Failure, WorkerErrorCode};
use kage_core::file_text_wire::{
    MAX_PART_INFLATED_BYTES, MAX_TOTAL_INFLATED_BYTES, MAX_ZIP_ENTRIES,
};
use std::collections::HashMap;
use std::io::Cursor;
use zip::ZipArchive;

/// Characters of a core-properties title kept before cleaning.
const RAW_TITLE_CHARS: usize = 1024;

/// One internal relationship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rel {
    pub id: String,
    /// The type's last segment, lowercase (`officedocument`, `slide`, …).
    pub kind: String,
    /// The target part's name: no leading slash, folders resolved.
    pub target: String,
}

/// The first target of a relationship kind.
pub fn target_of<'r>(rels: &'r [Rel], kind: &str) -> Option<&'r str> {
    rels.iter()
        .find(|r| r.kind == kind)
        .map(|r| r.target.as_str())
}

/// An open package with what is left of its inflate budget.
pub struct Package<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    /// Lowercase entry name → index (the first entry of each name).
    names: HashMap<String, usize>,
    part_cap: u64,
    budget: u64,
}

impl<'a> Package<'a> {
    /// Opens a package with this build's caps.
    pub fn open(bytes: &'a [u8]) -> Result<Self, Failure> {
        Self::with_caps(bytes, MAX_PART_INFLATED_BYTES, MAX_TOTAL_INFLATED_BYTES)
    }

    pub(crate) fn with_caps(bytes: &'a [u8], part_cap: u64, budget: u64) -> Result<Self, Failure> {
        let archive = open_package(bytes)?;
        let mut names = HashMap::with_capacity(archive.len());
        for i in 0..archive.len() {
            if let Some(name) = archive.name_for_index(i) {
                let key = name.replace('\\', "/");
                let key = key.trim_start_matches('/').to_ascii_lowercase();
                names.entry(key).or_insert(i);
            }
        }
        Ok(Self {
            archive,
            names,
            part_cap,
            budget,
        })
    }

    fn index_of(&self, part: &str) -> Option<usize> {
        let key = part.to_ascii_lowercase();
        self.names.get(&key).copied().or_else(|| {
            let decoded = percent_decode(&key)?;
            self.names.get(&decoded).copied()
        })
    }

    /// Inflates a part. `None` if the package has no such part.
    pub fn read(&mut self, part: &str) -> Result<Option<Vec<u8>>, Failure> {
        let Some(index) = self.index_of(part) else {
            return Ok(None);
        };
        let cap = self.part_cap.min(self.budget);
        let bytes = read_entry(&mut self.archive, index, cap)?;
        self.budget -= bytes.len() as u64;
        Ok(Some(bytes))
    }

    /// The relationships of a part (`""` for the package's own). Missing:
    /// none. External targets are left out.
    pub fn rels(&mut self, source: &str) -> Result<Vec<Rel>, Failure> {
        let Some(bytes) = self.read(&rels_name(source))? else {
            return Ok(Vec::new());
        };
        parse_rels(&xml_text(&bytes)?, source)
    }

    /// The main part: the package's `officeDocument` relationship, else
    /// the usual name.
    pub fn main_part(rels: &[Rel], usual: &str) -> String {
        target_of(rels, "officedocument")
            .unwrap_or(usual)
            .to_string()
    }

    /// `dc:title` from the core properties, raw. Best effort: a damaged
    /// core part has no title.
    pub fn core_title(&mut self, package_rels: &[Rel]) -> Result<Option<String>, Failure> {
        let part = target_of(package_rels, "core-properties").unwrap_or("docProps/core.xml");
        let Some(bytes) = secondary(self.read(part))?.flatten() else {
            return Ok(None);
        };
        let Some(text) = secondary(xml_text(&bytes))? else {
            return Ok(None);
        };
        Ok(secondary(core_title(&text))?.flatten())
    }
}

/// For parts the text doesn't depend on (styles, notes, core properties):
/// damage there is ignored (`None`), but the safety limits still stop the
/// whole read.
pub fn secondary<T>(result: Result<T, Failure>) -> Result<Option<T>, Failure> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(f) if f.code == WorkerErrorCode::Malformed => Ok(None),
        Err(f) => Err(f),
    }
}

/// `word/document.xml` → `word/_rels/document.xml.rels`; `""` →
/// `_rels/.rels`.
pub fn rels_name(source: &str) -> String {
    match source.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{source}.rels"),
    }
}

/// The part a relationship target names, from the part that holds the
/// relationship. `None` if it climbs out of the package.
pub fn resolve(source: &str, target: &str) -> Option<String> {
    let target = target
        .split('#')
        .next()
        .unwrap_or_default()
        .replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    if !target.starts_with('/') {
        if let Some((dir, _)) = source.rsplit_once('/') {
            parts.extend(dir.split('/').filter(|s| !s.is_empty()));
        }
    }
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `%20` and friends, for part names stored decoded.
fn percent_decode(name: &str) -> Option<String> {
    if !name.contains('%') {
        return None;
    }
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = name.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(|s| s.to_ascii_lowercase())
}

#[derive(Clone, Copy, PartialEq)]
enum RelTag {
    Relationship,
    Title,
    Other,
}

fn classify(local: &[u8]) -> RelTag {
    match local {
        b"Relationship" => RelTag::Relationship,
        b"title" => RelTag::Title,
        _ => RelTag::Other,
    }
}

fn parse_rels(text: &str, source: &str) -> Result<Vec<Rel>, Failure> {
    let mut xml = Xml::new(text, classify);
    let mut rels = Vec::new();
    loop {
        match xml.next()? {
            Node::Open(RelTag::Relationship, e) => {
                if rels.len() >= MAX_ZIP_ENTRIES {
                    continue;
                }
                let external = xml::attr(&e, b"TargetMode")
                    .is_some_and(|m| m.trim().eq_ignore_ascii_case("external"));
                let (Some(id), Some(kind), Some(target)) = (
                    xml::attr(&e, b"Id"),
                    xml::attr(&e, b"Type"),
                    xml::attr(&e, b"Target"),
                ) else {
                    continue;
                };
                if external {
                    continue;
                }
                let Some(target) = resolve(source, target.trim()) else {
                    continue;
                };
                let kind = kind.trim().rsplit('/').next().unwrap_or_default();
                rels.push(Rel {
                    id: id.trim().to_string(),
                    kind: kind.to_ascii_lowercase(),
                    target,
                });
            }
            Node::Eof => return Ok(rels),
            _ => {}
        }
    }
}

/// The text of the first `title` (`dc:title`) right under the root.
fn core_title(text: &str) -> Result<Option<String>, Failure> {
    let mut xml = Xml::new(text, classify);
    let mut title: Option<String> = None;
    loop {
        match xml.next()? {
            Node::Open(RelTag::Title, _) if xml.depth() == 2 && title.is_none() => {
                title = Some(String::new());
            }
            Node::Text(t) if xml.depth() == 2 && xml.up(0) == Some(RelTag::Title) => {
                if let Some(title) = title.as_mut() {
                    let room = RAW_TITLE_CHARS.saturating_sub(title.chars().count());
                    title.extend(t.chars().take(room));
                }
            }
            Node::Close(RelTag::Title) if xml.depth() == 1 && title.is_some() => {
                return Ok(title.filter(|t| !t.trim().is_empty()));
            }
            Node::Eof => return Ok(None),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn rels_live_next_to_their_part() {
        assert_eq!(rels_name(""), "_rels/.rels");
        assert_eq!(
            rels_name("word/document.xml"),
            "word/_rels/document.xml.rels"
        );
        assert_eq!(
            rels_name("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
    }

    #[test]
    fn targets_resolve_from_the_source_folder() {
        assert_eq!(
            resolve("", "word/document.xml").as_deref(),
            Some("word/document.xml")
        );
        assert_eq!(
            resolve("", "/word/document.xml").as_deref(),
            Some("word/document.xml")
        );
        assert_eq!(
            resolve("word/document.xml", "styles.xml").as_deref(),
            Some("word/styles.xml")
        );
        assert_eq!(
            resolve("ppt/slides/slide1.xml", "../notesSlides/notesSlide1.xml").as_deref(),
            Some("ppt/notesSlides/notesSlide1.xml")
        );
        assert_eq!(
            resolve("ppt/presentation.xml", "/ppt/slides/./slide2.xml").as_deref(),
            Some("ppt/slides/slide2.xml")
        );
        assert_eq!(
            resolve("word\\document.xml", "media\\a.png#x").as_deref(),
            Some("media/a.png")
        );
        assert_eq!(resolve("word/document.xml", "../../x.xml"), None);
        assert_eq!(resolve("", ""), None);
    }

    #[test]
    fn rels_keep_internal_targets_by_kind() {
        let xml = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
  <Relationship Id="rId2" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/footnotes" Target="/word/footnotes.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
  <Relationship Id="rId4" Type="x/y" Target="../../escape.xml"/>
</Relationships>"#;
        let rels = parse_rels(xml, "word/document.xml").unwrap();
        assert_eq!(
            rels,
            [
                Rel {
                    id: "rId1".into(),
                    kind: "styles".into(),
                    target: "word/styles.xml".into()
                },
                Rel {
                    id: "rId2".into(),
                    kind: "footnotes".into(),
                    target: "word/footnotes.xml".into()
                },
            ]
        );
        assert_eq!(target_of(&rels, "footnotes"), Some("word/footnotes.xml"));
    }

    #[test]
    fn core_title_is_the_first_title_under_the_root() {
        let xml = r#"<cp:coreProperties xmlns:cp="c" xmlns:dc="d"><x><dc:title>no</dc:title></x><dc:title>Q3 &amp; plan</dc:title><dc:title>later</dc:title></cp:coreProperties>"#;
        assert_eq!(core_title(xml).unwrap().as_deref(), Some("Q3 & plan"));
        assert_eq!(
            core_title("<cp:coreProperties><dc:title> </dc:title></cp:coreProperties>").unwrap(),
            None
        );
        assert_eq!(core_title("<a/>").unwrap(), None);
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn parts_match_without_case_and_decoded() {
        let bytes = zip_of(&[("Word/Document.XML", b"a"), ("word/my doc.xml", b"b")]);
        let mut pkg = Package::open(&bytes).unwrap();
        assert_eq!(pkg.read("word/document.xml").unwrap().unwrap(), b"a");
        assert_eq!(pkg.read("word/my%20doc.xml").unwrap().unwrap(), b"b");
        assert_eq!(pkg.read("word/missing.xml").unwrap(), None);
    }

    #[test]
    fn the_inflate_budget_covers_all_parts() {
        let part = vec![b' '; 600];
        let bytes = zip_of(&[("a.xml", &part), ("b.xml", &part), ("c.xml", &part)]);
        // One part over its own cap.
        let mut pkg = Package::with_caps(&bytes, 500, 10_000).unwrap();
        assert_eq!(
            pkg.read("a.xml").unwrap_err().code,
            WorkerErrorCode::TooComplex
        );
        // Each part fits, but not all three together.
        let mut pkg = Package::with_caps(&bytes, 1_000, 1_500).unwrap();
        assert!(pkg.read("a.xml").unwrap().is_some());
        assert!(pkg.read("b.xml").unwrap().is_some());
        assert_eq!(
            pkg.read("c.xml").unwrap_err().code,
            WorkerErrorCode::TooComplex
        );
        // Reading the same part again counts again.
        let mut pkg = Package::with_caps(&bytes, 1_000, 1_000).unwrap();
        assert!(pkg.read("a.xml").unwrap().is_some());
        assert_eq!(
            pkg.read("a.xml").unwrap_err().code,
            WorkerErrorCode::TooComplex
        );
    }

    #[test]
    fn damage_in_secondary_parts_is_ignored_but_limits_are_not() {
        let malformed = Failure::new(WorkerErrorCode::Malformed, "x");
        let deep = Failure::new(WorkerErrorCode::TooComplex, "x");
        assert_eq!(secondary::<()>(Err(malformed)).unwrap(), None);
        assert_eq!(
            secondary::<()>(Err(deep)).unwrap_err().code,
            WorkerErrorCode::TooComplex
        );
        assert_eq!(secondary(Ok(1)).unwrap(), Some(1));
    }
}
