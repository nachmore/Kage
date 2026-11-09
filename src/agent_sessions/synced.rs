//! Chats from the user's other computers, read-only (docs/design/SYNC.md
//! §4.6). Folder sync writes them as mirrors under
//! `<data_local>/kage/sync/chats/<device>/`; this provider lists them like
//! any agent's sessions, under "On your other computers". They can't be
//! resumed here: this computer's agent never had them. **Continue here**
//! starts a new chat with the transcript attached.

use super::{AgentMessage, AgentSession, AgentSessionProvider, SessionLocator};
use crate::error::AppError;
use std::path::PathBuf;

/// The provider id the history uses for mirrored chats.
pub const PROVIDER_ID: &str = "synced";

/// Mirrors of other computers' chats.
pub struct SyncedChatsProvider {
    /// `<data_local>/kage/sync/chats`, or `None` when it can't be resolved.
    dir: Option<PathBuf>,
}

impl SyncedChatsProvider {
    pub fn new() -> Self {
        Self {
            dir: crate::paths::data_local_dir().map(|d| {
                d.join("kage")
                    .join("sync")
                    .join(crate::sync::sources::chats::MIRRORS)
            }),
        }
    }

    /// A provider over `dir`, for tests.
    pub fn at(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }
}

impl Default for SyncedChatsProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339()
}

impl AgentSessionProvider for SyncedChatsProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn label(&self) -> &'static str {
        "On your other computers"
    }

    fn is_available(&self) -> bool {
        self.dir.as_ref().is_some_and(|d| d.is_dir())
    }

    fn list_sessions(&self, limit: usize) -> Result<Vec<AgentSession>, AppError> {
        let Some(dir) = &self.dir else {
            return Ok(Vec::new());
        };
        Ok(crate::sync::sources::chats::list_mirrors(dir)
            .into_iter()
            .take(limit)
            .map(|(path, m)| AgentSession {
                provider_id: PROVIDER_ID.to_string(),
                session_id: m.id.clone(),
                title: m.title.clone(),
                updated_at: rfc3339(m.updated),
                message_count: m.messages as usize,
                container: None,
                locator: serde_json::json!({ "path": path, "id": m.id }),
                extras: serde_json::json!({
                    "read_only": true,
                    "origin_provider": m.provider,
                    "device": m.device,
                }),
            })
            .collect())
    }

    fn load_session(&self, locator: &SessionLocator) -> Result<Vec<AgentMessage>, AppError> {
        let Some(dir) = &self.dir else {
            return Ok(Vec::new());
        };
        let path = locator
            .get("path")
            .and_then(|p| p.as_str())
            .map(PathBuf::from)
            .ok_or_else(|| AppError::internal("a synced chat without a path"))?;
        // Only mirrors sync wrote: never a path a locator made up.
        if !path.starts_with(dir) || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            return Err(AppError::internal("not a synced chat"));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| AppError::internal(format!("reading a synced chat failed: {e}")))?;
        Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .map(|v| AgentMessage {
                role: v
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("assistant")
                    .to_string(),
                content: v
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string(),
                extras: v.get("extras").cloned().unwrap_or(serde_json::Value::Null),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::sources::chats::{mirror_key, MirrorMeta};

    #[test]
    fn lists_and_loads_mirrors_but_nothing_outside_them() {
        let root = tempfile::tempdir().unwrap();
        let dev = root.path().join("a9x3h6t1qq8m0ve2");
        std::fs::create_dir_all(&dev).unwrap();
        let id = "a9x3h6t1qq8m0ve2:kiro-cli:s1";
        let key = mirror_key(id);
        let meta = MirrorMeta {
            id: id.into(),
            device: "a9x3h6t1qq8m0ve2".into(),
            provider: "kiro-cli".into(),
            session_id: "s1".into(),
            title: "Plan".into(),
            created: 1,
            updated: 2,
            messages: 1,
            bytes: 10,
        };
        std::fs::write(
            dev.join(format!("{key}.json")),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dev.join(format!("{key}.jsonl")),
            "{\"role\":\"user\",\"content\":\"hi\"}\n",
        )
        .unwrap();
        let p = SyncedChatsProvider::at(root.path().to_path_buf());
        assert!(p.is_available());
        let list = p.list_sessions(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Plan");
        let msgs = p.load_session(&list[0].locator).unwrap();
        assert_eq!(msgs[0].content, "hi");
        let outside = serde_json::json!({ "path": "/etc/passwd" });
        assert!(p.load_session(&outside).is_err());
    }
}
