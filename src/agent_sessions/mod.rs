//! Agent session providers.
//!
//! Read-only views over the on-disk session state of various AI agent
//! backends (Kiro CLI's sqlite, Kiro IDE's JSON files, eventually Claude
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

pub mod claude_code;
pub mod kiro_cli;
pub mod kiro_desktop;

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
                hide_throwaway(kiro_cli::KiroCliProvider::new()),
                hide_throwaway(kiro_desktop::KiroDesktopProvider::new()),
                hide_throwaway(claude_code::ClaudeCodeProvider::new()),
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

/// Register `provider` so its listing never shows one of Kage's throwaway
/// sessions. The agent may keep their history in its own store, which
/// Kage can't delete; see `ephemeral_session::is_hidden`.
fn hide_throwaway(provider: impl AgentSessionProvider + 'static) -> Arc<dyn AgentSessionProvider> {
    Arc::new(HideThrowaway(Box::new(provider)))
}

/// A provider whose `list_sessions` skips Kage's throwaway sessions.
struct HideThrowaway(Box<dyn AgentSessionProvider>);

impl AgentSessionProvider for HideThrowaway {
    fn id(&self) -> &'static str {
        self.0.id()
    }

    fn label(&self) -> &'static str {
        self.0.label()
    }

    fn is_available(&self) -> bool {
        self.0.is_available()
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError> {
        list_visible(&*self.0, limit, &crate::ephemeral_session::hidden_ids())
    }

    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
        self.0.load_session(locator)
    }

    fn check_session_updated(
        &self,
        locator: &SessionLocator,
        since_ms: i64,
    ) -> Result<Option<i64>, AppError> {
        self.0.check_session_updated(locator, since_ms)
    }
}

/// The newest `limit` sessions of `provider` whose id isn't in `hidden`.
///
/// Asks for `limit` first. When hidden sessions took some of those places
/// and the provider had more, asks again for more (at least twice as many,
/// at most `limit + hidden.len()`). Usually no hidden session is among the
/// newest, so one call is enough.
fn list_visible(
    provider: &dyn AgentSessionProvider,
    limit: usize,
    hidden: &std::collections::HashSet<String>,
) -> Result<Vec<AgentSession>, AppError> {
    if hidden.is_empty() {
        return provider.list_sessions(limit);
    }
    let most = limit.saturating_add(hidden.len());
    let mut ask = limit;
    loop {
        let listed = provider.list_sessions(ask)?;
        let got = listed.len();
        let mut visible: Vec<AgentSession> = listed
            .into_iter()
            .filter(|s| !hidden.contains(&s.session_id))
            .collect();
        let skipped = got - visible.len();
        // At least double, so even a page of hidden sessions takes only a
        // few calls.
        let next = limit
            .saturating_add(skipped)
            .max(ask.saturating_mul(2))
            .min(most);
        if visible.len() >= limit || got < ask || next <= ask {
            visible.truncate(limit);
            return Ok(visible);
        }
        ask = next;
    }
}

/// A user message's own text: without the extension tool steering Kage puts
/// ahead of it when it drives the agent (`crate::state::strip_tool_steering`),
/// which isn't the user's and is hidden in Kage's own transcript too. Empty
/// when the text was only that.
pub fn user_text(text: &str) -> &str {
    crate::state::strip_tool_steering(text)
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

/// Per-file `(mtime, size)` fingerprint used by the file-backed providers
/// (claude_code, kiro_desktop) to detect when a cached parse is stale.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct FileFingerprint {
    pub mtime: std::time::SystemTime,
    pub size: u64,
}

/// Build a fingerprint from filesystem metadata. `None` if the platform
/// can't report mtime.
pub fn fingerprint(md: &std::fs::Metadata) -> Option<FileFingerprint> {
    let mtime = md.modified().ok()?;
    Some(FileFingerprint {
        mtime,
        size: md.len(),
    })
}

/// A parsed session cached alongside the fingerprint it was parsed from.
/// File-backed providers keep a `HashMap<PathBuf, CachedSession>` and
/// reuse the parse when the on-disk fingerprint is unchanged.
#[derive(Clone)]
pub struct CachedSession {
    pub fp: FileFingerprint,
    pub session: AgentSession,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_known_providers() {
        let reg = AgentSessionRegistry::new();
        let ids: Vec<String> = reg.list_providers().into_iter().map(|p| p.id).collect();
        assert!(ids.contains(&"kiro-cli".to_string()));
        assert!(ids.contains(&"kiro-desktop".to_string()));
        assert!(ids.contains(&"claude-code".to_string()));
    }

