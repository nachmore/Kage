//! DOCX and PPTX files built in the tests (EXTENSION_FILE_TEXT §12): no
//! third-party documents are checked in. Shared by the `ooxml_*` tests.

#![allow(dead_code)]

use kage_file_text::{extract_as, Format, Job, Pages, Warning, WorkerErrorCode, WorkerResponse};
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const W_STRICT: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const CORE_REL: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const DOCX_MAIN: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const PPTX_MAIN: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const NS: &str = r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#;

/// A ZIP of these entries, deflated.
pub fn zip_of(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in entries {
        w.start_file(name.as_str(), opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// XML-escapes text.
pub fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn rels(entries: &[(&str, &str, String)]) -> Vec<u8> {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, kind, target) in entries {
        xml.push_str(&format!(
            r#"<Relationship Id="{id}" Type="{kind}" Target="{}"/>"#,
            esc(target)
        ));
    }
    xml.push_str("</Relationships>");
    xml.into_bytes()
}

fn content_types(main_part: &str, main_type: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/{main_part}" ContentType="{main_type}"/></Types>"#
    )
    .into_bytes()
}

fn core(title: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator>someone</dc:creator><dc:title>{}</dc:title></cp:coreProperties>"#,
        esc(title)
    )
    .into_bytes()
}

// --- DOCX ------------------------------------------------------------

/// A Word document: the body XML (inside `w:body`) and optional parts.
#[derive(Default, Clone)]
pub struct Docx {
    pub body: String,
    pub styles: Option<String>,
    pub footnotes: Option<String>,
    pub endnotes: Option<String>,
    pub title: Option<String>,
    pub strict: bool,
    /// More parts (full names and bytes).
    pub extra: Vec<(String, Vec<u8>)>,
    /// More relationships of the main part: (type's last segment, target).
    pub extra_rels: Vec<(String, String)>,
}

impl Docx {
    pub fn new(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            ..Self::default()
        }
    }

    fn ns(&self) -> &'static str {
        if self.strict {
            W_STRICT
        } else {
            W
        }
    }

    fn rel_type(&self, kind: &str) -> String {
        format!("{}/{kind}", if self.strict { REL_STRICT } else { REL })
    }

    /// A whole part: `inner` under a `w:<root>` element.
    pub fn part(&self, root: &str, inner: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{root} xmlns:w="{}" {NS}>{inner}</w:{root}>"#,
            self.ns()
        )
        .into_bytes()
    }

    /// `word/document.xml`.
    pub fn document_xml(&self) -> Vec<u8> {
        self.part("document", &format!("<w:body>{}</w:body>", self.body))
    }

    pub fn build(&self) -> Vec<u8> {
        let mut package_rels = vec![(
            "rId1",
            self.rel_type("officeDocument"),
            "word/document.xml".to_string(),
        )];
        let mut entries = vec![(
            "[Content_Types].xml".to_string(),
            content_types("word/document.xml", DOCX_MAIN),
        )];
        if let Some(title) = &self.title {
            package_rels.push(("rId2", CORE_REL.to_string(), "docProps/core.xml".into()));
            entries.push(("docProps/core.xml".into(), core(title)));
        }
        let package_rels: Vec<(&str, &str, String)> = package_rels
            .iter()
            .map(|(id, kind, target)| (*id, kind.as_str(), target.clone()))
            .collect();
        entries.push(("_rels/.rels".into(), rels(&package_rels)));
        entries.push(("word/document.xml".into(), self.document_xml()));
        let mut doc_rels: Vec<(String, String, String)> = Vec::new();
        for (kind, root, content) in [
            ("styles", "styles", &self.styles),
            ("footnotes", "footnotes", &self.footnotes),
            ("endnotes", "endnotes", &self.endnotes),
        ] {
            if let Some(content) = content {
                doc_rels.push((
                    format!("rId{}", doc_rels.len() + 10),
                    self.rel_type(kind),
                    format!("{root}.xml"),
                ));
                entries.push((format!("word/{root}.xml"), self.part(root, content)));
            }
        }
        for (kind, target) in &self.extra_rels {
            doc_rels.push((
                format!("rId{}", doc_rels.len() + 10),
                self.rel_type(kind),
                target.clone(),
            ));
        }
        let doc_rels: Vec<(&str, &str, String)> = doc_rels
            .iter()
            .map(|(id, kind, target)| (id.as_str(), kind.as_str(), target.clone()))
            .collect();
        entries.push(("word/_rels/document.xml.rels".into(), rels(&doc_rels)));
        entries.extend(self.extra.iter().cloned());
        zip_of(&entries)
    }
}

