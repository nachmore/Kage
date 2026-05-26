//! Claude Code session provider — reads JSONL session files at
//! `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
//!
//! The on-disk shape:
//!   - Each project (Claude Code working directory) gets one subdir
//!     under `~/.claude/projects/`. The dir name is the absolute cwd
//!     with `\`, `/`, `:`, `.` rewritten to `-` — irreversible, so
//!     we read the actual `cwd` field from inside each session for
//!     display.
//!   - Each session is one `.jsonl` file under that project dir,
//!     named `<sessionId>.jsonl`.
//!   - Project dirs may also contain a `memory/` directory (auto-memory
//!     stores) and `<sessionId>/` directories (subagent traces). We
//!     skip both — only `.jsonl` files are sessions.
//!
//! Line types we care about (others are skipped as chrome):
//!   - `user`: when `message.content` is a string, render as user
//!     message. When it's an array of `tool_result` blocks, render
//!     each as a tool message.
//!   - `assistant`: iterate `message.content`. `text` → assistant
//!     message. `tool_use` → tool message with name + input. `thinking`
//!     → skipped (the visible text is empty in Claude Code; only an
//!     encrypted signature is present).
//!
//! Caching matches `KageDesktopProvider`: per-file `(mtime, size)`
//! fingerprint, evicted on next-scan retain.

use super::{
    clip_title, file_mtime_ms, rfc3339_from_system_time, AgentMessage, AgentSession,
    AgentSessionProvider, SessionLocator,
};
use crate::error::AppError;
use crate::lock_ext::LockExt;
use log::info;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const PROVIDER_ID: &str = "claude-code";
const PROVIDER_LABEL: &str = "Claude Code";

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileFingerprint {
    mtime: SystemTime,
    size: u64,
}

fn fingerprint(md: &std::fs::Metadata) -> Option<FileFingerprint> {
    let mtime = md.modified().ok()?;
    Some(FileFingerprint {
        mtime,
        size: md.len(),
    })
}

#[derive(Clone)]
struct CachedSession {
    fp: FileFingerprint,
    session: AgentSession,
}

#[derive(Default)]
pub struct ClaudeCodeProvider {
    sessions: Mutex<HashMap<PathBuf, CachedSession>>,
}

impl ClaudeCodeProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve `~/.claude/projects/`. Identical layout on every OS
    /// (Claude Code uses the home directory regardless of platform).
    fn projects_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude").join("projects"))
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeCodeLocator {
    file_path: String,
}

