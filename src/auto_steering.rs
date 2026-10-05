//! Auto-steering document generation.
//!
//! Periodically extracts user preferences and facts from conversation history
//! and writes them to the auto-steering markdown file. This file is then
//! injected into new sessions as a steering message, giving the assistant
//! personalized context about the user.
//!
//! Triggers:
//! - Every 5 user messages, but no more than once per hour
//! - On application quit (bypasses the hourly cooldown)

/// Prefix used to mark steering messages that should be hidden in the UI.
pub const STEERING_MSG_PREFIX: &str = "[KAGE_STEERING_IGNORE]";

/// Built-in steering document embedded at compile time.
pub const BUILTIN_STEERING: &str = include_str!("builtin_steering.md");

// The extraction uses the ACP connection itself: we send a special prompt
// asking the model to analyze recent conversations and produce a structured
// preference document.

use crate::acp_client::AcpClient;
use crate::config::Config;
use crate::lock_ext::LockExt;
use anyhow::{Context, Result};
use log::{error, info, warn};
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Number of user messages between auto-steering updates
const UPDATE_INTERVAL_MESSAGES: u32 = 5;

/// Minimum time between periodic updates (1 hour). On-exit updates bypass this.
const MIN_UPDATE_INTERVAL_SECS: u64 = 3600;

/// Global counter for user messages since last steering update
static MESSAGE_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Total messages sent since the last steering generation (periodic or on-quit).
/// Used to skip on-quit generation when there's nothing new to analyze.
static MESSAGES_SINCE_GENERATION: AtomicU32 = AtomicU32::new(0);

/// Timestamp of the last periodic steering generation.
/// Initialized to None so the first eligible trigger is always allowed through.
static LAST_GENERATION: std::sync::LazyLock<Mutex<Option<Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// True if at least one user message has been sent since the last generation.
/// Lets shutdown short-circuit the cancel-and-wait dance when there's nothing
/// to summarize anyway.
pub fn has_pending_messages() -> bool {
    MESSAGES_SINCE_GENERATION.load(Ordering::Relaxed) > 0
}

/// Increment the message counter and return true if it's time to update.
/// Requires both the message count threshold AND the cooldown to have elapsed.
pub fn tick_message_counter() -> bool {
    MESSAGES_SINCE_GENERATION.fetch_add(1, Ordering::Relaxed);
    let count = MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    if count >= UPDATE_INTERVAL_MESSAGES {
        MESSAGE_COUNTER.store(0, Ordering::Relaxed);
        // Check the hourly cooldown
        if let Ok(last) = LAST_GENERATION.lock() {
            let elapsed = last.map(|t| t.elapsed().as_secs()).unwrap_or(u64::MAX);
            if elapsed >= MIN_UPDATE_INTERVAL_SECS {
                return true;
            }
            info!(
                "Auto-steering: message threshold reached but cooldown not elapsed ({}s remaining)",
                MIN_UPDATE_INTERVAL_SECS.saturating_sub(elapsed)
            );
        }
    }
    false
}

/// Record that a generation just completed (periodic or on-quit).
fn mark_generation() {
    MESSAGES_SINCE_GENERATION.store(0, Ordering::Relaxed);
    if let Ok(mut last) = LAST_GENERATION.lock() {
        *last = Some(Instant::now());
    }
}

/// Hard ceiling on the generated document body, in bytes. The doc is
/// injected into EVERY new session, so each byte here is paid for on every
/// session start — and an uncapped merge grows monotonically (one user's
/// doc reached 18KB of task trivia). The prompt asks for ~1500; this is
/// the deterministic backstop enforced by `compact_steering_doc`.
pub const MAX_DOC_CHARS: usize = 2500;

/// Max bullet (or prose) lines kept under any one heading.
const MAX_LINES_PER_SECTION: usize = 5;

/// Max bytes per line; longer lines are cut at a word boundary.
const MAX_LINE_CHARS: usize = 240;

