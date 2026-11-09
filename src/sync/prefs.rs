//! This computer's sync settings, `<data_local>/kage/sync/device.json`
//! (docs/design/SYNC.md §6.1). They live outside `config.json`, because an
//! older Kage drops unknown `config.json` fields when it saves, which would
//! silently turn sync off.

use super::types::Category;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePrefs {
    #[serde(default)]
    pub v: u32,
    #[serde(default)]
    pub enabled: bool,
    /// The parent folder the user picked.
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    /// §8.9: tells a copied profile from the computer it came from.
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub previous_device_ids: Vec<String>,
    #[serde(default)]
    pub categories: SyncCategories,
    /// Extensions whose data sync is off on this computer.
    #[serde(default)]
    pub ext_data_off: Vec<String>,
    /// RFC 3339, or `forever`.
    #[serde(default)]
    pub paused_until: Option<String>,
    /// `keychain`, `credential_manager`, `secret_service` or `file`.
    #[serde(default)]
    pub key_store: Option<String>,
    #[serde(default)]
    pub key_epoch: u32,
    #[serde(default)]
    pub joined_at: Option<String>,
    /// Guard key → SHA-256 of the runnable part that was approved here.
    #[serde(default)]
    pub approved_runnables: BTreeMap<String, String>,
    /// `<kind>:<id>` the user said not to suggest again (§4.5).
    #[serde(default)]
    pub install_exclusions: Vec<String>,
}

/// Which categories sync on this computer (§4.1). Chat history is off by
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCategories {
    #[serde(default = "default_true")]
    pub settings: bool,
    #[serde(default = "default_true")]
    pub automations: bool,
    #[serde(default = "default_true")]
    pub extensions: bool,
    #[serde(default)]
    pub chats: bool,
}

impl Default for SyncCategories {
    fn default() -> Self {
        Self {
            settings: true,
            automations: true,
            extensions: true,
            chats: false,
        }
    }
}

/// The file in `<data_local>/kage/sync/`.
pub const FILE: &str = "device.json";

/// The version this build writes.
pub const PREFS_VERSION: u32 = 1;

/// `paused_until` for a pause with no end.
pub const FOREVER: &str = "forever";

impl DevicePrefs {
    /// Read `device.json` from `dir`. A missing file is "sync is off". A
    /// file that doesn't parse is treated as off too (§15); it is left as
    /// it is, so nothing is lost, and the next save replaces it.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<DevicePrefs>(&bytes) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("sync: device.json doesn't parse ({e}); treating sync as off");
                    DevicePrefs::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DevicePrefs::default(),
            Err(e) => {
                log::warn!("sync: device.json can't be read ({e}); treating sync as off");
                DevicePrefs::default()
            }
        }
    }

    /// Write `device.json` into `dir`, owner-only and atomically.
    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut p = self.clone();
        p.v = PREFS_VERSION;
        let mut json = serde_json::to_vec_pretty(&p)?;
        json.push(b'\n');
        crate::atomic_write::write(&dir.join(FILE), json, crate::atomic_write::Perms::Private)?;
        Ok(())
    }

    /// Whether `category` syncs on this computer. Sync's own bookkeeping
    /// always does.
    pub fn category_on(&self, category: Category) -> bool {
        match category {
            Category::Settings => self.categories.settings,
            Category::Automations => self.categories.automations,
            Category::Extensions => self.categories.extensions,
            Category::Chats => self.categories.chats,
            Category::System => true,
        }
    }

    pub fn set_category(&mut self, category: Category, on: bool) {
        match category {
            Category::Settings => self.categories.settings = on,
            Category::Automations => self.categories.automations = on,
            Category::Extensions => self.categories.extensions = on,
            Category::Chats => self.categories.chats = on,
            Category::System => {}
        }
    }

    /// How many of the four categories are on, for telemetry.
    pub fn categories_on(&self) -> usize {
        [
            self.categories.settings,
            self.categories.automations,
            self.categories.extensions,
            self.categories.chats,
        ]
        .iter()
        .filter(|c| **c)
        .count()
    }

    /// Whether sync is paused at `now_ms`. A pause whose time has passed
    /// is over; one that doesn't parse counts as over.
    pub fn paused_at(&self, now_ms: i64) -> bool {
        match self.paused_until.as_deref() {
            None => false,
            Some(FOREVER) => true,
            Some(t) => chrono::DateTime::parse_from_rfc3339(t)
                .map(|t| t.timestamp_millis() > now_ms)
                .unwrap_or(false),
        }
    }

    /// Whether this extension's data sync is off on this computer.
    pub fn ext_data_is_off(&self, ext: &str) -> bool {
        self.ext_data_off.iter().any(|e| e == ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_broken_file_reads_as_off_and_save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(DevicePrefs::load(dir.path()), DevicePrefs::default());
        std::fs::write(dir.path().join(FILE), b"{ not json").unwrap();
        assert!(!DevicePrefs::load(dir.path()).enabled);

        let mut p = DevicePrefs {
            enabled: true,
            folder_id: Some("5qg2vbn8h1xk0d3m".into()),
            ..Default::default()
        };
        p.approved_runnables
            .insert("list.macros/s1".into(), "ab".into());
        p.set_category(Category::Chats, true);
        p.save(dir.path()).unwrap();
        let back = DevicePrefs::load(dir.path());
        assert_eq!(back.v, PREFS_VERSION);
        assert!(back.enabled && back.categories.chats);
        assert_eq!(back.approved_runnables.len(), 1);
        assert_eq!(back.categories_on(), 4);
        assert!(back.category_on(Category::System));
    }

    #[test]
    fn pauses_end() {
        let mut p = DevicePrefs::default();
        assert!(!p.paused_at(0));
        p.paused_until = Some(FOREVER.into());
        assert!(p.paused_at(i64::MAX));
        p.paused_until = Some("2026-11-09T10:00:00Z".into());
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-09T10:00:00Z")
            .unwrap()
            .timestamp_millis();
        assert!(p.paused_at(at - 1));
        assert!(!p.paused_at(at));
        p.paused_until = Some("soon".into());
        assert!(!p.paused_at(0));
    }
}
