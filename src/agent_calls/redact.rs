//! Credentials and secret-shaped text in a request
//! (docs/design/EXTENSION_AGENT_CALLS.md §5.9).
//!
//! Safety nets, not the boundary: the extension decides what it sends.
//! 1. A request that contains, verbatim, one of the extension's own saved
//!    credentials is refused (`contains_credential`).
//! 2. Secret-shaped substrings in `instructions` and in every string of
//!    `input` are replaced with `[redacted]`, using the backup classifier's
//!    shapes (`config_export::sensitive::secret_spans`).

use serde_json::Value;

use super::request::ValidRequest;
use crate::config_export::sensitive::secret_spans;

/// What a redacted span becomes.
pub const REDACTED: &str = "[redacted]";

/// Shortest saved setting value treated as a credential.
pub const MIN_SECRET_CHARS: usize = 8;

/// The string values of 8 or more characters among an extension's saved
/// settings that are credentials: a top-level field its manifest marks
/// `sensitive`, or any field (at any depth) whose name the safety net
/// flags ([`crate::extensions::is_sensitive_name`]). Everything inside such
/// a field counts.
pub fn collect_secrets(settings: &Value, sensitive_fields: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Value::Object(map) = settings {
        for (field, value) in map {
            let flagged = sensitive_fields.iter().any(|f| f == field)
                || crate::extensions::is_sensitive_name(field);
            walk_secrets(value, flagged, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn walk_secrets(value: &Value, flagged: bool, out: &mut Vec<String>) {
    match value {
        Value::String(s) if flagged && s.chars().count() >= MIN_SECRET_CHARS => {
            out.push(s.clone());
        }
        Value::Array(items) => {
            for v in items {
                walk_secrets(v, flagged, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                walk_secrets(v, flagged || crate::extensions::is_sensitive_name(k), out);
            }
        }
        _ => {}
    }
}

/// Every string in `value`, keys included.
fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(s) => out.push(s),
        Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        Value::Object(map) => {
            for (k, v) in map {
                out.push(k);
                strings(v, out);
            }
        }
        _ => {}
    }
}

/// Whether any text that would reach the agent (instructions, input,
/// schema) contains one of `secrets` verbatim.
pub fn contains_credential(req: &ValidRequest, secrets: &[String]) -> bool {
    if secrets.is_empty() {
        return false;
    }
    let mut texts: Vec<&str> = vec![req.instructions.as_str()];
    strings(&req.input, &mut texts);
    if let Some(schema) = &req.schema {
        strings(schema, &mut texts);
    }
    texts.iter().any(|t| {
        secrets
            .iter()
            .any(|s| !s.is_empty() && t.contains(s.as_str()))
    })
}

/// `s` with each secret-shaped span replaced; the number of spans.
pub fn redact_text(s: &str) -> (String, u32) {
    let spans = secret_spans(s);
    if spans.is_empty() {
        return (s.to_string(), 0);
    }
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    for r in &spans {
        out.push_str(&s[at..r.start]);
        out.push_str(REDACTED);
        at = r.end;
    }
    out.push_str(&s[at..]);
    (out, u32::try_from(spans.len()).unwrap_or(u32::MAX))
}

fn redact_value(value: &mut Value) -> u32 {
    match value {
        Value::String(s) => {
            let (clean, n) = redact_text(s);
            if n > 0 {
                *s = clean;
            }
            n
        }
        Value::Array(items) => items.iter_mut().map(redact_value).sum(),
        // Values only: rewriting keys could merge two of them.
        Value::Object(map) => map.values_mut().map(redact_value).sum(),
        _ => 0,
    }
}

/// Redact `instructions` and every string value in `input`, in place.
/// Returns how many spans were replaced. The schema is the extension's
/// own fixed text and is left as it is (redacting it could break an enum).
pub fn redact_request(req: &mut ValidRequest) -> u32 {
    let (clean, n) = redact_text(&req.instructions);
    req.instructions = clean;
    n + redact_value(&mut req.input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(instructions: &str, input: Value) -> ValidRequest {
        ValidRequest {
            label: None,
            instructions: instructions.into(),
            input,
            schema: None,
            max_response_chars: 8000,
            timeout_ms: 60000,
        }
    }

    #[test]
    fn collects_sensitive_setting_values() {
        let settings = json!({
            "api_key": "abcdefgh12345",
            "endpoint": "https://example.com/api",
            "vault": { "anything": "long enough value" },
            "nested": { "password": "hunter2hunter2", "short": "x" },
            "pin": "1234",
            "token": ["tok-aaaaaaaa", 5]
        });
        let secrets = collect_secrets(&settings, &["vault".to_string()]);
        assert_eq!(
            secrets,
            vec![
                "abcdefgh12345".to_string(),
                "hunter2hunter2".to_string(),
                "long enough value".to_string(),
                "tok-aaaaaaaa".to_string(),
            ]
        );
    }

    #[test]
    fn credential_anywhere_that_reaches_the_agent() {
        let secrets = vec!["abcdefgh12345".to_string()];
        assert!(contains_credential(
            &req("use abcdefgh12345", json!({})),
            &secrets
        ));
        assert!(contains_credential(
            &req("i", json!({ "notes": [{ "text": "key=abcdefgh12345!" }] })),
            &secrets
        ));
        assert!(contains_credential(
            &req("i", json!({ "abcdefgh12345": 1 })),
            &secrets
        ));
        let mut r = req("i", json!("x"));
        r.schema = Some(json!({ "description": "abcdefgh12345" }));
        assert!(contains_credential(&r, &secrets));
        assert!(!contains_credential(
            &req("i", json!("abcdefgh1234")),
            &secrets
        ));
        assert!(!contains_credential(&req("anything", json!("x")), &[]));
    }

    #[test]
    fn each_secret_shape_family_is_redacted() {
        let cases = [
            "ghp_abcdefghijklmnop1234",
            "github_pat_abcdefghijklmnop_1234",
            "glpat-abcdefghijklmnop12",
            "xoxb-12345678-abcdef",
            "sk-ant-abcdefghijklmnop1234",
            "AIzaabcdefghijklmnopqrstuvw",
            "AKIAABCDEFGHIJKLMNOP",
            "eyJhbGciOi.eyJzdWIiOi.abcdefghij",
            "Bearer abcdefgh12345678",
            "https://user:pa55word@example.com/x",
            "gsk_abcdefghijklmnop1234",
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQ",
            "https://example.com/cb?access_token=abc123def456",
        ];
        for secret in cases {
            let text = format!("before {secret} after");
            let (clean, n) = redact_text(&text);
            assert!(n >= 1, "{secret}");
            assert!(!clean.contains(secret), "{secret} → {clean}");
            assert!(clean.starts_with("before "), "{clean}");
            assert!(clean.contains(REDACTED));
        }
    }

    #[test]
    fn labels_are_extended_over_their_value() {
        let (clean, n) = redact_text("my password: **hunter2hunter2** ok");
        assert_eq!(n, 1);
        assert!(!clean.contains("hunter2"), "{clean}");
        assert!(clean.ends_with(" ok"), "{clean}");
        let (clean, _) = redact_text("api key = s3cr3tvalue then more");
        assert!(!clean.contains("s3cr3tvalue"), "{clean}");
        assert!(clean.ends_with(" then more"), "{clean}");
    }

    #[test]
    fn putty_key_files_are_redacted_whole() {
        let text = "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\nPublic-Lines: 2\nAAAAC3NzaC1lZDI1NTE5\nAAAAIOd2\nnext line";
        let (clean, n) = redact_text(text);
        assert_eq!(n, 1);
        assert!(!clean.contains("AAAAC3NzaC1lZDI1NTE5"), "{clean}");
        assert!(clean.ends_with("\nnext line"), "{clean}");
    }

    #[test]
    fn key_files_are_redacted_whole() {
        let text = "key:\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEA\nAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----\nafter";
        let (clean, n) = redact_text(text);
        assert!(n >= 1);
        assert!(!clean.contains("b3BlbnNzaC1rZXktdjEA"), "{clean}");
        assert!(!clean.contains("END OPENSSH"), "{clean}");
        assert!(clean.ends_with("\nafter"), "{clean}");
    }

    #[test]
    fn plain_urls_and_ids_are_untouched() {
        for text in [
            "See https://example.com/docs/page?id=42 for details",
            "Ticket ABC-12345 and build 7f3c2a9e1b",
            "uuid 1b4e28ba-2fa1-11d2-883f-0016d3cca427",
            "The author: Dana wrote this",
        ] {
            let (clean, n) = redact_text(text);
            assert_eq!(n, 0, "{text}");
            assert_eq!(clean, text);
        }
    }

    #[test]
    fn request_redaction_counts_and_keeps_keys() {
        let mut r = req(
            "token: abcdefghijklmnop",
            json!({ "password: x": "ghp_abcdefghijklmnop1234", "n": [1, "AKIAABCDEFGHIJKLMNOP"] }),
        );
        let n = redact_request(&mut r);
        assert_eq!(n, 3);
        // The label goes with its value.
        assert_eq!(r.instructions, "[redacted]");
        assert_eq!(r.input["password: x"], json!("[redacted]"));
        assert_eq!(r.input["n"][1], json!("[redacted]"));
        assert_eq!(r.input["n"][0], json!(1));
    }
}
