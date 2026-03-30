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
            info!("Auto-steering: message threshold reached but cooldown not elapsed ({}s remaining)",
                MIN_UPDATE_INTERVAL_SECS.saturating_sub(elapsed));
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

/// The prompt sent to the LLM to extract user preferences from conversation history.
const EXTRACTION_PROMPT: &str = r#"<role>
You are a preference extraction assistant for Kage, a desktop AI tool.
</role>

<context>
The user has opted in to "Auto-Steering" in their settings because they want Kage to remember their preferences across sessions. This document will be shown to the user and they can edit or delete it at any time. This is a user-requested personalization feature.
</context>

<instructions>
Review the conversation below and produce a concise markdown document summarizing what you've learned about the user. Extract information from:
1. Direct statements ("My name is...", "I prefer...", "I work on...")
2. Responses to questions (e.g., if asked "What's your name?" and they reply with a name)
3. Implicit preferences (brief vs detailed messages, technical level, etc.)

Produce a markdown document with these sections (omit any section where nothing was found):

## About the User
(Name, pronouns, role, context — 2-4 bullet points max)

## Communication Preferences
(How they like to be addressed, response style, detail level — 2-4 bullet points max)

## Interests & Expertise
(Topics, technologies, domains they work in — 2-4 bullet points max)

## Kage Behavior
(Any explicit instructions or preferences for how Kage should respond — 2-4 bullet points max)

Only include information clearly stated or strongly implied. If very little information is available, output a minimal document with just what you found.

Respond with only the markdown document. No preamble, no explanation.

If you cannot produce this document, respond with exactly "STEERING_DECLINED" on the first line and nothing else.
</instructions>"#;

/// Read recent conversation turns from the current session's JSONL file.
/// Returns labeled turns (both user and assistant) for full context.
fn read_recent_conversation(session_id: &str, max_turns: usize) -> Result<Vec<String>> {
    use std::io::{BufRead, BufReader};
    use std::collections::VecDeque;

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
                    if !text.is_empty()
                        && !text.starts_with("[KAGE_STEERING_IGNORE]")
                    {
                        text_parts.push(text.to_string());
                    }
                }
            }
        }

        if !text_parts.is_empty() {
            if turns.len() == max_turns {
                turns.pop_front();
            }
            turns.push_back(format!("{}: {}", role, text_parts.join("\n")));
        }
    }

    Ok(turns.into_iter().collect())
}