impl AgentSessionProvider for ClaudeCodeProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn label(&self) -> &'static str {
        PROVIDER_LABEL
    }

    fn is_available(&self) -> bool {
        Self::projects_dir().map(|p| p.exists()).unwrap_or(false)
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError> {
        let base = Self::projects_dir()
            .ok_or_else(|| AppError::internal("Claude Code projects dir unresolvable"))?;
        if !base.exists() {
            return Ok(Vec::new());
        }

        // Walk every project subdir for `.jsonl` files. Skip nested
        // directories (memory/, <sessionId>/ for subagent traces).
        let mut seen_files: Vec<(PathBuf, FileFingerprint)> = Vec::new();
        let project_entries =
            std::fs::read_dir(&base).map_err(|e| AppError::internal(e.to_string()))?;
        for proj in project_entries.flatten() {
            if !proj.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(proj.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if !path.is_file() {
                    continue;
                }
                if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                    continue;
                }
                let Ok(md) = file.metadata() else { continue };
                let Some(fp) = fingerprint(&md) else { continue };
                seen_files.push((path, fp));
            }
        }

        let seen_keys: std::collections::HashSet<PathBuf> =
            seen_files.iter().map(|(p, _)| p.clone()).collect();
        let mut sessions: Vec<AgentSession> = Vec::with_capacity(seen_files.len());
        let mut misses: Vec<(PathBuf, FileFingerprint)> = Vec::new();
        {
            let mut guard = self.sessions.lock_or_recover();
            guard.retain(|k, _| seen_keys.contains(k));
            for (path, fp) in seen_files {
                match guard.get(&path) {
                    Some(cached) if cached.fp == fp => sessions.push(cached.session.clone()),
                    _ => misses.push((path, fp)),
                }
            }
        }

        let mut fresh: Vec<(PathBuf, CachedSession)> = Vec::with_capacity(misses.len());
        for (path, fp) in misses {
            let Some(session) = parse_session_metadata(&path) else {
                continue;
            };
            fresh.push((
                path,
                CachedSession {
                    fp,
                    session: session.clone(),
                },
            ));
            sessions.push(session);
        }
        if !fresh.is_empty() {
            let mut guard = self.sessions.lock_or_recover();
            for (path, entry) in fresh {
                guard.insert(path, entry);
            }
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions.truncate(limit);
        Ok(sessions)
    }

    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
        let loc: ClaudeCodeLocator = serde_json::from_value(locator.clone())
            .map_err(|e| AppError::internal(format!("claude-code locator: {}", e)))?;
        let path = PathBuf::from(&loc.file_path);
        let messages = parse_session_messages(&path)?;
        info!(
            "Loaded claude-code session {}: {} messages",
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("<unknown>"),
            messages.len()
        );
        Ok(messages)
    }

    fn check_session_updated(
        &self,
        locator: &SessionLocator,
        since_ms: i64,
    ) -> Result<Option<i64>, AppError> {
        let loc: ClaudeCodeLocator = serde_json::from_value(locator.clone())
            .map_err(|e| AppError::internal(format!("claude-code locator: {}", e)))?;
        let path = PathBuf::from(&loc.file_path);
        let Some(current) = file_mtime_ms(&path) else {
            return Ok(None);
        };
        if current > since_ms {
            Ok(Some(current))
        } else {
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Listing — extract title + cwd + message_count from a single jsonl pass
// ---------------------------------------------------------------------------

/// Read a session file, projecting it into an `AgentSession`. Returns
/// `None` if the file is empty/unreadable or has no real user content
/// (sessions that contain only tool_result + assistant lines are
/// recoverable but typically uninteresting; we still emit them with
/// "Untitled" rather than silently hiding them).
fn parse_session_metadata(path: &Path) -> Option<AgentSession> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let session_id = path.file_stem()?.to_str()?.to_string();
    let mut title: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut model: Option<String> = None;
    let mut user_count = 0usize;
    let mut assistant_count = 0usize;

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(o) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        // Cwd / gitBranch can appear on any line; capture the first.
        if cwd.is_none() {
            if let Some(c) = o.get("cwd").and_then(|v| v.as_str()) {
                cwd = Some(c.to_string());
            }
        }
        if git_branch.is_none() {
            if let Some(b) = o.get("gitBranch").and_then(|v| v.as_str()) {
                if !b.is_empty() {
                    git_branch = Some(b.to_string());
                }
            }
        }

        let kind = o.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "user" => {
                let content = o.get("message").and_then(|m| m.get("content"));
                if let Some(s) = content.and_then(|c| c.as_str()) {
                    user_count += 1;
                    if title.is_none() && !s.trim().is_empty() {
                        title = Some(clip_title(s.trim(), 80));
                    }
                } else if let Some(arr) = content.and_then(|c| c.as_array()) {
                    // tool_result-only user lines don't count as turns.
                    let has_text = arr
                        .iter()
                        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"));
                    if has_text {
                        user_count += 1;
                    }
                }
            }
            "assistant" => {
                assistant_count += 1;
                if model.is_none() {
                    if let Some(m) = o
                        .get("message")
                        .and_then(|msg| msg.get("model"))
                        .and_then(|v| v.as_str())
                    {
                        model = Some(m.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    let updated_at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(rfc3339_from_system_time)
        .unwrap_or_default();

    let title = title.unwrap_or_else(|| "Untitled".to_string());
    let message_count = user_count + assistant_count;

    let mut extras = json!({
        "file_path": path.to_string_lossy(),
    });
    let extras_obj = extras.as_object_mut().expect("just built");
    if let Some(c) = cwd.clone() {
        extras_obj.insert("cwd".to_string(), serde_json::Value::String(c));
    }
    if let Some(b) = git_branch {
        extras_obj.insert("git_branch".to_string(), serde_json::Value::String(b));
    }
    if let Some(m) = model {
        extras_obj.insert("model".to_string(), serde_json::Value::String(m));
    }

    Some(AgentSession {
        provider_id: PROVIDER_ID.to_string(),
        session_id,
        title,
        updated_at,
        message_count,
        container: cwd,
        locator: json!({ "file_path": path.to_string_lossy() }),
        extras,
    })
}

// ---------------------------------------------------------------------------
// Loading — render every line into AgentMessages
// ---------------------------------------------------------------------------

fn parse_session_messages(path: &Path) -> Result<Vec<AgentMessage>, AppError> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).map_err(|e| AppError::internal(format!("Open: {}", e)))?;
    let reader = BufReader::new(file);

    let mut out: Vec<AgentMessage> = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(o) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = o.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match kind {
            "user" => render_user(&o, &mut out),
            "assistant" => render_assistant(&o, &mut out),
            // Skip chrome: permission-mode, file-history-snapshot,
            // last-prompt, attachment, system. None of these are
            // user-visible turns; surfacing them inline in v1 would
            // bury the actual conversation under metadata.
            _ => {}
        }
    }
    Ok(out)
}

fn render_user(o: &serde_json::Value, out: &mut Vec<AgentMessage>) {
    let content = match o.get("message").and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return,
    };

    if let Some(s) = content.as_str() {
        if !s.trim().is_empty() {
            out.push(AgentMessage {
                role: "user".to_string(),
                content: s.to_string(),
                extras: serde_json::Value::Null,
            });
        }
        return;
    }

    let Some(arr) = content.as_array() else {
        return;
    };

    for block in arr {
        let Some(t) = block.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        match t {
            "text" => {
                if let Some(s) = block.get("text").and_then(|v| v.as_str()) {
                    if !s.trim().is_empty() {
                        out.push(AgentMessage {
                            role: "user".to_string(),
                            content: s.to_string(),
                            extras: serde_json::Value::Null,
                        });
                    }
                }
            }
            "tool_result" => {
                let result_content = block.get("content");
                let text = stringify_tool_result(result_content);
                if !text.trim().is_empty() {
                    out.push(AgentMessage {
                        role: "tool".to_string(),
                        content: text,
                        extras: serde_json::Value::Null,
                    });
                }
            }
            // image / other block types: skip in v1
            _ => {}
        }
    }
}

fn render_assistant(o: &serde_json::Value, out: &mut Vec<AgentMessage>) {
    let Some(arr) = o
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return;
    };

    for block in arr {
        let Some(t) = block.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        match t {
            "text" => {
                if let Some(s) = block.get("text").and_then(|v| v.as_str()) {
                    if !s.trim().is_empty() {
                        out.push(AgentMessage {
                            role: "assistant".to_string(),
                            content: s.to_string(),
                            extras: serde_json::Value::Null,
                        });
                    }
                }
            }
            "tool_use" => {
                let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let input = block
                    .get("input")
                    .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
                    .unwrap_or_default();
                out.push(AgentMessage {
                    role: "tool".to_string(),
                    content: format!("🔧 {} {}", name, input),
                    extras: serde_json::Value::Null,
                });
            }
            // thinking blocks are always empty visible text in Claude
            // Code (only the encrypted signature is present), so we
            // skip them rather than rendering empty bubbles.
            _ => {}
        }
    }
}

