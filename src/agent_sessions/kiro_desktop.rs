//! Kiro Desktop (IDE) session provider.
//!
//! Kiro stores workspace JSON sessions and historical `.chat` files under
//! one globalStorage directory. The provider merges those sources while
//! caching parsed metadata by file fingerprint.

mod chat;
mod content;
mod workspace;

use super::{
    file_mtime_ms, AgentMessage, AgentSession, AgentSessionProvider, CachedSession, SessionLocator,
};
use crate::error::{AppError, ErrorKind};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub use workspace::{list_workspaces, KiroDesktopWorkspace};

const PROVIDER_ID: &str = "kiro-desktop";
const PROVIDER_LABEL: &str = "Kiro IDE & CLI";

#[derive(Default)]
pub struct KiroDesktopProvider {
    workspace_sessions: Mutex<HashMap<PathBuf, CachedSession>>,
    chat_files: Mutex<HashMap<PathBuf, CachedSession>>,
}

impl KiroDesktopProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn data_dir() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            dirs::config_dir().map(|d| {
                d.join("Kiro")
                    .join("User")
                    .join("globalStorage")
                    .join("kiro.kiroagent")
            })
        }
        #[cfg(target_os = "macos")]
        {
            dirs::home_dir().map(|d| {
                d.join("Library")
                    .join("Application Support")
                    .join("Kiro")
                    .join("User")
                    .join("globalStorage")
                    .join("kiro.kiroagent")
            })
        }
        #[cfg(target_os = "linux")]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|d| d.join(".config")))
                .map(|d| {
                    d.join("Kiro")
                        .join("User")
                        .join("globalStorage")
                        .join("kiro.kiroagent")
                })
        }
    }
}

/// Locator shapes differentiated by the frontend's `kind` discriminator.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum KiroDesktopLocator {
    #[serde(rename = "workspace_session")]
    WorkspaceSession {
        workspace_encoded: String,
        session_id: String,
    },
    #[serde(rename = "chat_file")]
    ChatFile { file_path: String },
}

impl AgentSessionProvider for KiroDesktopProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn label(&self) -> &'static str {
        PROVIDER_LABEL
    }

    fn is_available(&self) -> bool {
        Self::data_dir().map(|d| d.exists()).unwrap_or(false)
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError> {
        let base = Self::data_dir().ok_or_else(dir_unavailable)?;
        if !base.exists() {
            return Ok(Vec::new());
        }

        let mut sessions =
            workspace::scan_sessions(self, &base.join("workspace-sessions"), None, limit * 2)?;
        sessions.extend(chat::scan_sessions(self, &base, limit * 2)?);
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions.truncate(limit);
        Ok(sessions)
    }

    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
        match parse_locator(locator)? {
            KiroDesktopLocator::WorkspaceSession {
                workspace_encoded,
                session_id,
            } => workspace::load_session(&workspace_encoded, &session_id),
            KiroDesktopLocator::ChatFile { file_path } => chat::load_file(&file_path),
        }
    }

    fn check_session_updated(
        &self,
        locator: &SessionLocator,
        since_ms: i64,
    ) -> Result<Option<i64>, AppError> {
        let path = match parse_locator(locator)? {
            KiroDesktopLocator::WorkspaceSession {
                workspace_encoded,
                session_id,
            } => Self::data_dir()
                .ok_or_else(dir_unavailable)?
                .join("workspace-sessions")
                .join(workspace_encoded)
                .join(format!("{session_id}.json")),
            KiroDesktopLocator::ChatFile { file_path } => PathBuf::from(file_path),
        };

        Ok(file_mtime_ms(&path).filter(|current| *current > since_ms))
    }
}

fn parse_locator(locator: &SessionLocator) -> Result<KiroDesktopLocator, AppError> {
    serde_json::from_value(locator.clone()).map_err(|error| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.session.parse_failed",
            &[("reason", &error.to_string())],
        )
    })
}

fn dir_unavailable() -> AppError {
    AppError::keyed(
        ErrorKind::Internal,
        "errors.session.dir_unavailable",
        &[("reason", "Kiro Desktop data directory could not be located")],
    )
}

#[cfg(test)]
mod tests {
    use super::content::{extract_text_content, extract_user_text_from_chat, is_system_message};

    #[test]
    fn extract_user_text_strips_user_rule_block() {
        let s = "<user-rule>blah</user-rule>\n\nactual prompt";
        assert_eq!(extract_user_text_from_chat(s), "actual prompt");
    }

    #[test]
    fn extract_user_text_returns_empty_for_pure_steering() {
        assert_eq!(extract_user_text_from_chat("## Included Rules\nfoo"), "");
        assert_eq!(extract_user_text_from_chat("<identity>x</identity>"), "");
    }

    #[test]
    fn extract_user_text_strips_environment_context_suffix() {
        let s = "real text<EnvironmentContext>os=linux</EnvironmentContext>";
        assert_eq!(extract_user_text_from_chat(s), "real text");
    }

    #[test]
    fn is_system_message_only_flags_obvious_prompts() {
        assert!(is_system_message("<identity>foo"));
        assert!(!is_system_message("hello"));
        assert!(!is_system_message("Follow these instructions: do X"));
    }

    #[test]
    fn extract_text_content_handles_array_of_text_blocks() {
        let value = serde_json::json!([
            {"type": "text", "text": "hello"},
            {"type": "image", "url": "..."},
            {"type": "text", "text": "world"},
        ]);
        assert_eq!(extract_text_content(Some(&value)), "hello\nworld");
    }
}
