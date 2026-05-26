//! Background AI-summarised session titles.
//!
//! After a successful user→assistant turn, fire a hidden prompt on the
//! same session asking the agent to summarise the conversation in a
//! few words. The reply becomes the session's persistent title.
//!
//! # Why same-session
//!
//! Two reasonable shapes for "ask the LLM for a title":
//!
//! 1. **Same session.** Send a hidden prompt prefixed
//!    `[KAGE_STEERING_IGNORE]` so the chat UI hides it; the agent
//!    already has the conversation context, so the result is
//!    high-quality. Cost: one short prompt's worth of tokens added to
//!    the session's permanent context.
//!
//! 2. **Ephemeral session.** `client.create_session()` → prompt with
//!    a copy of the conversation → discard. No context bloat in the
//!    real session. Cost: extra session-creation roundtrip + we have
//!    to pay tokens to feed it the conversation.
//!
//! We picked (1) for now (simpler, and the token cost is one-shot
//! per session lifetime). The trade-off is logged here so a future
//! switch to (2) is a clear change rather than a re-discovery.
//!
//! # Gating
//!
//! - Skip when the session has a cached title with source `Manual`
//!   (user has renamed) or `Ai` (already summarised).
//! - Skip when the session has fewer than ~2 turns of content (the
//!   prompt would either fail or produce something useless).
//! - Skip when the agent is mid-compaction (`wait_for_compaction`
//!   would block too long; we'll try again next message_complete).
//!
//! # Fallback
//!
//! If the agent refuses, returns empty, or returns something we can't
//! clean into a usable title, we leave the cache entry alone — the
//! next message_complete attempts again. No retry storms because the
//! gating is per-completion, not per-session.

use anyhow::Result;
use log::{info, warn};

use crate::acp_client::AcpClient;

/// Maximum length of the cleaned title (chars, not bytes). The
/// session list UI clips at 60; we cap at the same value so the cache
/// matches what the user will see.
const MAX_TITLE_CHARS: usize = 60;

/// Hidden steering prefix the agent's session prompt is wrapped in.
/// Mirrors `auto_steering::STEERING_MSG_PREFIX` semantics — the chat
/// UI's `stripKageTags` hides messages starting with this from the
/// rendered transcript.
const TITLE_PROMPT_PREFIX: &str = "[KAGE_STEERING_IGNORE] [KAGE_TITLE]";

/// The actual instructions sent to the agent. Kept short and explicit:
/// the agent has the full conversation context, so we don't need to
/// restate it.
const TITLE_INSTRUCTIONS: &str = "Summarise this conversation in 3 to 6 words. \
     Output ONLY the title, plain text, no quotes, no trailing punctuation, \
     no markdown, no explanation. If you can't summarise, output nothing.";

/// Issue the hidden title-summarisation prompt on `session_id` and
/// return the cleaned title. Returns `Ok(None)` when the agent
/// produced something we can't use (empty, refusal, or only garbage)
/// — caller should leave the cache untouched and try again next turn.
pub fn generate_title(client: &AcpClient, session_id: &str) -> Result<Option<String>> {
    info!(
        "Generating AI title for session {}",
        &session_id[..session_id.len().min(12)]
    );

    // Don't pollute the user's accumulator if a real prompt is in
    // flight. The flush thread coalesces by session, so resetting here
    // is safe — the user's prompt completed before we ran (we're in
    // the message_complete epilogue).
    client.reset_session_accumulator(session_id);

    let prompt = format!("{} {}", TITLE_PROMPT_PREFIX, TITLE_INSTRUCTIONS);
    let response = client.send_request(
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": prompt }]
        }),
    )?;

    if let Some(error) = response.error {
        warn!("Title generation failed: {}", error.message);
        client.reset_session_accumulator(session_id);
        return Ok(None);
    }

    let raw = client.take_session_accumulator(session_id);
    Ok(clean_title(&raw))
}

