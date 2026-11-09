//! This computer's sync settings, `<data_local>/kage/sync/device.json`
//! (docs/design/SYNC.md §6.1). They live outside `config.json`, because an
//! older Kage drops unknown `config.json` fields when it saves, which would
//! silently turn sync off.
//!
//! C0 skeleton: the types only. WP-S3 reads and writes the file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
