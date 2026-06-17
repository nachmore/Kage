//! Kiro CLI slash-result formatters.
//!
//! Result shapes captured from kiro-cli 2.6.2 via `scripts/probe_slash.py`.
//! Only commands whose plain `message` discards useful structured data are
//! formatted here; everything else falls through (returns `None`).

use super::{fmt_tokens, usage_bar, SlashFormatter};
use serde_json::Value;

pub struct KiroFormatter;

impl SlashFormatter for KiroFormatter {
    fn format(&self, command: &str, result: &Value) -> Option<String> {
        match command {
            "context" => format_context(result),
            _ => None,
        }
    }
}

/// Render `/context` from `data.breakdown`. Real shape:
/// ```json
/// { "success": true, "message": "Context breakdown - 3% used",
///   "data": {
///     "model": "auto",
///     "contextUsagePercentage": 2.77,
///     "breakdown": {
///       "contextFiles": { "tokens": 7754, "percent": 0.77,
///                         "items": [ { "name": "...", "tokens": 2145, "matched": true } ] },
///       "tools":        { "tokens": 19931, "percent": 1.99 },
///       "kiroResponses":{ "tokens": 0, "percent": 0.0 },
///       "yourPrompts":  { "tokens": 58, "percent": 0.0 },
///       "sessionFiles": { "tokens": 0, "percent": 0.0 }
///     } } }
/// ```
fn format_context(result: &Value) -> Option<String> {
    let data = result.get("data")?;
    let breakdown = data.get("breakdown")?;
    // Bail to the plain message if the structure isn't what we expect.
    if !breakdown.is_object() {
        return None;
    }

    let pct = data
        .get("contextUsagePercentage")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let model = data.get("model").and_then(Value::as_str).unwrap_or("");

    let mut out = String::new();
    out.push_str("### Context usage\n\n");
    if !model.is_empty() {
        out.push_str(&format!("**Model:** `{model}`\n\n"));
    }
    // Usage bar + headline percentage.
    out.push_str(&format!("`{}` **{:.1}%**\n\n", usage_bar(pct, 24), pct));

    // Category table, in a stable, meaningful order. Each entry: (key, label).
    let categories = [
        ("contextFiles", "Context files"),
        ("tools", "Tools"),
        ("kiroResponses", "Agent responses"),
        ("yourPrompts", "Your prompts"),
        ("sessionFiles", "Session files"),
    ];

    out.push_str("| Category | Tokens | % |\n|---|--:|--:|\n");
    let mut any_row = false;
    for (key, label) in categories {
        if let Some(cat) = breakdown.get(key) {
            let tokens = cat.get("tokens").and_then(Value::as_f64).unwrap_or(0.0);
            let cpct = cat.get("percent").and_then(Value::as_f64).unwrap_or(0.0);
            out.push_str(&format!(
                "| {} | {} | {:.1}% |\n",
                label,
                fmt_tokens(tokens),
                cpct
            ));
            any_row = true;
        }
    }
    // If none of the known categories were present, the shape isn't what we
    // think it is — fall back rather than show an empty table.
    if !any_row {
        return None;
    }

    // Per-file detail for context files, when present and non-empty. Only the
    // files the agent actually matched into context are interesting.
    if let Some(items) = breakdown
        .get("contextFiles")
        .and_then(|c| c.get("items"))
        .and_then(Value::as_array)
    {
        let matched: Vec<&Value> = items
            .iter()
            .filter(|it| {
                it.get("matched").and_then(Value::as_bool).unwrap_or(false)
                    && it.get("tokens").and_then(Value::as_f64).unwrap_or(0.0) > 0.0
            })
            .collect();
        if !matched.is_empty() {
            out.push_str("\n**Context files**\n\n");
            for it in matched {
                let name = it.get("name").and_then(Value::as_str).unwrap_or("?");
                let tokens = it.get("tokens").and_then(Value::as_f64).unwrap_or(0.0);
                out.push_str(&format!("- `{}` — {}\n", name, fmt_tokens(tokens)));
            }
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_presets::AgentKind;
    use crate::slash_format::{format_slash_result, SlashFormatter};

    // Trimmed copy of a real kiro-cli /context reply.
    fn context_reply() -> Value {
        serde_json::json!({
            "success": true,
            "message": "Context breakdown - 3% used",
            "data": {
                "model": "auto",
                "contextUsagePercentage": 2.7743,
                "breakdown": {
                    "contextFiles": {
                        "tokens": 7754, "percent": 0.775,
                        "items": [
                            { "name": "AGENTS.md", "tokens": 0, "matched": false },
                            { "name": "README.md", "tokens": 2145, "matched": true },
                            { "name": "tech.md", "tokens": 1486, "matched": true }
                        ]
                    },
                    "tools":         { "tokens": 19931, "percent": 1.993 },
                    "kiroResponses": { "tokens": 0, "percent": 0.0 },
                    "yourPrompts":   { "tokens": 58, "percent": 0.0058 },
                    "sessionFiles":  { "tokens": 0, "percent": 0.0 }
                }
            }
        })
    }

    #[test]
    fn formats_context_with_bar_table_and_files() {
        let md = KiroFormatter.format("context", &context_reply()).unwrap();
        assert!(md.contains("### Context usage"));
        assert!(md.contains("`auto`"));
        assert!(md.contains("2.8%")); // headline percent, rounded
        assert!(md.contains("| Category | Tokens | % |"));
        assert!(md.contains("Context files"));
        assert!(md.contains("19.9k")); // tools tokens, compacted
                                       // Only matched, non-zero files listed.
        assert!(md.contains("`README.md`"));
        assert!(md.contains("`tech.md`"));
        assert!(!md.contains("AGENTS.md"));
    }

    #[test]
    fn unknown_command_falls_through() {
        let v = serde_json::json!({ "message": "ok", "data": { "models": [] } });
        assert_eq!(KiroFormatter.format("model", &v), None);
    }

    #[test]
    fn context_without_breakdown_falls_through() {
        let v = serde_json::json!({ "message": "Context breakdown - 3% used", "data": {} });
        assert_eq!(KiroFormatter.format("context", &v), None);
        // And the top-level entry point agrees.
        assert_eq!(format_slash_result(AgentKind::Kiro, "context", &v), None);
    }

    #[test]
    fn context_with_empty_breakdown_object_falls_through() {
        // breakdown present but none of the known categories — don't render
        // an empty table.
        let v = serde_json::json!({
            "message": "x",
            "data": { "breakdown": { "somethingElse": { "tokens": 1 } } }
        });
        assert_eq!(KiroFormatter.format("context", &v), None);
    }
}