/// Per-turn caps on the conversation excerpt fed to the extractor. User
/// turns carry the preference signal; assistant replies are mostly task
/// content, so they get a short leash. Bounds input tokens per pass.
const MAX_USER_TURN_CHARS: usize = 1500;
const MAX_ASSISTANT_TURN_CHARS: usize = 400;

/// The prompt sent to the LLM to extract user preferences from conversation history.
const EXTRACTION_PROMPT: &str = r#"<role>
You are a preference extraction assistant for Kage, a desktop AI tool.
</role>

<context>
The user has opted in to "Auto-Steering" in their settings because they want Kage to remember their preferences across sessions. This document will be shown to the user and they can edit or delete it at any time. This is a user-requested personalization feature.

The document is injected at the start of EVERY session, so it must be short. Every word costs tokens on every session.
</context>

<instructions>
Produce a terse markdown profile of DURABLE facts about the user: who they are, how they want Kage to respond, and what they generally work on.

Keep:
- Identity and role (name, pronouns, job, team), one line each.
- Stable response preferences (tone, length, format, things to avoid).
- Broad domains of expertise, named in a few words ("PKI / X.509", "Rust", "AWS pricing").
- Explicit standing instructions for Kage.

Drop, never record:
- Task content: facts, figures, limits, quotas, API names, command output, document titles, URLs, specific projects or tickets.
- Anything learned while answering a question rather than about the user.
- One-off requests and anything unlikely to matter next week.
- Examples, quotes, and explanations of why.

Format, strictly:
- Sections in this order, omitting any that are empty: `## About the User`, `## Communication Preferences`, `## Interests & Expertise`, `## Kage Behavior`.
- At most 4 bullets per section. Each bullet is one fragment of at most 20 words. No sub-bullets.
- Whole document under 1500 characters. Shorter is better.

Respond with only the markdown document. No preamble, no explanation.

If you cannot produce this document, respond with exactly "STEERING_DECLINED" on the first line and nothing else.
</instructions>"#;

/// Read recent conversation turns from the current session's JSONL file.
/// Returns labeled turns (both user and assistant) for full context.
fn read_recent_conversation(session_id: &str, max_turns: usize) -> Result<Vec<String>> {
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader};

    let home = dirs::home_dir().context("Failed to get home directory")?;
    let jsonl_path = crate::agent_presets::default_sessions_dir()
        .unwrap_or_else(|| home.join(".kiro").join("sessions").join("cli"))
        .join(format!("{}.jsonl", session_id));

    if !jsonl_path.exists() {
        return Ok(vec![]);
    }

    let file = fs::File::open(&jsonl_path).context("Failed to open session JSONL")?;
    let reader = BufReader::new(file);

    // Ring buffer: keep only the most recent max_turns entries as we stream
    let mut turns = VecDeque::with_capacity(max_turns + 1);

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let val: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let kind = val.get("kind").and_then(|k| k.as_str()).unwrap_or("");

        let role = match kind {
            "Prompt" => "User",
            "AssistantMessage" => "Assistant",
            _ => continue,
        };

        let data = match val.get("data") {
            Some(d) => d,
            None => continue,
        };

        let content_arr = match data.get("content").and_then(|c| c.as_array()) {
            Some(arr) => arr,
            None => continue,
        };

        // Extract text content from this turn
        let mut text_parts = Vec::new();
        for item in content_arr {
            let item_kind = item.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            if item_kind == "text" {
                if let Some(text) = item.get("data").and_then(|d| d.as_str()) {
                    let text = text.trim();
                    // Skip steering messages and extraction prompts
                    if !text.is_empty() && !text.starts_with("[KAGE_STEERING_IGNORE]") {
                        text_parts.push(text.to_string());
                    }
                }
            }
        }

        if !text_parts.is_empty() {
            if turns.len() == max_turns {
                turns.pop_front();
            }
            let cap = if role == "User" {
                MAX_USER_TURN_CHARS
            } else {
                MAX_ASSISTANT_TURN_CHARS
            };
            let joined = text_parts.join("\n");
            turns.push_back(format!("{}: {}", role, truncate_at_word(&joined, cap)));
        }
    }

    Ok(turns.into_iter().collect())
}

