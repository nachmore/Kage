//! DOCX text conventions (EXTENSION_FILE_TEXT §3.7, §5.9, §12).

mod ooxml_fixtures;

use kage_core::file_text_wire::{
    write_request, Format, WorkerRequest, WorkerResponse, READS_P1_FORMATS, WORKER_FLAG,
};
use kage_file_text::{Job, Warning, WorkerErrorCode};
use ooxml_fixtures::*;
use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};

#[test]
fn headings_lists_and_paragraphs() {
    let body = [
        styled("Title", "Q3 plan"),
        p("Intro text."),
        styled("Heading1", "Goals"),
        item(0, "Ship it"),
        item(1, "Sub point"),
        item(0, "Tell people"),
        p("After the list."),
        styled("Heading2", "Details"),
        styled("heading3", "More"),
        styled("Heading4", "Deep"),
        p(""),
        "<w:p/>".to_string(),
        p("   "),
        p("Last."),
    ]
    .concat();
    let out = docx(&Docx::new(body));
    assert_eq!(
        out.text,
        "# Q3 plan\n\nIntro text.\n\n# Goals\n\n- Ship it\n  - Sub point\n- Tell people\n\n\
         After the list.\n\n## Details\n\n### More\n\nDeep\n\nLast."
    );
    assert_eq!(out.title.as_deref(), Some("Q3 plan"));
    assert_eq!(out.pages, None);
    assert_eq!(out.page_starts, None);
    assert!(out.warnings.is_empty());
    assert!(!out.truncated);
    assert_eq!(out.total_chars, Some(out.chars));
}

const LOCALISED_STYLES: &str = r#"
<w:style w:type="paragraph" w:default="1" w:styleId="Standard"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/><w:basedOn w:val="Standard"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="berschrift2"><w:name w:val="heading 2"/><w:basedOn w:val="berschrift1"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Titel"><w:name w:val="Title"/></w:style>
<w:style w:type="paragraph" w:styleId="Aufzhlungszeichen"><w:name w:val="List Bullet"/><w:pPr><w:numPr><w:numId w:val="2"/></w:numPr></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="Not a heading"/></w:style>"#;

#[test]
fn headings_and_lists_from_localised_styles() {
    let body = [
        styled("Titel", "Bericht"),
        styled("berschrift1", "Ziele"),
        styled("berschrift2", "Teil"),
        styled("Aufzhlungszeichen", "Eins"),
        styled("Aufzhlungszeichen", "Zwei"),
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Aufzhlungszeichen"/><w:numPr><w:numId w:val="0"/></w:numPr></w:pPr>{}</w:p>"#,
            r("Nicht nummeriert")
        ),
        format!(r#"<w:p><w:pPr><w:outlineLvl w:val="2"/></w:pPr>{}</w:p>"#, r("Direkt")),
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="berschrift1"/><w:outlineLvl w:val="9"/></w:pPr>{}</w:p>"#,
            r("Body after all")
        ),
        // An id that looks like a heading, but the style says otherwise.
        styled("Heading1", "Plain"),
    ]
    .concat();
    let out = docx(&Docx {
        styles: Some(LOCALISED_STYLES.into()),
        ..Docx::new(body)
    });
    assert_eq!(
        out.text,
        "# Bericht\n\n# Ziele\n\n## Teil\n\n- Eins\n- Zwei\n\nNicht nummeriert\n\n### Direkt\n\n\
         Body after all\n\nPlain"
    );
}

#[test]
fn tables_are_lines_of_tab_separated_cells() {
    let nested = table(&[&["x", "y"]]);
    let body = [
        p("Before"),
        table(&[&["Name", "Role"], &["Ana", "Lead"], &["", "Empty first"]]),
        format!(
            "<w:tbl><w:tr><w:tc>{}{}</w:tc><w:tc>{nested}<w:p/></w:tc><w:tc><w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
            p("one"),
            p("two"),
        ),
        styled("Heading1", "After"),
    ]
    .concat();
    let out = docx(&Docx::new(body));
    assert_eq!(
        out.text,
        "Before\n\nName\tRole\nAna\tLead\n\tEmpty first\n\none two\tx y\ta b c\n\n# After"
    );
}

#[test]
fn text_boxes_read_where_they_stand_and_only_once() {
    let boxed = |text: &str| {
        format!(
            "<w:txbxContent>{}{}</w:txbxContent>",
            p(text),
            styled("Heading2", "Box heading")
        )
    };
    // An old document: VML only.
    let legacy = format!(
        "<w:p><w:r><w:pict><v:shape><v:textbox>{}</v:textbox></v:shape></w:pict></w:r></w:p>",
        boxed("Legacy box")
    );
    let body = format!(
        r#"<w:p>{}<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor><a:graphic><a:graphicData><wps:wsp><wps:txbx>{}</wps:txbx></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape><v:textbox>{}</v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r>{}</w:p>{legacy}"#,
        r("Before box."),
        boxed("Box text"),
        boxed("Box text"),
        r(" after box."),
    );
    let out = docx(&Docx::new(body));
    assert_eq!(
        out.text,
        "Before box.\n\nBox text\n\n## Box heading\n\nafter box.\n\nLegacy box\n\n## Box heading"
    );
}

