use super::*;
use crate::{extract_as, Format, Warning, WorkerResponse};

struct Read {
    text: String,
    title: Option<String>,
    warnings: Vec<Warning>,
    chars: u32,
    truncated: bool,
    total: Option<u32>,
}

fn read_capped(rtf: &[u8], cap: u32) -> Result<Read, WorkerErrorCode> {
    match extract_as(&Job::new(Format::Rtf, rtf, cap)) {
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
            assert_eq!(format, Format::Rtf);
            assert!(pages.is_none() && page_starts.is_none());
            Ok(Read {
                text,
                title,
                warnings,
                chars,
                truncated,
                total: total_chars,
            })
        }
        WorkerResponse::Err { code, .. } => Err(code),
    }
}

fn read(rtf: &str) -> Read {
    read_capped(rtf.as_bytes(), 100_000).expect("reads")
}

fn text(rtf: &str) -> String {
    read(rtf).text
}

/// A document with the usual header around `body`.
fn doc(body: &str) -> String {
    format!(
        "{{\\rtf1\\ansi\\ansicpg1252\\deff0{{\\fonttbl{{\\f0\\fswiss\\fcharset0 Helvetica;}}\
         {{\\f1\\fnil\\fcharset204 Arial Cyr;}}{{\\f2\\fnil\\fcharset128 MS Gothic;}}}}\
         {{\\colortbl;\\red255\\green0\\blue0;}}\n\\pard\\plain\\f0\\fs24 {body}}}"
    )
}

#[test]
fn paragraphs_lines_and_symbols() {
    assert_eq!(
        text(&doc(
            "Hello, world.\\par Second\\line line.\\par\\par Third"
        )),
        "Hello, world.\n\nSecond\nline.\n\nThird"
    );
    assert_eq!(
        text(&doc(
            "a\\tab b \\emdash  c\\endash d \\lquote q\\rquote  \\ldblquote qq\\rdblquote \\bullet"
        )),
        "a\tb \u{2014} c\u{2013}d \u{2018}q\u{2019} \u{201c}qq\u{201d}\u{2022}"
    );
    assert_eq!(
        text(&doc(
            "\\{braces\\} back\\\\slash non\\~breaking soft\\-hyphen non\\_breaking"
        )),
        "{braces} back\\slash non\u{a0}breaking softhyphen non\u{2011}breaking"
    );
    // A backslash before a line break is \par; bare line breaks are nothing.
    assert_eq!(text(&doc("one\\\ntwo\r\nthree")), "one\n\ntwothree");
}

#[test]
fn textedit_lists_read_as_markdown_lists() {
    let rtf = "{\\rtf1\\ansi\\ansicpg1252\\cocoartf2759\n\
        {\\fonttbl\\f0\\fswiss\\fcharset0 Helvetica;}\n\
        {\\colortbl;\\red255\\green255\\blue255;}\n\
        {\\*\\expandedcolortbl;;}\n\
        {\\*\\listtable{\\list\\listtemplateid1\\listhybrid{\\listlevel\\levelnfc23{\\*\\levelmarker \\{disc\\}}\
        {\\leveltext\\leveltemplateid1\\'01\\uc0\\u8226 ;}{\\levelnumbers;}\\fi-360\\li720\\lin720 }\
        {\\listname ;}\\listid1}}\n\
        {\\*\\listoverridetable{\\listoverride\\listid1\\listoverridecount0\\ls1}}\n\
        \\paperw11900\\paperh16840\\margl1440\\margr1440\\vieww11520\\viewh8400\\viewkind0\n\
        \\pard\\tx720\\pardirnatural\\partightenfactor0\n\n\
        \\f0\\fs24 \\cf0 Shopping\\\n\
        \\pard\\tx220\\tx720\\li720\\fi-720\\pardirnatural\\partightenfactor0\n\
        \\ls1\\ilvl0\\cf0 {\\listtext\t\\uc0\\u8226 \t}Milk\\\n\
        {\\listtext\t\\uc0\\u8226 \t}Eggs\\\n\
        \\pard\\tx720\\pardirnatural\\partightenfactor0\n\
        \\cf0 \\\nDone.}";
    assert_eq!(text(rtf), "Shopping\n\n- Milk\n- Eggs\n\nDone.");
}

