//! Fonts, before `pdf-extract` reads them (EXTENSION_FILE_TEXT §6.2 step
//! 7, and one addition).
//!
//! [`add_cjk_maps`]: Quartz embeds Hiragino and Apple SD Gothic Neo as CID
//! fonts with an Adobe ordering and no `/ToUnicode` map, and neither PDF
//! crate reads their text. A Type0 font with `Identity-H` or `Identity-V`
//! encoding (so a code is its CID), no `/ToUnicode`, and a descendant
//! whose `/CIDSystemInfo` names Adobe-Japan1, -Korea1, -GB1 or -CNS1 gets
//! a ToUnicode stream built from that ordering's table (`cid_tables.rs`).
//! `pdf-extract` reads a font's map again on every page, and its parser
//! takes about 140 ms for all of a table, so the map holds only the CIDs
//! the page shows: [`CjkMaps::fit`] refills it before each page from what
//! `guard.rs` found in the page's content.
//!
//! [`drop_unusable`] (the addition): the CMap and Type 1 font parsers
//! `pdf-extract` uses recurse once per nested string, array, dictionary
//! or procedure, and panic on an empty stream (which a capped bomb
//! becomes). A font stream they can't take is dropped, so the font reads
//! as if it had none: a ToUnicode map nested deeper than
//! `MAX_NESTING_DEPTH` or empty, and a Type 1 or CFF font program
//! likewise. A Type0 font's encoding CMap is required, so a bad one
//! becomes `Identity-H`.

use super::cid_tables::{CidTable, Mapping, Ordering};
use super::scan;
use super::streams;
use kage_core::file_text_wire::{MAX_NESTING_DEPTH, MAX_PDF_STREAM_BYTES};
use pdf_extract::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;

/// Nested dictionaries searched for font keys inside one object.
const MAX_WALK_DEPTH: usize = 8;
/// Entries per `beginbfrange` or `beginbfchar` block.
const BLOCK: usize = 100;

const IDENTITY_ENCODING: &[u8] = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
1 begincodespacerange <0000> <FFFF> endcodespacerange \
1 begincidrange <0000> <FFFF> 0 endcidrange endcmap end end\n";

/// Keys whose stream a parser in `pdf-extract` reads, dropped when bad.
const DROPPABLE: [&[u8]; 3] = [b"ToUnicode", b"FontFile", b"FontFile3"];

/// What a parser makes of a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parsed {
    /// `/ToUnicode` of any font.
    ToUnicode,
    /// `/FontFile` (Type 1) or `/FontFile3` (CFF) of a descriptor.
    Program,
    /// `/Encoding` of a Type0 font: replaced, not dropped.
    Encoding,
}

fn name_of<'a>(dict: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    dict.get(key).and_then(Object::as_name).ok()
}

fn reference(dict: &Dictionary, key: &[u8]) -> Option<ObjectId> {
    dict.get(key).and_then(Object::as_reference).ok()
}

/// Streams the parsers will read, found in `dict` and the dictionaries
/// inside it.
fn collect_parsed(dict: &Dictionary, depth: usize, out: &mut HashMap<ObjectId, Parsed>) {
    for key in DROPPABLE {
        if let Some(id) = reference(dict, key) {
            let kind = if key == b"ToUnicode" {
                Parsed::ToUnicode
            } else {
                Parsed::Program
            };
            out.insert(id, kind);
        }
    }
    if name_of(dict, b"Subtype") == Some(b"Type0") {
        if let Some(id) = reference(dict, b"Encoding") {
            out.insert(id, Parsed::Encoding);
        }
    }
    if depth >= MAX_WALK_DEPTH {
        return;
    }
    for (_, value) in dict.iter() {
        if let Object::Dictionary(inner) = value {
            collect_parsed(inner, depth + 1, out);
        }
    }
}

