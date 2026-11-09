use super::*;
use pdf_extract::Dictionary;
use std::io::Write;

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn stream(filters: &[&str], content: Vec<u8>) -> Stream {
    let mut dict = Dictionary::new();
    match filters {
        [] => {}
        [one] => dict.set("Filter", Object::Name(one.as_bytes().to_vec())),
        many => dict.set(
            "Filter",
            Object::Array(
                many.iter()
                    .map(|f| Object::Name(f.as_bytes().to_vec()))
                    .collect(),
            ),
        ),
    }
    Stream::new(dict, content)
}

fn bomb() -> Vec<u8> {
    zlib(&vec![0u8; CAP + 1])
}

#[test]
fn a_flate_bomb_is_emptied() {
    let mut s = stream(&["FlateDecode"], bomb());
    assert!(cap_stream(&mut s));
    assert!(s.content.is_empty());
    assert!(s.dict.get(b"Filter").is_err());
}

#[test]
fn a_stream_just_under_the_cap_is_kept() {
    let data = zlib(&vec![b'a'; CAP]);
    let mut s = stream(&["FlateDecode"], data.clone());
    assert!(!cap_stream(&mut s));
    assert_eq!(s.content, data);
}

#[test]
fn a_bomb_behind_ascii85_or_hex_is_found() {
    let hex: Vec<u8> = bomb()
        .iter()
        .flat_map(|b| format!("{b:02x}").into_bytes())
        .collect();
    let mut s = stream(&["ASCIIHexDecode", "FlateDecode"], hex);
    assert!(cap_stream(&mut s));
    let mut a85 = stream(&["A85", "FlateDecode"], to_ascii85(&bomb()));
    assert!(cap_stream(&mut a85));
}

#[test]
fn a_nested_bomb_is_found() {
    let inner = zlib(&vec![0u8; CAP + 1]);
    let outer = zlib(&inner);
    let mut s = stream(&["FlateDecode", "FlateDecode"], outer);
    assert!(cap_stream(&mut s));
}

#[test]
fn images_are_emptied_without_inflating() {
    let mut s = stream(&["DCTDecode"], vec![1, 2, 3]);
    s.dict.set("Subtype", Object::Name(b"Image".to_vec()));
    assert!(cap_stream(&mut s));
    assert!(s.content.is_empty());
}

#[test]
fn the_filter_caps_streams_and_copies_the_rest() {
    let mut object = Object::Stream(stream(&["FlateDecode"], bomb()));
    let (_, returned) = cap_object((1, 0), &mut object).unwrap();
    assert_eq!(returned, Object::Null);
    assert!(object.as_stream().unwrap().content.is_empty());
    let mut plain = Object::Integer(7);
    let (id, returned) = cap_object((2, 0), &mut plain).unwrap();
    assert_eq!((id, returned), ((2, 0), Object::Integer(7)));
}

#[test]
fn raw_deflate_without_a_header_is_counted() {
    let mut raw = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    raw.write_all(&vec![0u8; CAP + 1]).unwrap();
    let mut data = vec![0x78, 0x00]; // a zlib header lopdf skips on failure
    data.extend(raw.finish().unwrap());
    assert!(inflates_past(&stream(&["FlateDecode"], data), CAP));
}

#[test]
fn decoded_applies_filters_within_the_cap() {
    let s = stream(&["FlateDecode"], zlib(b"BT (hi) Tj ET"));
    assert_eq!(decoded(&s, 100).unwrap(), b"BT (hi) Tj ET");
    assert!(decoded(&s, 4).is_none());
    let plain = stream(&[], b"plain".to_vec());
    assert_eq!(decoded(&plain, 5).unwrap(), b"plain");
    assert!(decoded(&plain, 4).is_none());
}

fn to_ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        let mut group = [0u8; 4];
        group[..chunk.len()].copy_from_slice(chunk);
        let mut n = u32::from_be_bytes(group);
        if chunk.len() == 4 && n == 0 {
            out.push(b'z');
            continue;
        }
        let mut digits = [0u8; 5];
        for d in digits.iter_mut().rev() {
            *d = (n % 85) as u8 + b'!';
            n /= 85;
        }
        out.extend_from_slice(&digits[..chunk.len() + 1]);
    }
    out.extend_from_slice(b"~>");
    out
}

#[test]
fn ascii85_and_hex_decode() {
    let text = b"Man is distinguished, not only by his reason.";
    assert_eq!(ascii85(&to_ascii85(text)), text);
    assert_eq!(ascii85(&to_ascii85(&[0, 0, 0, 0, 1])), [0, 0, 0, 0, 1]);
    assert_eq!(ascii_hex(b"48 65 6c6C 6f>"), b"Hello");
    assert_eq!(ascii_hex(b"4>"), [0x40]);
}
