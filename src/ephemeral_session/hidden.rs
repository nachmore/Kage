//! The ids of Kage's throwaway sessions, kept so no Kage session list
//! shows them.
//!
//! The cleanup guard deletes a throwaway session's files from Kage's
//! sessions folder. Some agents also keep their own history elsewhere (a
//! database, or per-project transcript files) that Kage can't delete. So
//! every throwaway session id is recorded in
//! `<data_local_dir>/kage/ephemeral-sessions.json`, and the session
//! scanner and the `agent_sessions` providers skip those ids.
//!
//! The id is recorded as soon as the session exists, before any prompt,
//! so a crash during the work still leaves it hidden. The file keeps at
//! most [`MAX_IDS`] ids for [`KEEP_DAYS`] days. It is device-local: never
//! synced and never in a backup.

use chrono::{DateTime, Utc};
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// File name under `<data_local_dir>/kage/`.
pub const FILE_NAME: &str = "ephemeral-sessions.json";

/// Most ids kept. The oldest go first.
pub const MAX_IDS: usize = 1000;

/// Days an id is kept.
pub const KEEP_DAYS: i64 = 30;

#[derive(Debug, Default, Serialize, Deserialize)]
struct HiddenFile {
    #[serde(default)]
    sessions: Vec<HiddenEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HiddenEntry {
    #[serde(default)]
    id: String,
    /// RFC 3339 time the session was created.
    #[serde(default)]
    at: String,
}

/// The recorded ids, in memory, backed by the file at `path`.
#[derive(Debug)]
pub struct HiddenSessions {
    path: Option<PathBuf>,
    /// Oldest first.
    entries: Vec<HiddenEntry>,
    ids: HashSet<String>,
}

/// Where the file lives for this user.
pub fn default_path() -> Option<PathBuf> {
    crate::paths::data_local_dir().map(|d| d.join("kage").join(FILE_NAME))
}

impl HiddenSessions {
    /// Read the file at `path`. A missing file is an empty list. A file
    /// that can't be read or parsed is logged and treated as empty; the
    /// next [`record`](Self::record) replaces it.
    pub fn load(path: Option<PathBuf>, now: DateTime<Utc>) -> Self {
        let entries = path.as_deref().map(read_entries).unwrap_or_default();
        let mut store = Self {
            path,
            entries,
            ids: HashSet::new(),
        };
        store.prune(now);
        store
    }

    /// Whether `id` is a recorded throwaway session.
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Every recorded id.
    pub fn ids(&self) -> HashSet<String> {
        self.ids.clone()
    }

    /// Record `id` and write the file. A failed write is logged; the id
    /// stays hidden for the rest of this run.
    pub fn record(&mut self, id: &str, now: DateTime<Utc>) {
        if id.is_empty() {
            return;
        }
        self.entries.retain(|e| e.id != id);
        self.entries.push(HiddenEntry {
            id: id.to_string(),
            at: now.to_rfc3339(),
        });
        self.prune(now);
        self.save();
    }

    /// Drop ids older than [`KEEP_DAYS`] (or with no readable time), keep
    /// the newest [`MAX_IDS`], and rebuild the lookup set.
    fn prune(&mut self, now: DateTime<Utc>) {
        let oldest = now - chrono::Duration::days(KEEP_DAYS);
        self.entries.retain(|e| {
            !e.id.is_empty()
                && DateTime::parse_from_rfc3339(&e.at)
                    .is_ok_and(|at| at.with_timezone(&Utc) >= oldest)
        });
        if self.entries.len() > MAX_IDS {
            let extra = self.entries.len() - MAX_IDS;
            self.entries.drain(..extra);
        }
        self.ids = self.entries.iter().map(|e| e.id.clone()).collect();
    }