#[test]
fn headings_from_outline_levels_and_style_names() {
    let rtf = "{\\rtf1\\ansi{\\stylesheet{\\s0 Normal;}{\\s1\\outlinelevel0 heading 1;}\
        {\\s2 Heading 2,h2;}{\\s3\\sbasedon0 heading 3;}{\\s4 heading 4;}{\\s5 Title;}\
        {\\*\\cs10 \\additive heading 1 Char;}}\
        \\pard\\s5 Report\\par\\pard\\s1 Intro\\par\\pard\\s0 Body text.\\par\
        \\pard\\s2 Detail\\par\\pard\\s3 More\\par\\pard\\s4 Deep\\par\
        \\pard\\outlinelevel1 Outline two\\par\\pard\\outlinelevel9 Plain\\par}";
    assert_eq!(
        text(rtf),
        "# Report\n\n# Intro\n\nBody text.\n\n## Detail\n\n### More\n\nDeep\n\n## Outline two\n\nPlain"
    );
}

#[test]
fn a_word_document() {
    let rtf = "{\\rtf1\\adeflang1025\\ansi\\ansicpg1252\\uc1\\adeff31507\\deff0\\deflang1033\
        {\\fonttbl{\\f0\\fbidi \\froman\\fcharset0\\fprq2{\\*\\panose 02020603050405020304}Times New Roman;}\n\
        {\\flomajor\\f31500\\fbidi \\froman\\fcharset0\\fprq2{\\*\\panose 02020603050405020304}Times New Roman;}}\n\
        {\\colortbl;\\red0\\green0\\blue0;}{\\*\\defchp \\fs22\\loch\\af31506 }\
        {\\*\\defpap \\ql \\li0\\ri0\\sa160 }\\noqfpromote {\\stylesheet{\\ql \\li0\\ri0\\sa160\\widctlpar \
        \\rtlch\\fcs1 \\af31507\\afs22 \\ltrch\\fcs0 \\fs22\\lang1033 \\snext0 \\sqformat \\spriority0 Normal;}\
        {\\s1\\ql \\li0\\sb240\\keepn\\outlinelevel0\\adjustright \\rtlch\\fcs1 \\af31503\\afs32 \\ltrch\\fcs0 \\fs32 \
        \\sbasedon0 \\snext0 \\slink15 \\sqformat \\spriority9 \\styrsid1 heading 1;}\
        {\\*\\cs15 \\additive \\rtlch\\fcs1 \\af31503 \\ltrch\\fcs0 \\sbasedon10 \\slink1 Heading 1 Char;}}\n\
        {\\*\\rsidtbl \\rsid1\\rsid2}{\\mmathPr\\mmathFont34\\mbrkBin0}\
        {\\info{\\title My Doc}{\\author Ann}{\\operator Ann}{\\creatim\\yr2024\\mo1\\dy2}{\\version2}}\
        {\\*\\xmlnstbl {\\xmlns1 http://schemas.example.com/wordml}}\n\
        \\paperw12240\\paperh15840\\ltrsect {\\*\\pnseclvl1\\pnucrm\\pnstart1{\\pntxta .}}\n\
        \\pard\\plain \\ltrpar\\s1\\ql \\li0\\sb240\\keepn\\outlinelevel0\\pararsid1 \\rtlch\\fcs1 \\af31503 \
        \\ltrch\\fcs0 \\fs32 {\\rtlch\\fcs1 \\af31503 \\ltrch\\fcs0 \\insrsid1 Heading}\
        {\\rtlch\\fcs1 \\af31503 \\ltrch\\fcs0 \\insrsid1 \\par }\n\
        \\pard\\plain \\ltrpar\\ql \\li0\\sa160 \\rtlch\\fcs1 \\af31507 \\ltrch\\fcs0 \\fs22 \
        {\\rtlch\\fcs1 \\af31507 \\ltrch\\fcs0 \\insrsid1 Body text with \\u8217\\'92 quotes.}\
        {\\rtlch\\fcs1 \\af31507 \\ltrch\\fcs0 \\insrsid1 \\par }}";
    let r = read(rtf);
    assert_eq!(r.text, "# Heading\n\nBody text with \u{2019} quotes.");
    assert_eq!(r.title.as_deref(), Some("My Doc"));
    assert!(r.warnings.is_empty());
}

