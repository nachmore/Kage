//! The reply schema subset, JSON extraction from a reply, and validation
//! (docs/design/EXTENSION_AGENT_CALLS.md §5.3).
//!
//! ACP has no structured-output mode, so a JSON reply is asked for in the
//! framing and then checked here. The host never repairs a reply and never
//! retries.

use serde_json::Value;

use super::limits::{MAX_SCHEMA_BYTES, MAX_SCHEMA_DEPTH, MAX_SCHEMA_NODES};

/// Why a schema was refused at send time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// Serialized size in bytes, over [`MAX_SCHEMA_BYTES`].
    TooLarge(usize),
    /// The keyword (or limit) Kage doesn't support.
    Unsupported(String),
}

const TYPES: &[&str] = &[
    "object", "array", "string", "number", "integer", "boolean", "null",
];

/// Check `schema` against the supported subset and its limits.
pub fn check(schema: &Value) -> Result<(), SchemaError> {
    let size = schema.to_string().len();
    if size > MAX_SCHEMA_BYTES {
        return Err(SchemaError::TooLarge(size));
    }
    let mut nodes = 0usize;
    check_node(schema, 1, &mut nodes)
}

fn unsupported(s: impl Into<String>) -> SchemaError {
    SchemaError::Unsupported(s.into())
}

fn check_node(node: &Value, depth: usize, nodes: &mut usize) -> Result<(), SchemaError> {
    let Some(obj) = node.as_object() else {
        return Err(unsupported("a schema that isn't an object"));
    };
    if depth > MAX_SCHEMA_DEPTH {
        return Err(unsupported(format!(
            "nesting deeper than {MAX_SCHEMA_DEPTH}"
        )));
    }
    *nodes += 1;
    if *nodes > MAX_SCHEMA_NODES {
        return Err(unsupported(format!("more than {MAX_SCHEMA_NODES} schemas")));
    }
    for (key, value) in obj {
        match key.as_str() {
            "type" => {
                let ok = match value {
                    Value::String(t) => TYPES.contains(&t.as_str()),
                    Value::Array(ts) => {
                        !ts.is_empty()
                            && ts
                                .iter()
                                .all(|t| t.as_str().is_some_and(|t| TYPES.contains(&t)))
                    }
                    _ => false,
                };
                if !ok {
                    return Err(unsupported("type"));
                }
            }
            "properties" => {
                let Some(props) = value.as_object() else {
                    return Err(unsupported("properties"));
                };
                for child in props.values() {
                    check_node(child, depth + 1, nodes)?;
                }
            }
            "required" => {
                let ok = value
                    .as_array()
                    .is_some_and(|r| r.iter().all(Value::is_string));
                if !ok {
                    return Err(unsupported("required"));
                }
            }
            "additionalProperties" => {
                if !value.is_boolean() {
                    return Err(unsupported("additionalProperties"));
                }
            }
            "items" => {
                if !value.is_object() {
                    return Err(unsupported("items"));
                }
                check_node(value, depth + 1, nodes)?;
            }
            "enum" => {
                let ok = value.as_array().is_some_and(|vs| {
                    vs.iter().all(|v| {
                        matches!(
                            v,
                            Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null
                        )
                    })
                });
                if !ok {
                    return Err(unsupported("enum"));
                }
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" => {
                if value.as_u64().is_none() {
                    return Err(unsupported(key.as_str()));
                }
            }
            "minimum" | "maximum" => {
                if !value.is_number() {
                    return Err(unsupported(key.as_str()));
                }
            }
            "description" | "title" => {
                if !value.is_string() {
                    return Err(unsupported(key.as_str()));
                }
            }
            other => return Err(unsupported(other)),
        }
    }
    Ok(())
}

/// Most `{` / `[` positions tried when the reply isn't JSON as a whole.
const MAX_CANDIDATES: usize = 32;

