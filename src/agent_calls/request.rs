//! The wire shapes extensions send, the manifest declaration, and the
//! checks that run on them before anything is sent
//! (docs/design/EXTENSION_AGENT_CALLS.md §3.2, §3.5, §5.6).

use serde::{Deserialize, Serialize};

use super::limits;
use super::schema;
use crate::error::{AppError, ErrorKind};

/// When a reserved call may start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    /// `now`: right after a user's click. Skips only the idle conditions.
    Now,
    /// Default: while the user isn't using Kage. Listed last so an unknown
    /// value falls back to it, the safe choice.
    #[default]
    #[serde(other)]
    Idle,
}

/// `agent_call_reserve` args.
#[derive(Debug, Clone, Deserialize)]
pub struct ReserveArgs {
    pub purpose: String,
    #[serde(default)]
    pub when: Option<When>,
    #[serde(default, alias = "dedupeKey")]
    pub dedupe_key: Option<String>,
}

/// `agent_call_ready` and `agent_call_cancel` args.
#[derive(Debug, Clone, Deserialize)]
pub struct CallIdArgs {
    #[serde(alias = "callId")]
    pub call_id: String,
}

/// `agent_call_send` args.
#[derive(Debug, Clone, Deserialize)]
pub struct SendArgs {
    #[serde(alias = "callId")]
    pub call_id: String,
    pub request: AgentCallRequest,
}

/// What an extension asks the agent to do. The host builds the prompt
/// around it (§5.2).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentCallRequest {
    /// Shown in the log, never sent to the agent.
    #[serde(default)]
    pub label: Option<String>,
    pub instructions: String,
    /// A string, or JSON (pretty-printed into the prompt).
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    #[serde(default, alias = "maxResponseChars")]
    pub max_response_chars: Option<u32>,
    #[serde(default, alias = "timeoutMs")]
    pub timeout_ms: Option<u32>,
}

/// A request that passed [`validate`]: sizes within the limits, the label
/// cleaned, the numbers clamped, the schema within the subset.
#[derive(Debug, Clone)]
pub struct ValidRequest {
    pub label: Option<String>,
    pub instructions: String,
    pub input: serde_json::Value,
    pub schema: Option<serde_json::Value>,
    pub max_response_chars: u32,
    pub timeout_ms: u32,
}

/// The manifest's `agentBackground` block, normalised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentBackgroundDecl {
    /// Plain text (≤300 characters) or a whole `__MSG_key__` token, which
    /// the UI resolves.
    pub reason: String,
    pub purposes: Vec<String>,
    pub max_prompt_chars: Option<u32>,
    pub daily_calls: Option<u32>,
}

/// Longest `reason`, in characters.
pub const MAX_REASON_CHARS: usize = 300;
/// Most purposes a manifest may declare.
pub const MAX_PURPOSES: usize = 8;

impl AgentBackgroundDecl {
    /// Normalise the raw `agentBackground` value. Anything malformed means
    /// "not declared" (gate 2 fails with `not_available`).
    pub fn from_manifest(raw: &serde_json::Value) -> Option<Self> {
        let obj = raw.as_object()?;
        let reason = obj.get("reason")?.as_str()?.trim().to_string();
        let reason_ok = !reason.is_empty()
            && (is_msg_token(&reason) || reason.chars().count() <= MAX_REASON_CHARS);
        if !reason_ok {
            return None;
        }
        let list = obj.get("purposes")?.as_array()?;
        if list.is_empty() || list.len() > MAX_PURPOSES {
            return None;
        }
        let mut purposes: Vec<String> = Vec::with_capacity(list.len());
        for p in list {
            let p = p.as_str()?;
            if !is_valid_purpose(p) {
                return None;
            }
            if !purposes.iter().any(|q| q == p) {
                purposes.push(p.to_string());
            }
        }
        let positive = |key: &str| -> Result<Option<u32>, ()> {
            match obj.get(key) {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(v) => match v.as_u64() {
                    Some(n) if n >= 1 => Ok(Some(u32::try_from(n).unwrap_or(u32::MAX))),
                    _ => Err(()),
                },
            }
        };
        let max_prompt_chars = positive("maxPromptChars").ok()?;
        let daily_calls = positive("dailyCalls").ok()?;
        Some(Self {
            reason,
            purposes,
            max_prompt_chars,
            daily_calls,
        })
    }

    pub fn declares(&self, purpose: &str) -> bool {
        self.purposes.iter().any(|p| p == purpose)
    }
}

