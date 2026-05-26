//! Kiro CLI session provider — reads the on-disk SQLite database
//! kiro-cli writes to (`%LOCALAPPDATA%/kiro-cli/data.sqlite3` on Windows;
//! `~/.local/share/kiro-cli/data.sqlite3` elsewhere). All access is
//! read-only, opened with `SQLITE_OPEN_NO_MUTEX` to avoid disturbing the
//! CLI when it's writing concurrently.

use super::{
    clip_title, rfc3339_from_epoch_ms, AgentMessage, AgentSession, AgentSessionProvider,
    SessionLocator,
};
use crate::error::AppError;
use log::info;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

const PROVIDER_ID: &str = "kiro-cli";
const PROVIDER_LABEL: &str = "Kiro CLI";

/// Locator shape — just the conversation id. Kept as its own struct for
/// readability, deserialised on the fly from the opaque
/// `SessionLocator`.
#[derive(Debug, Deserialize)]
struct KiroCliLocator {
    conversation_id: String,
}

#[derive(Default)]
pub struct KiroCliProvider;

impl KiroCliProvider {
    pub fn new() -> Self {
        Self
    }

    fn db_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA")
                .ok()
                .map(|d| PathBuf::from(d).join("kiro-cli").join("data.sqlite3"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir().map(|d| {
                d.join(".local")
                    .join("share")
                    .join("kiro-cli")
                    .join("data.sqlite3")
            })
        }
    }

    fn open_db() -> Result<rusqlite::Connection, AppError> {
        let db_path = Self::db_path().ok_or_else(|| AppError::internal("kiro-cli path resolve"))?;
        rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| AppError::internal(format!("SQLite open: {}", e)))
    }
}

impl AgentSessionProvider for KiroCliProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn label(&self) -> &'static str {
        PROVIDER_LABEL
    }

    fn is_available(&self) -> bool {
        Self::db_path().map(|p| p.exists()).unwrap_or(false)
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError> {
        let db_path = Self::db_path()
            .ok_or_else(|| AppError::internal("kiro-cli database path unresolvable"))?;
        if !db_path.exists() {
            return Ok(Vec::new());
        }
        let db = Self::open_db()?;

        let mut stmt = db
            .prepare(
                "SELECT key, conversation_id, value, created_at, updated_at \
                 FROM conversations_v2 ORDER BY updated_at DESC LIMIT ?1",
            )
            .map_err(|e| AppError::internal(format!("SQLite prepare: {}", e)))?;

        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| AppError::internal(format!("SQLite query: {}", e)))?;

        let db_path_str = db_path.to_string_lossy().to_string();
        let mut sessions = Vec::new();
        for row in rows {
            let Ok((workspace, conv_id, value_json, _created_at, updated_at)) = row else {
                continue;
            };
            let title = extract_title(&value_json);
            let message_count = count_messages(&value_json);

            sessions.push(AgentSession {
                provider_id: PROVIDER_ID.to_string(),
                session_id: conv_id.clone(),
                title,
                updated_at: rfc3339_from_epoch_ms(updated_at),
                message_count,
                container: Some(workspace.clone()),
                locator: json!({ "conversation_id": conv_id }),
                extras: json!({
                    "workspace": workspace,
                    "file_path": db_path_str,
                }),
            });
        }
        Ok(sessions)
    }

    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
        let loc: KiroCliLocator = serde_json::from_value(locator.clone())
            .map_err(|e| AppError::internal(format!("kiro-cli locator: {}", e)))?;
        let db = Self::open_db()?;

        let value_json: String = db
            .query_row(
                "SELECT value FROM conversations_v2 WHERE conversation_id = ?1",
                [&loc.conversation_id],
                |row| row.get(0),
            )
            .map_err(|e| AppError::internal(format!("SQLite query: {}", e)))?;

        let json: serde_json::Value = serde_json::from_str(&value_json)
            .map_err(|e| AppError::internal(format!("JSON parse: {}", e)))?;

        let messages = render_history(&json);
        info!(
            "Loaded kiro-cli session {}: {} messages",
            loc.conversation_id,
            messages.len()
        );
        Ok(messages)
    }

    fn check_session_updated(
        &self,
        locator: &SessionLocator,
        since_ms: i64,
    ) -> Result<Option<i64>, AppError> {
        let loc: KiroCliLocator = serde_json::from_value(locator.clone())
            .map_err(|e| AppError::internal(format!("kiro-cli locator: {}", e)))?;
        let db = Self::open_db()?;
        let current: i64 = db
            .query_row(
                "SELECT updated_at FROM conversations_v2 WHERE conversation_id = ?1",
                [&loc.conversation_id],
                |row| row.get(0),
            )
            .map_err(|e| AppError::internal(format!("SQLite query: {}", e)))?;
        if current > since_ms {
            Ok(Some(current))
        } else {
            Ok(None)
        }
    }
}

/// Pull the title from the first transcript entry.
fn extract_title(value_json: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(value_json) else {
        return "Untitled".to_string();
    };
    let Some(transcript) = json.get("transcript").and_then(|t| t.as_array()) else {
        return "Untitled".to_string();
    };
    let Some(first) = transcript.first().and_then(|t| t.as_str()) else {
        return "Untitled".to_string();
    };
    let clean = first.trim().trim_start_matches('>').trim();
    if clean.is_empty() {
        "Untitled".to_string()
    } else {
        clip_title(clean, 80)
    }
}

