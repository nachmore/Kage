//! Code pages and the control words the RTF reader knows by name.

use encoding_rs::{
    Encoding, BIG5, EUC_JP, EUC_KR, GB18030, GBK, IBM866, ISO_8859_13, ISO_8859_15, ISO_8859_2,
    ISO_8859_3, ISO_8859_4, ISO_8859_5, ISO_8859_6, ISO_8859_7, ISO_8859_8, KOI8_R, KOI8_U,
    MACINTOSH, SHIFT_JIS, UTF_8, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252, WINDOWS_1253,
    WINDOWS_1254, WINDOWS_1255, WINDOWS_1256, WINDOWS_1257, WINDOWS_1258, WINDOWS_874,
    X_MAC_CYRILLIC,
};

/// The encoding of a Windows code page (`\ansicpgN`, `\cpgN`). `None`
/// for code pages `encoding_rs` doesn't have (the DOS ones but 866): the
/// caller keeps what it had.
pub fn code_page(n: i32) -> Option<&'static Encoding> {
    Some(match n {
        866 => IBM866,
        874 => WINDOWS_874,
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1252 | 28591 => WINDOWS_1252,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        10000 => MACINTOSH,
        10007 => X_MAC_CYRILLIC,
        20866 => KOI8_R,
        21866 => KOI8_U,
        20932 | 51932 => EUC_JP,
        28592 => ISO_8859_2,
        28593 => ISO_8859_3,
        28594 => ISO_8859_4,
        28595 => ISO_8859_5,
        28596 => ISO_8859_6,
        28597 => ISO_8859_7,
        28598 => ISO_8859_8,
        28603 => ISO_8859_13,
        28605 => ISO_8859_15,
        54936 => GB18030,
        65001 => UTF_8,
        _ => return None,
    })
}

/// The encoding of a font's character set (`\fcharsetN`). ANSI (0) is
/// Windows-1252 whatever the document's code page, as LibreOffice and
/// Cocoa read it. `None` for default and symbol fonts (1, 2), which use
/// the document's code page, and for sets `encoding_rs` doesn't have.
pub fn charset(n: i32) -> Option<&'static Encoding> {
    code_page(match n {
        0 => 1252,
        77 => 10000,
        78 | 128 => 932,
        129 => 949,
        134 => 936,
        136 => 950,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        _ => return None,
    })
}

/// Destinations whose text is never part of the body: tables, pictures,
/// objects, headers, footers, field instructions, comments, bookmarks,
/// index and contents entries, list numbering, shape internals, and the
/// document's private data. Starred destinations the reader doesn't know
/// are skipped too.
pub fn is_skipped_destination(name: &[u8]) -> bool {
    matches!(
        name,
        b"colortbl"
            | b"listtable"
            | b"listoverridetable"
            | b"revtbl"
            | b"rsidtbl"
            | b"pict"
            | b"object"
            | b"header"
            | b"headerl"
            | b"headerr"
            | b"headerf"
            | b"footer"
            | b"footerl"
            | b"footerr"
            | b"footerf"
            | b"fldinst"
            | b"ftnsep"
            | b"ftnsepc"
            | b"ftncn"
            | b"aftnsep"
            | b"aftnsepc"
            | b"aftncn"
            | b"pn"
            | b"pntxta"
            | b"pntxtb"
            | b"xe"
            | b"tc"
            | b"tcn"
            | b"txe"
            | b"do"
            | b"sp"
            | b"shprslt"
            | b"shppict"
            | b"nonshppict"
            | b"annotation"
            | b"atnid"
            | b"atnauthor"
            | b"atndate"
            | b"atnref"
            | b"atntime"
            | b"atnicn"
            | b"atnparent"
            | b"bkmkstart"
            | b"bkmkend"
            | b"datafield"
            | b"formfield"
            | b"generator"
            | b"filetbl"
            | b"themedata"
            | b"colorschememapping"
            | b"datastore"
            | b"latentstyles"
            | b"xmlnstbl"
            | b"pgdsctbl"
            | b"template"
            | b"docvar"
            | b"userprops"
            | b"private"
            | b"objdata"
            | b"result"
            | b"falt"
            | b"panose"
            | b"keycode"
    )
}

/// Control words that stand for one character.
pub fn char_word(name: &[u8]) -> Option<char> {
    Some(match name {
        b"tab" => '\t',
        b"emdash" => '\u{2014}',
        b"endash" => '\u{2013}',
        b"emspace" => '\u{2003}',
        b"enspace" => '\u{2002}',
        b"qmspace" => '\u{2005}',
        b"bullet" => '\u{2022}',
        b"lquote" => '\u{2018}',
        b"rquote" => '\u{2019}',
        b"ldblquote" => '\u{201C}',
        b"rdblquote" => '\u{201D}',
        b"zwj" => '\u{200D}',
        b"zwnj" => '\u{200C}',
        b"zwbo" => '\u{200B}',
        b"ltrmark" => '\u{200E}',
        b"rtlmark" => '\u{200F}',
        _ => return None,
    })
}

/// The heading level of a paragraph style, from its name: Word's and
/// LibreOffice's English names ("heading 1", "Heading 1,h1", "Title").
pub fn heading_of_style_name(name: &str) -> u8 {
    let name = name.split([';', ',']).next().unwrap_or("").trim();
    match name.to_ascii_lowercase().as_str() {
        "title" | "heading 1" => 1,
        "heading 2" => 2,
        "heading 3" => 3,
        _ => 0,
    }
}

/// The heading level of `\outlinelevelN` (0-based): levels 0–2 are
/// headings 1–3; deeper levels and 9 (body text) are none.
pub fn heading_of_outline_level(n: i32) -> u8 {
    match n {
        0..=2 => n as u8 + 1,
        _ => 0,
    }
}
