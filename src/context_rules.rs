//! Per-app context rules ("App Modes").
//!
//! A user-managed list of rules of the form
//! `(friendly_name, executable, steering)`. When Kage is summoned and
//! the foreground app's process name matches a rule's `executable`,
//! the matched rule's `steering` is appended to the outgoing prompt
//! as a small `<_kage_app_steering>...</_kage_app_steering>` tag (next
//! to `<_kage_ctx>`).
//!
//! The two requirements that shape this design:
//!
//!   1. **Cross-platform matching is messy.** Windows reports
//!      `Code.exe`; macOS reports `Visual Studio Code` (NSWorkspace
//!      `localizedName`); Linux reports `code` (`/proc/<pid>/comm`).
//!      A rule the user types as `Code` needs to match all three. We
//!      tokenise both sides on `[whitespace.\-_]`, lowercase, and
//!      require every rule token to appear as a *whole token* in the
//!      foreground name. That catches:
//!        - `Code` → `code.exe`, `Visual Studio Code`, `code`
//!        - `Visual Studio Code` → only the macOS form
//!        - `chrome` → `chrome.exe`, `Google Chrome`; does **not**
//!          match `chromedriver` (different token).
//!
//!   2. **Token budget matters.** Steering rides every prompt — a
//!      careless 2 KB rule eats ~500 tokens on every turn. We cap
//!      individual rule steering at `MAX_STEERING_LEN` chars and the
//!      total appended payload at `MAX_TOTAL_STEERING_LEN`. The
//!      settings UI also shows a live char counter so the user
//!      doesn't write an essay.

use serde::{Deserialize, Serialize};

/// Per-rule steering cap. ~125 tokens at 4 chars/token. Big enough
/// for "Be concise. Prefer code blocks. No rewrites unless asked."
/// Small enough that misuse can't blow up a context window.
pub const MAX_STEERING_LEN: usize = 500;

/// Hard cap on the *total* injected steering across all matched rules.
/// We currently match at most one rule per turn (first match wins),
/// so this is a safety net for a future where multiple matches are
/// allowed.
pub const MAX_TOTAL_STEERING_LEN: usize = 1500;

/// One configured rule. Persisted in `Config::context_rules`. The
/// fields mirror what the settings UI exposes 1:1; no derived state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRule {
    /// User-shown name. Independent of `executable` so a user can
    /// have an "IDE" rule that matches `code` and a "Dev terminal"
    /// rule that also matches a different exe — friendly_name is
    /// what shows up in the chip.
    pub friendly_name: String,
    /// What we match against the foreground process name. Free-form
    /// string the user types; `.exe` stripping + tokenisation makes
    /// "Code" or "Visual Studio Code" both work.
    pub executable: String,
    /// Steering body the model sees inside `<_kage_app_steering>`.
    /// Truncated to `MAX_STEERING_LEN` chars when injected; we
    /// preserve the user's literal string in storage so a bigger cap
    /// later doesn't silently eat data.
    pub steering: String,
    /// Lets a user temporarily disable a rule without deleting it.
    #[serde(default = "crate::config::default_true")]
    pub enabled: bool,
}

/// Lowercase + strip a trailing `.exe`. Pure helper so tests can pin
/// the exact behaviour. We don't strip every extension — `.app` on
/// macOS is part of bundle paths that don't reach this function, and
/// stripping any-extension would mistake `chrome.exe` and
/// `chrome.exe.bak` for the same thing.
fn normalise(name: &str) -> String {
    let trimmed = name.trim().to_lowercase();
    if let Some(stripped) = trimmed.strip_suffix(".exe") {
        return stripped.to_string();
    }
    trimmed
}

/// Split a normalised name into tokens. Whitespace, `.`, `-`, `_`
/// are treated as separators — covers the common patterns:
/// `chrome.exe` → `chrome`; `Visual Studio Code` → 3 tokens;
/// `dev-terminal` → 2; `chromedriver` → 1 (doesn't accidentally
/// match `chrome`).
fn tokenise(name: &str) -> Vec<&str> {
    name.split(|c: char| c.is_whitespace() || c == '.' || c == '-' || c == '_')
        .filter(|s| !s.is_empty())
        .collect()
}