/// Generate the auto-steering document by sending conversation excerpts
/// to the LLM. The caller passes the session id to analyse — typically
/// the one the user just sent a message on (post-prompt epilogue) or
/// `window_sessions["main"]` (quit-time hook).
pub fn generate_steering_document(client: &AcpClient, session_id: &str) -> Result<()> {
    info!(
        "Starting auto-steering document generation for session {}",
        session_id
    );

    // Read recent conversation turns (last 50 turns = ~25 exchanges)
    let turns = read_recent_conversation(session_id, 50)?;

    if turns.len() < 2 {
        info!(
            "Too few conversation turns ({}) for meaningful extraction — skipping",
            turns.len()
        );
        return Ok(());
    }

    // Build the extraction prompt with conversation excerpts
    let excerpts = turns.join("\n\n");

    let full_prompt = format!(
        "{}\n\n---\n\nConversation to analyze:\n\n{}",
        EXTRACTION_PROMPT, excerpts
    );

    // Read existing steering content to include for incremental updates
    // Strip the HTML header comment so the LLM doesn't echo it back
    let existing_content = Config::get_auto_steering_path()
        .ok()
        .and_then(|p| fs::read_to_string(&p).ok())
        .unwrap_or_default();
    let existing_body = strip_header_comment(&existing_content);

    let prompt_with_existing = if existing_body.trim().is_empty() || !existing_body.contains("## ")
    {
        full_prompt
    } else {
        format!(
            "{}\n\n---\n\n<existing_preferences>\nThis is the current document. Rewrite it from scratch rather than appending: keep identity facts (name, role) and still-valid preferences, fold in what the new conversation shows, merge near-duplicates into one bullet, generalize specifics into broad domains, and delete anything stale, task-specific, or not reinforced. When space is tight, drop the least durable bullets. The result must obey every format rule above, even where this document does not.\n\n{}\n</existing_preferences>",
            full_prompt, compact_steering_doc(&existing_body)
        )
    };

    // `try_send_prompt` resets this session's accumulator under the
    // prompt lock right before sending, so the read below sees just the
    // extraction reply (and a skipped attempt can't wipe a real prompt's
    // in-flight stream).
    //
    // Send as a regular prompt on the current session
    // We use a special prefix so the UI can potentially hide this exchange
    let steering_prompt = format!(
        "[KAGE_STEERING_IGNORE] [AUTO_STEERING_EXTRACTION]\n{}",
        prompt_with_existing
    );

    let response = match client.try_send_prompt(
        session_id,
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": steering_prompt }]
        }),
    )? {
        Some(r) => r,
        // A real prompt is in flight — defer extraction so we don't make
        // the user wait behind this background pass. Runs again on the
        // next eligible message_complete.
        None => {
            info!("Prompt in progress — deferring auto-steering extraction to next turn");
            return Ok(()); // Non-fatal
        }
    };

    if let Some(error) = response.error {
        warn!("Auto-steering extraction failed: {}", error.message);
        // Drop the (possibly partial) accumulator so it doesn't bleed into
        // the next read on the same session.
        client.reset_session_accumulator(session_id);
        return Ok(()); // Non-fatal
    }

    // Read the accumulated response and clear its bucket.
    let result = client.take_session_accumulator(session_id);

    let cleaned = match parse_extraction_response(&result) {
        ExtractionOutcome::Empty => {
            warn!("Auto-steering extraction returned empty result");
            return Ok(());
        }
        ExtractionOutcome::Refusal => {
            info!("Agent declined auto-steering generation — keeping existing document");
            return Ok(());
        }
        ExtractionOutcome::Content(c) => c,
    };

    // Write to the auto-steering file
    let auto_path = Config::get_auto_steering_path()?;
    if let Some(parent) = auto_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let compacted = compact_steering_doc(&cleaned);
    if compacted.len() < cleaned.trim().len() {
        info!(
            "Auto-steering: compacted extraction reply {} -> {} bytes",
            cleaned.trim().len(),
            compacted.len()
        );
    }
    let content = format!("{}{}", auto_steering_header(), compacted);
    fs::write(&auto_path, &content)?;

    info!(
        "Auto-steering document updated ({} bytes) at {:?}",
        content.len(),
        auto_path
    );

    Ok(())
}