/// Trim, strip surrounding quotes/punctuation, drop trailing
/// punctuation, clip at MAX_TITLE_CHARS. Returns `None` for an empty
/// or refusal-shaped result.
pub fn clean_title(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_string();

    // Strip a leading/trailing pair of matching quotes (the agent
    // sometimes ignores "no quotes" — common smart-quote variants too).
    for (open, close) in &[('"', '"'), ('\'', '\''), ('“', '”'), ('‘', '’')] {
        if s.starts_with(*open) && s.ends_with(*close) && s.chars().count() >= 2 {
            let mut chars: Vec<char> = s.chars().collect();
            chars.remove(0);
            chars.pop();
            s = chars.into_iter().collect();
            s = s.trim().to_string();
        }
    }

    // Drop trailing terminal punctuation.
    while let Some(last) = s.chars().last() {
        if matches!(last, '.' | '!' | '?' | ',' | ';' | ':') {
            s.pop();
            s = s.trim_end().to_string();
        } else {
            break;
        }
    }

    // Refusal patterns: the agent says it can't, asks for input, etc.
    // Cheap heuristic — anything obviously prefixed with "I" + "can"
    // or with a question mark in it is suspicious for a title.
    let lower = s.to_lowercase();
    let refusal_starts = [
        "i can't",
        "i cannot",
        "i'm sorry",
        "sorry",
        "i don't",
        "i do not",
        "no title",
        "untitled",
    ];
    if refusal_starts.iter().any(|p| lower.starts_with(p)) {
        return None;
    }

    if s.is_empty() {
        return None;
    }

    // Clip at MAX_TITLE_CHARS without slicing through a multi-byte
    // boundary; append … if we cut something off.
    let count = s.chars().count();
    if count > MAX_TITLE_CHARS {
        let truncated: String = s.chars().take(MAX_TITLE_CHARS).collect();
        s = format!("{}…", truncated.trim_end());
    }

    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_title_strips_surrounding_quotes() {
        assert_eq!(
            clean_title("\"Auth refactor\""),
            Some("Auth refactor".into())
        );
        assert_eq!(clean_title("'Auth refactor'"), Some("Auth refactor".into()));
        assert_eq!(clean_title("“Auth refactor”"), Some("Auth refactor".into()));
    }

    #[test]
    fn clean_title_drops_trailing_punctuation() {
        assert_eq!(clean_title("Auth refactor."), Some("Auth refactor".into()));
        assert_eq!(clean_title("Auth refactor!"), Some("Auth refactor".into()));
        assert_eq!(clean_title("Auth refactor?!"), Some("Auth refactor".into()));
    }

    #[test]
    fn clean_title_handles_quote_then_punctuation() {
        // Real model output: `"Auth refactor."` — quotes outside the
        // period. After quote-strip we still have "Auth refactor." to
        // de-punctuate.
        assert_eq!(
            clean_title("\"Auth refactor.\""),
            Some("Auth refactor".into())
        );
    }

    #[test]
    fn clean_title_returns_none_on_refusal() {
        assert_eq!(clean_title("I can't summarise this."), None);
        assert_eq!(clean_title("I'm sorry, but..."), None);
        assert_eq!(clean_title("Sorry, no title available"), None);
        // Case-insensitive
        assert_eq!(clean_title("UNTITLED"), None);
    }

    #[test]
    fn clean_title_returns_none_on_empty() {
        assert_eq!(clean_title(""), None);
        assert_eq!(clean_title("   "), None);
        assert_eq!(clean_title("\"\""), None);
    }

    #[test]
    fn clean_title_clips_long_output() {
        let long = "a".repeat(MAX_TITLE_CHARS + 20);
        let cleaned = clean_title(&long).expect("non-empty");
        // Truncated string + ellipsis = MAX_TITLE_CHARS + 1 char wide.
        assert_eq!(cleaned.chars().count(), MAX_TITLE_CHARS + 1);
        assert!(cleaned.ends_with('…'));
    }

    #[test]
    fn clean_title_passes_through_a_normal_title() {
        assert_eq!(
            clean_title("Refactoring auth middleware"),
            Some("Refactoring auth middleware".into())
        );
    }

    #[test]
    fn clean_title_handles_multibyte_clip_boundary() {
        // 30 emoji + 30 emoji = 60 chars, no clip needed. 70 emoji
        // exceeds and should clip without panicking on byte boundary.
        let text: String = "🎉".repeat(70);
        let cleaned = clean_title(&text).expect("non-empty");
        assert_eq!(cleaned.chars().count(), MAX_TITLE_CHARS + 1);
        assert!(cleaned.ends_with('…'));
    }
}
