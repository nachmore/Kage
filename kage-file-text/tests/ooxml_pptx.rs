//! PPTX text conventions (EXTENSION_FILE_TEXT §3.7, §5.9, §12).

mod ooxml_fixtures;

use kage_file_text::{Format, Job, Pages, Warning, WorkerErrorCode};
use ooxml_fixtures::*;

fn offset(text: &str, needle: &str) -> u32 {
    let byte = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in {text:?}"));
    text[..byte].chars().count() as u32
}

#[test]
fn slides_in_show_order_with_headings_shapes_and_notes() {
    let slides = vec![
        Slide::new(format!(
            "{}{}{}",
            title("Kickoff"),
            sp(Some(""), &["Agenda", "Goals"]),
            sp(None, &["Free text box"])
        ))
        .with_notes("Say hello\nThen the plan"),
        Slide::new(sp(Some(""), &["No title here"])),
        // The title shape is drawn last but read first.
        Slide::new(format!(
            "{}{}",
            sp(Some("body"), &["Body first"]),
            title("Late title")
        )),
    ];
    let deck = Pptx {
        order: Some(vec![3, 1, 2]),
        ..Pptx::new(slides)
    };
    let out = pptx(&deck);
    assert_eq!(
        out.text,
        "## Late title\n\nBody first\n\n## Kickoff\n\nAgenda\nGoals\n\nFree text box\n\n\
         Notes:\nSay hello\nThen the plan\n\n## Slide 3\n\nNo title here"
    );
    assert_eq!(
        out.page_starts,
        Some(vec![
            0,
            offset(&out.text, "## Kickoff"),
            offset(&out.text, "## Slide 3")
        ])
    );
    assert_eq!(out.pages, Some(Pages { count: 3, read: 3 }));
    assert_eq!(out.title.as_deref(), Some("Late title"));
    assert!(out.warnings.is_empty());
    assert_eq!(out.total_chars, Some(out.chars));
}

#[test]
fn hidden_slides_and_shapes_are_skipped_and_slide_numbers_keep_counting() {
    let slides = vec![
        Slide::new(title("One")),
        Slide::new(title("Secret slide")).hidden(),
        Slide::new(format!(
            "{}{}{}{}{}",
            shape(None, true, &["hidden shape"]),
            sp(Some("sldNum"), &["3"]),
            sp(Some("dt"), &["11/9/2026"]),
            sp(Some("ftr"), &["Footer"]),
            sp(Some("subTitle"), &["Subtitle kept"])
        )),
    ];
    let out = pptx(&Pptx::new(slides));
    assert_eq!(out.text, "## One\n\n## Slide 3\n\nSubtitle kept");
    assert_eq!(out.pages, Some(Pages { count: 3, read: 2 }));
    assert_eq!(out.page_starts.as_ref().map(Vec::len), Some(2));
    assert_eq!(out.warnings, [Warning::HiddenTextSkipped]);

    // A hidden slide or shape with no text: nothing to warn about.
    let quiet = vec![
        Slide::new(format!("{}{}", title("A"), shape(None, true, &[" "]))),
        Slide::new(sp(Some("sldNum"), &["2"])).hidden(),
    ];
    let out = pptx(&Pptx::new(quiet));
    assert_eq!(out.text, "## A");
    assert!(out.warnings.is_empty());
}

#[test]
fn titles_pick_the_first_visible_title_placeholder() {
    let slides = vec![
        Slide::new(format!(
            "{}{}{}",
            shape(Some("title"), true, &["Hidden title"]),
            sp(Some("title"), &["Two", "lines"]),
            sp(Some("title"), &["Second title"])
        )),
        Slide::new(format!(
            "{}{}",
            sp(Some("ctrTitle"), &["Big title"]),
            sp(Some("subTitle"), &["Sub"])
        )),
    ];
    let out = pptx(&Pptx::new(slides));
    assert_eq!(
        out.text,
        "## Two lines\n\nSecond title\n\n## Big title\n\nSub"
    );
    assert_eq!(out.warnings, [Warning::HiddenTextSkipped]);
}