/// Removes droppable keys that point at a bad stream.
fn drop_keys(dict: &mut Dictionary, depth: usize, bad: &HashSet<ObjectId>) {
    for key in DROPPABLE {
        if reference(dict, key).is_some_and(|id| bad.contains(&id)) {
            dict.remove(key);
        }
    }
    if depth >= MAX_WALK_DEPTH {
        return;
    }
    for (_, value) in dict.iter_mut() {
        if let Object::Dictionary(inner) = value {
            drop_keys(inner, depth + 1, bad);
        }
    }
}

/// Whether a parser can take this stream: not empty, not too deep.
fn usable(stream: &Stream, kind: Parsed) -> bool {
    let Some(data) = streams::decoded(stream, MAX_PDF_STREAM_BYTES as usize) else {
        return false;
    };
    if data.iter().all(u8::is_ascii_whitespace) {
        return false;
    }
    // A CFF program is binary, and its parser doesn't recurse.
    let cff = kind == Parsed::Program && stream.dict.has(b"Subtype");
    cff || scan::max_nesting(&data) <= MAX_NESTING_DEPTH
}

/// Drops or replaces font streams the parsers can't take (module docs).
/// Returns how many.
pub fn drop_unusable(doc: &mut Document) -> usize {
    let mut parsed = HashMap::new();
    for object in doc.objects.values() {
        match object {
            Object::Dictionary(d) => collect_parsed(d, 0, &mut parsed),
            Object::Stream(s) => collect_parsed(&s.dict, 0, &mut parsed),
            _ => {}
        }
    }
    let mut dropped = HashSet::new();
    let mut fixed = 0;
    for (id, kind) in parsed {
        let Ok(Object::Stream(stream)) = doc.get_object_mut(id) else {
            continue;
        };
        if usable(stream, kind) {
            continue;
        }
        fixed += 1;
        match kind {
            Parsed::Encoding => streams::replace(stream, IDENTITY_ENCODING.to_vec()),
            Parsed::ToUnicode | Parsed::Program => {
                dropped.insert(id);
            }
        }
    }
    if !dropped.is_empty() {
        for object in doc.objects.values_mut() {
            match object {
                Object::Dictionary(d) => drop_keys(d, 0, &dropped),
                Object::Stream(s) => drop_keys(&mut s.dict, 0, &dropped),
                _ => {}
            }
        }
    }
    fixed
}

/// The ordering of a Type0 font that needs a map (module docs).
fn needs_map(doc: &Document, font: &Dictionary) -> Option<Ordering> {
    if name_of(font, b"Subtype") != Some(b"Type0") || font.has(b"ToUnicode") {
        return None;
    }
    if !matches!(
        name_of(font, b"Encoding"),
        Some(b"Identity-H" | b"Identity-V")
    ) {
        return None;
    }
    let descendants = resolve(doc, font.get(b"DescendantFonts").ok()?)?;
    let first = resolve(doc, descendants.as_array().ok()?.first()?)?;
    let info = resolve(doc, first.as_dict().ok()?.get(b"CIDSystemInfo").ok()?)?;
    let info = info.as_dict().ok()?;
    let text = |key: &[u8]| info.get(key).and_then(Object::as_str).ok();
    Ordering::from_names(text(b"Registry")?, text(b"Ordering")?)
}

fn resolve<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
    doc.dereference(object).ok().map(|(_, o)| o)
}

/// The CJK fonts that got a map, and one map stream per ordering.
#[derive(Default)]
pub struct CjkMaps {
    fonts: HashMap<ObjectId, Ordering>,
    maps: HashMap<Ordering, (ObjectId, CidTable)>,
}

impl CjkMaps {
    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    pub fn fonts(&self) -> HashSet<ObjectId> {
        self.fonts.keys().copied().collect()
    }

    /// Before a page: fills each map with the CIDs its fonts show there.
    /// A map nothing on the page uses keeps what it had.
    pub fn fit(&self, doc: &mut Document, shown: &HashMap<ObjectId, BTreeSet<u16>>) {
        for (ordering, (stream, table)) in &self.maps {
            let cids: BTreeSet<u16> = shown
                .iter()
                .filter(|(font, _)| self.fonts.get(font) == Some(ordering))
                .flat_map(|(_, codes)| codes.iter().copied())
                .collect();
            if cids.is_empty() {
                continue;
            }
            if let Ok(Object::Stream(s)) = doc.get_object_mut(*stream) {
                s.set_content(to_unicode_cmap(table, *ordering, &cids));
            }
        }
    }
}