#[test]
fn tables_are_tab_separated_rows() {
    let rtf = doc(
        "Before\\par\\trowd\\cellx1000\\cellx2000\\pard\\intbl Name\\cell Owner\\cell\\row\
         \\trowd\\cellx1000\\cellx2000\\pard\\intbl Alpha\\cell Ann\\cell\\row\
         \\pard\\intbl\\cell Bob\\cell\\row\\pard After\\par",
    );
    assert_eq!(
        text(&rtf),
        "Before\n\nName\tOwner\nAlpha\tAnn\n\tBob\nAfter"
    );
}

#[test]
fn code_pages_and_font_charsets() {
    assert_eq!(text(&doc("caf\\'e9 cr\\'e8me")), "café crème");
    // The font's charset beats the document's code page.
    assert_eq!(
        text(&doc("{\\f1 \\'cf\\'f0\\'e8\\'e2\\'e5\\'f2} ok")),
        "Привет ok"
    );
    // Double-byte characters across escapes, and a literal trail byte.
    assert_eq!(
        text(&doc("{\\f2 \\'93\\'fa\\'96\\'7b\\'8c\\'ea}")),
        "日本語"
    );
    assert_eq!(text(&doc("{\\f2 \\'83A}")), "ア");
    // \ansicpg sets the default; \cpg on a font overrides it.
    let rtf = "{\\rtf1\\ansi\\ansicpg1251{\\fonttbl{\\f0 Arial;}{\\f1\\cpg1253 Greek;}}\
        \\f0 \\'c4\\'e0 {\\f1 \\'e1\\'e5}}";
    assert_eq!(text(rtf), "Да αε");
    // An ANSI font is Windows-1252 whatever the code page (Cocoa writes
    // its quotes that way in Japanese files).
    let rtf = "{\\rtf1\\ansi\\ansicpg932{\\fonttbl\\f0\\fnil\\fcharset0 LucidaGrande;\
        \\f1\\fnil\\fcharset128 Hiragino;\\f2\\fnil Default;}\
        \\f0 \\'93Quote\\'94 \\f1 \\'82\\'a0 \\f2 \\'82\\'a2}";
    assert_eq!(text(rtf), "\u{201c}Quote\u{201d} あ い");
    // UTF-8 as the code page; bytes that aren't UTF-8 there are
    // Windows-1252, as older Cocoa writers meant them.
    let utf8 = "{\\rtf1\\ansi\\ansicpg65001 gr\\'c3\\'bc\\'c3\\'9f {f\\'fcr}}";
    let r = read(utf8);
    assert_eq!(r.text, "grüß für");
    assert!(r.warnings.is_empty());
    // 8-bit text that isn't escaped is in the code page too.
    let r = read_capped(b"{\\rtf1\\ansi na\xefve}", 100).unwrap();
    assert_eq!(r.text, "na\u{ef}ve");
}