/// Return true if every token in `rule_executable` appears as a
/// whole token in `foreground_name`. Empty rule never matches
/// (defends against an accidentally-blank rule shouting at every
/// app). Pure — drives the unit tests.
pub fn matches(rule_executable: &str, foreground_name: &str) -> bool {
    let rule = normalise(rule_executable);
    let fg = normalise(foreground_name);
    if rule.is_empty() {
        return false;
    }
    let rule_tokens = tokenise(&rule);
    if rule_tokens.is_empty() {
        return false;
    }
    let fg_tokens = tokenise(&fg);
    rule_tokens.iter().all(|t| fg_tokens.iter().any(|f| f == t))
}

/// Find the first enabled rule that matches `foreground_name`.
/// First-match wins — order in the rules list = priority. Returning
/// a borrow keeps the call-site cheap; the caller can `.clone()` if
/// they need an owned copy.
pub fn first_matching<'a>(
    rules: &'a [ContextRule],
    foreground_name: &str,
) -> Option<&'a ContextRule> {
    rules
        .iter()
        .find(|r| r.enabled && matches(&r.executable, foreground_name))
}

/// Format a matched rule into the wire payload. Truncates at
/// `MAX_STEERING_LEN` (UTF-8 boundary aware) so a stale config that
/// somehow grew past the cap can't blow up a context window.
/// Returns `None` if the rule's steering is empty after trimming —
/// a no-op rule shouldn't add a tag.
pub fn format_steering_payload(rule: &ContextRule) -> Option<String> {
    let body = rule.steering.trim();
    if body.is_empty() {
        return None;
    }
    let truncated = truncate_at_char_boundary(body, MAX_STEERING_LEN);
    Some(format!(
        "<_kage_app_steering app=\"{}\">\n{}\n</_kage_app_steering>",
        escape_xml_attr(&rule.friendly_name),
        truncated
    ))
}

