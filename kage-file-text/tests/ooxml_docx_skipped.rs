//! What DOCX leaves out (EXTENSION_FILE_TEXT §3.7, §5.9, §12): deleted
//! and moved-away text, field codes, comments, headers and footers, and
//! hidden text set on a run, through a style or as the document default.

mod ooxml_fixtures;

use kage_file_text::{Format, Job, Warning};
use ooxml_fixtures::*;
use std::time::{Duration, Instant};

fn field_char(kind: &str) -> String {
    format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#)
}

#[test]
fn field_results_span_paragraphs_and_broken_fields_end_with_theirs() {
    // A table of contents: the code in the first paragraph, the result
    // over several, the end in the last.
    let body = [
        format!(
            r#"<w:p>{}<w:r><w:instrText> TOC \o "1-3" </w:instrText></w:r>{}{}</w:p>"#,
            field_char("begin"),
            field_char("separate"),
            r("Intro 1"),
        ),
        p("Goals 2"),
        format!("<w:p>{}{}</w:p>", r("Plan 3"), field_char("end")),
        // A field that never reaches `separate` hides only the rest of its
        // paragraph.
        format!(
            "<w:p>{}{}{}</w:p>",
            r("Before "),
            field_char("begin"),
            r("broken code")
        ),
        p("Next paragraph"),
    ]
    .concat();
    assert_eq!(
        docx(&Docx::new(body)).text,
        "Intro 1\n\nGoals 2\n\nPlan 3\n\nBefore\n\nNext paragraph"
    );
}

#[test]
fn many_open_fields_stay_cheap() {
    let opens = field_char("begin").repeat(100_000);
    let body = format!(
        "<w:p>{}{}{}</w:p>{}",
        r("start"),
        opens,
        r("code").repeat(100_000),
        p("after")
    );
    let bytes = Docx::new(body).build();
    let start = Instant::now();
    let out = read_with(&Job::new(Format::Docx, &bytes, 1000));
    assert_eq!(out.text, "start\n\nafter");
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn deleted_moved_field_code_comment_header_and_footer_text_is_left_out() {
    let body = [
        format!(
            r#"<w:p>{}<w:del w:id="1" w:author="x"><w:r><w:delText>deleted </w:delText></w:r></w:del><w:ins w:id="2" w:author="x">{}</w:ins><w:moveFrom w:id="3">{}</w:moveFrom><w:moveTo w:id="4">{}</w:moveTo>{}</w:p>"#,
            r("Kept "),
            r("inserted "),
            r("moved away "),
            r("moved here "),
            r("end."),
        ),
        format!(
            r#"<w:p>{}<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:t>CODE</w:t></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r>{}<w:fldSimple w:instr=" NUMPAGES ">{}</w:fldSimple></w:p>"#,
            r("Page "),
            r("7"),
            r(" of "),
            r("9"),
        ),
        format!(
            r#"<w:p><w:commentRangeStart w:id="0"/>{}<w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r></w:p>"#,
            r("Commented")
        ),
        format!(
            r#"<w:p><w:hyperlink r:id="rId99">{}</w:hyperlink><w:sdt><w:sdtPr><w:alias w:val="x"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p>"#,
            r("Link"),
            r(" in a control")
        ),
        r#"<w:sectPr><w:headerReference w:type="default" r:id="rId20"/><w:footerReference w:type="default" r:id="rId21"/></w:sectPr>"#.to_string(),
    ]
    .concat();
    let part = |root: &str, text: &str| {
        format!(r#"<w:{root} xmlns:w="{W}">{}</w:{root}>"#, p(text)).into_bytes()
    };
    let doc = Docx {
        extra: vec![
            ("word/comments.xml".into(), part("comments", "COMMENT TEXT")),
            ("word/header1.xml".into(), part("hdr", "HEADER TEXT")),
            ("word/footer1.xml".into(), part("ftr", "FOOTER TEXT")),
        ],
        extra_rels: vec![
            ("comments".into(), "comments.xml".into()),
            ("header".into(), "header1.xml".into()),
            ("footer".into(), "footer1.xml".into()),
        ],
        ..Docx::new(body)
    };
    let out = docx(&doc);
    assert_eq!(
        out.text,
        "Kept inserted moved here end.\n\nPage 7 of 9\n\nCommented\n\nLink in a control"
    );
}

const HIDDEN_STYLES: &str = r#"
<w:style w:type="character" w:styleId="Hidden"><w:name w:val="Hidden"/><w:rPr><w:vanish/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="HiddenPara"><w:name w:val="Hidden para"/><w:rPr><w:vanish/></w:rPr></w:style>"#;

#[test]
fn hidden_text_is_skipped_and_flagged() {
    let run = |props: &str, text: &str| {
        format!(r#"<w:r><w:rPr>{props}</w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#)
    };
    let body = [
        format!(
            "<w:p>{}{}{}{}</w:p>",
            r("Shown "),
            run("<w:vanish/>", "secret one "),
            run(r#"<w:vanish w:val="0"/>"#, "visible "),
            run(r#"<w:rStyle w:val="Hidden"/>"#, "secret two"),
        ),
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="HiddenPara"/></w:pPr>{}{}</w:p>"#,
            run("", "secret three"),
            run(r#"<w:vanish w:val="false"/>"#, "unhidden"),
        ),
        // The paragraph mark is hidden, not the text.
        format!(
            "<w:p><w:pPr><w:rPr><w:vanish/></w:rPr></w:pPr>{}</w:p>",
            r("Mark hidden only")
        ),
        // Formatting it had before a tracked change.
        format!(
            "<w:p>{}</w:p>",
            run(
                r#"<w:b/><w:rPrChange w:id="1" w:author="x"><w:rPr><w:vanish/></w:rPr></w:rPrChange>"#,
                "Was hidden"
            )
        ),
    ]
    .concat();
    let out = docx(&Docx {
        styles: Some(HIDDEN_STYLES.into()),
        ..Docx::new(body)
    });
    assert_eq!(
        out.text,
        "Shown visible\n\nunhidden\n\nMark hidden only\n\nWas hidden"
    );
    assert_eq!(out.warnings, [Warning::HiddenTextSkipped]);

    // Hidden whitespace is no reason to warn.
    let quiet = format!("<w:p>{}{}</w:p>", r("Hi"), run("<w:vanish/>", " "));
    assert!(docx(&Docx::new(quiet)).warnings.is_empty());
}

#[test]
fn hidden_by_document_defaults() {
    let styles =
        r#"<w:docDefaults><w:rPrDefault><w:rPr><w:vanish/></w:rPr></w:rPrDefault></w:docDefaults>"#;
    let body = format!(
        r#"{}<w:p><w:r><w:rPr><w:vanish w:val="0"/></w:rPr><w:t>Shown</w:t></w:r></w:p>"#,
        p("all hidden")
    );
    let out = docx(&Docx {
        styles: Some(styles.into()),
        ..Docx::new(body)
    });
    assert_eq!(out.text, "Shown");
    assert_eq!(out.warnings, [Warning::HiddenTextSkipped]);
}
