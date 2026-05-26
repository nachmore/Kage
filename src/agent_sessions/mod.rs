//! Agent session providers.
//!
//! Read-only views over the on-disk session state of various AI agent
//! backends (Kiro CLI's sqlite, Kage IDE's JSON files, eventually Claude
//! Code's jsonl, Codex, Ollama, ...). Each provider implements
//! `AgentSessionProvider` and registers in `AgentSessionRegistry`.
//!
//! The Tauri surface in `commands::agent_sessions` dispatches by
//! `provider_id`, so adding a new agent format is a single-impl change
//! plus a single registry line — no new commands, no frontend wiring
//! beyond rendering.
//!
//! Provider-specific *chrome* (open folder, list workspaces, delete file,
//! …) stays as typed Tauri commands rather than going through this trait.
//! The trait is for the listing/loading hot path; chrome is for the few
//! provider-specific UI affordances and is small enough not to be worth
//! genericizing.

pub mod kage_desktop;
pub mod kiro_cli;

use crate::error::AppError;
use serde::Serialize;
use std::sync::Arc;

/// Public-facing session metadata. Core fields (id, title, updated_at,
/// message_count) are the same for every provider; per-provider
/// differences live under `extras`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentSession {
    pub provider_id: String,
    pub session_id: String,
    pub title: String,
    /// rfc3339 string. Providers normalise their native timestamps
    /// (sqlite epoch ms, file mtime, jsonl tail, ...) to this format.
    pub updated_at: String,
    pub message_count: usize,
    /// Display label for "where does this session live" — workspace path,
    /// db path, project dir, etc. None when the provider has no useful
    /// container concept (e.g. kiro-cli is just one global db).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Opaque per-provider addressing token. Frontend stores it on the
    /// session item and passes it back to `load_session` /
    /// `check_session_updated` without inspecting it.
    pub locator: serde_json::Value,
    /// Provider-specific UI-relevant extras (model, branch,
    /// permission_mode, ...). Frontend reads optional fields by key.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extras: serde_json::Value,
}

/// One message in a loaded session. Like AgentSession, core fields are
/// uniform and provider-specific data lives under `extras`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentMessage {
    /// "user" | "assistant" | "tool" | "system".
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extras: serde_json::Value,
}

/// What the frontend sees in `agent_session_providers()` — enough to
/// render the source toggle and decide whether a provider is usable.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub label: String,
    pub available: bool,
}

/// Locator passed back to load/check_updated. Each provider deserializes
/// it into its own concrete shape.
pub type SessionLocator = serde_json::Value;

pub trait AgentSessionProvider: Send + Sync {
    /// Stable identifier — frontends store it on session items.
    fn id(&self) -> &'static str;
    /// Human-readable label for the source toggle.
    fn label(&self) -> &'static str;
    /// Cheap availability check (typically: does the on-disk root exist?).
    /// Called on every render of the source toggle, so must not block.
    fn is_available(&self) -> bool;

    /// List the most recent `limit` sessions, newest first.
    fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError>;

    /// Load all messages for a session identified by `locator`.
    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError>;

    /// Return the new `updated_at` epoch-ms if the session has changed
    /// since `since_ms`, otherwise `None`. Used by the frontend to poll
    /// for live updates while a CLI session is being edited externally.
    /// Default impl returns `None` — providers without a cheap mtime
    /// check can opt out.
    fn check_session_updated(
        &self,
        _locator: &SessionLocator,
        _since_ms: i64,
    ) -> Result<Option<i64>, AppError> {
        Ok(None)
    }
}

/// Runtime registry — process-singleton, stored in `FeatureServices`.
/// Constructed once at startup with a static set of providers; future
/// "discoverable" providers (e.g. detect-and-register at runtime) would
/// extend this with a registration API.
pub struct AgentSessionRegistry {
    providers: Vec<Arc<dyn AgentSessionProvider>>,
}

impl AgentSessionRegistry {
    pub fn new() -> Self {
        Self {
            providers: vec![
                Arc::new(kiro_cli::KiroCliProvider::new()),
                Arc::new(kage_desktop::KageDesktopProvider::new()),
            ],
        }
    }

    /// All registered providers, regardless of availability. The frontend
    /// asks for this and renders the source toggle, gating each entry by
    /// `available`.
    pub fn list_providers(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                label: p.label().to_string(),
                available: p.is_available(),
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn AgentSessionProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }
}

impl Default for AgentSessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper used by providers when titles need clipping for the session list.
pub fn clip_title(s: &str, max_chars: usize) -> String {
    let collected: String = s.chars().take(max_chars).collect();
    let cleaned = collected.replace(['\n', '\r'], " ");
    if s.chars().count() > max_chars {
        format!("{}...", cleaned.trim())
    } else {
        cleaned.trim().to_string()
    }
}

/// rfc3339 formatter for epoch-ms timestamps used by sqlite providers.
pub fn rfc3339_from_epoch_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp(ms / 1000, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// rfc3339 formatter for `SystemTime`s (file mtimes).
pub fn rfc3339_from_system_time(t: std::time::SystemTime) -> String {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// Look up a session's mtime in epoch ms, used by check_session_updated
/// callers that want a fast "has the file changed" signal.
#[allow(dead_code)]
pub fn file_mtime_ms(path: &std::path::Path) -> Option<i64> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?;
    let dur = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((dur.as_secs() as i64) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_known_providers() {
        let reg = AgentSessionRegistry::new();
        let ids: Vec<String> = reg.list_providers().into_iter().map(|p| p.id).collect();
        assert!(ids.contains(&"kiro-cli".to_string()));
        assert!(ids.contains(&"kage-desktop".to_string()));
    }

    #[test]
    fn registry_get_returns_none_for_unknown() {
        let reg = AgentSessionRegistry::new();
        assert!(reg.get("nope").is_none());
        assert!(reg.get("kiro-cli").is_some());
    }

    #[test]
    fn clip_title_trims_and_appends_ellipsis() {
        assert_eq!(clip_title("hello", 10), "hello");
        assert_eq!(clip_title("12345678901234567890", 5), "12345...");
        assert_eq!(clip_title("a\nb\rc", 10), "a b c");
    }
}