/// `^[a-z][a-z0-9_]{0,23}$`.
pub fn is_valid_purpose(p: &str) -> bool {
    let mut chars = p.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    p.len() <= 24
        && first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `s` is a whole `__MSG_key__` manifest token.
fn is_msg_token(s: &str) -> bool {
    s.len() > "__MSG___".len() && s.starts_with("__MSG_") && s.ends_with("__")
}

pub(crate) fn error(kind: ErrorKind, key: &str, params: &[(&str, &str)]) -> AppError {
    AppError::keyed(kind, format!("errors.agent_call.{key}"), params)
}

fn too_long(field: &str, size: usize, limit: usize) -> AppError {
    error(
        ErrorKind::InvalidInput,
        "field_too_long",
        &[
            ("field", field),
            ("size", &size.to_string()),
            ("limit", &limit.to_string()),
        ],
    )
}

fn invalid(field: &str) -> AppError {
    error(
        ErrorKind::InvalidInput,
        "field_invalid",
        &[("field", field)],
    )
}

/// Check a `dedupe_key`: 1–64 characters of `[A-Za-z0-9_.:-]`.
pub fn validate_dedupe_key(key: &str) -> Result<(), AppError> {
    let len = key.chars().count();
    if len > limits::MAX_DEDUPE_KEY_CHARS {
        return Err(too_long("dedupe_key", len, limits::MAX_DEDUPE_KEY_CHARS));
    }
    let ok = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(invalid("dedupe_key"))
    }
}

/// Bidi embedding, override and isolate characters.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