/// Run auto-steering generation in the background if enabled.
/// Called from the message completion handler with the session id of
/// the message that just completed. Triggers every 5 messages but no
/// more than once per hour. On-exit generation bypasses the cooldown.
pub fn maybe_generate_steering(
    client: Arc<AcpClient>,
    config: Arc<std::sync::Mutex<Config>>,
    session_id: String,
) {
    if !tick_message_counter() {
        return;
    }

    // Spawn a background task so we don't block the message flow
    tauri::async_runtime::spawn(async move {
        {
            let config = config.lock_or_recover();
            if !config.acp.agent.auto_steering_enabled {
                return;
            }
        }

        if !client.is_connected() {
            return;
        }

        info!(
            "Auto-steering update triggered (every {} messages, ≥{}s cooldown)",
            UPDATE_INTERVAL_MESSAGES, MIN_UPDATE_INTERVAL_SECS
        );
        match generate_steering_document(&client, &session_id) {
            Ok(()) => mark_generation(),
            Err(e) => error!("Auto-steering generation failed: {}", e),
        }
    });
}

/// Force an immediate steering document generation (e.g., on quit).
/// This runs synchronously and blocks until complete. Skipped if no
/// messages have been sent since the last generation. The caller picks
/// the session id — quit-time uses `window_sessions["main"]`.
pub fn generate_steering_on_quit(client: &AcpClient, config: &Config, session_id: &str) {
    if !config.acp.agent.auto_steering_enabled {
        return;
    }

    if !client.is_connected() {
        return;
    }

    if MESSAGES_SINCE_GENERATION.load(Ordering::Relaxed) == 0 {
        info!("Auto-steering: no new messages since last generation, skipping on-quit update");
        return;
    }

    info!("Generating auto-steering document before quit");
    match generate_steering_document(client, session_id) {
        Ok(()) => mark_generation(),
        Err(e) => error!("Auto-steering generation on quit failed: {}", e),
    }
}

/// Outcome of running the LLM extraction response through the parser.
#[derive(Debug, PartialEq, Eq)]
pub enum ExtractionOutcome {
    /// Response was blank or whitespace-only — skip the write.
    Empty,
    /// Agent refused the extraction prompt — skip the write, keep existing doc.
    Refusal,
    /// Cleaned response body to write to disk (fences already stripped).
    Content(String),
}

/// Phrases an agent uses to decline. Matched as substrings against the
/// trimmed response body. Kept inline because the surface is small and the
/// phrases come from observed model behavior — adding a new phrase shouldn't
/// require touching anything else.
const REFUSAL_PHRASES: &[&str] = &[
    "I cannot generate this",
    "I'm not going to perform",
    "not going to perform that",
    "inconsistent with how I operate",
];

/// True if the response looks like the agent refused the extraction.
/// Public so the same definition can be exercised from tests.
pub fn is_refusal_response(trimmed: &str) -> bool {
    if trimmed.starts_with("STEERING_DECLINED") {
        return true;
    }
    REFUSAL_PHRASES.iter().any(|p| trimmed.contains(p))
}

/// Pure response classifier: empty / refusal / cleaned content. Lifts the
/// fence-stripping + refusal-detection logic out of `generate_steering_document`
/// so the brittle string-contains rules are testable in isolation.
pub fn parse_extraction_response(raw: &str) -> ExtractionOutcome {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ExtractionOutcome::Empty;
    }
    if is_refusal_response(trimmed) {
        return ExtractionOutcome::Refusal;
    }
    ExtractionOutcome::Content(strip_code_fences(raw))
}