fn count_messages(value_json: &str) -> usize {
    serde_json::from_str::<serde_json::Value>(value_json)
        .ok()
        .and_then(|j| {
            j.get("transcript")
                .and_then(|t| t.as_array())
                .map(|a| a.len())
        })
        .unwrap_or(0)
}

/// Walk the kiro-cli `history` array and project each entry into one or
/// more `AgentMessage`s. The structure is
/// `{ user: { content: { Prompt | ToolUseResults } }, assistant: { ToolUse | Response | Message } }`,
/// so we may emit several messages per history entry (user prompt, then
/// tool results, then assistant text, then tool calls).
fn render_history(json: &serde_json::Value) -> Vec<AgentMessage> {
    let Some(history) = json.get("history").and_then(|h| h.as_array()) else {
        return Vec::new();
    };

    let mut messages = Vec::new();
    for entry in history {
        if let Some(user) = entry.get("user") {
            push_user_messages(user, &mut messages);
        }
        if let Some(assistant) = entry.get("assistant") {
            push_assistant_messages(assistant, &mut messages);
        }
    }
    messages
}

fn push_user_messages(user: &serde_json::Value, out: &mut Vec<AgentMessage>) {
    let Some(content) = user.get("content") else {
        return;
    };

    if let Some(prompt) = content
        .get("Prompt")
        .and_then(|p| p.get("prompt"))
        .and_then(|p| p.as_str())
    {
        if !prompt.is_empty() {
            out.push(AgentMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
                extras: serde_json::Value::Null,
            });
        }
    }

    let Some(tool_results) = content
        .get("ToolUseResults")
        .and_then(|t| t.get("tool_use_results"))
        .and_then(|t| t.as_array())
    else {
        return;
    };

    for tr in tool_results {
        let tool_content = tr
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("Text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if !tool_content.is_empty() {
            out.push(AgentMessage {
                role: "tool".to_string(),
                content: tool_content,
                extras: serde_json::Value::Null,
            });
        }
    }
}

fn push_assistant_messages(assistant: &serde_json::Value, out: &mut Vec<AgentMessage>) {
    if let Some(tool_use) = assistant.get("ToolUse") {
        let content = tool_use
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        if !content.is_empty() {
            out.push(AgentMessage {
                role: "assistant".to_string(),
                content: content.to_string(),
                extras: serde_json::Value::Null,
            });
        }
        if let Some(tools) = tool_use.get("tool_uses").and_then(|t| t.as_array()) {
            for tool in tools {
                let name = tool
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown");
                let args = tool
                    .get("args")
                    .map(|a| serde_json::to_string_pretty(a).unwrap_or_default())
                    .unwrap_or_default();
                out.push(AgentMessage {
                    role: "tool".to_string(),
                    content: format!("🔧 {} {}", name, args),
                    extras: serde_json::Value::Null,
                });
            }
        }
    }

    if let Some(response) = assistant.get("Response") {
        let content = response
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        if !content.is_empty() {
            out.push(AgentMessage {
                role: "assistant".to_string(),
                content: content.to_string(),
                extras: serde_json::Value::Null,
            });
        }
    }

    if let Some(msg) = assistant.get("Message") {
        let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if !content.is_empty() {
            out.push(AgentMessage {
                role: "assistant".to_string(),
                content: content.to_string(),
                extras: serde_json::Value::Null,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_title_handles_empty_transcript() {
        assert_eq!(extract_title("{}"), "Untitled");
        assert_eq!(extract_title(r#"{"transcript":[]}"#), "Untitled");
    }

    #[test]
    fn extract_title_clips_long_first_entry() {
        let v = serde_json::json!({"transcript": [
            "> ".to_string() + &"a".repeat(120)
        ]})
        .to_string();
        let title = extract_title(&v);
        assert!(title.ends_with("..."));
        // 80 chars + "..."
        assert_eq!(title.chars().count(), 83);
    }

    #[test]
    fn extract_title_strips_leading_caret_prefix() {
        let v = r#"{"transcript":["> hello world"]}"#;
        assert_eq!(extract_title(v), "hello world");
    }

    #[test]
    fn count_messages_returns_transcript_length() {
        assert_eq!(count_messages(r#"{"transcript":["a","b","c"]}"#), 3);
        assert_eq!(count_messages(r#"{}"#), 0);
        assert_eq!(count_messages("not json"), 0);
    }

    #[test]
    fn render_history_emits_user_then_tool_then_assistant() {
        let v = serde_json::json!({
            "history": [{
                "user": {
                    "content": {
                        "Prompt": { "prompt": "hello" }
                    }
                },
                "assistant": {
                    "Response": { "content": "hi back" }
                }
            }]
        });
        let msgs = render_history(&v);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "hello");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content, "hi back");
    }

    #[test]
    fn render_history_includes_tool_calls_with_args() {
        let v = serde_json::json!({
            "history": [{
                "assistant": {
                    "ToolUse": {
                        "content": "thinking...",
                        "tool_uses": [{
                            "name": "fs_read",
                            "args": { "path": "/etc/hosts" }
                        }]
                    }
                }
            }]
        });
        let msgs = render_history(&v);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "assistant");
        assert_eq!(msgs[0].content, "thinking...");
        assert_eq!(msgs[1].role, "tool");
        assert!(msgs[1].content.contains("fs_read"));
        assert!(msgs[1].content.contains("/etc/hosts"));
    }
}
