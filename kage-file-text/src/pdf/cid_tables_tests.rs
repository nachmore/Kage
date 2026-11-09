//! The tables' tests, and the generator that writes them from Adobe's
//! files (`regenerate_adobe_tables`).

use super::*;

/// Where Adobe's `pdf2unicode` CMaps are, for the two tests that read
/// them. Unset: those tests skip.
const SOURCE_ENV: &str = "KAGE_ADOBE_CMAPS";

// --- The generator -----------------------------------------------------

fn source_name(ordering: Ordering) -> String {
    format!("Adobe-{}-UCS2", ordering.name())
}

fn file_name(ordering: Ordering) -> String {
    format!("adobe-{}.bin", ordering.name().to_ascii_lowercase())
}

fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

impl CidTable {
    fn set(&mut self, cid: u32, text: &str) {
        let text: String = text
            .chars()
            .filter(|&c| !is_variation_selector(c))
            .collect();
        let cid = cid as usize;
        if text.is_empty() || text == "\u{FFFD}" || cid >= MAX_CIDS {
            return;
        }
        if self.entries.len() <= cid {
            self.entries.resize(cid + 1, 0);
        }
        let mut chars = text.chars();
        self.entries[cid] = match (chars.next(), chars.next()) {
            (Some(c), None) => c as u32,
            _ => {
                assert!(self.pool.len() < MAX_POOL && text.chars().count() <= MAX_POOL_STRING);
                self.pool.push(text);
                POOL_BASE + (self.pool.len() - 1) as u32
            }
        };
    }

    /// Reads one of Adobe's `Adobe-*-UCS2` CMaps: `bfchar` and `bfrange`
    /// blocks from a two-byte CID to UTF-16BE.
    fn from_adobe_cmap(source: &str) -> CidTable {
        let mut table = CidTable::default();
        let mut range: Option<bool> = None;
        for line in source.lines() {
            let line = line.trim();
            if line.ends_with("beginbfchar") || line.ends_with("beginbfrange") {
                range = Some(line.ends_with("beginbfrange"));
                continue;
            }
            if line == "endbfchar" || line == "endbfrange" {
                range = None;
                continue;
            }
            let tokens = hex_tokens(line);
            match (range, tokens.as_slice()) {
                (Some(false), [cid, dst]) => {
                    if let (Some(cid), Some(text)) = (code(cid), utf16(dst, 0)) {
                        table.set(cid, &text);
                    }
                }
                (Some(true), [lo, hi, rest @ ..]) => {
                    let (Some(lo), Some(hi)) = (code(lo), code(hi)) else {
                        continue;
                    };
                    let array = line.contains('[');
                    for (i, cid) in (lo..=hi).enumerate() {
                        let text = if array {
                            rest.get(i).and_then(|t| utf16(t, 0))
                        } else {
                            rest.first().and_then(|t| utf16(t, i as u16))
                        };
                        if let Some(text) = text {
                            table.set(cid, &text);
                        }
                    }
                }
                _ => {}
            }
        }
        table
    }

    /// The compact file form (module docs).
    fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let mut entries = self.entries.clone();
        while entries.last() == Some(&0) {
            entries.pop();
        }
        put(&mut out, entries.len() as u64);
        let mut previous = 0i64;
        for &entry in &entries {
            let delta = i64::from(entry) - previous;
            put(&mut out, ((delta << 1) ^ (delta >> 63)) as u64);
            previous = i64::from(entry);
        }
        put(&mut out, self.pool.len() as u64);
        for text in &self.pool {
            put(&mut out, text.chars().count() as u64);
            for c in text.chars() {
                put(&mut out, u64::from(c as u32));
            }
        }
        out
    }
}

fn put(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The `<…>` tokens of a line, without the brackets.
fn hex_tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else { break };
        out.push(after[..end].trim());
        rest = &after[end + 1..];
    }
    out
}

fn code(hex: &str) -> Option<u32> {
    u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() <= 8)
}

/// UTF-16BE hex, with `offset` added to the last unit (a `bfrange`).
fn utf16(hex: &str, offset: u16) -> Option<String> {
    let hex: String = hex.split_whitespace().collect();
    if hex.is_empty() || !hex.len().is_multiple_of(4) {
        return None;
    }
    let mut units = (0..hex.len())
        .step_by(4)
        .map(|i| u16::from_str_radix(&hex[i..i + 4], 16).ok())
        .collect::<Option<Vec<u16>>>()?;
    let last = units.last_mut()?;
    *last = last.checked_add(offset)?;
    String::from_utf16(&units).ok()
}

fn source(ordering: Ordering) -> Option<String> {
    let dir = std::env::var_os(SOURCE_ENV)?;
    let path = std::path::Path::new(&dir).join(source_name(ordering));
    Some(std::fs::read_to_string(&path).expect("read an Adobe CMap"))
}

/// Rewrites `cmaps/*.bin` from Adobe's files:
///
///   KAGE_ADOBE_CMAPS=/path/to/mapping-resources-pdf/pdf2unicode \
///     cargo test -p kage-file-text --lib regenerate_adobe_tables -- --ignored
#[test]
#[ignore = "writes the checked-in tables; needs KAGE_ADOBE_CMAPS"]
fn regenerate_adobe_tables() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pdf/cmaps");
    for ordering in Ordering::ALL {
        let text = source(ordering).expect("set KAGE_ADOBE_CMAPS");
        let table = CidTable::from_adobe_cmap(&text);
        assert!(
            table.entries.len() > 10_000,
            "{} looks empty",
            ordering.name()
        );
        std::fs::write(dir.join(file_name(ordering)), table.encode()).unwrap();
    }
}

