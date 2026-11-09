//! The `chat.sessions` namespace: Kage's own read-only copies of this
//! computer's chats, off by default (docs/design/SYNC.md §4.6).
//!
//! - Record `<origin device>:<provider>:<session id>`, fields `title`,
//!   `created`, `updated`, `messages`, `bytes` and `blob` (the transcript:
//!   the normalised messages). Only the origin computer reports its chats,
//!   once each has been idle for 10 minutes, from the last 90 days, newest
//!   first within 200 MB, none over 8 MiB, never a throwaway or hidden
//!   session.
//! - Other computers' chats are written as mirrors under
//!   `<data_local>/kage/sync/chats/<device>/`: `<key>.json` (the fields
//!   without the transcript) and `<key>.jsonl` (one message per line).
//!   `agent_sessions::synced` lists them read-only. A mirror's title can be
//!   changed (a rename syncs); deleting a mirror hides the chat on every
//!   computer. The agent's own files are never written.
//! - A transcript is only loaded when its chat changed since it was last
//!   reported; otherwise the field reads [`super::unchanged`].

use super::{
    is_unchanged, unchanged, LocalRecord, LocalState, LocalWrite, NsSpec, ProjectResult, Source,
};
use crate::sync::prefs::DevicePrefs;
use crate::sync::types::{Category, DefaultScope, Origin, Structure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const NS: &str = "chat.sessions";
/// The transcript field.
pub const BLOB: &str = "blob";
/// The mirrors' folder inside `<data_local>/kage/sync/`.
pub const MIRRORS: &str = "chats";
/// Own chats renamed on another computer: record id → title.
const TITLES: &str = "titles.json";
/// Chats deleted on another computer: hidden everywhere.
const HIDDEN: &str = "hidden.json";

/// Chats from the last this many days are copied.
pub const BACKFILL_DAYS: i64 = 90;
/// A chat is copied once it has been idle this long.
pub const IDLE_MS: i64 = 10 * 60 * 1000;
/// Largest transcript copied.
pub const MAX_CHAT_BYTES: usize = 8 << 20;
/// Most transcript bytes this computer copies, newest first.
pub const BUDGET_BYTES: usize = 200_000_000;
/// Most chats listed per provider.
const LIST_LIMIT: usize = 2000;

/// One of this computer's chats, as the history lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMeta {
    pub provider: String,
    pub session_id: String,
    pub title: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// What [`ChatHistory::load`] needs to find it again.
    pub locator: Value,
}

/// This computer's chat history, so tests can stand in for the agents'
/// stores.
pub trait ChatHistory: Send + Sync {
    /// This computer's chats, newest first, without throwaway sessions.
    fn list(&self) -> Vec<ChatMeta>;
    /// One chat's messages, normalised (`{ role, content, extras? }`).
    fn load(&self, chat: &ChatMeta) -> anyhow::Result<Vec<Value>>;
}

/// The agents' stores through [`crate::agent_sessions::AgentSessionRegistry`],
/// which works for every agent format. Mirrors of other computers' chats
/// are never listed as this computer's.
pub struct RegistryHistory(pub Arc<crate::agent_sessions::AgentSessionRegistry>);

impl ChatHistory for RegistryHistory {
    fn list(&self) -> Vec<ChatMeta> {
        let mut out = Vec::new();
        for info in self.0.list_providers() {
            if !info.available || info.id == crate::agent_sessions::synced::PROVIDER_ID {
                continue;
            }
            let Some(p) = self.0.get(&info.id) else {
                continue;
            };
            let Ok(sessions) = p.list_sessions(LIST_LIMIT) else {
                continue;
            };
            for s in sessions {
                if crate::ephemeral_session::is_active(&s.session_id)
                    || crate::ephemeral_session::is_hidden(&s.session_id)
                {
                    continue;
                }
                let updated = chrono::DateTime::parse_from_rfc3339(&s.updated_at)
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0);
                out.push(ChatMeta {
                    provider: s.provider_id.clone(),
                    session_id: s.session_id.clone(),
                    title: s.title.clone(),
                    created_ms: updated,
                    updated_ms: updated,
                    locator: s.locator.clone(),
                });
            }
        }
        out.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        out
    }

    fn load(&self, chat: &ChatMeta) -> anyhow::Result<Vec<Value>> {
        let p = self
            .0
            .get(&chat.provider)
            .ok_or_else(|| anyhow::anyhow!("no such chat provider"))?;
        let messages = p
            .load_session(&chat.locator)
            .map_err(|e| anyhow::anyhow!("loading a chat failed: {e}"))?;
        Ok(messages
            .into_iter()
            .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
            .collect())
    }
}

