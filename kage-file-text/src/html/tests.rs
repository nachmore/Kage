use super::charset::prescan;
use super::*;
use crate::{extract_as, Format, Job, WorkerResponse};
use encoding_rs::{SHIFT_JIS, UTF_8, WINDOWS_1251, WINDOWS_1252};

struct Read {
    text: String,
    title: Option<String>,
    warnings: Vec<Warning>,
    chars: u32,
    truncated: bool,
    total: Option<u32>,
}

fn read_bytes_capped(bytes: &[u8], cap: u32) -> Result<Read, WorkerErrorCode> {
    match extract_as(&Job::new(Format::Html, bytes, cap)) {
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
            assert_eq!(format, Format::Html);
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

fn read_bytes(bytes: &[u8]) -> Read {
    read_bytes_capped(bytes, 100_000).expect("reads")
}

fn read(html: &str) -> Read {
    read_bytes(html.as_bytes())
}

fn text(html: &str) -> String {
    read(html).text
}

#[test]
fn blocks_headings_lists_and_breaks() {
    let html = "<!doctype html><html><head><title>Plan</title></head><body>\
        <h1>Q3 plan</h1><p>First   paragraph,\n  wrapped.</p>\
        <h2>Goals</h2><ul><li>Ship it</li><li>Tell people</li></ul>\
        <h3>Risks</h3><h4>Small</h4><div>one<br>two</div><p>end</p></body></html>";
    assert_eq!(
        text(html),
        "# Q3 plan\n\nFirst paragraph, wrapped.\n\n## Goals\n\n- Ship it\n- Tell people\n\n\
         ### Risks\n\nSmall\n\none\ntwo\n\nend"
    );
}

#[test]
fn inline_markup_joins_and_spaces_collapse() {
    assert_eq!(
        text("<p>  a<b>b</b><i> c </i>\t\n d&nbsp;&nbsp;e</p>"),
        "ab c d\u{a0}\u{a0}e"
    );
    assert_eq!(text("<span>x</span><span>y</span>"), "xy");
    assert_eq!(text("<div>x</div><div>y</div>"), "x\ny");
    // <br>s add up; a block edge after one doesn't.
    assert_eq!(text("a<br><br>b"), "a\n\nb");
    assert_eq!(text("<div>a<br></div><div>b</div>"), "a\nb");
}

#[test]
fn list_items_with_blocks_inside_keep_their_dash() {
    assert_eq!(
        text("<ol><li><p>one</p></li><li>two</li></ol><p>after</p>"),
        "- one\n\n- two\n\nafter"
    );
    // An empty item's dash doesn't leak into the next block.
    assert_eq!(text("<ul><li></li></ul><p>para</p>"), "para");
}

#[test]
fn tables_are_tab_separated_rows() {
    let html = "<table><tr><th>Name</th> <th>Owner</th></tr>\
        <tr><td> <p>Alpha</p> <p>beta</p></td><td>Ann</td></tr>\
        <tr><td></td><td>Bob</td></tr></table><p>after</p>";
    assert_eq!(text(html), "Name\tOwner\nAlpha beta\tAnn\n\tBob\n\nafter");
}

#[test]
fn preformatted_text_keeps_its_whitespace() {
    assert_eq!(
        text("<p>x</p><pre>\n  fn main() {\n      go();\n  }\n</pre><p>y</p>"),
        "x\n\n  fn main() {\n      go();\n  }\n\ny"
    );
    assert_eq!(
        text("<textarea>\nline 1\n  line 2</textarea>"),
        "line 1\n  line 2"
    );
    assert_eq!(
        text("<p>a</p><plaintext><b>not bold</b>\n  </plaintext> still"),
        "a\n\n<b>not bold</b>\n  </plaintext> still"
    );
}

#[test]
fn character_references_decode_and_compose() {
    assert_eq!(
        text("<p>Tom &amp; Jerry &lt;3 &eacute;&#233;&#xE9;</p>"),
        "Tom & Jerry <3 ééé"
    );
    // A combining mark after a reference still meets its base (NFC).
    assert_eq!(text("<p>e&#x301;t&#x65;&#x301;</p>"), "\u{e9}t\u{e9}");
}

#[test]
fn silent_elements_are_skipped() {
    let html = "<head><style>p{color:red}</style><script>var a = '<p>no</p>';</script>\
        <noscript><p>enable js</p></noscript></head><body><p>shown</p>\
        <template><p>template</p></template><svg><text>chart</text><title>t</title></svg>\
        <iframe><p>frame</p></iframe><datalist><option>dl</option></datalist>\
        <!-- a comment --><p>also shown</p></body>";
    let r = read(html);
    assert_eq!(r.text, "shown\n\nalso shown");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn raw_text_inside_a_hidden_element_doesnt_end_it() {
    let html = "<div hidden>a<script>if (x) { document.write('</div>') }</script>b</div>c";
    assert_eq!(text(html), "c");
}

#[test]
fn hidden_and_display_none_are_skipped_with_a_warning() {
    let cases = [
        "<p>a</p><div hidden>secret</div><p>b</p>",
        "<p>a</p><div hidden=\"until-found\">secret</div><p>b</p>",
        "<p>a</p><span style=\"color: red; DISPLAY : None !important\">secret</span><p>b</p>",
        "<p>a</p><div style=\"display:block; display:none\">secret</div><p>b</p>",
    ];
    for html in cases {
        let r = read(html);
        assert_eq!(r.text, "a\n\nb", "{html}");
        assert_eq!(r.warnings, vec![Warning::HiddenTextSkipped], "{html}");
    }
    // Not hidden: a later display wins, unless the earlier is !important;
    // aria-hidden only hides from screen readers.
    assert_eq!(text("<p style='display:none; display:block'>x</p>"), "x");
    assert_eq!(
        text("<p style='display:none!important; display:block'>x</p>"),
        ""
    );
    assert_eq!(text("<p aria-hidden=\"true\">x</p>"), "x");
    // A hidden element with only whitespace isn't worth a warning.
    assert!(read("<div hidden> </div>x").warnings.is_empty());
}

#[test]
fn nested_elements_of_the_same_name_are_counted() {
    assert_eq!(
        text("<div hidden><div>a</div>still hidden<div><div>b</div></div></div>shown"),
        "shown"
    );
    assert_eq!(
        text("<span style='display:none'>x<div>y</div>z</span>w"),
        "w"
    );
}

#[test]
fn implied_ends_close_hidden_elements() {
    assert_eq!(text("<p hidden>x<p>shown"), "shown");
    assert_eq!(text("<div><p hidden>x</div>shown"), "shown");
    assert_eq!(text("<p hidden>x<b>y</b><ul><li>shown</ul>"), "- shown");
    assert_eq!(text("<ul><li hidden>a<li>b</ul>"), "- b");
    assert_eq!(text("<ul><li hidden>a<ul><li>b</ul>c<li>d</ul>"), "- d");
    assert_eq!(text("<ul><li hidden>a</ul><p>after</p>"), "after");
    assert_eq!(text("<dl><dt hidden>t<dd>d</dl>"), "d");
    assert_eq!(
        text("<table><tr><td hidden>a<td>b<tr hidden><td>c<tr><td>d</table>"),
        "b\nd"
    );
    assert_eq!(
        text("<table><tr><td hidden><table><tr><td>x</table><td>y</table>"),
        "y"
    );
    assert_eq!(text("<select><option hidden>a<option>b</select>"), "b");
    // The end of the body ends anything.
    assert_eq!(text("<div hidden>x</body>y"), "y");
}

#[test]
fn self_closing_and_void_elements_are_empty() {
    assert_eq!(text("<div hidden/>shown"), "shown");
    assert_eq!(text("<script src=\"a.js\"/>shown"), "shown");
    assert_eq!(text("<svg/>shown"), "shown");
    assert_eq!(text("<img hidden src=x>shown<input hidden>"), "shown");
}

#[test]
fn foreign_content() {
    // CDATA is text in MathML and skipped with the rest of an SVG.
    assert_eq!(text("<math><mi>x</mi><![CDATA[ + y]]></math>"), "x + y");
    assert_eq!(text("<svg><![CDATA[</svg>]]></svg>shown"), "shown");
    // No raw text in foreign content: a <style> in SVG doesn't swallow
    // the rest.
    assert_eq!(text("<svg><style>a</style></svg>shown"), "shown");
}

#[test]
fn the_title_is_the_first_title() {
    let r = read("<title>\n  Q3 &amp;\n plan </title><title>second</title><p>body</p>");
    assert_eq!(r.title.as_deref(), Some("Q3 & plan"));
    assert_eq!(r.text, "body");
    // In the body, still not text; inside SVG, not the title.
    let r = read("<p>a</p><svg><title>icon</title></svg><title>Late</title>");
    assert_eq!(r.title.as_deref(), Some("Late"));
    assert_eq!(r.text, "a");
    assert_eq!(read("<title> </title>x").title, None);
    // A <title> in markup-looking form is still raw text.
    let r = read("<title>a <b>b</b></title>");
    assert_eq!(r.title.as_deref(), Some("a <b>b</b>"));
    // Long titles are cut to 200 characters.
    let long = format!("<title>{}</title>", "word ".repeat(10_000));
    assert_eq!(read(&long).title.unwrap().chars().count(), 199);
}

#[test]
fn stray_text_in_the_head_shows_as_in_browsers() {
    assert_eq!(
        text("<head><meta charset=utf-8>stray<link rel=x></head>"),
        "stray"
    );
}

#[test]
fn nothing_visible_is_no_text() {
    let r = read("<html><head><script>x()</script></head><body> </body></html>");
    assert_eq!(r.text, "");
    assert_eq!(r.warnings, vec![Warning::NoText]);
    assert_eq!(read("").warnings, vec![Warning::NoText]);
}

#[test]
fn caps_count_the_whole_document() {
    let html = format!("<p>{}</p><p>tail words</p>", "lorem ipsum. ".repeat(2_000));
    let full = read(&html);
    assert!(!full.truncated);
    let r = read_bytes_capped(html.as_bytes(), 1_000).unwrap();
    assert!(r.truncated && r.chars <= 1_000);
    assert_eq!(r.total, Some(full.chars));
    assert!(
        r.text.ends_with('.'),
        "cut at a sentence end: {:?}",
        &r.text[r.text.len() - 20..]
    );
}

#[test]
fn truncation_prefers_paragraphs_then_sentences_then_spaces() {
    let cut = |html: String| read_bytes_capped(html.as_bytes(), 1_000).unwrap();
    let words = |n: usize| "word ".repeat(n / 5);
    // A paragraph break in the last 5 %.
    let r = cut(format!("<p>{}end</p><p>{}</p>", words(975), words(200)));
    assert_eq!(r.text, format!("{}end", words(975)));
    // Else a sentence end there.
    let r = cut(format!("<p>{}end. {}</p>", words(970), words(200)));
    assert_eq!(r.text, format!("{}end.", words(970)));
    // Else a space.
    let r = cut(format!("<p>{}</p>", words(2_000)));
    assert!(r.text.ends_with("word") && r.chars < 1_000);
    // Else exactly at the cap.
    let r = cut(format!("<p>{}</p>", "x".repeat(2_000)));
    assert_eq!((r.chars, r.total), (1_000, Some(2_000)));
    assert!(r.truncated);
}

#[test]
fn long_blocks_are_pushed_in_pieces_without_changing_the_text() {
    let word = "cafe\u{301} ";
    let html = format!("<p>{}</p>", word.repeat(15_000));
    let r = read(&html);
    assert!(!r.truncated);
    assert_eq!(r.text, "caf\u{e9} ".repeat(15_000).trim_end());
}

#[test]
fn deep_and_broken_markup_finishes() {
    let deep = "<div>".repeat(100_000) + "x" + &"</div>".repeat(3);
    assert_eq!(text(&deep), "x");
    let unclosed = "<div hidden>".repeat(10_000) + "x";
    assert_eq!(text(&unclosed), "");
    let junk = "<<<a <b =\"<p> <!-- <![CDATA[ &#xFFFFFF; &; </ >".repeat(1_000);
    let _ = read(&junk);
}

#[test]
fn encodings() {
    // <meta charset>
    let mut html = b"<html><head><meta charset=\"windows-1251\"></head><p>".to_vec();
    html.extend_from_slice(&WINDOWS_1251.encode("Привет мир").0);
    let r = read_bytes(&html);
    assert_eq!(r.text, "Привет мир");
    assert!(r.warnings.is_empty());
    // http-equiv content-type
    let mut html =
        b"<meta http-equiv='Content-Type' content='text/html; charset=Shift_JIS'><p>".to_vec();
    html.extend_from_slice(&SHIFT_JIS.encode("日本語の文書").0);
    assert_eq!(read_bytes(&html).text, "日本語の文書");
    // No meta, not UTF-8: guessed, with a warning.
    let mut html = b"<p>".to_vec();
    html.extend_from_slice(&WINDOWS_1252.encode("Café crème, déjà vu, naïve").0);
    let r = read_bytes(&html);
    assert_eq!(r.text, "Café crème, déjà vu, naïve");
    assert_eq!(r.warnings, vec![Warning::LegacyEncoding]);
    // A BOM beats the meta tag.
    let mut html = b"\xEF\xBB\xBF<meta charset=windows-1251><p>".to_vec();
    html.extend_from_slice("ünïcode".as_bytes());
    assert_eq!(read_bytes(&html).text, "ünïcode");
}

#[test]
fn prescan_follows_the_standard() {
    let find = |s: &str| prescan(s.as_bytes()).map(|e| e.name());
    assert_eq!(find("<meta charset=windows-1251>"), Some("windows-1251"));
    assert_eq!(find("<META CHARSET='KOI8-R'/>"), Some("KOI8-R"));
    assert_eq!(find("<meta charset=utf-16le>"), Some(UTF_8.name()));
    assert_eq!(
        find("<meta charset=x-user-defined>"),
        Some(WINDOWS_1252.name())
    );
    assert_eq!(
        find("<meta charset=nonsense><meta charset=gbk>"),
        Some("GBK")
    );
    // content= needs http-equiv=content-type.
    assert_eq!(find("<meta content='text/html; charset=gbk'>"), None);
    assert_eq!(
        find("<meta content=\"text/html;charset = 'big5'\" http-equiv=\"content-type\">"),
        Some("Big5")
    );
    // Comments and other tags' attributes are stepped over.
    assert_eq!(
        find("<!-- <meta charset=gbk> --><meta charset=euc-kr>"),
        Some("EUC-KR")
    );
    assert_eq!(find("<!--><meta charset=gbk>"), Some("GBK"));
    assert_eq!(
        find("<a title='<meta charset=gbk>'><meta charset=big5>"),
        Some("Big5")
    );
    assert_eq!(find("<metadata charset=gbk>"), None);
    // Only the first 1,024 bytes.
    let late = format!("{}<meta charset=gbk>", " ".repeat(1_020));
    assert_eq!(find(&late), None);
    assert_eq!(find("<meta charset=gbk"), None);
}

#[test]
fn binary_and_other_formats_are_refused() {
    let mut binary = b"<html>".to_vec();
    binary.extend((0u8..32).cycle().take(4_000));
    assert_eq!(
        read_bytes_capped(&binary, 100).err(),
        Some(WorkerErrorCode::NotText)
    );
    assert_eq!(
        read_bytes_capped(b"%PDF-1.7\n<html>", 100).err(),
        Some(WorkerErrorCode::WrongContent)
    );
    assert_eq!(
        read_bytes_capped(b"{\\rtf1 <p>x</p>}", 100).err(),
        Some(WorkerErrorCode::WrongContent)
    );
}