// --- The tests ---------------------------------------------------------

#[test]
fn the_built_in_tables_match_adobes_files() {
    for ordering in Ordering::ALL {
        let Some(text) = source(ordering) else {
            eprintln!("{SOURCE_ENV} isn't set; skipping");
            return;
        };
        let fresh = CidTable::from_adobe_cmap(&text).encode();
        assert!(fresh == ordering.data(), "{} is stale", file_name(ordering));
    }
}

#[test]
fn the_built_in_tables_decode() {
    let sizes = [
        (Ordering::Japan1, 23_000),
        (Ordering::Korea1, 18_000),
        (Ordering::Gb1, 30_000),
        (Ordering::Cns1, 19_000),
    ];
    for (ordering, at_least) in sizes {
        let table = ordering.table().expect("decodes");
        let n = table.entries.len();
        assert!(n >= at_least, "{}: {n}", ordering.name());
        assert_eq!(
            table.lookup(1),
            Some(Mapping::Char(' ')),
            "{}",
            ordering.name()
        );
        assert_eq!(table.lookup(0), None, "U+FFFD is left out");
        assert!(table.lookup(n as u32).is_none());
    }
}

#[test]
fn known_characters_are_where_adobe_puts_them() {
    let cases = [
        (Ordering::Japan1, 843, 'あ'),
        (Ordering::Japan1, 1339, '音'),
        (Ordering::Japan1, 3722, '本'),
        (Ordering::Korea1, 3296, '한'),
        (Ordering::Korea1, 1086, '가'),
        (Ordering::Gb1, 4559, '中'),
        (Ordering::Gb1, 3795, '文'),
        (Ordering::Cns1, 661, '中'),
        (Ordering::Cns1, 4221, '臺'),
    ];
    for (ordering, cid, expected) in cases {
        let table = ordering.table().unwrap();
        assert_eq!(
            table.lookup(cid),
            Some(Mapping::Char(expected)),
            "{ordering:?} {cid}"
        );
    }
}

#[test]
fn variation_selectors_are_dropped_and_long_mappings_kept() {
    let table = Ordering::Japan1.table().unwrap();
    for (_, mapping) in table.iter() {
        let text = match mapping {
            Mapping::Char(c) => c.to_string(),
            Mapping::Text(t) => t.to_string(),
        };
        assert!(!text.chars().any(is_variation_selector), "{text:?}");
        assert!(!text.contains('\u{FFFD}'));
    }
    // Japan1 has "sec" and "min" units as one glyph each.
    assert!(table.iter().any(|(_, m)| m == Mapping::Text("sec")));
}

#[test]
fn reads_bfchar_and_bfrange_with_and_without_arrays() {
    let source = "\
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
3 beginbfchar
<0001> <0041>
<0002> <0030fe00>
<0003> <d840dc00>
endbfchar
2 beginbfrange
<0010> <0012> <3042>
<0020> <0021> [<0066 0066> <0058>]
endbfrange
1 beginbfchar
<0005> <fffd>
endbfchar
";
    let table = CidTable::from_adobe_cmap(source);
    assert_eq!(table.lookup(1), Some(Mapping::Char('A')));
    assert_eq!(table.lookup(2), Some(Mapping::Char('0')), "VS1 dropped");
    assert_eq!(table.lookup(3), Some(Mapping::Char('\u{20000}')));
    assert_eq!(table.lookup(0x11), Some(Mapping::Char('\u{3043}')));
    assert_eq!(table.lookup(0x12), Some(Mapping::Char('\u{3044}')));
    assert_eq!(table.lookup(0x20), Some(Mapping::Text("ff")));
    assert_eq!(table.lookup(0x21), Some(Mapping::Char('X')));
    assert_eq!(table.lookup(5), None);
    assert_eq!(table.lookup(4), None);
}

#[test]
fn encode_and_decode_round_trip() {
    let mut table = CidTable::default();
    table.set(0, "a");
    table.set(7, "\u{20B9F}");
    table.set(8, "ff");
    table.set(9, "b");
    let bytes = table.encode();
    assert_eq!(CidTable::decode(&bytes), Some(table));
}

#[test]
fn broken_files_are_refused() {
    let mut table = CidTable::default();
    table.set(3, "ab");
    let good = table.encode();
    assert!(CidTable::decode(&good).is_some());
    assert!(CidTable::decode(b"").is_none());
    assert!(CidTable::decode(b"KCID\x02").is_none());
    assert!(
        CidTable::decode(&good[..good.len() - 1]).is_none(),
        "cut short"
    );
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(CidTable::decode(&trailing).is_none(), "trailing bytes");
    // One entry pointing at a pool string that isn't there.
    let mut bad = MAGIC.to_vec();
    put(&mut bad, 1);
    put(&mut bad, u64::from(POOL_BASE) << 1);
    put(&mut bad, 0);
    assert!(CidTable::decode(&bad).is_none());
    // A surrogate as a character.
    let mut bad = MAGIC.to_vec();
    put(&mut bad, 1);
    put(&mut bad, 0xD800 << 1);
    put(&mut bad, 0);
    assert!(CidTable::decode(&bad).is_none());
}

#[test]
fn orderings_by_name() {
    assert_eq!(
        Ordering::from_names(b"Adobe", b"Japan1"),
        Some(Ordering::Japan1)
    );
    assert_eq!(Ordering::from_names(b"Adobe", b"GB1"), Some(Ordering::Gb1));
    assert_eq!(Ordering::from_names(b"Adobe", b"Identity"), None);
    assert_eq!(Ordering::from_names(b"Other", b"Japan1"), None);
}