    fn save(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if let Err(e) = write_entries(path, &self.entries) {
            warn!("Failed to save the throwaway session list: {}", e);
        }
    }
}

fn read_entries(path: &Path) -> Vec<HiddenEntry> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warn!("Failed to read the throwaway session list: {}", e);
            return Vec::new();
        }
    };
    match serde_json::from_str::<HiddenFile>(&text) {
        Ok(file) => file.sessions,
        Err(e) => {
            warn!(
                "The throwaway session list is not valid JSON ({}); starting a new one",
                e
            );
            Vec::new()
        }
    }
}

fn write_entries(path: &Path, entries: &[HiddenEntry]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = HiddenFile {
        sessions: entries.to_vec(),
    };
    let json = serde_json::to_vec_pretty(&file)?;
    crate::atomic_write::write(path, json, crate::atomic_write::Perms::Private)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn recorded_ids_survive_a_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("kage").join(FILE_NAME);
        let now = at("2026-11-09T10:00:00Z");
        let mut store = HiddenSessions::load(Some(path.clone()), now);
        assert!(!store.contains("a"));
        store.record("a", now);
        store.record("b", now);
        assert!(store.contains("a"));

        let reloaded = HiddenSessions::load(Some(path), now);
        assert!(reloaded.contains("a"));
        assert!(reloaded.contains("b"));
        assert!(!reloaded.contains("c"));
        assert_eq!(reloaded.ids().len(), 2);
    }

    #[test]
    fn ids_older_than_thirty_days_are_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let mut store = HiddenSessions::load(Some(path.clone()), at("2026-10-01T00:00:00Z"));
        store.record("old", at("2026-10-01T00:00:00Z"));
        store.record("recent", at("2026-11-01T00:00:00Z"));

        let later = HiddenSessions::load(Some(path), at("2026-11-02T00:00:00Z"));
        assert!(!later.contains("old"), "31 days old");
        assert!(later.contains("recent"));
    }

    #[test]
    fn only_the_newest_thousand_ids_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let now = at("2026-11-09T10:00:00Z");
        // In memory first (each write is synced to disk), then one write.
        let mut store = HiddenSessions::load(None, now);
        for i in 0..(MAX_IDS + 4) {
            store.record(&format!("s{i}"), now);
        }
        store.path = Some(path.clone());
        store.record(&format!("s{}", MAX_IDS + 4), now);
        let reloaded = HiddenSessions::load(Some(path), now);
        assert_eq!(reloaded.ids().len(), MAX_IDS);
        assert!(!reloaded.contains("s0"));
        assert!(!reloaded.contains("s4"));
        assert!(reloaded.contains("s5"));
        assert!(reloaded.contains(&format!("s{}", MAX_IDS + 4)));
    }

    #[test]
    fn recording_an_id_twice_keeps_one_entry() {
        let now = at("2026-11-09T10:00:00Z");
        let mut store = HiddenSessions::load(None, now);
        store.record("a", now);
        store.record("a", now);
        assert_eq!(store.entries.len(), 1);
        store.record("", now);
        assert_eq!(store.entries.len(), 1, "an empty id is never recorded");
    }

    #[test]
    fn a_corrupt_file_starts_an_empty_list_and_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        std::fs::write(&path, b"{ not json").unwrap();
        let now = at("2026-11-09T10:00:00Z");
        let mut store = HiddenSessions::load(Some(path.clone()), now);
        assert!(store.ids().is_empty());
        store.record("a", now);
        let reloaded = HiddenSessions::load(Some(path), now);
        assert!(reloaded.contains("a"));
    }

    #[test]
    fn entries_without_a_readable_time_are_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        std::fs::write(
            &path,
            br#"{"sessions":[{"id":"no-time"},{"id":"bad","at":"yesterday"},{"id":"ok","at":"2026-11-09T09:00:00Z"},{"at":"2026-11-09T09:00:00Z"}]}"#,
        )
        .unwrap();
        let store = HiddenSessions::load(Some(path), at("2026-11-09T10:00:00Z"));
        assert_eq!(store.ids(), HashSet::from(["ok".to_string()]));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let now = at("2026-11-09T10:00:00Z");
        HiddenSessions::load(Some(path.clone()), now).record("a", now);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
