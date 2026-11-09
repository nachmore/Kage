//! Small PDFs for the PDF reader's tests, built here with `lopdf` 0.47
//! (EXTENSION_FILE_TEXT §12: no third-party documents are checked in).
#![allow(dead_code)]

use kage_file_text::{extract_as, Format, Job, Pages, Warning, WorkerErrorCode, WorkerResponse};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use std::io::Write;

pub const CAP: usize = kage_core::file_text_wire::MAX_PDF_STREAM_BYTES as usize;

/// The reader's answer, unpacked.
#[derive(Debug)]
pub struct Read {
    pub text: String,
    pub chars: u32,
    pub truncated: bool,
    pub total_chars: Option<u32>,
    pub title: Option<String>,
    pub pages: Option<Pages>,
    pub page_starts: Option<Vec<u32>>,
    pub warnings: Vec<Warning>,
}

pub fn read_with(bytes: &[u8], max_chars: u32) -> Result<Read, WorkerErrorCode> {
    match extract_as(&Job::new(Format::Pdf, bytes, max_chars)) {
        WorkerResponse::Ok {
            format,
            text,
            chars,
            truncated,
            total_chars,
            title,
            pages,
            page_starts,
            warnings,
        } => {
            assert_eq!(format, Format::Pdf);
            assert_eq!(text.chars().count() as u32, chars);
            assert!(chars <= max_chars);
            Ok(Read {
                text,
                chars,
                truncated,
                total_chars,
                title,
                pages,
                page_starts,
                warnings,
            })
        }
        WorkerResponse::Err { code, .. } => Err(code),
    }
}

pub fn read(bytes: &[u8]) -> Result<Read, WorkerErrorCode> {
    read_with(bytes, 100_000)
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// A Flate stream that inflates to just past the stream cap.
pub fn bomb_stream(mut dict: Dictionary) -> Stream {
    dict.set("Filter", "FlateDecode");
    Stream::new(dict, zlib(&vec![b' '; CAP + 1]))
}

/// A literal PDF string, escaped.
pub fn literal(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        if matches!(c, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push(')');
    out
}

/// Content that shows each line with Helvetica (`/F1`), one below the
/// other; an empty line leaves a paragraph gap.
pub fn lines(lines: &[&str]) -> String {
    let mut out = String::from("BT /F1 12 Tf 72 720 Td\n");
    let mut gap = 0;
    for line in lines {
        if line.is_empty() {
            gap += 1;
            continue;
        }
        if gap > 0 || !out.ends_with("Td\n") {
            out.push_str(&format!("0 {} Td\n", -14 - 24 * gap));
        }
        gap = 0;
        out.push_str(&literal(line));
        out.push_str(" Tj\n");
    }
    out.push_str("ET\n");
    out
}

/// A document under construction.
pub struct Pdf {
    pub doc: Document,
    pub pages: ObjectId,
    pub font: ObjectId,
    pub kids: Vec<ObjectId>,
}

impl Default for Pdf {
    fn default() -> Self {
        Self::new()
    }
}

impl Pdf {
    pub fn new() -> Self {
        let mut doc = Document::with_version("1.5");
        let pages = doc.new_object_id();
        let font = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        Pdf {
            doc,
            pages,
            font,
            kids: Vec::new(),
        }
    }

    pub fn add(&mut self, object: impl Into<Object>) -> ObjectId {
        self.doc.add_object(object)
    }

    /// The standard resources: Helvetica as `/F1`, plus `extra` entries.
    pub fn resources(&self, extra: Dictionary) -> Dictionary {
        let mut fonts = dictionary! { "F1" => self.font };
        if let Ok(Object::Dictionary(more)) = extra.get(b"Font") {
            for (k, v) in more.iter() {
                fonts.set(k.clone(), v.clone());
            }
        }
        let mut res = dictionary! { "Font" => fonts };
        for (k, v) in extra.iter() {
            if k.as_slice() != b"Font" {
                res.set(k.clone(), v.clone());
            }
        }
        res
    }

    /// Adds a page with this content and these resources.
    pub fn page_with(&mut self, content: &[u8], resources: Dictionary) -> ObjectId {
        let content = self.add(Stream::new(Dictionary::new(), content.to_vec()));
        let page = self.add(dictionary! {
            "Type" => "Page",
            "Parent" => self.pages,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => resources,
            "Contents" => content,
        });
        self.kids.push(page);
        page
    }

    /// Adds a page that shows these lines.
    pub fn page(&mut self, text: &[&str]) -> ObjectId {
        let resources = self.resources(Dictionary::new());
        self.page_with(lines(text).as_bytes(), resources)
    }

    pub fn title(&mut self, title: Object) {
        let info = self.add(dictionary! { "Title" => title });
        self.doc.trailer.set("Info", info);
    }

    /// Writes the page tree and the file ID.
    pub fn finish(&mut self) {
        let kids: Vec<Object> = self.kids.iter().map(|&k| k.into()).collect();
        let count = kids.len() as i64;
        self.doc.objects.insert(
            self.pages,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
        );
        let id = Object::String(b"0123456789abcdef".to_vec(), StringFormat::Hexadecimal);
        self.doc.trailer.set("ID", vec![id.clone(), id]);
    }

    pub fn bytes(mut self) -> Vec<u8> {
        self.finish();
        save(&mut self.doc)
    }
}

pub fn save(doc: &mut Document) -> Vec<u8> {
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// A simple-font `/ToUnicode` CMap mapping one-byte codes to text.
pub fn to_unicode(pairs: &[(u8, &str)]) -> Vec<u8> {
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
         1 begincodespacerange <00> <FF> endcodespacerange\n",
    );
    out.push_str(&format!("{} beginbfchar\n", pairs.len()));
    for (code, text) in pairs {
        out.push_str(&format!("<{code:02x}> <"));
        for unit in text.encode_utf16() {
            out.push_str(&format!("{unit:04x}"));
        }
        out.push_str(">\n");
    }
    out.push_str("endbfchar endcmap end end\n");
    out.into_bytes()
}

/// Helvetica with a `/ToUnicode` map, as `/F2`.
pub fn mapped_font(pdf: &mut Pdf, pairs: &[(u8, &str)]) -> Dictionary {
    let map = pdf.add(Stream::new(Dictionary::new(), to_unicode(pairs)));
    let font = pdf.add(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
        "ToUnicode" => map,
    });
    dictionary! { "Font" => dictionary! { "F2" => font } }
}

/// A Quartz-style CID font: Identity-H, an Adobe ordering, no ToUnicode.
pub fn cid_font(pdf: &mut Pdf, ordering: &str) -> ObjectId {
    let descriptor = pdf.add(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "HiraginoSans-W3",
        "Flags" => 4,
    });
    let info = dictionary! {
        "Registry" => Object::string_literal("Adobe"),
        "Ordering" => Object::string_literal(ordering),
        "Supplement" => 6,
    };
    let descendant = pdf.add(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType0",
        "BaseFont" => "HiraginoSans-W3",
        "CIDSystemInfo" => info,
        "FontDescriptor" => descriptor,
        "DW" => 1000,
    });
    pdf.add(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "HiraginoSans-W3",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![descendant.into()],
    })
}

/// `<…> Tj` for two-byte CIDs.
pub fn show_cids(cids: &[u16]) -> String {
    let hex: String = cids.iter().map(|c| format!("{c:04X}")).collect();
    format!("<{hex}> Tj")
}