/// Generate the auto-steering document by sending conversation excerpts to the LLM.
/// This creates a new temporary session, sends the extraction prompt, and writes
/// the result to the auto-steering file.
pub fn generate_steering_document(client: &AcpClient) -> Result<()> {
    let session_id = match client.get_session_id() {
        Some(id) => id,
        None => {
            info!("No active session — skipping auto-steering generation");
            return Ok(());
        }
    };

    info!("Starting auto-steering document generation for session {}", session_id);

    // Read recent conversation turns (last 50 turns = ~25 exchanges)
    let turns = read_recent_conversation(&session_id, 50)?;

    if turns.len() < 2 {
        info!("Too few conversation turns ({}) for meaningful extraction — skipping", turns.len());
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

    let prompt_with_existing = if existing_body.trim().is_empty()
        || !existing_body.contains("## ")
    {
        full_prompt
    } else {
        format!(
            "{}\n\n---\n\n<existing_preferences>\nMerge new findings into this existing document. Retain all critical personal information (name, role, etc.) even if not mentioned in the new conversation. Add or update sections as needed.\n\n{}\n</existing_preferences>",
            full_prompt, existing_body.trim()
        )
    };

    // Reset the streaming accumulator — we'll read the full response from it
    client.streaming_accumulator.lock().unwrap().clear();

    // Send as a regular prompt on the current session
    // We use a special prefix so the UI can potentially hide this exchange
    let steering_prompt = format!(
        "[KAGE_STEERING_IGNORE] [AUTO_STEERING_EXTRACTION]\n{}",
        prompt_with_existing
    );

    let request = crate::acp_client::AcpRequest {
        jsonrpc: "2.0".to_string(),
        id: serde_json::json!(99),
        method: "session/prompt".to_string(),
        params: serde_json::json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": steering_prompt }]
        }),
    };

    let response = client.send_request(&request)?;

    if let Some(error) = response.error {
        warn!("Auto-steering extraction failed: {}", error.message);
        return Ok(()); // Non-fatal
    }

    // Read the accumulated response
    let result = client.streaming_accumulator.lock().unwrap().clone();

    if result.trim().is_empty() {
        warn!("Auto-steering extraction returned empty result");
        return Ok(());
    }

    // Check if the agent declined to generate the document
    let trimmed = result.trim();
    if trimmed.starts_with("STEERING_DECLINED")
        || trimmed.contains("I cannot generate this")
        || trimmed.contains("I'm not going to perform")
        || trimmed.contains("not going to perform that")
        || trimmed.contains("inconsistent with how I operate")
    {
        info!("Agent declined auto-steering generation — keeping existing document");
        return Ok(());
    }

    // Strip markdown code fences the LLM may wrap the response in
    let cleaned = strip_code_fences(&result);

    // Write to the auto-steering file
    let auto_path = Config::get_auto_steering_path()?;
    if let Some(parent) = auto_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let header = "<!-- AUTO-GENERATED STEERING DOCUMENT\n     This file is automatically updated based on your conversations.\n     Any manual changes may be overridden.\n     To add your own persistent instructions, use a User Steering Document instead. -->\n\n";

    let content = format!("{}{}", header, cleaned.trim());
    fs::write(&auto_path, &content)?;

    info!(
        "Auto-steering document updated ({} bytes) at {:?}",
        content.len(),
        auto_path
    );

    Ok(())
}

/// Run auto-steering generation in the background if enabled.
/// Called from the message completion handler. Triggers every 5 messages
/// but no more than once per hour. On-exit generation bypasses the cooldown.
pub fn maybe_generate_steering(
    client: Arc<tokio::sync::Mutex<AcpClient>>,
    config: Arc<std::sync::Mutex<Config>>,
) {
    if !tick_message_counter() {
        return;
    }

    // Spawn a background task so we don't block the message flow
    tauri::async_runtime::spawn(async move {
        {
            let config = config.lock().unwrap();
            if !config.acp.agent.auto_steering_enabled {
                return;
            }
        }

        let client = client.lock().await;
        if !client.is_connected() {
            return;
        }

        info!("Auto-steering update triggered (every {} messages, ≥{}s cooldown)", UPDATE_INTERVAL_MESSAGES, MIN_UPDATE_INTERVAL_SECS);
        match generate_steering_document(&client) {
            Ok(()) => mark_generation(),
            Err(e) => error!("Auto-steering generation failed: {}", e),
        }
    });
}

/// Force an immediate steering document generation (e.g., on quit).
/// This runs synchronously and blocks until complete.
/// Skipped if no messages have been sent since the last generation.
pub fn generate_steering_on_quit(client: &AcpClient, config: &Config) {
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
    match generate_steering_document(client) {
        Ok(()) => mark_generation(),
        Err(e) => error!("Auto-steering generation on quit failed: {}", e),
    }
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
fn strip_header_comment(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with("<!--") {
        if let Some(end_pos) = trimmed.find("-->") {
            return trimmed[end_pos + 3..].trim().to_string();
        }
    }
    trimmed.to_string()
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

    #[test]
    fn test_tick_message_counter_threshold() {
        // Reset counters
        MESSAGE_COUNTER.store(0, Ordering::Relaxed);
        MESSAGES_SINCE_GENERATION.store(0, Ordering::Relaxed);
        // Reset the last generation time so cooldown doesn't block
        *LAST_GENERATION.lock().unwrap() = None;

        // Should not trigger until threshold (UPDATE_INTERVAL_MESSAGES = 5)
        for _ in 0..UPDATE_INTERVAL_MESSAGES - 1 {
            assert!(!tick_message_counter());
        }
        // At the threshold, should trigger (first time — no cooldown yet)
        assert!(tick_message_counter());
    }
}