/// Header comment prepended to the auto-steering file. Kept as a function
/// (rather than `const`) so the literal isn't repeated across modules.
pub fn auto_steering_header() -> &'static str {
    "<!-- AUTO-GENERATED STEERING DOCUMENT\n     This file is automatically updated based on your conversations.\n     Any manual changes may be overridden.\n     To add your own persistent instructions, use a User Steering Document instead. -->\n\n"
}

/// Strip markdown code fences (```markdown ... ``` or ``` ... ```) from LLM output.
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();

    // Check if the entire response is wrapped in a code fence
    if trimmed.starts_with("```") {
        let after_opening = if let Some(first_newline) = trimmed.find('\n') {
            &trimmed[first_newline + 1..]
        } else {
            return trimmed.to_string();
        };

        // Strip trailing fence
        let result = if after_opening.trim_end().ends_with("```") {
            let end = after_opening.trim_end();
            &end[..end.len() - 3]
        } else {
            after_opening
        };

        result.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Strip the HTML header comment from the steering document content.
pub fn strip_header_comment(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with("<!--") {
        if let Some(end_pos) = trimmed.find("-->") {
            return trimmed[end_pos + 3..].trim().to_string();
        }
    }
    trimmed.to_string()
}

/// Cut `text` to at most `max` bytes including the trailing `…`, backing up
/// to a char boundary and then to the last whitespace so words aren't
/// split. Output never exceeds `max`, so re-truncating is a no-op.
fn truncate_at_word(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max.saturating_sub('…'.len_utf8());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = &text[..end];
    let cut = match cut.rfind(char::is_whitespace) {
        // Don't back up so far that one long token eats most of the line.
        Some(i) if i > end / 2 => &cut[..i],
        _ => cut,
    };
    format!("{}…", cut.trim_end())
}

/// Deterministic size bound on a steering document body. The prompt asks
/// the model to stay small, but models drift, and the merge pass feeds the
/// previous doc back in — without a hard cap the doc only ever grows.
///
/// Headings are kept; at most `MAX_LINES_PER_SECTION` content lines per
/// heading (earlier lines win — the prompt orders by importance); each line
/// cut to `MAX_LINE_CHARS`; blank lines dropped; and once the body would
/// exceed `MAX_DOC_CHARS`, everything after is dropped. A heading is only
/// emitted once one of its lines fits, so no empty sections survive.
pub fn compact_steering_doc(body: &str) -> String {
    let mut out = String::with_capacity(body.len().min(MAX_DOC_CHARS + 64));
    let mut pending_heading: Option<&str> = None;
    let mut lines_in_section = 0usize;

    for raw in body.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_start().starts_with('#') {
            pending_heading = Some(line.trim());
            lines_in_section = 0;
            continue;
        }
        if lines_in_section >= MAX_LINES_PER_SECTION {
            continue;
        }
        let line = truncate_at_word(line, MAX_LINE_CHARS);
        let heading_cost = pending_heading.map_or(0, |h| h.len() + 2);
        if out.len() + heading_cost + line.len() + 1 > MAX_DOC_CHARS {
            break;
        }
        if let Some(h) = pending_heading.take() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(h);
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
        lines_in_section += 1;
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_code_fences_markdown() {
        let input = "```markdown\n# Hello\nWorld\n```";
        assert_eq!(strip_code_fences(input), "# Hello\nWorld");
    }

    #[test]
    fn test_strip_code_fences_plain() {
        let input = "```\nsome content\n```";
        assert_eq!(strip_code_fences(input), "some content");
    }

    #[test]
    fn test_strip_code_fences_no_fences() {
        let input = "just plain text";
        assert_eq!(strip_code_fences(input), "just plain text");
    }

    #[test]
    fn test_strip_code_fences_with_whitespace() {
        let input = "  \n```markdown\ncontent here\n```\n  ";
        assert_eq!(strip_code_fences(input), "content here");
    }

    #[test]
    fn test_strip_code_fences_no_closing() {
        let input = "```markdown\ncontent without closing";
        assert_eq!(strip_code_fences(input), "content without closing");
    }

    #[test]
    fn test_strip_header_comment() {
        let input = "<!-- Auto-generated -->\n# My Steering";
        assert_eq!(strip_header_comment(input), "# My Steering");
    }

    #[test]
    fn test_strip_header_comment_no_comment() {
        let input = "# Just a heading";
        assert_eq!(strip_header_comment(input), "# Just a heading");
    }

    #[test]
    fn test_strip_header_comment_multiline() {
        let input = "<!-- This is a\nmultiline comment -->\nContent";
        assert_eq!(strip_header_comment(input), "Content");
    }

    #[test]
    fn test_strip_header_comment_empty() {
        assert_eq!(strip_header_comment(""), "");
    }

    // ---- is_refusal_response / parse_extraction_response ----------------

    #[test]
    fn refusal_detected_for_steering_declined_sentinel() {
        assert!(is_refusal_response("STEERING_DECLINED"));
        // Agent sometimes adds a trailing newline/period. Sentinel must
        // still match as long as it's the prefix of the trimmed body.
        assert!(is_refusal_response("STEERING_DECLINED\nreason omitted"));
    }

    #[test]
    fn refusal_detected_for_each_observed_phrase() {
        // These all have to keep matching — they came from real refusals.
        // If the model rephrases, that's a follow-up; this test pins what we
        // know catches refusals today.
        for phrase in [
            "I cannot generate this",
            "I'm not going to perform",
            "not going to perform that",
            "inconsistent with how I operate",
        ] {
            assert!(
                is_refusal_response(&format!("Some preamble. {} Some postamble.", phrase)),
                "expected refusal match for: {}",
                phrase
            );
        }
    }

    #[test]
    fn refusal_not_triggered_by_legitimate_content() {
        // A normal steering document must NOT match. The phrases above are
        // distinct enough that this is plausible, but worth pinning.
        let body = "## About the User\n- Name: Alice\n- Role: backend engineer\n";
        assert!(!is_refusal_response(body));
    }

    #[test]
    fn parse_returns_empty_for_blank_body() {
        assert_eq!(parse_extraction_response(""), ExtractionOutcome::Empty);
        assert_eq!(
            parse_extraction_response("   \n\t  "),
            ExtractionOutcome::Empty
        );
    }

    #[test]
    fn parse_returns_refusal_when_phrase_present() {
        let raw = "STEERING_DECLINED";
        assert_eq!(parse_extraction_response(raw), ExtractionOutcome::Refusal);

        let raw = "Sorry, that's inconsistent with how I operate today.";
        assert_eq!(parse_extraction_response(raw), ExtractionOutcome::Refusal);
    }

    #[test]
    fn parse_returns_cleaned_content_with_fences_stripped() {
        let raw = "```markdown\n## About the User\n- Name: Alice\n```";
        let outcome = parse_extraction_response(raw);
        assert_eq!(
            outcome,
            ExtractionOutcome::Content("## About the User\n- Name: Alice".to_string())
        );
    }

    #[test]
    fn parse_returns_unfenced_content_unchanged_aside_from_trim() {
        let raw = "## About the User\n- Name: Alice\n";
        match parse_extraction_response(raw) {
            ExtractionOutcome::Content(c) => {
                assert_eq!(c, "## About the User\n- Name: Alice");
            }
            other => panic!("expected Content, got {:?}", other),
        }
    }

    #[test]
    fn auto_steering_header_starts_with_html_comment_marker() {
        // The comment marker is what `strip_header_comment` looks for when
        // re-reading existing docs — it must remain HTML-comment shaped.
        let header = auto_steering_header();
        assert!(header.starts_with("<!--"));
        assert!(header.contains("AUTO-GENERATED STEERING DOCUMENT"));
        assert!(header.contains("-->"));
    }

    #[test]
    fn test_tick_message_counter_threshold() {
        // Reset counters
        MESSAGE_COUNTER.store(0, Ordering::Relaxed);
        MESSAGES_SINCE_GENERATION.store(0, Ordering::Relaxed);
        // Reset the last generation time so cooldown doesn't block
        *LAST_GENERATION.lock_or_recover() = None;

        // Should not trigger until threshold (UPDATE_INTERVAL_MESSAGES = 5)
        for _ in 0..UPDATE_INTERVAL_MESSAGES - 1 {
            assert!(!tick_message_counter());
        }
        // At the threshold, should trigger (first time — no cooldown yet)
        assert!(tick_message_counter());
    }

    #[test]
    fn truncate_at_word_leaves_short_text_alone() {
        assert_eq!(truncate_at_word("short line", 100), "short line");
    }

    #[test]
    fn truncate_at_word_cuts_on_whitespace_and_marks_cut() {
        let out = truncate_at_word("alpha beta gamma delta", 13);
        assert_eq!(out, "alpha beta…");
    }

    #[test]
    fn truncate_at_word_never_splits_a_codepoint() {
        // 'é' is 2 bytes; a cap landing mid-codepoint must back up, not panic.
        let s = "é".repeat(50);
        let out = truncate_at_word(&s, 7);
        assert!(out.ends_with('…'));
        assert!(out.trim_end_matches('…').chars().all(|c| c == 'é'));
    }

    #[test]
    fn compact_keeps_a_small_doc_verbatim() {
        let doc = "## About the User\n- Sam, PM\n\n## Kage Behavior\n- Be brief";
        assert_eq!(compact_steering_doc(doc), doc);
    }

    #[test]
    fn compact_caps_lines_per_section() {
        let mut doc = String::from("## Interests & Expertise\n");
        for i in 0..20 {
            doc.push_str(&format!("- topic {}\n", i));
        }
        let out = compact_steering_doc(&doc);
        assert_eq!(
            out.lines().filter(|l| l.starts_with("- ")).count(),
            MAX_LINES_PER_SECTION
        );
        assert!(out.contains("- topic 0"), "earlier lines win");
        assert!(!out.contains("- topic 19"));
    }

    #[test]
    fn compact_bounds_the_observed_runaway_doc() {
        // Shape of the real regression: few sections, each holding a handful
        // of 1-2KB run-on bullets of task trivia. Must land under the cap.
        let wall = "Deep PKI/X.509 rate limits, quotas and API surface; ".repeat(40);
        let mut doc = String::new();
        for h in [
            "About the User",
            "Communication Preferences",
            "Interests & Expertise",
            "Kage Behavior",
        ] {
            doc.push_str(&format!("## {}\n", h));
            for _ in 0..6 {
                doc.push_str(&format!("- {}\n", wall));
            }
            doc.push('\n');
        }
        assert!(
            doc.len() > 18_000,
            "precondition: input resembles the 18KB doc"
        );
        let out = compact_steering_doc(&doc);
        assert!(out.len() <= MAX_DOC_CHARS, "got {} bytes", out.len());
        assert!(out.lines().all(|l| l.len() <= MAX_LINE_CHARS));
        assert!(
            out.starts_with("## About the User"),
            "identity section survives"
        );
    }

    #[test]
    fn compact_drops_headings_with_no_room_for_content() {
        // ~240-byte lines, 5 per section: A and B fill ~2400 of the 2500
        // budget, so C's first line can't fit.
        let filler = format!("- {}\n", "x ".repeat(119));
        let mut doc = String::new();
        for h in ["A", "B", "C"] {
            doc.push_str(&format!("## {}\n", h));
            for _ in 0..5 {
                doc.push_str(&filler);
            }
        }
        let out = compact_steering_doc(&doc);
        assert!(out.contains("## B"));
        assert!(
            !out.contains("## C"),
            "heading with no room for content is dropped"
        );
        assert!(!out.lines().last().unwrap_or("").starts_with('#'));
    }

    #[test]
    fn compact_is_idempotent() {
        let wall = "word ".repeat(400);
        let doc = format!("## A\n- {}\n- {}\n## B\n- {}\n", wall, wall, wall);
        let once = compact_steering_doc(&doc);
        assert_eq!(compact_steering_doc(&once), once);
    }
}