/// A mirror's metadata file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MirrorMeta {
    pub id: String,
    pub device: String,
    pub provider: String,
    pub session_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub updated: i64,
    #[serde(default)]
    pub messages: u64,
    #[serde(default)]
    pub bytes: u64,
}

/// The file stem of a mirror: record ids hold `:`, which Windows file
/// names can't.
pub fn mirror_key(id: &str) -> String {
    hex::encode(&Sha256::digest(id.as_bytes())[..8])
}

/// Kage's copies of this computer's chats, and mirrors of the others'.
pub struct ChatsSource {
    history: Arc<dyn ChatHistory>,
    /// `<data_local>/kage/sync`.
    dir: PathBuf,
    /// Chats whose transcript was last reported at this `updated`.
    reported: HashMap<String, i64>,
    now: fn() -> i64,
}

fn system_now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl ChatsSource {
    pub fn new(history: Arc<dyn ChatHistory>, dir: PathBuf) -> Self {
        Self {
            history,
            dir,
            reported: HashMap::new(),
            now: system_now,
        }
    }

    /// The same, with a clock, for tests.
    #[doc(hidden)]
    pub fn with_clock(history: Arc<dyn ChatHistory>, dir: PathBuf, now: fn() -> i64) -> Self {
        Self {
            now,
            ..Self::new(history, dir)
        }
    }

    fn mirrors(&self) -> PathBuf {
        self.dir.join(MIRRORS)
    }

    fn read_map(&self, name: &str) -> BTreeMap<String, String> {
        std::fs::read(self.mirrors().join(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write_map(&self, name: &str, m: &BTreeMap<String, String>) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.mirrors())?;
        crate::atomic_write::write(
            &self.mirrors().join(name),
            serde_json::to_vec_pretty(m)?,
            crate::atomic_write::Perms::Private,
        )?;
        Ok(())
    }

    fn me(&self) -> Option<String> {
        DevicePrefs::load(&self.dir).device_id
    }

    fn own(&mut self, me: &str, st: &mut LocalState) {
        let now = (self.now)();
        let titles = self.read_map(TITLES);
        let hidden: BTreeSet<String> = self.read_map(HIDDEN).into_keys().collect();
        let mut budget = BUDGET_BYTES;
        for chat in self.history.list() {
            if chat.updated_ms < now - BACKFILL_DAYS * 86_400_000 || chat.updated_ms > now - IDLE_MS
            {
                continue;
            }
            let id = format!("{me}:{}:{}", chat.provider, chat.session_id);
            if hidden.contains(&id) {
                continue;
            }
            let mut fields = Map::new();
            let title = titles
                .get(&id)
                .cloned()
                .unwrap_or_else(|| chat.title.clone());
            fields.insert("title".into(), Value::String(title));
            fields.insert("created".into(), Value::from(chat.created_ms));
            fields.insert("updated".into(), Value::from(chat.updated_ms));
            if self.reported.get(&id) == Some(&chat.updated_ms) {
                fields.insert(BLOB.into(), unchanged());
                fields.insert("messages".into(), unchanged());
                fields.insert("bytes".into(), unchanged());
            } else {
                let Ok(messages) = self.history.load(&chat) else {
                    continue;
                };
                let transcript = Value::Array(messages);
                let bytes = serde_json::to_vec(&transcript)
                    .map(|b| b.len())
                    .unwrap_or(0);
                if bytes > MAX_CHAT_BYTES || bytes > budget {
                    continue;
                }
                budget -= bytes;
                fields.insert(
                    "messages".into(),
                    Value::from(transcript.as_array().map_or(0, |a| a.len())),
                );
                fields.insert("bytes".into(), Value::from(bytes));
                fields.insert(BLOB.into(), transcript);
                self.reported.insert(id.clone(), chat.updated_ms);
            }
            st.put(
                NS,
                &id,
                LocalRecord {
                    fields,
                    avail: None,
                },
            );
        }
    }

    fn mirrored(&self, me: &str, st: &mut LocalState) {
        let Ok(devs) = std::fs::read_dir(self.mirrors()) else {
            return;
        };
        for dev in devs.flatten() {
            if !dev.path().is_dir() || dev.file_name().to_string_lossy() == me {
                continue;
            }
            let Ok(files) = std::fs::read_dir(dev.path()) else {
                continue;
            };
            for f in files.flatten() {
                let p = f.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Some(meta) = std::fs::read(&p)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<MirrorMeta>(&b).ok())
                else {
                    continue;
                };
                let mut fields = Map::new();
                fields.insert("title".into(), Value::String(meta.title.clone()));
                fields.insert("created".into(), Value::from(meta.created));
                fields.insert("updated".into(), Value::from(meta.updated));
                fields.insert("messages".into(), Value::from(meta.messages));
                fields.insert("bytes".into(), Value::from(meta.bytes));
                fields.insert(BLOB.into(), unchanged());
                st.put(
                    NS,
                    &meta.id,
                    LocalRecord {
                        fields,
                        avail: None,
                    },
                );
            }
        }
    }

    fn write_mirror(&self, id: &str, rec: &LocalRecord) -> anyhow::Result<()> {
        let mut parts = id.splitn(3, ':');
        let (Some(dev), Some(provider), Some(session)) = (parts.next(), parts.next(), parts.next())
        else {
            return Ok(());
        };
        if !crate::sync::crypto::is_id_text(dev) {
            return Ok(());
        }
        let dir = self.mirrors().join(dev);
        std::fs::create_dir_all(&dir)?;
        let key = mirror_key(id);
        let num = |k: &str| rec.fields.get(k).and_then(Value::as_i64).unwrap_or(0);
        let meta = MirrorMeta {
            id: id.to_string(),
            device: dev.to_string(),
            provider: provider.to_string(),
            session_id: session.to_string(),
            title: rec
                .fields
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            created: num("created"),
            updated: num("updated"),
            messages: num("messages").max(0) as u64,
            bytes: num("bytes").max(0) as u64,
        };
        if let Some(Value::Array(messages)) = rec.fields.get(BLOB) {
            let mut lines = Vec::new();
            for m in messages {
                lines.extend(serde_json::to_vec(m)?);
                lines.push(b'\n');
            }
            crate::atomic_write::write(
                &dir.join(format!("{key}.jsonl")),
                lines,
                crate::atomic_write::Perms::Private,
            )?;
        }
        crate::atomic_write::write(
            &dir.join(format!("{key}.json")),
            serde_json::to_vec_pretty(&meta)?,
            crate::atomic_write::Perms::Private,
        )?;
        Ok(())
    }

    fn remove_mirror(&self, id: &str) {
        let Some(dev) = id.split(':').next() else {
            return;
        };
        let dir = self.mirrors().join(dev);
        let key = mirror_key(id);
        for ext in ["json", "jsonl"] {
            let _ = std::fs::remove_file(dir.join(format!("{key}.{ext}")));
        }
    }
}