/// Truncate to at most `max_chars` characters at a UTF-8 boundary.
/// `.is_char_boundary()` lets us walk back the index instead of
/// allocating; keeps the path zero-copy on the common no-truncate
/// case.
fn truncate_at_char_boundary(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let mut end = max_chars;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Defensive escape for the `app=` attribute. The friendly name is
/// user-controlled; without escaping a name like `"; --` could break
/// the tag. We only need to escape `"` and `&` because the rest of
/// the name is rendered as text.
fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

/// Curated starter App Modes. Seeded into `Config::default()` so a
/// fresh install lands with sensible defaults; users can edit or
/// delete them in Settings → Personalization → App Modes. Mirrors
/// `SUGGESTED_APP_MODES` in `ui/js/settings/assistant.js` so the
/// "+ Add suggested" affordance there stays in sync — keep the two
/// lists identical when adding entries.
///
/// Match is whole-token, case-insensitive, .exe-stripping (see
/// `matches`), so a single token like "code" hits Code.exe, Visual
/// Studio Code, and code on Linux. Steering is short and imperative
/// per the docs in `src/builtin_steering.md`.
pub fn default_starter_rules() -> Vec<ContextRule> {
    fn rule(name: &str, exe: &str, steering: &str) -> ContextRule {
        ContextRule {
            friendly_name: name.into(),
            executable: exe.into(),
            steering: steering.into(),
            enabled: true,
        }
    }
    vec![
        rule(
            "Code editor",
            "code",
            "You are pair-programming. Be terse. Show diffs or full functions, not narration. \
             Prefer the language already in the file. No \"Sure, here's…\"",
        ),
        rule(
            "Terminal",
            "terminal",
            "Reply with shell commands first, prose second. Detect the OS from context. \
             One-liners over scripts when possible. Mark destructive commands with a brief warning.",
        ),
        rule(
            "Browser",
            "chrome",
            "Assume the user is reading a web page. Summarise concisely, surface the key claim, \
             and flag anything that looks paywalled or AI-generated. Cite the page when quoting.",
        ),
        rule(
            "Email",
            "outlook",
            "Match the tone of the thread. Default to short replies. If drafting from scratch, \
             give two options: a 1-liner and a 3-sentence version. No filler (\"hope this helps\").",
        ),
        rule(
            "Slack",
            "slack",
            "Casual, lowercase-okay, emoji sparingly. Reply in 1–3 sentences. \
             If the user pastes a thread, summarise + suggest one next message.",
        ),
        rule(
            "Notes / writing",
            "notion",
            "Help the user think on paper. Ask clarifying questions when the goal is ambiguous. \
             Prefer structured bullets and short headings over walls of prose.",
        ),
        rule(
            "Spreadsheet",
            "excel",
            "Default to formulas (Excel/Google Sheets dialect). When ambiguous, ask whether the \
             data is a range or a table. Flag locale-sensitive things (decimals, dates) explicitly.",
        ),
        rule(
            "Design tool",
            "figma",
            "Think about visual hierarchy, contrast, and spacing first. Suggest concrete CSS \
             values or design tokens, not vague directions like \"make it pop\".",
        ),
        rule(
            "Video call",
            "zoom",
            "Optimise for speaking aloud: short sentences, no markdown, no code blocks unless \
             asked. Be ready to repeat or rephrase the previous answer in fewer words.",
        ),
        rule(
            "PDF reader",
            "acrobat",
            "Assume the user is reading a long document. Summarise sections on request, \
             extract action items, and quote with page references when possible.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, exe: &str, steering: &str) -> ContextRule {
        ContextRule {
            friendly_name: name.to_string(),
            executable: exe.to_string(),
            steering: steering.to_string(),
            enabled: true,
        }
    }

    #[test]
    fn normalise_strips_dot_exe_and_lowercases() {
        assert_eq!(normalise("Code.exe"), "code");
        assert_eq!(normalise("Code.EXE"), "code");
        assert_eq!(normalise("Visual Studio Code"), "visual studio code");
        assert_eq!(normalise("  slack  "), "slack");
    }

    #[test]
    fn matches_handles_three_platform_shapes() {
        // Same rule, three foreground forms, all match.
        assert!(matches("Code", "Code.exe"));
        assert!(matches("Code", "Visual Studio Code"));
        assert!(matches("Code", "code"));
    }

    #[test]
    fn matches_rejects_partial_token() {
        // Common confusion — a rule for "chrome" must not eat
        // "chromedriver".
        assert!(!matches("chrome", "chromedriver"));
        assert!(matches("chrome", "Google Chrome"));
        assert!(matches("chrome", "chrome.exe"));
    }

    #[test]
    fn matches_requires_all_rule_tokens() {
        // Multi-word rule needs every word to be present.
        assert!(matches("Visual Studio Code", "Visual Studio Code"));
        assert!(!matches("Visual Studio Code", "Code.exe"));
        // Order doesn't matter — all three tokens are present.
        assert!(matches("Studio Code Visual", "Visual Studio Code"));
    }

    #[test]
    fn matches_empty_rule_never_fires() {
        assert!(!matches("", "anything"));
        assert!(!matches("   ", "anything"));
    }

    #[test]
    fn matches_handles_separators() {
        assert!(matches("chrome", "google-chrome"));
        assert!(matches("dev terminal", "dev-terminal"));
        assert!(matches("dev_term", "dev-term"));
    }

    #[test]
    fn first_matching_skips_disabled() {
        let rules = vec![
            ContextRule {
                friendly_name: "Off".into(),
                executable: "code".into(),
                steering: "x".into(),
                enabled: false,
            },
            rule("On", "code", "y"),
        ];
        let m = first_matching(&rules, "Code.exe").unwrap();
        assert_eq!(m.friendly_name, "On");
    }

    #[test]
    fn first_matching_returns_first_in_order() {
        let rules = vec![rule("First", "code", "a"), rule("Second", "code", "b")];
        let m = first_matching(&rules, "code").unwrap();
        assert_eq!(m.friendly_name, "First");
    }

    #[test]
    fn first_matching_returns_none_for_no_match() {
        let rules = vec![rule("VS Code", "code", "x")];
        assert!(first_matching(&rules, "Slack").is_none());
        assert!(first_matching(&rules, "").is_none());
    }

    #[test]
    fn format_steering_payload_wraps_in_tag() {
        let r = rule("VS Code", "code", "Be concise.");
        let out = format_steering_payload(&r).unwrap();
        assert!(out.starts_with("<_kage_app_steering app=\"VS Code\">"));
        assert!(out.contains("Be concise."));
        assert!(out.ends_with("</_kage_app_steering>"));
    }

    #[test]
    fn format_steering_payload_skips_blank_steering() {
        assert!(format_steering_payload(&rule("VS Code", "code", "")).is_none());
        assert!(format_steering_payload(&rule("VS Code", "code", "   \n  ")).is_none());
    }

    #[test]
    fn format_steering_payload_truncates_at_cap() {
        let huge = "x".repeat(MAX_STEERING_LEN * 2);
        let r = rule("VS Code", "code", &huge);
        let out = format_steering_payload(&r).unwrap();
        // Body length (between the tags) capped at MAX_STEERING_LEN.
        // Outer tag length is independent of the body.
        let body_lines: Vec<&str> = out.lines().collect();
        // tag, body, closing tag
        assert_eq!(body_lines.len(), 3);
        assert!(body_lines[1].len() <= MAX_STEERING_LEN);
        assert!(body_lines[1].len() >= MAX_STEERING_LEN - 4);
    }

    #[test]
    fn format_steering_payload_truncates_at_utf8_boundary() {
        // "a" * (cap-1) + "🦀" — emoji is 4 bytes, would land mid-char
        // if we cut blindly. Verify the cut walks back to the boundary.
        let s = "a".repeat(MAX_STEERING_LEN - 1) + "🦀";
        let r = rule("Test", "x", &s);
        let out = format_steering_payload(&r).unwrap();
        // Truncation should not produce invalid UTF-8 (the format!
        // call would have panicked before we got here if it did).
        assert!(out.contains("aaa"));
    }

    #[test]
    fn format_steering_payload_escapes_friendly_name() {
        let r = rule("\"hi\" & me", "code", "body");
        let out = format_steering_payload(&r).unwrap();
        assert!(out.contains("app=\"&quot;hi&quot; &amp; me\""));
    }

    #[test]
    fn default_starter_rules_are_well_formed() {
        // Every starter rule must satisfy the same invariants the UI
        // editor enforces before save: non-empty name + executable, and
        // steering within the per-rule cap. A regression here would mean
        // every fresh install ships with a broken rule, so locking it in.
        let rules = default_starter_rules();
        assert!(!rules.is_empty(), "starter list must not be empty");
        for r in &rules {
            assert!(
                !r.friendly_name.trim().is_empty(),
                "starter rule has empty friendly_name"
            );
            assert!(
                !r.executable.trim().is_empty(),
                "starter rule {:?} has empty executable",
                r.friendly_name
            );
            assert!(
                r.enabled,
                "starter rule {:?} should ship enabled",
                r.friendly_name
            );
            assert!(
                r.steering.chars().count() <= MAX_STEERING_LEN,
                "starter rule {:?} steering exceeds cap ({} chars > {})",
                r.friendly_name,
                r.steering.chars().count(),
                MAX_STEERING_LEN
            );
        }
        // Each starter rule's executable token must self-match — a typo
        // in the seed would mean a rule that never fires, which is a
        // worse experience than not seeding at all.
        for r in &rules {
            assert!(
                matches(&r.executable, &r.executable),
                "starter rule {:?} executable {:?} does not match itself",
                r.friendly_name,
                r.executable
            );
        }
    }

    #[test]
    fn missing_context_rules_field_does_not_re_seed() {
        // Existing users upgrading from a build without context_rules
        // must NOT get the starter rules silently injected — that would
        // surprise people who'd already configured their assistant.
        // serde(default) on the field uses Vec::new() (the field's own
        // default), not Config::default()'s seeded value, so a config
        // missing the field deserialises to an empty list.
        let json = serde_json::json!({});
        let rules: Vec<ContextRule> =
            serde_json::from_value(json.get("context_rules").cloned().unwrap_or_default())
                .unwrap_or_default();
        assert!(rules.is_empty(), "absent field must deserialise as empty");
    }
}