    #[test]
    fn registry_get_returns_none_for_unknown() {
        let reg = AgentSessionRegistry::new();
        assert!(reg.get("nope").is_none());
        assert!(reg.get("kiro-cli").is_some());
    }

    /// A provider listing `ids` newest first, recording each `limit` asked.
    struct FakeProvider {
        ids: Vec<&'static str>,
        asked: std::sync::Mutex<Vec<usize>>,
    }

    impl FakeProvider {
        fn new(ids: &[&'static str]) -> Self {
            Self {
                ids: ids.to_vec(),
                asked: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<usize> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl AgentSessionProvider for FakeProvider {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn label(&self) -> &'static str {
            "Fake"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError> {
            self.asked.lock().unwrap().push(limit);
            Ok(self
                .ids
                .iter()
                .take(limit)
                .map(|id| AgentSession {
                    provider_id: "fake".into(),
                    session_id: id.to_string(),
                    title: String::new(),
                    updated_at: String::new(),
                    message_count: 0,
                    container: None,
                    locator: serde_json::Value::Null,
                    extras: serde_json::Value::Null,
                })
                .collect())
        }
        fn load_session(&self, _: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
            Ok(Vec::new())
        }
    }

    fn ids(sessions: &[AgentSession]) -> Vec<&str> {
        sessions.iter().map(|s| s.session_id.as_str()).collect()
    }

    fn hidden(ids: &[&str]) -> std::collections::HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn listing_skips_hidden_sessions_and_still_fills_the_page() {
        let p = FakeProvider::new(&["a", "h1", "b", "h2", "c", "d", "e"]);
        let got = list_visible(&p, 3, &hidden(&["h1", "h2", "elsewhere"])).unwrap();
        assert_eq!(ids(&got), ["a", "b", "c"]);
        assert_eq!(p.asked(), [3, 6], "asked again, for twice as many");
    }

    #[test]
    fn listing_asks_once_when_no_hidden_session_is_among_the_newest() {
        let p = FakeProvider::new(&["a", "b", "c", "h1"]);
        let got = list_visible(&p, 3, &hidden(&["h1"])).unwrap();
        assert_eq!(ids(&got), ["a", "b", "c"]);
        assert_eq!(p.asked(), [3]);

        let p = FakeProvider::new(&["a", "b"]);
        assert_eq!(ids(&list_visible(&p, 3, &hidden(&[])).unwrap()), ["a", "b"]);
        assert_eq!(p.asked(), [3]);
    }

    #[test]
    fn listing_stops_when_the_provider_runs_out() {
        let p = FakeProvider::new(&["h1", "a", "h2"]);
        let got = list_visible(&p, 5, &hidden(&["h1", "h2"])).unwrap();
        assert_eq!(ids(&got), ["a"]);
        assert_eq!(p.asked(), [5]);
    }

    #[test]
    fn listing_never_asks_for_more_than_the_hidden_ids_could_take() {
        let p = FakeProvider::new(&["h1", "h2", "h3", "a", "b", "c"]);
        let got = list_visible(&p, 1, &hidden(&["h1", "h2", "h3"])).unwrap();
        assert_eq!(ids(&got), ["a"]);
        assert!(p.asked().iter().all(|&n| n <= 4), "{:?}", p.asked());
    }

    #[test]
    fn registered_providers_hide_throwaway_sessions() {
        // Every registered provider goes through the filter, which reads
        // `ephemeral_session::hidden_ids`.
        let wrapped = hide_throwaway(FakeProvider::new(&["a"]));
        assert_eq!(wrapped.id(), "fake");
        assert_eq!(ids(&wrapped.list_sessions(5).unwrap()), ["a"]);
    }

    #[test]
    fn user_text_drops_kage_tool_steering() {
        let block = format!(
            "{}\n<extension_tools>\nquotes\n</extension_tools>\n</_kage_tool_steering>",
            crate::state::TOOL_STEERING_TAG
        );
        assert_eq!(user_text(&block), "");
        assert_eq!(
            user_text(&format!("{block}\nwhat's AAPL at?")),
            "what's AAPL at?"
        );
        assert_eq!(user_text("plain"), "plain");
    }

    #[test]
    fn clip_title_trims_and_appends_ellipsis() {
        assert_eq!(clip_title("hello", 10), "hello");
        assert_eq!(clip_title("12345678901234567890", 5), "12345...");
        assert_eq!(clip_title("a\nb\rc", 10), "a b c");
    }
}