/// Pull the JSON value out of a reply:
/// 1. trim and remove one surrounding code fence;
/// 2. the whole text, if it parses;
/// 3. otherwise the first value that starts at a `{` or `[` (text around
///    it is ignored; of two values, the first wins).
pub fn extract_json(reply: &str) -> Option<Value> {
    let text = strip_fence(reply.trim());
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    text.char_indices()
        .filter(|(_, c)| matches!(c, '{' | '['))
        .take(MAX_CANDIDATES)
        .find_map(|(i, _)| {
            serde_json::Deserializer::from_str(&text[i..])
                .into_iter::<Value>()
                .next()
                .and_then(Result::ok)
        })
}

/// `text` without one surrounding ```` ``` ```` / ```` ```json ```` fence.
fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let Some(body) = rest.strip_suffix("```") else {
        return text;
    };
    // The info string runs to the end of the first line.
    match body.find('\n') {
        Some(nl) if body[..nl].trim().chars().all(|c| c.is_ascii_alphanumeric()) => {
            body[nl + 1..].trim()
        }
        _ => body.trim(),
    }
}

/// Validate `value` against a schema that passed [`check`]. `Err` carries
/// the path of the first problem, such as `$.items[3].actions[0].quote`.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    validate_at(schema, value, &mut String::from("$"))
}

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => match v {
            Value::Number(n) => {
                n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0)
            }
            _ => false,
        },
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}

/// `$.name` for a plain key, `$["odd key"]` otherwise.
fn push_key(path: &mut String, key: &str) {
    let plain = key
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        path.push('.');
        path.push_str(key);
    } else {
        path.push('[');
        path.push_str(&Value::String(key.to_string()).to_string());
        path.push(']');
    }
}