/// A footnote or endnote (`tag`) with one paragraph.
fn note(tag: &str, id: &str, kind: &str, text: &str) -> String {
    let kind = if kind.is_empty() {
        String::new()
    } else {
        format!(r#" w:type="{kind}""#)
    };
    format!(
        r#"<w:{tag} w:id="{id}"{kind}><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>{}</w:p></w:{tag}>"#,
        r(text)
    )
}

#[test]
fn footnotes_and_endnotes_follow_under_notes() {
    let reference = |tag: &str, id: &str| {
        format!(
            r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:{tag} w:id="{id}"/></w:r>"#
        )
    };
    let body = format!(
        "<w:p>{}{}{}{}{}</w:p><w:p>{}{}</w:p><w:p>{}{}</w:p>",
        r("Claim"),
        reference("footnoteReference", "2"),
        r(" and more"),
        reference("endnoteReference", "1"),
        r("."),
        r("Second"),
        reference("footnoteReference", "3"),
        r("Again"),
        reference("footnoteReference", "2"),
    );
    let footnotes = [
        note("footnote", "-1", "separator", "SEPARATOR"),
        note("footnote", "0", "continuationSeparator", "CONTINUATION"),
        note("footnote", "2", "", " First note."),
        note("footnote", "3", "", " Second note."),
        note("footnote", "4", "", " Never referenced."),
    ]
    .concat();
    let endnotes = [
        note("endnote", "0", "separator", "SEPARATOR"),
        note("endnote", "1", "", "An endnote."),
    ]
    .concat();
    let out = docx(&Docx {
        footnotes: Some(footnotes),
        endnotes: Some(endnotes),
        ..Docx::new(body)
    });
    assert_eq!(
        out.text,
        "Claim[1] and more[2].\n\nSecond[3]\n\nAgain[1]\n\nNotes\n\n[1] First note.\n\n\
         [3] Second note.\n\n[2] An endnote."
    );
}

#[test]
fn title_from_core_properties_else_the_first_heading() {
    let body = [
        p("Intro"),
        styled("Heading2", "First heading"),
        styled("Title", "Later"),
    ]
    .concat();
    let from_core = docx(&Docx {
        title: Some("From core".into()),
        ..Docx::new(body.clone())
    });
    assert_eq!(from_core.title.as_deref(), Some("From core"));
    let blank_core = docx(&Docx {
        title: Some("  ".into()),
        ..Docx::new(body.clone())
    });
    assert_eq!(blank_core.title.as_deref(), Some("First heading"));
    assert_eq!(docx(&Docx::new(p("No headings"))).title, None);
    // Titles are cleaned like file names.
    let messy = docx(&Docx {
        title: Some("  Q3\u{202e}  plan \n".into()),
        ..Docx::new(p("x"))
    });
    assert_eq!(messy.title.as_deref(), Some("Q3 plan"));
}

#[test]
fn tabs_breaks_hyphens_and_references() {
    let body = r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t><w:cr/><w:t>d</w:t><w:noBreakHyphen/><w:t>e</w:t><w:softHyphen/><w:t>f</w:t></w:r></w:p><w:p><w:r><w:t>Q&amp;A &#x2014; &lt;tag&gt; &#169;&#10;wrapped</w:t></w:r></w:p><w:p><m:oMathPara><m:oMath><m:r><m:t>x</m:t></m:r><m:r><m:t>=2</m:t></m:r></m:oMath></m:oMathPara></w:p>"#;
    let out = docx(&Docx::new(body));
    assert_eq!(
        out.text,
        "a\tb\nc\nd-ef\n\nQ&A \u{2014} <tag> \u{a9} wrapped\n\nx=2"
    );
}

#[test]
fn runs_of_one_paragraph_are_normalised_together() {
    // A combining accent in its own run still composes (NFC per paragraph).
    let body = format!("<w:p>{}{}</w:p>", r("Cafe"), r("\u{301} ok"));
    assert_eq!(docx(&Docx::new(body)).text, "Caf\u{e9} ok");
    // Tag characters and bidi overrides are removed.
    let body = p("a\u{e0041}b\u{202e}c");
    assert_eq!(docx(&Docx::new(body)).text, "abc");
}

#[test]
fn strict_ooxml_reads_like_transitional() {
    let reference = r#"<w:r><w:footnoteReference w:id="1"/></w:r>"#;
    let body = [
        styled("Heading1", "Strict"),
        format!("<w:p>{}{reference}</w:p>", r("Body")),
        item(0, "Point"),
        table(&[&["a", "b"]]),
    ]
    .concat();
    let styles = r#"<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>"#;
    let doc = Docx {
        strict: true,
        styles: Some(styles.into()),
        footnotes: Some(note("footnote", "1", "", "A strict note.")),
        title: Some("Strict title".into()),
        ..Docx::new(body)
    };
    let out = docx(&doc);
    assert_eq!(
        out.text,
        "# Strict\n\nBody[1]\n\n- Point\n\na\tb\n\nNotes\n\n[1] A strict note."
    );
    assert_eq!(out.title.as_deref(), Some("Strict title"));
}

#[test]
fn truncation_stops_early_and_total_chars_is_known_otherwise() {
    let body: String = (1..=2000)
        .map(|n| p(&format!("Paragraph number {n}.")))
        .collect();
    let bytes = Docx::new(body).build();
    let short = read_with(&Job::new(Format::Docx, &bytes, 100));
    assert!(short.truncated);
    assert!(short.chars <= 100);
    assert_eq!(short.total_chars, None);
    assert!(short
        .text
        .starts_with("Paragraph number 1.\n\nParagraph number 2."));
    let full = read_with(&Job::new(Format::Docx, &bytes, 1_000_000));
    assert!(!full.truncated);
    assert_eq!(full.total_chars, Some(full.chars));
    assert!(full.text.ends_with("Paragraph number 2000."));
}

fn package(entries: &[(&str, &str)]) -> Vec<u8> {
    zip_of(
        &entries
            .iter()
            .map(|(n, d)| (n.to_string(), d.as_bytes().to_vec()))
            .collect::<Vec<_>>(),
    )
}

const TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

#[test]
fn the_main_part_by_relationship_or_by_its_usual_name() {
    let doc = |text: &str| {
        format!(
            r#"<w:document xmlns:w="{W}"><w:body>{}</w:body></w:document>"#,
            p(text)
        )
    };
    // No package relationships: the usual name.
    let plain = package(&[
        ("[Content_Types].xml", TYPES),
        ("word/document.xml", &doc("usual")),
    ]);
    assert_eq!(read(Format::Docx, &plain).text, "usual");
    // The relationship wins, wherever it points.
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="/Word/Main.XML"/></Relationships>"#;
    let moved = package(&[
        ("[Content_Types].xml", TYPES),
        ("_rels/.rels", rels),
        ("word/document.xml", &doc("decoy")),
        ("word/main.xml", &doc("moved")),
    ]);
    assert_eq!(read(Format::Docx, &moved).text, "moved");
}

#[test]
fn damage_in_the_body_is_malformed_but_not_in_the_styles() {
    let missing = package(&[("[Content_Types].xml", TYPES)]);
    assert_eq!(error(Format::Docx, &missing), WorkerErrorCode::Malformed);
    let broken = package(&[
        ("[Content_Types].xml", TYPES),
        ("word/document.xml", "<w:document><w:body><w:p></w:body>"),
    ]);
    assert_eq!(error(Format::Docx, &broken), WorkerErrorCode::Malformed);
    let not_utf8 = zip_of(&[
        ("[Content_Types].xml".into(), TYPES.as_bytes().to_vec()),
        (
            "word/document.xml".into(),
            b"<w:document>\xff\xfe\xfa</w:document>".to_vec(),
        ),
    ]);
    assert_eq!(error(Format::Docx, &not_utf8), WorkerErrorCode::Malformed);
    let doc = Docx {
        styles: Some("<w:style><unclosed></w:style>".into()),
        ..Docx::new(styled("Heading1", "Still read"))
    };
    assert_eq!(docx(&doc).text, "# Still read");
}

#[test]
fn an_empty_document_has_no_text() {
    let out = docx(&Docx::new(""));
    assert_eq!(out.text, "");
    assert_eq!(out.warnings, [Warning::NoText]);
}

#[test]
fn docx_is_offered_now_and_pptx_with_the_p1_switch() {
    let doc = Docx::new(p("Offered")).build();
    let req = WorkerRequest::new(vec![Format::Docx, Format::Pptx], "DOCX", 1000);
    match kage_file_text::extract(&req, &doc) {
        WorkerResponse::Ok { text, .. } => assert_eq!(text, "Offered"),
        other => panic!("{other:?}"),
    }
    let deck = Pptx::new(vec![Slide::new(title("Deck"))]).build();
    let req = WorkerRequest::new(vec![Format::Docx, Format::Pptx], "pptx", 1000);
    let response = kage_file_text::extract(&req, &deck);
    if READS_P1_FORMATS {
        assert!(matches!(response, WorkerResponse::Ok { .. }));
    } else {
        assert!(matches!(
            response,
            WorkerResponse::Err {
                code: WorkerErrorCode::Unsupported,
                ..
            }
        ));
    }
}

#[test]
fn a_docx_through_the_worker_process() {
    let doc = Docx {
        title: Some("Worker".into()),
        ..Docx::new([styled("Heading1", "Read"), p("by the worker")].concat())
    };
    let mut frame = Vec::new();
    let req = WorkerRequest::new(vec![Format::Docx], "docx", 1000);
    write_request(&mut frame, &req, &doc.build()).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kage-file-text"));
    cmd.env_clear();
    if let Some(root) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
    let mut child = cmd
        .arg(WORKER_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&frame).unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    assert!(child.wait().unwrap().success());
    match serde_json::from_slice(&out).unwrap() {
        WorkerResponse::Ok { text, title, .. } => {
            assert_eq!(text, "# Read\n\nby the worker");
            assert_eq!(title.as_deref(), Some("Worker"));
        }
        other => panic!("{other:?}"),
    }
}