/// Points Quartz-style CJK fonts at a ToUnicode map of their ordering
/// (module docs), empty until [`CjkMaps::fit`] fills it for a page.
pub fn add_cjk_maps(doc: &mut Document) -> CjkMaps {
    let wanted: Vec<(ObjectId, Ordering)> = doc
        .objects
        .iter()
        .filter_map(|(&id, object)| Some((id, needs_map(doc, object.as_dict().ok()?)?)))
        .collect();
    let mut cjk = CjkMaps::default();
    for (font, ordering) in wanted {
        let map = match cjk.maps.get(&ordering) {
            Some((id, _)) => *id,
            None => {
                let (Some(table), Some(id)) = (ordering.table(), free_id(doc)) else {
                    continue;
                };
                let empty = to_unicode_cmap(&table, ordering, &BTreeSet::new());
                doc.objects
                    .insert(id, Object::Stream(Stream::new(Dictionary::new(), empty)));
                doc.max_id = doc.max_id.max(id.0);
                cjk.maps.insert(ordering, (id, table));
                id
            }
        };
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(font) {
            dict.set("ToUnicode", Object::Reference(map));
            cjk.fonts.insert(font, ordering);
        }
    }
    cjk
}

fn free_id(doc: &Document) -> Option<ObjectId> {
    let last = doc.objects.keys().next_back().map_or(0, |id| id.0);
    Some((last.max(doc.max_id).checked_add(1)?, 0))
}

fn utf16_hex(text: &str, out: &mut String) {
    for unit in text.encode_utf16() {
        let _ = write!(out, "{unit:04x}");
    }
}

/// A ToUnicode CMap from two-byte CIDs to the table's text for `cids`, as
/// `pdf-extract`'s CMap parser reads it: runs of consecutive BMP
/// characters as `bfrange`, everything else as `bfchar`.
pub fn to_unicode_cmap(table: &CidTable, ordering: Ordering, cids: &BTreeSet<u16>) -> Vec<u8> {
    let mut ranges: Vec<(u32, u32, u32)> = Vec::new();
    let mut chars: Vec<(u32, String)> = Vec::new();
    for cid in cids.iter().map(|&c| u32::from(c)) {
        match table.lookup(cid) {
            None => {}
            Some(Mapping::Char(c)) if (c as u32) <= 0xFFFF => match ranges.last_mut() {
                Some((lo, hi, dst)) if *hi + 1 == cid && *dst + (cid - *lo) == c as u32 => {
                    *hi = cid;
                }
                _ => ranges.push((cid, cid, c as u32)),
            },
            Some(Mapping::Char(c)) => chars.push((cid, c.to_string())),
            Some(Mapping::Text(t)) => chars.push((cid, t.to_string())),
        }
    }
    let mut out = String::with_capacity(ranges.len() * 22 + chars.len() * 24 + 512);
    let _ = write!(
        out,
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CMapName /Kage-Adobe-{}-UCS2 def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <ffff>\nendcodespacerange\n",
        ordering.name()
    );
    for block in ranges.chunks(BLOCK) {
        let _ = writeln!(out, "{} beginbfrange", block.len());
        for &(lo, hi, dst) in block {
            let _ = writeln!(out, "<{lo:04x}> <{hi:04x}> <{dst:04x}>");
        }
        out.push_str("endbfrange\n");
    }
    for block in chars.chunks(BLOCK) {
        let _ = writeln!(out, "{} beginbfchar", block.len());
        for (cid, text) in block {
            let _ = write!(out, "<{cid:04x}> <");
            utf16_hex(text, &mut out);
            out.push_str(">\n");
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out.into_bytes()
}