/// Every mirrored chat under `dir` (`<data_local>/kage/sync/chats`), for the
/// read-only history provider.
pub fn list_mirrors(dir: &Path) -> Vec<(PathBuf, MirrorMeta)> {
    let mut out = Vec::new();
    let Ok(devs) = std::fs::read_dir(dir) else {
        return out;
    };
    for dev in devs.flatten() {
        let Ok(files) = std::fs::read_dir(dev.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(meta) = std::fs::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice::<MirrorMeta>(&b).ok())
            {
                out.push((p.with_extension("jsonl"), meta));
            }
        }
    }
    out.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
    out
}

impl Source for ChatsSource {
    fn id(&self) -> &'static str {
        "chats"
    }

    fn specs(&self) -> Vec<NsSpec> {
        vec![NsSpec {
            ns: NS.to_string(),
            category: Category::Chats,
            schema: 1,
            // Only the origin publishes a chat (§4.6).
            echo: false,
            structure: Structure::default(),
            default_scope: DefaultScope::Shared,
            large: true,
            label_field: Some("title".to_string()),
        }]
    }

    fn read_local(&mut self) -> anyhow::Result<LocalState> {
        let mut st = LocalState::default();
        let Some(me) = self.me() else {
            return Ok(st);
        };
        self.own(&me, &mut st);
        self.mirrored(&me, &mut st);
        Ok(st)
    }

    fn project(
        &mut self,
        writes: Vec<LocalWrite>,
        _origin: Origin,
    ) -> anyhow::Result<ProjectResult> {
        let me = self.me().unwrap_or_default();
        let mut titles = self.read_map(TITLES);
        let mut hidden = self.read_map(HIDDEN);
        let (t0, h0) = (titles.clone(), hidden.clone());
        for w in writes.iter().filter(|w| w.ns == NS) {
            let own = !me.is_empty() && w.id.starts_with(&format!("{me}:"));
            match (&w.value, own) {
                (Some(rec), true) => {
                    if let Some(Value::String(t)) = rec.fields.get("title") {
                        titles.insert(w.id.clone(), t.clone());
                    }
                }
                // Deleted on another computer: hidden here too. The
                // agent's files stay.
                (None, true) => {
                    hidden.insert(w.id.clone(), String::new());
                }
                (Some(rec), false) => {
                    let mut rec = rec.clone();
                    if rec.fields.get(BLOB).is_some_and(is_unchanged) {
                        rec.fields.remove(BLOB);
                    }
                    self.write_mirror(&w.id, &rec)?;
                }
                (None, false) => self.remove_mirror(&w.id),
            }
        }
        if titles != t0 {
            self.write_map(TITLES, &titles)?;
        }
        if hidden != h0 {
            self.write_map(HIDDEN, &hidden)?;
        }
        Ok(ProjectResult::Applied {
            read_back: self.read_local()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    struct History(Mutex<Vec<ChatMeta>>);

    impl ChatHistory for History {
        fn list(&self) -> Vec<ChatMeta> {
            self.0.lock().unwrap().clone()
        }
        fn load(&self, chat: &ChatMeta) -> anyhow::Result<Vec<Value>> {
            Ok(vec![json!({ "role": "user", "content": chat.title })])
        }
    }

    const NOW: i64 = 1_762_686_000_000;

    fn now() -> i64 {
        NOW
    }

    fn chat(id: &str, updated: i64) -> ChatMeta {
        ChatMeta {
            provider: "kiro-cli".into(),
            session_id: id.into(),
            title: format!("chat {id}"),
            created_ms: updated,
            updated_ms: updated,
            locator: json!({}),
        }
    }

    #[test]
    fn own_chats_are_reported_once_idle_and_transcripts_only_when_changed() {
        let dir = tempfile::tempdir().unwrap();
        DevicePrefs {
            device_id: Some("k7m2p9xqa3rt5vwe".into()),
            ..Default::default()
        }
        .save(dir.path())
        .unwrap();
        let history = Arc::new(History(Mutex::new(vec![
            chat("busy", NOW - 60_000),
            chat("idle", NOW - IDLE_MS - 1),
            chat("old", NOW - 100 * 86_400_000),
        ])));
        let mut src = ChatsSource::with_clock(history, dir.path().to_path_buf(), now);
        let st = src.read_local().unwrap();
        let ids: Vec<&str> = st.records.keys().map(|(_, id)| id.as_str()).collect();
        assert_eq!(ids, vec!["k7m2p9xqa3rt5vwe:kiro-cli:idle"]);
        let rec = st.get(NS, ids[0]).unwrap();
        assert!(rec.fields[BLOB].is_array());
        let again = src.read_local().unwrap();
        assert!(is_unchanged(&again.get(NS, ids[0]).unwrap().fields[BLOB]));
    }

    #[test]
    fn other_computers_chats_become_mirrors_and_a_deleted_mirror_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        DevicePrefs {
            device_id: Some("k7m2p9xqa3rt5vwe".into()),
            ..Default::default()
        }
        .save(dir.path())
        .unwrap();
        let mut src = ChatsSource::with_clock(
            Arc::new(History(Mutex::new(vec![]))),
            dir.path().to_path_buf(),
            now,
        );
        let id = "a9x3h6t1qq8m0ve2:claude-code:s1";
        let mut fields = Map::new();
        fields.insert("title".into(), json!("Plan"));
        fields.insert("updated".into(), json!(NOW));
        fields.insert("messages".into(), json!(1));
        fields.insert(BLOB.into(), json!([{ "role": "user", "content": "hi" }]));
        let ProjectResult::Applied { read_back } = src
            .project(
                vec![LocalWrite {
                    ns: NS.into(),
                    id: id.into(),
                    value: Some(LocalRecord {
                        fields,
                        avail: None,
                    }),
                }],
                Origin::Sync,
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            read_back.get(NS, id).unwrap().fields["title"],
            json!("Plan")
        );
        let mirrors = list_mirrors(&dir.path().join(MIRRORS));
        assert_eq!(mirrors.len(), 1);
        assert!(mirrors[0].0.exists());
        src.project(
            vec![LocalWrite {
                ns: NS.into(),
                id: id.into(),
                value: None,
            }],
            Origin::Sync,
        )
        .unwrap();
        assert!(src.read_local().unwrap().get(NS, id).is_none());
    }
}
