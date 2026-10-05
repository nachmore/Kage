//! Kiro CLI session provider — reads the on-disk SQLite database
//! kiro-cli writes to (`%LOCALAPPDATA%/kiro-cli/data.sqlite3` on Windows;
//! `~/.local/share/kiro-cli/data.sqlite3` elsewhere). All access is
//! read-only, opened with `SQLITE_OPEN_NO_MUTEX` to avoid disturbing the
//! CLI when it's writing concurrently.

use super::{
    clip_title, rfc3339_from_epoch_ms, AgentMessage, AgentSession, AgentSessionProvider,
    SessionLocator,
};
use crate::error::{AppError, ErrorKind};
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
        let db_path = Self::db_path().ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.dir_unavailable",
                &[("reason", "Kiro CLI database path could not be located")],
            )
        })?;
        rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.read_failed",
                &[("reason", &e.to_string())],
            )
        })
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
        let db_path = Self::db_path().ok_or_else(|| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.dir_unavailable",
                &[("reason", "Kiro CLI database path could not be located")],
            )
        })?;
        if !db_path.exists() {
            return Ok(Vec::new());
        }
        let db = Self::open_db()?;

        let rows = query_session_rows(&db, limit).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.read_failed",
                &[("reason", &e.to_string())],
            )
        })?;

        let db_path_str = db_path.to_string_lossy().to_string();
        let sessions = rows
            .into_iter()
            .map(|row| AgentSession {
                provider_id: PROVIDER_ID.to_string(),
                session_id: row.conversation_id.clone(),
                title: title_from_first_entry(row.first_entry.as_deref()),
                updated_at: rfc3339_from_epoch_ms(row.updated_at),
                message_count: row
                    .transcript_len
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0),
                container: Some(row.workspace.clone()),
                locator: json!({ "conversation_id": row.conversation_id }),
                extras: json!({
                    "workspace": row.workspace,
                    "file_path": db_path_str,
                }),
            })
            .collect();
        Ok(sessions)
    }

    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
        let loc: KiroCliLocator = serde_json::from_value(locator.clone()).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        let db = Self::open_db()?;

        let value_json: String = db
            .query_row(
                "SELECT value FROM conversations_v2 WHERE conversation_id = ?1",
                [&loc.conversation_id],
                |row| row.get(0),
            )
            .map_err(|e| {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.session.read_failed",
                    &[("reason", &e.to_string())],
                )
            })?;

        let json: serde_json::Value = serde_json::from_str(&value_json).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", &e.to_string())],
            )
        })?;

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
        let loc: KiroCliLocator = serde_json::from_value(locator.clone()).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.session.parse_failed",
                &[("reason", &e.to_string())],
            )
        })?;
        let db = Self::open_db()?;
        let current: i64 = db
            .query_row(
                "SELECT updated_at FROM conversations_v2 WHERE conversation_id = ?1",
                [&loc.conversation_id],
                |row| row.get(0),
            )
            .map_err(|e| {
                AppError::keyed(
                    ErrorKind::Internal,
                    "errors.session.read_failed",
                    &[("reason", &e.to_string())],
                )
            })?;
        if current > since_ms {
            Ok(Some(current))
        } else {
            Ok(None)
        }
    }
}

/// Listing query. `value` holds the whole conversation (history + tool
/// results, often MBs), so SQLite projects out just `transcript[0]` and
/// the transcript length instead of copying every blob into Rust. The
/// `json_valid` guards keep one malformed row from erroring the step
/// (which would drop every row after it); the `json_type` guard keeps a
/// non-string first entry from surfacing as raw JSON text. CASE only
/// evaluates the branch it takes, so the guards short-circuit. The JSON
/// functions are always present: rusqlite is built `bundled`.
const LIST_SESSIONS_SQL: &str = "SELECT key, conversation_id, \
     CASE WHEN json_valid(value) THEN \
         CASE WHEN json_type(value, '$.transcript[0]') = 'text' \
         THEN json_extract(value, '$.transcript[0]') END \
     END, \
     CASE WHEN json_valid(value) THEN json_array_length(value, '$.transcript') END, \
     updated_at \
     FROM conversations_v2 ORDER BY updated_at DESC LIMIT ?1";