#[test]
fn unicode_and_its_fallback() {
    assert_eq!(
        text(&doc("\\u8364?5 and \\u8364\\'80 6")),
        "\u{20ac}5 and \u{20ac} 6"
    );
    assert_eq!(
        text(&doc("\\uc2\\u8364\\'80\\'80 x\\uc0\\u8364 y")),
        "\u{20ac} x\u{20ac}y"
    );
    // A control word counts as one fallback unit; a group ends the skip.
    assert_eq!(text(&doc("\\u233\\'e9 {\\u233}a")), "\u{e9} \u{e9}a");
    // Surrogate pairs, and negative values.
    assert_eq!(text(&doc("\\u-10179 ?\\u-8704 ?!")), "\u{1f600}!");
    // A combining mark composes with the letter before it (NFC).
    assert_eq!(text(&doc("e\\uc0\\u769 t\\u233 ")), "\u{e9}t\u{e9}");
    // \upr: only the Unicode part.
    assert_eq!(
        text(&doc("{\\upr{ANSI text}{\\*\\ud{Unicode \\u8364? text}}}")),
        "Unicode \u{20ac} text"
    );
    // A lone surrogate is dropped, with a warning.
    let r = read(&doc("a\\u-10179 ?b"));
    assert_eq!(r.text, "ab");
    assert_eq!(r.warnings, vec![Warning::ReplacedCharacters]);
}

