//! DOCX and PPTX (WP-FT2).
//!
//! A mutated ZIP rarely gets past its CRC checks, so most inputs are XML
//! that this target packs into a valid package itself. The first byte
//! picks the mode (its low two bits) and the character cap (the rest,
//! passed on to `check`):
//!
//! - 0: the rest is a whole file, read as DOCX and as PPTX;
//! - 1: the rest is `word/document.xml`, with styles and notes parts;
//! - 2: the rest is `word/styles.xml`, `footnotes.xml` and `endnotes.xml`
//!   under a fixed body that refers to notes 1 to 3;
//! - 3: the rest is a slide, with itself as its speaker notes.
//!
//! Seeds: `KAGE_FILE_TEXT_OOXML_SEEDS=<dir> cargo test -p kage-file-text
//! --test ooxml_seeds` writes the test fixtures in this format.
#![no_main]

use kage_file_text::Format;
use libfuzzer_sys::fuzz_target;

const DOCX_MAIN: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const PPTX_MAIN: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const STYLES: &[u8] = br#"<w:styles><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style></w:styles>"#;
const FOOTNOTES: &[u8] = br#"<w:footnotes><w:footnote w:id="1"><w:p><w:r><w:t>Note one</w:t></w:r></w:p></w:footnote></w:footnotes>"#;
const BODY: &[u8] = br#"<w:document><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p><w:p><w:r><w:t>A</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:footnoteReference w:id="2"/></w:r><w:r><w:endnoteReference w:id="3"/></w:r></w:p></w:body></w:document>"#;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const CRC: [u32; 256] = crc_table();

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| {
        CRC[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8)
    })
}

/// A ZIP of stored entries with correct CRCs.
fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let mut fields = Vec::new();
        fields.extend(20u16.to_le_bytes());
        fields.extend([0u8; 8]); // flags, method (stored), time, date
        fields.extend(crc32(data).to_le_bytes());
        fields.extend((data.len() as u32).to_le_bytes());
        fields.extend((data.len() as u32).to_le_bytes());
        fields.extend((name.len() as u16).to_le_bytes());
        fields.extend(0u16.to_le_bytes());
        out.extend(b"PK\x03\x04");
        out.extend(&fields);
        out.extend(name.as_bytes());
        out.extend(*data);
        central.extend(b"PK\x01\x02");
        central.extend(20u16.to_le_bytes());
        central.extend(&fields);
        central.extend([0u8; 10]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let at = out.len() as u32;
    let count = (entries.len() as u16).to_le_bytes();
    out.extend(&central);
    out.extend(b"PK\x05\x06");
    out.extend([0u8; 4]);
    out.extend(count);
    out.extend(count);
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

fn types(part: &str, main: &str) -> Vec<u8> {
    format!(r#"<Types><Override PartName="/{part}" ContentType="{main}"/></Types>"#).into_bytes()
}

fn rels(entries: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut xml = String::from("<Relationships>");
    for (id, kind, target) in entries {
        xml.push_str(&format!(
            r#"<Relationship Id="{id}" Type="{REL}/{kind}" Target="{target}"/>"#
        ));
    }
    xml.push_str("</Relationships>");
    xml.into_bytes()
}

fn docx(document: &[u8], styles: &[u8], footnotes: &[u8], endnotes: &[u8]) -> Vec<u8> {
    stored_zip(&[
        (
            "[Content_Types].xml",
            &types("word/document.xml", DOCX_MAIN),
        ),
        (
            "_rels/.rels",
            &rels(&[("rId1", "officeDocument", "word/document.xml")]),
        ),
        ("word/document.xml", document),
        (
            "word/_rels/document.xml.rels",
            &rels(&[
                ("rId1", "styles", "styles.xml"),
                ("rId2", "footnotes", "footnotes.xml"),
                ("rId3", "endnotes", "endnotes.xml"),
            ]),
        ),
        ("word/styles.xml", styles),
        ("word/footnotes.xml", footnotes),
        ("word/endnotes.xml", endnotes),
    ])
}

fn pptx(slide: &[u8]) -> Vec<u8> {
    stored_zip(&[
        ("[Content_Types].xml", &types("ppt/presentation.xml", PPTX_MAIN)),
        ("_rels/.rels", &rels(&[("rId1", "officeDocument", "ppt/presentation.xml")])),
        (
            "ppt/presentation.xml",
            br#"<p:presentation><p:sldIdLst><p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId1"/></p:sldIdLst></p:presentation>"#,
        ),
        ("ppt/_rels/presentation.xml.rels", &rels(&[("rId1", "slide", "slides/slide1.xml")])),
        ("ppt/slides/slide1.xml", slide),
        (
            "ppt/slides/_rels/slide1.xml.rels",
            &rels(&[("rId1", "notesSlide", "../notesSlides/notesSlide1.xml")]),
        ),
        ("ppt/notesSlides/notesSlide1.xml", slide),
    ])
}

fn with_cap(cap: u8, file: Vec<u8>) -> Vec<u8> {
    let mut data = Vec::with_capacity(file.len() + 1);
    data.push(cap);
    data.extend(file);
    data
}

fuzz_target!(|data: &[u8]| {
    let Some((&head, rest)) = data.split_first() else {
        return;
    };
    let cap = head >> 2;
    match head & 3 {
        0 => {
            let file = with_cap(cap, rest.to_vec());
            kage_file_text_fuzz::check(Format::Docx, &file);
            kage_file_text_fuzz::check(Format::Pptx, &file);
        }
        1 => kage_file_text_fuzz::check(
            Format::Docx,
            &with_cap(cap, docx(rest, STYLES, FOOTNOTES, FOOTNOTES)),
        ),
        2 => kage_file_text_fuzz::check(Format::Docx, &with_cap(cap, docx(BODY, rest, rest, rest))),
        _ => kage_file_text_fuzz::check(Format::Pptx, &with_cap(cap, pptx(rest))),
    }
});