/// A run of text.
pub fn r(text: &str) -> String {
    format!(
        r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
        esc(text)
    )
}

/// A plain paragraph.
pub fn p(text: &str) -> String {
    format!("<w:p>{}</w:p>", r(text))
}

/// A paragraph with a style.
pub fn styled(style: &str, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr>{}</w:p>"#,
        r(text)
    )
}

/// A numbered or bulleted paragraph.
pub fn item(level: u8, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="1"/></w:numPr></w:pPr>{}</w:p>"#,
        r(text)
    )
}

/// A table of plain cells.
pub fn table(rows: &[&[&str]]) -> String {
    let mut xml = String::from("<w:tbl><w:tblPr/>");
    for row in rows {
        xml.push_str("<w:tr>");
        for cell in *row {
            xml.push_str(&format!("<w:tc><w:tcPr/>{}</w:tc>", p(cell)));
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str("</w:tbl>");
    xml
}

// --- PPTX ------------------------------------------------------------

/// One slide: its shapes (inside `p:spTree`), whether it's hidden, and
/// its speaker notes.
#[derive(Default, Clone)]
pub struct Slide {
    pub shapes: String,
    pub hidden: bool,
    pub notes: Option<String>,
}

impl Slide {
    pub fn new(shapes: impl Into<String>) -> Self {
        Self {
            shapes: shapes.into(),
            ..Self::default()
        }
    }

    pub fn with_notes(mut self, notes: &str) -> Self {
        self.notes = Some(notes.to_string());
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }
}

/// A presentation. `order` lists slide files (1-based) in show order;
/// by default the slides' own order.
#[derive(Default, Clone)]
pub struct Pptx {
    pub slides: Vec<Slide>,
    pub order: Option<Vec<usize>>,
    pub title: Option<String>,
    pub strict: bool,
    /// Parts that replace (or add to) the built ones: full name, bytes.
    pub replace: Vec<(String, Vec<u8>)>,
}

const P_NS: &str = r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;
const P_NS_STRICT: &str = r#"xmlns:p="http://purl.oclc.org/ooxml/presentationml/main" xmlns:a="http://purl.oclc.org/ooxml/drawingml/main" xmlns:r="http://purl.oclc.org/ooxml/officeDocument/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

impl Pptx {
    pub fn new(slides: Vec<Slide>) -> Self {
        Self {
            slides,
            ..Self::default()
        }
    }

    fn rel_type(&self, kind: &str) -> String {
        format!("{}/{kind}", if self.strict { REL_STRICT } else { REL })
    }

    fn ns(&self) -> &'static str {
        if self.strict {
            P_NS_STRICT
        } else {
            P_NS
        }
    }

    /// A slide part.
    pub fn slide_xml(&self, slide: &Slide) -> Vec<u8> {
        let show = if slide.hidden { r#" show="0""# } else { "" };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {}{show}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{}</p:spTree></p:cSld></p:sld>"#,
            self.ns(),
            slide.shapes
        )
        .into_bytes()
    }

    pub fn build(&self) -> Vec<u8> {
        let mut entries = vec![(
            "[Content_Types].xml".to_string(),
            content_types("ppt/presentation.xml", PPTX_MAIN),
        )];
        let office = self.rel_type("officeDocument");
        let mut package_rels = vec![("rId1", office.as_str(), "ppt/presentation.xml".to_string())];
        if let Some(title) = &self.title {
            package_rels.push(("rId2", CORE_REL, "docProps/core.xml".into()));
            entries.push(("docProps/core.xml".into(), core(title)));
        }
        entries.push(("_rels/.rels".into(), rels(&package_rels)));
        let order = self
            .order
            .clone()
            .unwrap_or_else(|| (1..=self.slides.len()).collect());
        let mut list = String::new();
        for (i, n) in order.iter().enumerate() {
            list.push_str(&format!(r#"<p:sldId id="{}" r:id="rId{n}"/>"#, 256 + i));
        }
        entries.push((
            "ppt/presentation.xml".into(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation {}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId99"/></p:sldMasterIdLst><p:sldIdLst>{list}</p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>"#,
                self.ns()
            )
            .into_bytes(),
        ));
        let slide_type = self.rel_type("slide");
        let ids: Vec<String> = (1..=self.slides.len()).map(|n| format!("rId{n}")).collect();
        let targets: Vec<(&str, &str, String)> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    id.as_str(),
                    slide_type.as_str(),
                    format!("slides/slide{}.xml", i + 1),
                )
            })
            .collect();
        entries.push(("ppt/_rels/presentation.xml.rels".into(), rels(&targets)));
        let notes_type = self.rel_type("notesSlide");
        for (i, slide) in self.slides.iter().enumerate() {
            let n = i + 1;
            entries.push((format!("ppt/slides/slide{n}.xml"), self.slide_xml(slide)));
            if let Some(notes) = &slide.notes {
                entries.push((
                    format!("ppt/slides/_rels/slide{n}.xml.rels"),
                    rels(&[(
                        "rId1",
                        notes_type.as_str(),
                        format!("../notesSlides/notesSlide{n}.xml"),
                    )]),
                ));
                let shapes = format!(
                    "{}{}{}",
                    sp(Some("sldImg"), &[]),
                    sp(Some("body"), &notes.split('\n').collect::<Vec<_>>()),
                    sp(Some("sldNum"), &[&n.to_string()]),
                );
                entries.push((
                    format!("ppt/notesSlides/notesSlide{n}.xml"),
                    format!(
                        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notes {}><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:notes>"#,
                        self.ns()
                    )
                    .into_bytes(),
                ));
            }
        }
        entries.retain(|(name, _)| !self.replace.iter().any(|(n, _)| n == name));
        entries.extend(self.replace.iter().cloned());
        zip_of(&entries)
    }
}