struct SessionRow {
    workspace: String,
    conversation_id: String,
    /// `transcript[0]` when it is a string; None when missing, non-text,
    /// or the row's JSON is malformed.
    first_entry: Option<String>,
    /// `transcript` length; 0 when it is not an array, None when absent.
    transcript_len: Option<i64>,
    updated_at: i64,
}

fn query_session_rows(
    db: &rusqlite::Connection,
    limit: usize,
) -> rusqlite::Result<Vec<SessionRow>> {
    let mut stmt = db.prepare(LIST_SESSIONS_SQL)?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(SessionRow {
            workspace: row.get(0)?,
            conversation_id: row.get(1)?,
            first_entry: row.get(2)?,
            transcript_len: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    // A row that fails to map (e.g. NULL key) is skipped, not fatal.
    let rows: Vec<SessionRow> = rows.filter_map(Result::ok).collect();
    Ok(rows)
}

/// Title from the first transcript entry (already projected by SQLite).
fn title_from_first_entry(first: Option<&str>) -> String {
    let clean = first.unwrap_or("").trim().trim_start_matches('>').trim();
    if clean.is_empty() {
        "Untitled".to_string()
    } else {
        clip_title(clean, 80)
    }
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
    fn title_handles_missing_or_blank_first_entry() {
        assert_eq!(title_from_first_entry(None), "Untitled");
        assert_eq!(title_from_first_entry(Some("  > ")), "Untitled");
    }

    #[test]
    fn title_clips_long_first_entry() {
        let first = "> ".to_string() + &"a".repeat(120);
        let title = title_from_first_entry(Some(&first));
        assert!(title.ends_with("..."));
        // 80 chars + "..."
        assert_eq!(title.chars().count(), 83);
    }

    #[test]
    fn title_strips_leading_caret_prefix() {
        assert_eq!(title_from_first_entry(Some("> hello world")), "hello world");
    }

    /// The listing projection runs in SQLite; verify it tolerates the
    /// row shapes the old Rust-side parse tolerated (malformed JSON,
    /// missing/non-array transcript, non-string first entry) without
    /// dropping rows or erroring the query.
    #[test]
    fn query_session_rows_projects_and_tolerates_bad_rows() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE conversations_v2 (key TEXT, conversation_id TEXT, value TEXT, \
             created_at INTEGER, updated_at INTEGER);",
        )
        .unwrap();
        let rows: [(&str, &str, i64); 5] = [
            ("ok", r#"{"transcript":["> hi","b","c"]}"#, 5),
            ("bad", "not json", 4),
            ("empty", "{}", 3),
            ("obj", r#"{"transcript":[{"a":1}]}"#, 2),
            ("notarr", r#"{"transcript":"x"}"#, 1),
        ];
        for (id, value, updated) in rows {
            db.execute(
                "INSERT INTO conversations_v2 VALUES ('ws', ?1, ?2, 0, ?3)",
                rusqlite::params![id, value, updated],
            )
            .unwrap();
        }

        let got = query_session_rows(&db, 10).unwrap();
        let ids: Vec<&str> = got.iter().map(|r| r.conversation_id.as_str()).collect();
        assert_eq!(ids, ["ok", "bad", "empty", "obj", "notarr"]);

        assert_eq!(got[0].first_entry.as_deref(), Some("> hi"));
        assert_eq!(got[0].transcript_len, Some(3));
        assert_eq!(got[0].workspace, "ws");
        assert_eq!(got[0].updated_at, 5);
        for r in &got[1..] {
            assert_eq!(r.first_entry, None, "row {}", r.conversation_id);
        }
        assert_eq!(got[1].transcript_len, None);
        assert_eq!(got[2].transcript_len, None);
        assert_eq!(got[3].transcript_len, Some(1));
        assert_eq!(got[4].transcript_len, Some(0));

        assert_eq!(query_session_rows(&db, 2).unwrap().len(), 2);
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