/// Render a `tool_result.content` value to plain text. The shape varies
/// by tool: it can be a string, or an array of `{type: "text", text}`
/// blocks (matching the Anthropic API content-block convention).
fn stringify_tool_result(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| {
                let t = b.get("type").and_then(|v| v.as_str())?;
                if t == "text" {
                    b.get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_user_handles_string_content() {
        let v = json!({
            "type": "user",
            "message": { "role": "user", "content": "hello world" }
        });
        let mut out = Vec::new();
        render_user(&v, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "user");
        assert_eq!(out[0].content, "hello world");
    }

    #[test]
    fn render_user_promotes_tool_result_blocks_to_tool_role() {
        let v = json!({
            "type": "user",
            "message": {
                "content": [
                    { "type": "tool_result", "content": "some output" }
                ]
            }
        });
        let mut out = Vec::new();
        render_user(&v, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "tool");
        assert_eq!(out[0].content, "some output");
    }

    #[test]
    fn render_user_handles_text_block_arrays() {
        let v = json!({
            "type": "user",
            "message": {
                "content": [
                    { "type": "text", "text": "first" },
                    { "type": "text", "text": "second" }
                ]
            }
        });
        let mut out = Vec::new();
        render_user(&v, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "first");
        assert_eq!(out[1].content, "second");
    }

    #[test]
    fn render_assistant_skips_thinking_blocks() {
        // Thinking blocks are always empty visible text; rendering them
        // as empty assistant messages would clutter the viewer.
        let v = json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "thinking", "thinking": "", "signature": "sig" },
                    { "type": "text", "text": "actual answer" }
                ]
            }
        });
        let mut out = Vec::new();
        render_assistant(&v, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "assistant");
        assert_eq!(out[0].content, "actual answer");
    }

    #[test]
    fn render_assistant_emits_tool_use_with_input() {
        let v = json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "tool_use", "name": "Read", "input": { "path": "/etc/hosts" } }
                ]
            }
        });
        let mut out = Vec::new();
        render_assistant(&v, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "tool");
        assert!(out[0].content.contains("Read"));
        assert!(out[0].content.contains("/etc/hosts"));
    }

    #[test]
    fn stringify_tool_result_handles_text_block_arrays() {
        let v = json!([
            { "type": "text", "text": "line 1" },
            { "type": "image", "source": "..." },
            { "type": "text", "text": "line 2" }
        ]);
        assert_eq!(stringify_tool_result(Some(&v)), "line 1\nline 2");
    }

    #[test]
    fn stringify_tool_result_passes_through_strings() {
        let v = json!("plain output");
        assert_eq!(stringify_tool_result(Some(&v)), "plain output");
    }
}
