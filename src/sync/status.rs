//! What the engine reports to the frontend (docs/design/SYNC.md §19.4,
//! §19.5): the status, scope views, conflict summaries, reviews, devices
//! and the join preview. Every type here is serialised as the commands
//! return it.
//!
//! C0 skeleton: the shapes. WP-S3 fills them in.

use super::keystore::KeyStoreKind;
use super::prefs::SyncCategories;
use super::types::{Category, ConflictKind, OsKind, PinReason};
use crate::os::cloud_files::Provider;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One state, one status line (§13.4). `Unavailable` is sync not built or
/// not offered on this build; `Off` is offered but not set up here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Off,
    #[default]
    Unavailable,
    SettingUp,
    UpToDate,
    Syncing,
    Waiting,
    Placeholders,
    Paused,
    Locked,
    FolderMissing,
    FolderDenied,
    FolderEmpty,
    Reset,
    Removed,
    UpdateRequired,
    DiskFull,
    WriteFailed,
    FolderReverting,
    OtherEngine,
}

/// The sync folder as Settings shows it: `Dropbox › Kage Sync`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderView {
    pub display: String,
    pub provider: Provider,
}

/// How another computer is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Ok,
    Stale,
    Inactive,
    NeedsUpdate,
    Retired,
}

/// One computer in "Your computers" (`sync_list_devices`, `SyncStatus`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub os: OsKind,
    pub kage_version: String,
    pub last_seen_at: i64,
    /// This computer.
    pub this: bool,
    pub state: DeviceState,
}

/// A problem shown beside the state, such as `clock_warning`. The window
/// renders `key` with `params`; Rust never renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub kind: String,
    pub key: String,
    pub params: BTreeMap<String, String>,
}

/// `sync_get_status` and the `sync_status_changed` payload.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SyncStatus {
    pub state: SyncState,
    pub folder: Option<FolderView>,
    pub last_sync_at: Option<i64>,
    pub last_publish_at: Option<i64>,
    /// RFC 3339, or `forever`.
    pub paused_until: Option<String>,
    pub pending_local: u32,
    pub waiting_files: u32,
    pub categories: SyncCategories,
    pub devices: Vec<DeviceView>,
    pub conflicts_open: u32,
    pub held: u32,
    pub reviews: u32,
    pub suggestions: u32,
    pub problems: Vec<Problem>,
    pub key_store: Option<KeyStoreKind>,
    pub undo_join_until: Option<String>,
    pub used_bytes: u64,
}

impl SyncStatus {
    /// The status while sync isn't built or offered: Settings hides the
    /// Sync item.
    pub fn unavailable() -> Self {
        Self::default()
    }
}

/// A catalog row's rule, as the where-it-applies control names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeRule {
    Shared,
    PerOs,
    ThisComputer,
    Never,
}

/// Where the value on this computer applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    All,
    Os,
    ThisComputer,
    NotSynced,
}

/// Another OS's own value, as a preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtherOs {
    pub os: OsKind,
    pub preview: String,
}

/// One catalog path's where-it-applies state (§9.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeView {
    pub synced: bool,
    pub rule: ScopeRule,
    pub applies: Applies,
    /// Why it isn't synced: a `settings.sync_scope.reason.*` key.
    pub reason_key: Option<String>,
    /// Why it is held here, when it is.
    pub held: Option<PinReason>,
    /// The values per scope: `all`, `macos`, `windows`, `linux`.
    pub values: BTreeMap<String, serde_json::Value>,
    pub others: Vec<OtherOs>,
}

/// An entry's state, the same on every computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictState {
    Open,
    Dismissed,
    Restored,
}

/// The replaced version in a conflict summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoserView {
    pub computer: String,
    pub at: i64,
    pub deleted: bool,
    /// Text, at most 200 characters.
    pub preview: String,
}

/// The current version in a conflict summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinnerView {
    pub computer: String,
    pub at: i64,
}

/// One row of the conflicts list (`sync_list_conflicts`). Labels are built
/// at read time from `label_key` and `label_params`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictSummary {
    pub id: String,
    pub kind: ConflictKind,
    pub category: Category,
    pub label_key: String,
    pub label_params: BTreeMap<String, String>,
    pub loser: LoserView,
    pub winner: Option<WinnerView>,
    pub state: ConflictState,
    /// Entries made by one action (a join, a restored backup) share a group.
    pub group: Option<String>,
}

/// One field of **Compare**: both versions as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDiff {
    pub field: String,
    pub now: Option<String>,
    pub replaced: Option<String>,
}

/// `sync_conflict_detail`: the summary plus the fields side by side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictDetail {
    #[serde(flatten)]
    pub summary: ConflictSummary,
    pub fields: Vec<FieldDiff>,
}

/// One item that waits for the user. `id` is what the matching command
/// takes: the guard key for runnables, `kind:id` for suggestions and
/// removals, the namespace for mass changes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReviewItem {
    pub kind: String,
    pub id: String,
    pub label_key: String,
    pub label_params: BTreeMap<String, String>,
    pub computer: Option<String>,
    /// What runs, for runnables: the command or script, shown LTR.
    pub detail: Option<String>,
    pub count: Option<u64>,
}

/// `sync_list_reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reviews {
    pub runnables: Vec<ReviewItem>,
    pub suggestions: Vec<ReviewItem>,
    pub removals: Vec<ReviewItem>,
    pub mass_changes: Vec<ReviewItem>,
    pub approvals: Vec<ReviewItem>,
}

/// A computer already in the folder, once the passphrase unlocked it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinComputer {
    pub name: String,
    pub os: OsKind,
    pub last_seen_at: i64,
}

/// Counts per category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CategoryCounts {
    pub settings: u32,
    pub automations: u32,
    pub extensions: u32,
    pub chats: u32,
}

/// `sync_join`: what joining would change, before the user picks a side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinPreview {
    pub computers: Vec<JoinComputer>,
    pub same: CategoryCounts,
    pub different: CategoryCounts,
    pub only_here: CategoryCounts,
    pub only_synced: CategoryCounts,
}
