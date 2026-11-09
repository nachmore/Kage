//! Canonical JSON bytes and their hash, so equal values compare equal on
//! every computer and in every Kage version (docs/design/SYNC.md §5.4,
//! §7.2).
//!
//! The form follows RFC 8785 (JCS):
//! - object keys sorted by their UTF-16 code units, no whitespace;
//! - strings escape only `"`, `\` and control characters (`\b`, `\t`,
//!   `\n`, `\f`, `\r` short, the rest as `\u00xx`);
//! - numbers as ECMAScript prints them: an integral float has no `.0`
//!   (`1.0` and `1` are the same value), exponents above 1e21 and below
//!   1e-6 are written `1e+21`, `1e-7`, and `-0` is `0`.
//!
//! The JavaScript side (`JSON.stringify`) never writes `1.0`, and Rust
//! writes `1.0` for an `f64`; this form makes both the same value.

use sha2::{Digest, Sha256};

/// `v` as canonical JSON.
pub fn canonical(v: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    write_value(&mut out, v);
    out
}

/// SHA-256 of [`canonical`].
pub fn hash(v: &serde_json::Value) -> [u8; 32] {
    Sha256::digest(canonical(v)).into()
}

/// Whether two values have the same canonical form, without building it
/// when they are plainly equal or plainly different.
pub fn same(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match (a, b) {
        (Number(x), Number(y)) => x == y || number_text(x) == number_text(y),
        (String(x), String(y)) => x == y,
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Object(x), Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q)))
        }
        _ => a == b,
    }
}

fn write_value(out: &mut Vec<u8>, v: &serde_json::Value) {
    use serde_json::Value::*;
    match v {
        Null => out.extend_from_slice(b"null"),
        Bool(true) => out.extend_from_slice(b"true"),
        Bool(false) => out.extend_from_slice(b"false"),
        Number(n) => out.extend_from_slice(number_text(n).as_bytes()),
        String(s) => write_string(out, s),
        Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(out, x);
            }
            out.push(b']');
        }
        Object(m) => {
            let mut keys: Vec<&std::string::String> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push(b'{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(out, k);
                out.push(b':');
                write_value(out, &m[k]);
            }
            out.push(b'}');
        }
    }
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for &b in s.as_bytes() {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x0a => out.extend_from_slice(b"\\n"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0d => out.extend_from_slice(b"\\r"),
            0x00..=0x1f => {
                out.extend_from_slice(b"\\u00");
                out.push(HEX[(b >> 4) as usize]);
                out.push(HEX[(b & 0xf) as usize]);
            }
            // Multi-byte UTF-8 sequences pass through unchanged.
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

fn number_text(n: &serde_json::Number) -> String {
    if let Some(u) = n.as_u64() {
        u.to_string()
    } else if let Some(i) = n.as_i64() {
        i.to_string()
    } else {
        es_number(n.as_f64().unwrap_or(0.0))
    }
}

/// ECMAScript `Number.prototype.toString` for a finite `f64`, from the
/// shortest digits that round-trip (Rust's `{:e}`).
fn es_number(f: f64) -> String {
    if f == 0.0 || !f.is_finite() {
        return "0".into();
    }
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // The value is 0.<digits> × 10^n.
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
    let mut out = String::new();
    if f < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = n - 1;
        out.push('e');
        out.push(if e > 0 { '+' } else { '-' });
        out.push_str(&e.abs().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(v: &serde_json::Value) -> String {
        String::from_utf8(canonical(v)).unwrap()
    }

    #[test]
    fn keys_are_sorted_and_there_is_no_whitespace() {
        let v = json!({ "b": [1, { "z": null, "a": true }], "a": "x", "": false });
        assert_eq!(
            text(&v),
            r#"{"":false,"a":"x","b":[1,{"a":true,"z":null}]}"#
        );
    }

    #[test]
    fn keys_sort_by_utf16_code_units() {
        // U+FB01 is above the surrogates that encode U+1F600, so UTF-16
        // order differs from UTF-8 (and code point) order here.
        let v = json!({ "\u{fb01}": 1, "\u{1f600}": 2 });
        assert_eq!(text(&v), "{\"\u{1f600}\":2,\"\u{fb01}\":1}");
    }

    #[test]
    fn strings_escape_only_what_jcs_escapes() {
        let v = json!("q\"b\\\u{8}\t\n\u{c}\r\u{1}\u{1f}/é\u{2028}\u{7f}");
        assert_eq!(
            text(&v),
            "\"q\\\"b\\\\\\b\\t\\n\\f\\r\\u0001\\u001f/é\u{2028}\u{7f}\""
        );
    }

    #[test]
    fn numbers_print_like_ecmascript() {
        let cases: &[(serde_json::Value, &str)] = &[
            (json!(0), "0"),
            (json!(-0.0), "0"),
            (json!(1.0), "1"),
            (json!(1), "1"),
            (json!(-3), "-3"),
            (json!(u64::MAX), "18446744073709551615"),
            (json!(i64::MIN), "-9223372036854775808"),
            (json!(0.1), "0.1"),
            (json!(123.456), "123.456"),
            (json!(-1.5), "-1.5"),
            (json!(1e20), "100000000000000000000"),
            (json!(1e21), "1e+21"),
            (json!(1.5e300), "1.5e+300"),
            (json!(0.000001), "0.000001"),
            (json!(1e-7), "1e-7"),
            (json!(-2.5e-8), "-2.5e-8"),
            (json!(5e-324), "5e-324"),
        ];
        for (v, want) in cases {
            assert_eq!(text(v), *want, "{v}");
        }
    }

    #[test]
    fn same_treats_one_and_one_point_zero_as_equal() {
        assert!(same(&json!(1), &json!(1.0)));
        assert!(same(&json!({ "a": [1.0, 2] }), &json!({ "a": [1, 2.0] })));
        assert!(!same(&json!(1), &json!(1.5)));
        assert!(!same(&json!({ "a": 1 }), &json!({ "a": 1, "b": 2 })));
        assert!(!same(&json!("1"), &json!(1)));
        assert!(!same(&json!(null), &json!(false)));
        assert_eq!(hash(&json!([1.0])), hash(&json!([1])));
        assert_ne!(hash(&json!([1])), hash(&json!(["1"])));
    }

    /// `same` and the canonical bytes agree, on random values.
    #[test]
    fn same_agrees_with_canonical_bytes() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        fn gen(r: &mut StdRng, depth: u32) -> serde_json::Value {
            match r.random_range(0..if depth > 2 { 4 } else { 6 }) {
                0 => json!(null),
                1 => json!(r.random_bool(0.5)),
                2 => {
                    if r.random_bool(0.5) {
                        json!(r.random_range(0..3))
                    } else {
                        json!(r.random_range(0..3) as f64)
                    }
                }
                3 => json!(["a", "b", "é"][r.random_range(0..3)]),
                4 => (0..r.random_range(0..3))
                    .map(|_| gen(r, depth + 1))
                    .collect(),
                _ => {
                    let mut m = serde_json::Map::new();
                    for _ in 0..r.random_range(0..3) {
                        m.insert(
                            ["x", "y", "z"][r.random_range(0..3)].into(),
                            gen(r, depth + 1),
                        );
                    }
                    serde_json::Value::Object(m)
                }
            }
        }
        let mut r = StdRng::seed_from_u64(7);
        for _ in 0..5_000 {
            let a = gen(&mut r, 0);
            let b = gen(&mut r, 0);
            assert_eq!(same(&a, &b), canonical(&a) == canonical(&b), "{a} {b}");
            assert!(same(&a, &a));
        }
    }
}