fn table_frame(rows: &[&[&[&str]]]) -> String {
    let mut xml = String::from(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid/>"#,
    );
    for row in rows {
        xml.push_str(r#"<a:tr h="370840">"#);
        for cell in *row {
            xml.push_str(&format!(
                "<a:tc><a:txBody><a:bodyPr/>{}</a:txBody><a:tcPr/></a:tc>",
                paras(cell)
            ));
        }
        xml.push_str("</a:tr>");
    }
    xml.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
    xml
}

#[test]
fn tables_are_lines_of_tab_separated_cells() {
    let shapes = format!(
        "{}{}",
        table_frame(&[&[&["Name"], &["Role", "lead"]], &[&[""], &["x"]]]),
        sp(None, &["After"])
    );
    let out = pptx(&Pptx::new(vec![Slide::new(shapes)]));
    assert_eq!(out.text, "## Slide 1\n\nName\tRole lead\n\tx\n\nAfter");
}

#[test]
fn groups_alternate_content_breaks_and_fields() {
    let group = |name: &str, hidden: bool, inner: &str| {
        let hidden = if hidden { r#" hidden="1""# } else { "" };
        format!(
            r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="5" name="{name}"{hidden}/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:grpSp>"#
        )
    };
    let shapes = [
        group(
            "Group",
            false,
            &format!("{}{}", sp(None, &["In a group"]), sp(None, &["Also grouped"])),
        ),
        group("Hidden group", true, &sp(None, &["hidden in group"])),
        format!(
            r#"<mc:AlternateContent><mc:Choice Requires="p14">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
            sp(None, &["Once"]),
            sp(None, &["Once"])
        ),
        r#"<p:sp><p:nvSpPr><p:cNvPr id="7" name="Lines"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Line one</a:t></a:r><a:br/><a:r><a:t>line two &amp; more</a:t></a:r></a:p><a:p/><a:p><a:fld id="{1}" type="datetime1"><a:t>Today</a:t></a:fld></a:p></p:txBody></p:sp>"#.to_string(),
    ]
    .concat();
    let out = pptx(&Pptx::new(vec![Slide::new(shapes)]));
    assert_eq!(
        out.text,
        "## Slide 1\n\nIn a group\n\nAlso grouped\n\nOnce\n\nLine one\nline two & more\nToday"
    );
    assert_eq!(out.warnings, [Warning::HiddenTextSkipped]);
}

#[test]
fn title_from_core_properties_else_the_first_visible_slide() {
    let slides = || {
        vec![
            Slide::new(title("Hidden first")).hidden(),
            Slide::new(title("Opening")),
            Slide::new(title("Later")),
        ]
    };
    let from_core = pptx(&Pptx {
        title: Some("Deck title".into()),
        ..Pptx::new(slides())
    });
    assert_eq!(from_core.title.as_deref(), Some("Deck title"));
    assert_eq!(pptx(&Pptx::new(slides())).title.as_deref(), Some("Opening"));
    // The first visible slide has no title: none, not a later one.
    let untitled = vec![Slide::new(sp(None, &["text"])), Slide::new(title("Later"))];
    assert_eq!(pptx(&Pptx::new(untitled)).title, None);
}

#[test]
fn slides_past_the_cap_are_skipped() {
    let slides: Vec<Slide> = (1..=3)
        .map(|n| Slide::new(title(&format!("Slide title {n}"))))
        .collect();
    let bytes = Pptx::new(slides).build();
    let job = Job {
        max_slides: 2,
        ..Job::new(Format::Pptx, &bytes, 1000)
    };
    let out = read_with(&job);
    assert_eq!(out.text, "## Slide title 1\n\n## Slide title 2");
    assert_eq!(out.pages, Some(Pages { count: 3, read: 2 }));
    assert_eq!(out.warnings, [Warning::PagesSkipped]);
    assert_eq!(out.total_chars, None);
    assert!(!out.truncated);
}

#[test]
fn truncation_stops_at_the_slide_it_reaches() {
    let slides: Vec<Slide> = (1..=50)
        .map(|n| {
            Slide::new(format!(
                "{}{}",
                title(&format!("Title {n}")),
                sp(None, &[&format!("Some body text for slide {n}.")])
            ))
            .with_notes("Notes too.")
        })
        .collect();
    let bytes = Pptx::new(slides).build();
    let out = read_with(&Job::new(Format::Pptx, &bytes, 200));
    assert!(out.truncated);
    assert!(out.chars <= 200);
    let pages = out.pages.unwrap();
    assert_eq!(pages.count, 50);
    assert!(pages.read < 10, "{pages:?}");
    let starts = out.page_starts.unwrap();
    assert!(starts.len() <= pages.read as usize);
    assert!(starts.iter().all(|&s| s < out.chars));
}

#[test]
fn strict_presentations_read_like_transitional() {
    let deck = Pptx {
        strict: true,
        title: Some("Strict deck".into()),
        ..Pptx::new(vec![Slide::new(format!(
            "{}{}",
            title("Strict"),
            sp(None, &["Body"])
        ))
        .with_notes("Spoken")])
    };
    let out = pptx(&deck);
    assert_eq!(out.text, "## Strict\n\nBody\n\nNotes:\nSpoken");
    assert_eq!(out.title.as_deref(), Some("Strict deck"));
}

#[test]
fn missing_slides_are_skipped_and_damage_is_malformed() {
    // The list names a slide the relationships don't have.
    let deck = Pptx {
        order: Some(vec![1, 7, 2]),
        ..Pptx::new(vec![Slide::new(title("A")), Slide::new(title("B"))])
    };
    let out = pptx(&deck);
    assert_eq!(out.text, "## A\n\n## B");
    assert_eq!(out.pages, Some(Pages { count: 3, read: 2 }));

    let with = |name: &str, xml: &str| {
        Pptx {
            replace: vec![(name.to_string(), xml.as_bytes().to_vec())],
            ..Pptx::new(vec![Slide::new(title("A")).with_notes("Notes")])
        }
        .build()
    };
    let broken_slide = with("ppt/slides/slide1.xml", "<p:sld><p:cSld></p:sld>");
    assert_eq!(
        error(Format::Pptx, &broken_slide),
        WorkerErrorCode::Malformed
    );
    let broken_list = with(
        "ppt/presentation.xml",
        "<p:presentation><p:sldIdLst></p:presentation>",
    );
    assert_eq!(
        error(Format::Pptx, &broken_list),
        WorkerErrorCode::Malformed
    );
    // Damaged notes cost only the notes.
    let broken_notes = with("ppt/notesSlides/notesSlide1.xml", "<p:notes><a></p:notes>");
    assert_eq!(read(Format::Pptx, &broken_notes).text, "## A");
}

#[test]
fn an_empty_deck_has_no_text() {
    let out = pptx(&Pptx::new(Vec::new()));
    assert_eq!(out.text, "");
    assert_eq!(out.pages, Some(Pages { count: 0, read: 0 }));
    assert_eq!(out.page_starts, None);
    assert_eq!(out.warnings, [Warning::NoText]);
}
