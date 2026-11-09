//! DOCX and PPTX against hostile files (EXTENSION_FILE_TEXT §5.7, §5.9,
//! §12): ZIP bombs, huge directories, parts that lie about their size,
//! deep XML, entity tricks, encrypted entries.

mod ooxml_fixtures;

use kage_core::file_text_wire::{MAX_PART_INFLATED_BYTES, MAX_ZIP_ENTRIES};
use kage_file_text::{Format, Job, Pages, Warning, WorkerErrorCode};
use ooxml_fixtures::*;
use std::time::{Duration, Instant};

const TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

// --- A hand-made ZIP, for entries the zip crate won't write ----------

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

struct Entry {
    name: &'static str,
    method: u16,
    crc: u32,
    data: Vec<u8>,
    size: u32,
}

fn stored(name: &'static str, data: &[u8]) -> Entry {
    Entry {
        name,
        method: 0,
        crc: crc32(data),
        data: data.to_vec(),
        size: data.len() as u32,
    }
}

fn raw_zip(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let offset = out.len() as u32;
        let header = |sig: &[u8], central: bool| {
            let mut h = sig.to_vec();
            if central {
                h.extend(20u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h.extend(e.method.to_le_bytes());
            h.extend([0u8; 4]);
            h.extend(e.crc.to_le_bytes());
            h.extend((e.data.len() as u32).to_le_bytes());
            h.extend(e.size.to_le_bytes());
            h.extend((e.name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                // Comment length, disk, internal and external attributes.
                h.extend([0u8; 10]);
                h.extend(offset.to_le_bytes());
            }
            h.extend(e.name.as_bytes());
            h
        };
        out.extend(header(b"PK\x03\x04", false));
        out.extend(&e.data);
        central.extend(header(b"PK\x01\x02", true));
    }
    let at = out.len() as u32;
    out.extend(&central);
    out.extend(b"PK\x05\x06");
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// A raw deflate stream of `count` spaces: one literal, then copies of
/// 258 bytes at distance 1, in one fixed-Huffman block. About 13 bits per
/// 258 bytes, so 64 MiB costs about 400 KiB.
fn deflated_spaces(count: usize) -> Vec<u8> {
    struct Bits {
        out: Vec<u8>,
        acc: u32,
        n: u32,
    }
    impl Bits {
        fn put(&mut self, value: u32, len: u32) {
            self.acc |= value << self.n;
            self.n += len;
            while self.n >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.n -= 8;
            }
        }
        /// Huffman codes go most significant bit first.
        fn code(&mut self, code: u32, len: u32) {
            for i in (0..len).rev() {
                self.put((code >> i) & 1, 1);
            }
        }
    }
    let mut bits = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    bits.put(1, 1); // last block
    bits.put(1, 2); // fixed Huffman
    bits.code(0x30 + u32::from(b' '), 8);
    for _ in 0..(count - 1) / 258 {
        bits.code(0xC5, 8); // length 258
        bits.code(0, 5); // distance 1
    }
    bits.code(0, 7); // end of block
    bits.put(0, 7);
    bits.out
}

#[test]
fn a_high_ratio_part_is_too_complex() {
    let count = MAX_PART_INFLATED_BYTES as usize + 1024;
    let bomb = Entry {
        name: "word/document.xml",
        method: 8,
        crc: 0,
        data: deflated_spaces(count),
        // It claims a gigabyte; the cap stops reading at 64 MiB.
        size: 1 << 30,
    };
    assert!(bomb.data.len() < 1 << 20, "a high-ratio entry");
    let bytes = raw_zip(&[stored("[Content_Types].xml", TYPES.as_bytes()), bomb]);
    let start = Instant::now();
    assert_eq!(error(Format::Docx, &bytes), WorkerErrorCode::TooComplex);
    assert!(start.elapsed() < Duration::from_secs(20));

    // The same kind of stream, small, is a valid part: only spaces. So
    // the refusal above is the cap, not a broken stream.
    let size = 1 + 258 * 4;
    let small = Entry {
        name: "word/document.xml",
        method: 8,
        crc: crc32(&vec![b' '; size]),
        data: deflated_spaces(size),
        size: size as u32,
    };
    let bytes = raw_zip(&[stored("[Content_Types].xml", TYPES.as_bytes()), small]);
    let out = read(Format::Docx, &bytes);
    assert_eq!(out.text, "");
    assert_eq!(out.warnings, [Warning::NoText]);
}

#[test]
fn too_many_entries_is_too_complex() {
    let mut entries: Vec<(String, Vec<u8>)> = vec![("[Content_Types].xml".into(), TYPES.into())];
    entries.extend((0..MAX_ZIP_ENTRIES).map(|i| (format!("media/{i}.bin"), Vec::new())));
    let bytes = zip_of(&entries);
    assert_eq!(error(Format::Docx, &bytes), WorkerErrorCode::TooComplex);
}

/// Changes a field of one entry's central directory header.
fn patch_central(bytes: &mut [u8], name: &str, offset: usize, f: impl Fn(u32) -> u32) {
    let mut i = 0;
    while let Some(p) = find(&bytes[i..], b"PK\x01\x02") {
        let at = i + p;
        let len = u16::from_le_bytes([bytes[at + 28], bytes[at + 29]]) as usize;
        if &bytes[at + 46..at + 46 + len] == name.as_bytes() {
            let field = at + offset;
            let old = u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap());
            bytes[field..field + 4].copy_from_slice(&f(old).to_le_bytes());
            return;
        }
        i = at + 4;
    }
    panic!("no entry {name}");
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn a_part_that_inflates_past_its_declared_size_is_malformed() {
    let mut bytes = Docx::new(p(&"words ".repeat(200))).build();
    patch_central(&mut bytes, "word/document.xml", 24, |_| 10);
    assert_eq!(error(Format::Docx, &bytes), WorkerErrorCode::Malformed);
}

#[test]
fn an_encrypted_part_is_encrypted() {
    let mut bytes = Docx::new(p("secret")).build();
    // The general purpose flags sit right after the two version fields.
    patch_central(&mut bytes, "word/document.xml", 6, |v| v | (1 << 16));
    assert_eq!(error(Format::Docx, &bytes), WorkerErrorCode::Encrypted);
    let mut deck = Pptx::new(vec![Slide::new(title("secret"))]).build();
    patch_central(&mut deck, "ppt/slides/slide1.xml", 6, |v| v | (1 << 16));
    assert_eq!(error(Format::Pptx, &deck), WorkerErrorCode::Encrypted);
}

fn nested(depth: usize, inner: &str) -> String {
    format!(
        "{}{inner}{}",
        "<w:sdt><w:sdtContent>".repeat(depth / 2),
        "</w:sdtContent></w:sdt>".repeat(depth / 2)
    )
}

#[test]
fn xml_nested_past_the_cap_is_too_complex() {
    // Fine well inside the cap (the document adds a few levels).
    let ok = Docx::new(nested(240, &p("deep but legal")));
    assert_eq!(docx(&ok).text, "deep but legal");
    let deep = Docx::new(nested(300, &p("too deep"))).build();
    assert_eq!(error(Format::Docx, &deep), WorkerErrorCode::TooComplex);
    // In a part the text doesn't depend on, the limit still holds.
    let deep_styles = Docx {
        styles: Some(format!("{}{}", "<w:x>".repeat(300), "</w:x>".repeat(300))),
        ..Docx::new(p("x"))
    };
    assert_eq!(
        error(Format::Docx, &deep_styles.build()),
        WorkerErrorCode::TooComplex
    );
    let deep_slide = format!("{}{}", "<p:grpSp>".repeat(300), "</p:grpSp>".repeat(300));
    let deck = Pptx::new(vec![Slide::new(deep_slide)]).build();
    assert_eq!(error(Format::Pptx, &deck), WorkerErrorCode::TooComplex);
}

#[test]
fn entities_are_never_expanded_or_fetched() {
    let doc = format!(
        r#"<?xml version="1.0"?><!DOCTYPE w:document [<!ENTITY a "lol"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY x SYSTEM "file:///etc/hosts">]><w:document xmlns:w="{W}"><w:body><w:p><w:r><w:t>[&c;][&x;][&amp;]</w:t></w:r></w:p></w:body></w:document>"#
    );
    let bytes = zip_of(&[
        ("[Content_Types].xml".into(), TYPES.into()),
        ("word/document.xml".into(), doc.into_bytes()),
    ]);
    assert_eq!(read(Format::Docx, &bytes).text, "[][][&]");
}

#[test]
fn one_huge_paragraph_stops_at_the_cap() {
    let text = "word ".repeat(400_000);
    let bytes = Docx::new(format!(
        "<w:p><w:r><w:t>{text}</w:t></w:r><w:r><w:t>{text}</w:t></w:r></w:p>"
    ))
    .build();
    let start = Instant::now();
    let out = read_with(&Job::new(Format::Docx, &bytes, 1000));
    assert!(out.truncated);
    assert!(out.chars <= 1000);
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn a_long_slide_list_counts_every_slide_but_reads_at_most_the_cap() {
    let list: String = (0..5000)
        .map(|i| format!(r#"<p:sldId id="{}" r:id="rId{i}"/>"#, 256 + i))
        .collect();
    let presentation = format!(
        r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst>{list}</p:sldIdLst></p:presentation>"#
    );
    let deck = Pptx {
        replace: vec![("ppt/presentation.xml".into(), presentation.into_bytes())],
        ..Pptx::new(vec![Slide::new(title("Only one file"))])
    };
    let out = pptx(&deck);
    // rId0 isn't a slide; rId1 is the one slide file.
    assert_eq!(out.text, "## Only one file");
    assert_eq!(
        out.pages,
        Some(Pages {
            count: 5000,
            read: 1
        })
    );
    assert_eq!(out.warnings, [Warning::PagesSkipped]);
}