#[test]
fn skipped_destinations() {
    let rtf = "{\\rtf1\\ansi{\\fonttbl{\\f0 Times;}}{\\colortbl;\\red1;}\
        {\\stylesheet{\\s1 heading 1;}}{\\*\\generator Writer 1.0;}\
        {\\info{\\title The Plan}{\\author Ann}{\\*\\company Acme}{\\doccomm note}}\
        {\\header Page header\\par}{\\footerl Footer\\par}\
        Start {\\*\\bkmkstart b1}{\\*\\bkmkend b1}\
        {\\pict\\pngblip 89504e470d0a1a0a}{\\object\\objemb{\\*\\objdata 0102}{\\result shown?}}\
        {\\field{\\*\\fldinst HYPERLINK \"https://example.com\"}{\\fldrslt link}} \
        {\\*\\atnid AN}{\\*\\atnauthor Ann}\\chatn{\\*\\annotation a comment}\
        {\\xe index}{\\tc contents}{\\*\\unknowndest secret}\
        {\\shp{\\*\\shpinst{\\sp{\\sn shapeType}{\\sv 202}}{\\shptxt in a box}}{\\shprslt fallback}}\
        {\\*\\pn\\pnlvlblt{\\pntxtb \\'b7}}end.}";
    let r = read(rtf);
    assert_eq!(r.text, "Start link in a boxend.");
    assert_eq!(r.title.as_deref(), Some("The Plan"));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn hidden_and_deleted_text_is_left_out() {
    let r = read(&doc("a{\\v secret}b\\v more\\v0 c {\\deleted gone}d"));
    assert_eq!(r.text, "abc d");
    assert_eq!(r.warnings, vec![Warning::HiddenTextSkipped]);
    // \plain ends hidden text.
    assert_eq!(text(&doc("\\v x\\plain y")), "y");
    assert!(read(&doc("{\\v  }x")).warnings.is_empty());
}

#[test]
fn footnotes_go_under_notes_at_the_end() {
    let rtf = doc(
        "Body one{\\super\\chftn}{\\footnote\\pard\\plain{\\super\\chftn} First note.}.\\par \
         Body two{\\footnote\\ftnalt \\chftn  An endnote\\par with two parts.}.\\par",
    );
    assert_eq!(
        text(&rtf),
        "Body one.\n\nBody two.\n\nNotes\n\nFirst note.\n\nAn endnote\n\nwith two parts."
    );
}

#[test]
fn the_title_is_the_info_title() {
    let r = read("{\\rtf1{\\info{\\title Q3 \\'e9t\\'e9\\par plan}{\\title Second}}Body}");
    assert_eq!(r.title.as_deref(), Some("Q3 été plan"));
    assert_eq!(r.text, "Body");
    assert_eq!(read("{\\rtf1 {\\title Not info}x}").title, None);
}

#[test]
fn nesting_is_capped() {
    let ok = format!(
        "{}x{}",
        "{\\rtf1".to_string() + &"{".repeat(MAX_NESTING_DEPTH - 1),
        "}".repeat(MAX_NESTING_DEPTH)
    );
    assert_eq!(read(&ok).text, "x");
    let deep = format!("{{\\rtf1 {}x", "{".repeat(300));
    assert_eq!(
        read_capped(deep.as_bytes(), 100).err(),
        Some(WorkerErrorCode::TooComplex)
    );
}

#[test]
fn binary_data_is_skipped() {
    // The data holds braces and backslashes.
    let mut rtf = doc("a{\\pict\\bin4 {}\\\\}b").into_bytes();
    assert_eq!(read_capped(&rtf, 100).unwrap().text, "ab");
    // \bin past the end of the file.
    rtf = b"{\\rtf1 text \\bin999999 \x00\x01".to_vec();
    assert_eq!(read_capped(&rtf, 100).unwrap().text, "text");
    // A negative or missing length is no data.
    assert_eq!(text(&doc("a\\bin-5 b\\bin c")), "abc");
}

#[test]
fn broken_documents_read_as_far_as_they_go() {
    assert_eq!(text("{\\rtf1 unclosed {\\b bold"), "unclosed bold");
    assert_eq!(text("{\\rtf1 done} after the end"), "done");
    assert_eq!(text("{\\rtf1 a}}}} b"), "a");
    // Broken escapes, absurd words and parameters.
    let junk = format!(
        "{{\\rtf1 x{}\\{} \\par99999999999999999999 \\'zz\\'4y\\ y}}",
        "\\'".repeat(100_000),
        "a".repeat(1_000)
    );
    assert_eq!(text(&junk), "x\n\nzz4yy");
}

#[test]
fn caps_count_the_whole_document() {
    let body = "Lorem ipsum dolor. ".repeat(3_000) + "\\par tail";
    let full = read(&doc(&body));
    let r = read_capped(doc(&body).as_bytes(), 1_000).unwrap();
    assert!(r.truncated && r.chars <= 1_000);
    assert_eq!(r.total, Some(full.chars));
    assert!(r.text.ends_with('.'));
}

#[test]
fn truncation_prefers_paragraphs_then_sentences_then_spaces() {
    let cut = |body: String| read_capped(doc(&body).as_bytes(), 1_000).unwrap();
    let words = |n: usize| "word ".repeat(n / 5);
    let r = cut(format!("{}end\\par {}", words(975), words(200)));
    assert_eq!(r.text, format!("{}end", words(975)));
    let r = cut(format!("{}end. {}", words(970), words(200)));
    assert_eq!(r.text, format!("{}end.", words(970)));
    let r = cut(words(2_000));
    assert!(r.text.ends_with("word") && r.chars < 1_000);
    let r = cut("x".repeat(2_000));
    assert_eq!((r.chars, r.total), (1_000, Some(2_000)));
    assert!(r.truncated);
}

#[test]
fn long_paragraphs_are_pushed_in_pieces_without_changing_the_text() {
    let body = "cafe\\uc0\\u769  ".repeat(15_000);
    let r = read(&doc(&body));
    assert!(!r.truncated);
    assert_eq!(r.text, "caf\u{e9} ".repeat(15_000).trim_end());
}

#[test]
fn empty_documents_have_no_text() {
    let r = read("{\\rtf1\\ansi{\\fonttbl{\\f0 Arial;}}}");
    assert_eq!(r.text, "");
    assert_eq!(r.warnings, vec![Warning::NoText]);
}

#[test]
fn other_content_is_refused() {
    assert_eq!(
        read_capped(b"plain words", 100).err(),
        Some(WorkerErrorCode::WrongContent)
    );
    assert_eq!(
        read_capped(b"<html>{\\rtf1 x}</html>", 100).err(),
        Some(WorkerErrorCode::WrongContent)
    );
}