fn validate_at(schema: &Value, value: &Value, path: &mut String) -> Result<(), String> {
    let Some(s) = schema.as_object() else {
        return Ok(());
    };
    if let Some(t) = s.get("type") {
        let ok = match t {
            Value::String(t) => type_matches(t, value),
            Value::Array(ts) => ts
                .iter()
                .filter_map(Value::as_str)
                .any(|t| type_matches(t, value)),
            _ => true,
        };
        if !ok {
            return Err(path.clone());
        }
    }
    if let Some(options) = s.get("enum").and_then(Value::as_array) {
        if !options.iter().any(|o| json_eq(o, value)) {
            return Err(path.clone());
        }
    }
    match value {
        Value::String(text) => {
            let n = text.chars().count() as u64;
            if s.get("minLength")
                .and_then(Value::as_u64)
                .is_some_and(|m| n < m)
                || s.get("maxLength")
                    .and_then(Value::as_u64)
                    .is_some_and(|m| n > m)
            {
                return Err(path.clone());
            }
        }
        Value::Number(num) => {
            let x = num.as_f64().unwrap_or(0.0);
            if s.get("minimum")
                .and_then(Value::as_f64)
                .is_some_and(|m| x < m)
                || s.get("maximum")
                    .and_then(Value::as_f64)
                    .is_some_and(|m| x > m)
            {
                return Err(path.clone());
            }
        }
        Value::Array(items) => {
            let n = items.len() as u64;
            if s.get("minItems")
                .and_then(Value::as_u64)
                .is_some_and(|m| n < m)
                || s.get("maxItems")
                    .and_then(Value::as_u64)
                    .is_some_and(|m| n > m)
            {
                return Err(path.clone());
            }
            if let Some(item_schema) = s.get("items") {
                for (i, item) in items.iter().enumerate() {
                    let len = path.len();
                    path.push_str(&format!("[{i}]"));
                    validate_at(item_schema, item, path)?;
                    path.truncate(len);
                }
            }
        }
        Value::Object(map) => {
            if let Some(required) = s.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !map.contains_key(key) {
                        push_key(path, key);
                        return Err(path.clone());
                    }
                }
            }
            let props = s.get("properties").and_then(Value::as_object);
            let closed = s.get("additionalProperties") == Some(&Value::Bool(false));
            for (key, child) in map {
                let child_schema = props.and_then(|p| p.get(key));
                if child_schema.is_none() && closed {
                    push_key(path, key);
                    return Err(path.clone());
                }
                if let Some(cs) = child_schema {
                    let len = path.len();
                    push_key(path, key);
                    validate_at(cs, child, path)?;
                    path.truncate(len);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// JSON equality where `1` and `1.0` are the same number.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mind Map's organise schema (design §11.4), the largest real user.
    fn organize_schema() -> Value {
        json!({ "type": "object", "additionalProperties": false, "required": ["items"],
          "properties": { "items": { "type": "array", "maxItems": 15, "items": {
            "type": "object", "additionalProperties": false, "required": ["ref"],
            "properties": {
              "ref": { "type": "string", "maxLength": 4 },
              "skip": { "type": "boolean" },
              "summary": { "type": "string", "maxLength": 240 },
              "terms": { "type": "array", "maxItems": 5, "items": { "type": "string", "maxLength": 40 } },
              "entities": { "type": "array", "maxItems": 8, "items": {
                "type": "object", "additionalProperties": false, "required": ["name", "type"],
                "properties": {
                  "name": { "type": "string", "maxLength": 60 },
                  "type": { "type": "string", "enum": ["person", "team", "project", "topic", "org", "term"] } } } },
              "actions": { "type": "array", "maxItems": 5, "items": {
                "type": "object", "additionalProperties": false, "required": ["text", "quote"],
                "properties": {
                  "text": { "type": "string", "maxLength": 160 },
                  "quote": { "type": "string", "maxLength": 240 },
                  "owner": { "type": "string", "maxLength": 60 },
                  "due": { "type": "string", "maxLength": 10 } } } } } } } } })
    }

    #[test]
    fn every_supported_keyword_passes() {
        check(&organize_schema()).unwrap();
        check(&json!({
            "title": "t", "description": "d",
            "type": ["string", "null"], "minLength": 1, "maxLength": 3,
            "enum": ["a", 1, true, null]
        }))
        .unwrap();
        check(&json!({ "type": "number", "minimum": -1.5, "maximum": 2 })).unwrap();
        check(&json!({ "type": "array", "minItems": 0, "maxItems": 2, "items": {} })).unwrap();
        check(&json!({ "additionalProperties": true })).unwrap();
    }

    #[test]
    fn other_keywords_are_unsupported() {
        let cases = [
            (json!({ "$schema": "x" }), "$schema"),
            (json!({ "oneOf": [] }), "oneOf"),
            (json!({ "pattern": "a+" }), "pattern"),
            (json!({ "type": "date" }), "type"),
            (json!({ "type": [] }), "type"),
            (json!({ "items": [{}] }), "items"),
            (
                json!({ "additionalProperties": {} }),
                "additionalProperties",
            ),
            (json!({ "enum": [{}] }), "enum"),
            (json!({ "maxLength": -1 }), "maxLength"),
            (json!({ "required": [1] }), "required"),
            (
                json!({ "properties": { "a": { "format": "email" } } }),
                "format",
            ),
        ];
        for (schema, keyword) in cases {
            assert_eq!(
                check(&schema),
                Err(SchemaError::Unsupported(keyword.into())),
                "{schema}"
            );
        }
        assert!(matches!(
            check(&json!(true)),
            Err(SchemaError::Unsupported(_))
        ));
    }

    #[test]
    fn depth_node_and_size_caps() {
        // Depth 8 is fine, 9 isn't.
        let mut deep = json!({ "type": "string" });
        for _ in 0..7 {
            deep = json!({ "items": deep });
        }
        check(&deep).unwrap();
        let deeper = json!({ "items": deep });
        assert!(matches!(check(&deeper), Err(SchemaError::Unsupported(k)) if k.contains("deeper")));

        let props: serde_json::Map<String, Value> =
            (0..200).map(|i| (format!("p{i}"), json!({}))).collect();
        assert!(
            matches!(check(&json!({ "properties": props })), Err(SchemaError::Unsupported(k)) if k.contains("200"))
        );
        let props: serde_json::Map<String, Value> =
            (0..199).map(|i| (format!("p{i}"), json!({}))).collect();
        check(&json!({ "properties": props })).unwrap();

        let big = json!({ "description": "x".repeat(8 * 1024) });
        assert!(matches!(check(&big), Err(SchemaError::TooLarge(n)) if n > 8192));
    }

    #[test]
    fn extraction() {
        assert_eq!(extract_json(r#"{"a":1}"#), Some(json!({ "a": 1 })));
        assert_eq!(extract_json("  [1, 2]\n"), Some(json!([1, 2])));
        assert_eq!(
            extract_json("```json\n{\"a\": [1]}\n```"),
            Some(json!({ "a": [1] }))
        );
        assert_eq!(
            extract_json("```\n{\"a\": 2}\n```"),
            Some(json!({ "a": 2 }))
        );
        assert_eq!(
            extract_json("Here you go:\n{\"a\": 3}\nHope that helps."),
            Some(json!({ "a": 3 }))
        );
        // Two values: the first wins.
        assert_eq!(
            extract_json("{\"first\": 1} and {\"second\": 2}"),
            Some(json!({ "first": 1 }))
        );
        // A bracket in the prose before the value doesn't hide it.
        assert_eq!(
            extract_json("Notes [see below]: {\"a\": 4}"),
            Some(json!({ "a": 4 }))
        );
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json("{ broken"), None);
        assert_eq!(extract_json(""), None);
    }

    #[test]
    fn validation_paths() {
        let schema = organize_schema();
        let good = json!({ "items": [
            { "ref": "n1", "summary": "s", "terms": ["a"],
              "entities": [{ "name": "Dana", "type": "person" }],
              "actions": [{ "text": "t", "quote": "q", "due": "2026-11-10" }] },
            { "ref": "n2", "skip": true }
        ]});
        validate(&schema, &good).unwrap();

        let mut bad = good.clone();
        bad["items"][1]["entities"] = json!([{ "name": "X", "type": "alien" }]);
        assert_eq!(
            validate(&schema, &bad).unwrap_err(),
            "$.items[1].entities[0].type"
        );

        let mut bad = good.clone();
        bad["items"][0]["actions"][0]["quote"] = json!("q".repeat(241));
        assert_eq!(
            validate(&schema, &bad).unwrap_err(),
            "$.items[0].actions[0].quote"
        );

        let mut bad = good.clone();
        bad["items"][0]["kind"] = json!("note");
        assert_eq!(validate(&schema, &bad).unwrap_err(), "$.items[0].kind");

        let bad = json!({ "items": [{ "skip": true }] });
        assert_eq!(validate(&schema, &bad).unwrap_err(), "$.items[0].ref");

        assert_eq!(validate(&schema, &json!([])).unwrap_err(), "$");
        let many: Vec<Value> = (0..16).map(|i| json!({ "ref": format!("n{i}") })).collect();
        assert_eq!(
            validate(&schema, &json!({ "items": many })).unwrap_err(),
            "$.items"
        );
    }

    #[test]
    fn validation_of_scalars() {
        let int = json!({ "type": "integer", "minimum": 1, "maximum": 3 });
        validate(&int, &json!(2)).unwrap();
        validate(&int, &json!(2.0)).unwrap();
        assert!(validate(&int, &json!(2.5)).is_err());
        assert!(validate(&int, &json!(0)).is_err());
        assert!(validate(&int, &json!(4)).is_err());
        let s = json!({ "type": "string", "minLength": 2, "maxLength": 3 });
        validate(&s, &json!("éé")).unwrap();
        assert!(validate(&s, &json!("é")).is_err());
        assert!(validate(&s, &json!("éééé")).is_err());
        let e = json!({ "enum": [1, "a", null] });
        validate(&e, &json!(1.0)).unwrap();
        validate(&e, &json!(null)).unwrap();
        assert!(validate(&e, &json!("b")).is_err());
        let open = json!({ "type": "object", "properties": { "a": { "type": "string" } } });
        validate(&open, &json!({ "a": "x", "extra": 1 })).unwrap();
        let odd = json!({ "type": "object", "properties": { "a b": { "type": "string" } } });
        assert_eq!(
            validate(&odd, &json!({ "a b": 1 })).unwrap_err(),
            r#"$["a b"]"#
        );
        let either = json!({ "type": ["string", "null"] });
        validate(&either, &json!(null)).unwrap();
        assert!(validate(&either, &json!(1)).is_err());
    }
}