/// `a:p` elements, one per line.
pub fn paras(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|l| {
            format!(
                "<a:p><a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t></a:r></a:p>",
                esc(l)
            )
        })
        .collect()
}

/// A shape: a placeholder of `ph` type (`Some("")` for one without a
/// type), or a plain text box.
pub fn sp(ph: Option<&str>, lines: &[&str]) -> String {
    shape(ph, false, lines)
}

pub fn shape(ph: Option<&str>, hidden: bool, lines: &[&str]) -> String {
    let ph = match ph {
        Some("") => "<p:ph idx=\"1\"/>".to_string(),
        Some(t) => format!(r#"<p:ph type="{t}"/>"#),
        None => String::new(),
    };
    let hidden = if hidden { r#" hidden="1""# } else { "" };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape"{hidden}/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{}</p:txBody></p:sp>"#,
        paras(lines)
    )
}

/// A title placeholder.
pub fn title(text: &str) -> String {
    sp(Some("title"), &[text])
}

// --- Reading -----------------------------------------------------------

/// A successful read, unpacked.
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

pub fn response(format: Format, bytes: &[u8], max_chars: u32) -> WorkerResponse {
    extract_as(&Job::new(format, bytes, max_chars))
}

pub fn read_with(job: &Job<'_>) -> Read {
    match extract_as(job) {
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
            assert_eq!(format, job.format);
            assert_eq!(text.chars().count(), chars as usize);
            Read {
                text,
                chars,
                truncated,
                total_chars,
                title,
                pages,
                page_starts,
                warnings,
            }
        }
        WorkerResponse::Err { code, detail } => panic!("{code:?}: {detail}"),
    }
}

/// Reads with the default character cap; panics on an error.
pub fn read(format: Format, bytes: &[u8]) -> Read {
    read_with(&Job::new(format, bytes, 100_000))
}

pub fn docx(doc: &Docx) -> Read {
    read(Format::Docx, &doc.build())
}

pub fn pptx(deck: &Pptx) -> Read {
    read(Format::Pptx, &deck.build())
}

/// The error code of a read that must fail.
pub fn error(format: Format, bytes: &[u8]) -> WorkerErrorCode {
    match response(format, bytes, 100_000) {
        WorkerResponse::Err { code, .. } => code,
        WorkerResponse::Ok { text, .. } => panic!("expected an error, got {text:?}"),
    }
}
