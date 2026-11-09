//! Seeds for the `ooxml` fuzz target (EXTENSION_FILE_TEXT §12), built
//! from the same fixtures as the tests. Every sample must read cleanly.
//! With `KAGE_FILE_TEXT_OOXML_SEEDS` naming a folder, they are also
//! written there in the target's input format (`fuzz/fuzz_targets/ooxml.rs`):
//!
//!   KAGE_FILE_TEXT_OOXML_SEEDS=fuzz/corpus/ooxml cargo test -p kage-file-text --test ooxml_seeds

mod ooxml_fixtures;

use kage_file_text::{extract_as, Format, Job, WorkerResponse};
use ooxml_fixtures::*;

/// The fuzz target's modes (its first byte's low two bits).
const WHOLE_FILE: u8 = 0;
const DOCUMENT_XML: u8 = 1;
const SIDE_PARTS: u8 = 2;
const SLIDE_XML: u8 = 3;
/// Character caps, as indexes into the fuzz crate's table: 100,000 and 20.
const CAPS: [u8; 2] = [6, 2];

fn documents() -> Vec<Docx> {
    let notes = r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t> A note.</w:t></w:r></w:p></w:footnote>"#;
    vec![
        Docx {
            title: Some("Seed".into()),
            ..Docx::new(
                [
                    styled("Title", "Plan"),
                    styled("Heading1", "Goals"),
                    item(0, "One"),
                    item(1, "Two"),
                    p("Body & more."),
                ]
                .concat(),
            )
        },
        Docx::new([table(&[&["a", "b"], &["", "c"]]), p("after")].concat()),
        Docx {
            footnotes: Some(notes.into()),
            styles: Some(
                r#"<w:style w:type="paragraph" w:styleId="H"><w:name w:val="heading 2"/></w:style><w:style w:type="character" w:styleId="X"><w:rPr><w:vanish/></w:rPr></w:style>"#
                    .into(),
            ),
            ..Docx::new(format!(
                r#"<w:p><w:pPr><w:pStyle w:val="H"/></w:pPr>{}<w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:rPr><w:rStyle w:val="X"/></w:rPr><w:t>hidden</w:t></w:r></w:p>"#,
                r("Styled")
            ))
        },
        Docx::new(format!(
            r#"<w:p>{}<w:r><mc:AlternateContent><mc:Choice><w:drawing><wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx></w:drawing></mc:Choice><mc:Fallback><w:pict/></mc:Fallback></mc:AlternateContent></w:r><w:del><w:r><w:delText>x</w:delText></w:r></w:del><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:tab/><w:br/><w:t>&#x41;&amp;</w:t></w:r></w:p>"#,
            r("Box:"),
            p("inside"),
            r("3")
        )),
        Docx {
            strict: true,
            ..Docx::new(styled("Heading1", "Strict"))
        },
    ]
}

fn decks() -> Vec<Pptx> {
    let group = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="5" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{}</p:grpSp><mc:AlternateContent><mc:Choice>{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        sp(None, &["grouped"]),
        sp(None, &["choice"]),
        sp(None, &["fallback"])
    );
    let table = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="T"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><a:graphic><a:graphicData><a:tbl><a:tr><a:tc><a:txBody><a:p><a:r><a:t>x</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:fld><a:t>y</a:t></a:fld><a:br/></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#;
    vec![
        Pptx {
            title: Some("Deck".into()),
            order: Some(vec![2, 1, 3]),
            ..Pptx::new(vec![
                Slide::new(format!("{}{}", title("One"), sp(Some(""), &["a", "b"])))
                    .with_notes("spoken"),
                Slide::new(sp(Some("sldNum"), &["2"])),
                Slide::new(title("Hidden")).hidden(),
            ])
        },
        Pptx::new(vec![Slide::new(format!(
            "{group}{table}{}",
            shape(None, true, &["hidden"])
        ))]),
        Pptx {
            strict: true,
            ..Pptx::new(vec![Slide::new(sp(Some("ctrTitle"), &["Strict"]))])
        },
    ]
}

fn seeds() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut add = |name: String, mode: u8, data: Vec<u8>| {
        for cap in CAPS {
            let mut seed = vec![(cap << 2) | mode];
            seed.extend(&data);
            out.push((format!("{name}-cap{cap}"), seed));
        }
    };
    for (i, doc) in documents().iter().enumerate() {
        add(format!("docx-{i}"), WHOLE_FILE, doc.build());
        add(format!("document-{i}"), DOCUMENT_XML, doc.document_xml());
        for (part, root) in [(&doc.styles, "styles"), (&doc.footnotes, "footnotes")] {
            if let Some(inner) = part {
                add(format!("{root}-{i}"), SIDE_PARTS, doc.part(root, inner));
            }
        }
    }
    for (i, deck) in decks().iter().enumerate() {
        add(format!("pptx-{i}"), WHOLE_FILE, deck.build());
        for (j, slide) in deck.slides.iter().enumerate() {
            add(format!("slide-{i}-{j}"), SLIDE_XML, deck.slide_xml(slide));
        }
    }
    out
}

#[test]
fn samples_read_cleanly() {
    for doc in documents() {
        let bytes = doc.build();
        assert!(matches!(
            extract_as(&Job::new(Format::Docx, &bytes, 1000)),
            WorkerResponse::Ok { .. }
        ));
    }
    for deck in decks() {
        let bytes = deck.build();
        assert!(matches!(
            extract_as(&Job::new(Format::Pptx, &bytes, 1000)),
            WorkerResponse::Ok { .. }
        ));
    }
}

#[test]
fn write_seeds_when_asked() {
    let Some(dir) = std::env::var_os("KAGE_FILE_TEXT_OOXML_SEEDS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, seed) in seeds() {
        std::fs::write(dir.join(name), seed).unwrap();
    }
}