/// A label as plain text: control and bidi characters removed, spaces
/// collapsed, trimmed. Empty becomes `None`.
pub fn clean_label(label: &str) -> Option<String> {
    let cleaned: String = label
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

/// Characters of `input` as it is counted against its limit: a string's
/// own characters, anything else as compact JSON.
pub fn input_chars(input: &serde_json::Value) -> usize {
    match input {
        serde_json::Value::String(s) => s.chars().count(),
        other => other.to_string().chars().count(),
    }
}

/// Check sizes and the schema; clean the label; clamp the numbers.
pub fn validate(req: AgentCallRequest) -> Result<ValidRequest, AppError> {
    let label = match req.label.as_deref().and_then(clean_label) {
        Some(l) => {
            let n = l.chars().count();
            if n > limits::MAX_LABEL_CHARS {
                return Err(too_long("label", n, limits::MAX_LABEL_CHARS));
            }
            Some(l)
        }
        None => None,
    };
    let n = req.instructions.chars().count();
    if n > limits::MAX_INSTRUCTIONS_CHARS {
        return Err(too_long("instructions", n, limits::MAX_INSTRUCTIONS_CHARS));
    }
    let n = input_chars(&req.input);
    if n > limits::MAX_INPUT_CHARS {
        return Err(too_long("input", n, limits::MAX_INPUT_CHARS));
    }
    if let Some(s) = &req.schema {
        schema::check(s).map_err(|e| match e {
            schema::SchemaError::TooLarge(size) => {
                too_long("schema", size, limits::MAX_SCHEMA_BYTES)
            }
            schema::SchemaError::Unsupported(keyword) => error(
                ErrorKind::InvalidInput,
                "schema_unsupported",
                &[("keyword", &keyword)],
            ),
        })?;
    }
    Ok(ValidRequest {
        label,
        instructions: req.instructions,
        input: req.input,
        schema: req.schema,
        max_response_chars: limits::response_cap(req.max_response_chars),
        timeout_ms: limits::timeout_ms(req.timeout_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decl_normalises_a_valid_block() {
        let d = AgentBackgroundDecl::from_manifest(&json!({
            "reason": "__MSG_manifest.agent_background_reason__",
            "purposes": ["organize", "organize", "tag_2"],
            "maxPromptChars": 16000,
            "dailyCalls": 40
        }))
        .unwrap();
        assert_eq!(d.purposes, vec!["organize", "tag_2"]);
        assert_eq!(d.max_prompt_chars, Some(16000));
        assert_eq!(d.daily_calls, Some(40));
        assert!(d.declares("organize"));
        assert!(!d.declares("other"));
    }

    #[test]
    fn malformed_decl_means_not_declared() {
        let bad = [
            json!(null),
            json!("organize"),
            json!({ "purposes": ["organize"] }),
            json!({ "reason": "", "purposes": ["organize"] }),
            json!({ "reason": "x".repeat(301), "purposes": ["organize"] }),
            json!({ "reason": "Why", "purposes": [] }),
            json!({ "reason": "Why", "purposes": ["Organize"] }),
            json!({ "reason": "Why", "purposes": ["a-b"] }),
            json!({ "reason": "Why", "purposes": ["a".repeat(25)] }),
            json!({ "reason": "Why", "purposes": ["a","b","c","d","e","f","g","h","i"] }),
            json!({ "reason": "Why", "purposes": ["ok"], "dailyCalls": 0 }),
            json!({ "reason": "Why", "purposes": ["ok"], "dailyCalls": 2.5 }),
            json!({ "reason": "Why", "purposes": ["ok"], "maxPromptChars": "big" }),
        ];
        for raw in bad {
            assert!(AgentBackgroundDecl::from_manifest(&raw).is_none(), "{raw}");
        }
        // 300 characters is fine; a long token is resolved by the UI.
        assert!(AgentBackgroundDecl::from_manifest(
            &json!({ "reason": "é".repeat(300), "purposes": ["ok"] })
        )
        .is_some());
    }

    #[test]
    fn purposes_follow_the_pattern() {
        assert!(is_valid_purpose("organize"));
        assert!(is_valid_purpose("a"));
        assert!(is_valid_purpose(&format!("a{}", "0".repeat(23))));
        assert!(!is_valid_purpose(&format!("a{}", "0".repeat(24))));
        assert!(!is_valid_purpose("_a"));
        assert!(!is_valid_purpose("1a"));
        assert!(!is_valid_purpose(""));
    }

    #[test]
    fn when_defaults_to_idle_for_unknown_values() {
        let w: When = serde_json::from_value(json!("now")).unwrap();
        assert_eq!(w, When::Now);
        let w: When = serde_json::from_value(json!("whenever")).unwrap();
        assert_eq!(w, When::Idle);
        let args: ReserveArgs =
            serde_json::from_value(json!({ "purpose": "p", "dedupeKey": "k" })).unwrap();
        assert_eq!(args.dedupe_key.as_deref(), Some("k"));
        assert_eq!(args.when, None);
    }

    #[test]
    fn dedupe_keys() {
        assert!(validate_dedupe_key("organize:v1.2-x_y").is_ok());
        let e = validate_dedupe_key("has space").unwrap_err();
        assert_eq!(e.key, "errors.agent_call.field_invalid");
        assert!(validate_dedupe_key("").is_err());
        let e = validate_dedupe_key(&"k".repeat(65)).unwrap_err();
        assert_eq!(e.key, "errors.agent_call.field_too_long");
    }

    #[test]
    fn labels_are_plain_text() {
        assert_eq!(
            clean_label(" Organize\u{202E} 12\n\tnotes\u{0007} ").as_deref(),
            Some("Organize 12 notes")
        );
        assert_eq!(clean_label("\u{200F} \n"), None);
    }

    #[test]
    fn validate_checks_each_size() {
        let ok = AgentCallRequest {
            instructions: "Do it".into(),
            input: json!({ "a": 1 }),
            ..Default::default()
        };
        let v = validate(ok.clone()).unwrap();
        assert_eq!(v.max_response_chars, 8_000);
        assert_eq!(v.timeout_ms, 60_000);

        let mut r = ok.clone();
        r.label = Some("x".repeat(81));
        assert_eq!(validate(r).unwrap_err().params["field"], "label");

        let mut r = ok.clone();
        r.instructions = "é".repeat(4_001);
        let e = validate(r).unwrap_err();
        assert_eq!(e.params["field"], "instructions");
        assert_eq!(e.params["size"], "4001");
        assert!(matches!(e.kind, ErrorKind::InvalidInput));

        let mut r = ok.clone();
        r.input = json!("y".repeat(24_001));
        assert_eq!(validate(r).unwrap_err().params["field"], "input");

        let mut r = ok.clone();
        r.input = json!("y".repeat(24_000));
        assert!(validate(r).is_ok());

        let mut r = ok.clone();
        r.schema = Some(json!({ "type": "object", "patternProperties": {} }));
        let e = validate(r).unwrap_err();
        assert_eq!(e.key, "errors.agent_call.schema_unsupported");
        assert_eq!(e.params["keyword"], "patternProperties");

        let mut r = ok;
        r.max_response_chars = Some(100_000);
        r.timeout_ms = Some(1);
        let v = validate(r).unwrap();
        assert_eq!(v.max_response_chars, 32_000);
        assert_eq!(v.timeout_ms, 5_000);
    }

    #[test]
    fn request_accepts_camel_case_numbers() {
        let r: AgentCallRequest = serde_json::from_value(json!({
            "instructions": "i", "input": "x", "maxResponseChars": 12000, "timeoutMs": 90000
        }))
        .unwrap();
        assert_eq!(r.max_response_chars, Some(12000));
        assert_eq!(r.timeout_ms, Some(90000));
    }
}
