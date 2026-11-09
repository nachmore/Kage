//! Folder sync (docs/design/SYNC.md): Kage syncs settings, shortcuts and
//! automations, extensions and their data, and optionally chat history,
//! through a folder the user already syncs. Each computer writes only its
//! own encrypted, write-once files; every computer merges them the same way.
//!
//! - [`engine`]: one tick captures, ingests, merges, projects and
//!   publishes, on the `kage-sync` thread ([`scheduler`]).
//! - [`sources`]: the parts of Kage that sync plugs into.
//! - [`types`], [`merge`], [`replica`]: the data model and its merge.
//! - [`folder`], [`envelope`], [`crypto`], [`keystore`]: the folder.
//!
//! Sync is offered only where [`sync_available`] says so: debug builds and
//! the dev channel, until the owner turns it on for everyone.

pub mod canonical;
pub mod catalog;
pub mod conflicts;
pub mod crypto;
pub mod engine;
pub mod envelope;
pub mod folder;
pub mod fsio;
pub mod gc;
pub mod guards;
pub mod hlc;
pub mod identity;
pub mod ingest;
pub mod keystore;
pub mod membership;
pub mod merge;
pub mod prefs;
pub mod publish;
pub mod replica;
pub mod scheduler;
pub mod scope;
pub mod sources;
pub mod split;
pub mod status;
#[doc(hidden)]
pub mod testing;
pub mod types;

#[allow(unused_imports)] // The app starts using them with WP-S4.
pub use scheduler::{install_global, notify, SyncHandle};

use crate::error::{AppError, ErrorKind};
use crate::os::cloud_files::SuggestedFolder;
use folder::FolderInfo;
use prefs::SyncCategories;
use serde::{Deserialize, Serialize};
use status::{
    ConflictDetail, ConflictSummary, DeviceView, JoinPreview, Reviews, ScopeView, SyncStatus,
};
use std::collections::HashMap;
use std::path::PathBuf;
use types::{Category, OsKind, ScopeAction};

/// Sync is offered to everyone. Until the owner flips this, only debug
/// builds and the dev channel get it (docs/design/SYNC.md §20, WP-S4).
pub const RELEASED: bool = false;

/// Whether this build offers folder sync: a released feature, a debug
/// build, or a dev-channel build (`+dev.` in the version).
pub fn sync_available() -> bool {
    RELEASED || cfg!(debug_assertions) || env!("CARGO_PKG_VERSION").contains("+dev.")
}

/// Sync is off, locked or paused on this computer, or not in this build.
pub fn unavailable() -> AppError {
    AppError::keyed(ErrorKind::SyncUnavailable, "errors.sync.unavailable", &[])
}

/// A destructive command came without a valid token from
/// `sync_prepare_destructive`.
pub fn needs_confirmation() -> AppError {
    AppError::keyed(
        ErrorKind::NeedsConfirmation,
        "errors.sync.needs_confirmation",
        &[],
    )
}

/// An extension's synced data is over its limit (§12.7).
pub fn quota_exceeded() -> AppError {
    AppError::keyed(ErrorKind::QuotaExceeded, "errors.sync.quota_exceeded", &[])
}

/// `sync_create`: make a new sync folder from this computer.
pub struct CreateReq {
    pub path: PathBuf,
    pub passphrase: String,
    pub device_name: String,
    pub categories: SyncCategories,
}

/// `sync_join`: join the sync folder other computers use.
pub struct JoinReq {
    pub path: PathBuf,
    pub passphrase: String,
    pub device_name: String,
    pub categories: SyncCategories,
}

/// Which side wins where this computer and the folder differ (§8.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinChoice {
    Theirs,
    Mine,
}

/// The tabs of the conflicts screen (§10.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictTab {
    Replaced,
    Deleted,
    Resolved,
}

/// What to do with one conflicts-list entry (§10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictAction {
    Restore,
    Dismiss,
    DeleteAnyway,
}

/// The commands that need a token from `sync_prepare_destructive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructiveAction {
    Leave,
    StartOver,
    RemoveDevice,
    PurgeExtension,
    UndoJoin,
}

/// Everything the app asks the engine thread.
pub enum EngineRequest {
    Status,
    SuggestFolders,
    InspectFolder(PathBuf),
    GeneratePassphrase,
    Create(CreateReq),
    Join(JoinReq),
    ConfirmJoin(JoinChoice),
    Unlock {
        passphrase: String,
    },
    SetPaused {
        until: Option<String>,
    },
    SyncNow,
    SetCategory {
        category: Category,
        on: bool,
    },
    SetExtensionData {
        ext: String,
        on: bool,
    },
    GetScopes(Option<Vec<String>>),
    SetScope {
        path: String,
        action: ScopeAction,
    },
    ListConflicts(ConflictTab),
    ConflictDetail(String),
    ResolveConflict {
        id: String,
        action: ConflictAction,
    },
    DismissAll(Option<String>),
    ListReviews,
    ApproveRunnable(String),
    KeepOff(String),
    DismissSuggestion {
        kind: String,
        id: String,
    },
    AnswerMassChange {
        ns: String,
        accept: bool,
    },
    ListDevices,
    RenameDevice(String),
    RemoveDevice {
        id: String,
        lost: bool,
        new_passphrase: Option<String>,
    },
    ChangePassphrase(String),
    Leave,
    StartOver {
        passphrase: String,
    },
    UndoJoin,
    PurgeExtension(String),
    FindFolder,
    ApplyFolder(PathBuf),
    Ext(ExtRequest),
}

/// Collections-mode calls from an extension (§12.6).
pub enum ExtRequest {
    Status {
        ext: String,
    },
    Push {
        ext: String,
        collection: String,
        records: Vec<serde_json::Value>,
        deleted: Vec<String>,
        initial: bool,
    },
    Pull {
        ext: String,
        max: usize,
    },
    Ack {
        ext: String,
        cursor: String,
    },
}

/// The engine's answer to an [`EngineRequest`].
pub enum EngineReply {
    Done,
    Status(SyncStatus),
    Folders(Vec<SuggestedFolder>),
    Folder(FolderInfo),
    Passphrase(String),
    JoinPreview(JoinPreview),
    Scopes(HashMap<String, ScopeView>),
    Scope(ScopeView),
    Conflicts(Vec<ConflictSummary>),
    ConflictDetail(ConflictDetail),
    Reviews(Reviews),
    Devices(Vec<DeviceView>),
    FoundFolder(Option<PathBuf>),
    Ext(ExtReply),
}

/// The engine's answer to an [`ExtRequest`].
pub enum ExtReply {
    Status(ExtSyncStatus),
    Push(PushResult),
    Pull(PullResult),
    Ack,
}

/// `context.sync.status()`. No computer names: extensions never get them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtSyncStatus {
    pub available: bool,
    pub paused: bool,
    pub needs_initial_push: bool,
    /// Stable per extension and computer, not linkable across extensions.
    pub device_tag: String,
    pub devices: Vec<ExtDevice>,
    pub used_bytes: u64,
    pub max_bytes: u64,
    pub last_sync_at: Option<i64>,
}

/// One computer, as an extension sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtDevice {
    pub tag: String,
    pub os: OsKind,
    pub last_seen_at: i64,
    pub this_device: bool,
}

/// Why one pushed record was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    TooLarge,
    BadId,
    BadCollection,
    SensitiveField,
    NewerSchema,
    Quota,
    RateLimited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub id: String,
    pub code: RejectCode,
}

/// `context.sync.push()`. `off`: data sync is off for this extension here,
/// and nothing was stored.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PushResult {
    pub accepted: u32,
    pub unchanged: u32,
    pub rejected: Vec<Rejected>,
    pub off: bool,
}

/// Where a pulled change came from: a device tag and a wall time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeFrom {
    pub device: String,
    pub at: i64,
}

/// One pulled change. `record: None` is a delete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtChange {
    pub collection: String,
    pub id: String,
    pub record: Option<serde_json::Value>,
    pub from: ChangeFrom,
}

/// `context.sync.pull()`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PullResult {
    pub cursor: String,
    pub more: bool,
    pub changes: Vec<ExtChange>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::cloud_files::Provider;
    use crate::sync::types::{ConflictKind, PinReason};
    use std::time::Duration;

    fn en(key: &str) -> String {
        crate::i18n::translate_in("en", key, &[])
    }

    #[test]
    fn error_helpers_have_their_kinds_and_english_strings() {
        crate::i18n::init(Some("en"));
        for (e, wire) in [
            (unavailable(), "sync_unavailable"),
            (needs_confirmation(), "needs_confirmation"),
            (quota_exceeded(), "quota_exceeded"),
        ] {
            let json = serde_json::to_value(&e).unwrap();
            assert_eq!(json["kind"], wire);
            assert_ne!(en(&e.key), e.key, "{} has no EN string", e.key);
        }
    }

    #[test]
    fn a_pending_handle_answers_unavailable() {
        let h = SyncHandle::pending();
        let err = match h.request(EngineRequest::Status, Duration::from_millis(10)) {
            Err(e) => e,
            Ok(_) => panic!("a pending handle answered"),
        };
        assert_eq!(
            serde_json::to_value(&err).unwrap()["kind"],
            "sync_unavailable"
        );
        h.wake(engine::Wake::Manual);
        h.flush_for_exit(Duration::from_millis(10));
    }

    /// The fields of §19.4's `SyncStatus` JSON, and nothing else.
    #[test]
    fn status_serialises_with_the_documented_fields() {
        let json = serde_json::to_value(SyncStatus::unavailable()).unwrap();
        assert_eq!(json["state"], "unavailable");
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        let mut want = vec![
            "state",
            "folder",
            "last_sync_at",
            "last_publish_at",
            "paused_until",
            "pending_local",
            "waiting_files",
            "categories",
            "devices",
            "conflicts_open",
            "held",
            "reviews",
            "suggestions",
            "problems",
            "key_store",
            "undo_join_until",
            "used_bytes",
            "ext_data_off",
        ];
        want.sort_unstable();
        assert_eq!(keys, want);
        assert_eq!(
            json["categories"],
            serde_json::json!({ "settings": true, "automations": true, "extensions": true, "chats": false })
        );
    }

    #[test]
    fn every_status_state_has_an_english_string() {
        use status::SyncState::*;
        for state in [
            Off,
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
        ] {
            let wire = serde_json::to_value(state).unwrap();
            let key = format!("sync.status.{}", wire.as_str().unwrap());
            assert_ne!(en(&key), key, "{} has no EN string", key);
        }
    }

    #[test]
    fn every_conflict_kind_has_an_english_string() {
        use ConflictKind::*;
        for kind in [
            ConcurrentEdit,
            EditVsDelete,
            DeleteVsEdit,
            ReplacedOnJoin,
            RestoreBackup,
            ReplacedByRestore,
            DeletedWhileAway,
        ] {
            let wire = serde_json::to_value(kind).unwrap();
            let key = format!("sync.conflicts.kind.{}", wire.as_str().unwrap());
            assert_ne!(en(&key), key, "{} has no EN string", key);
        }
    }

    #[test]
    fn wire_names_match_the_design() {
        let wire = |v: serde_json::Value| v.as_str().unwrap().to_string();
        assert_eq!(
            wire(serde_json::to_value(ScopeAction::UseShared).unwrap()),
            "use_shared"
        );
        assert_eq!(
            wire(serde_json::to_value(ScopeAction::AvailThisComputer).unwrap()),
            "avail_this_computer"
        );
        assert_eq!(
            wire(serde_json::to_value(PinReason::OriginMismatch).unwrap()),
            "origin_mismatch"
        );
        assert_eq!(
            wire(serde_json::to_value(Category::Automations).unwrap()),
            "automations"
        );
        assert_eq!(wire(serde_json::to_value(OsKind::Macos).unwrap()), "macos");
        assert_eq!(
            wire(serde_json::to_value(ConflictAction::DeleteAnyway).unwrap()),
            "delete_anyway"
        );
        assert_eq!(
            wire(serde_json::to_value(DestructiveAction::StartOver).unwrap()),
            "start_over"
        );
        assert_eq!(
            wire(serde_json::to_value(JoinChoice::Theirs).unwrap()),
            "theirs"
        );
        for (p, name) in [
            (Provider::ICloud, "icloud"),
            (Provider::OneDrive, "onedrive"),
            (Provider::GoogleDrive, "google_drive"),
            (Provider::FileProvider, "file_provider"),
        ] {
            assert_eq!(wire(serde_json::to_value(p).unwrap()), name);
        }
        assert_eq!(
            wire(serde_json::to_value(RejectCode::SensitiveField).unwrap()),
            "sensitive_field"
        );
    }

    /// `context.sync.status()` uses the JavaScript names of §12.6.
    #[test]
    fn extension_status_uses_camel_case() {
        let json = serde_json::to_value(ExtSyncStatus::default()).unwrap();
        for key in [
            "available",
            "paused",
            "needsInitialPush",
            "deviceTag",
            "devices",
            "usedBytes",
            "maxBytes",
            "lastSyncAt",
        ] {
            assert!(json.get(key).is_some(), "missing {}", key);
        }
    }

    /// Commands deserialise their enum arguments from the names the
    /// frontend sends.
    #[test]
    fn command_arguments_parse_from_their_wire_names() {
        let tab: ConflictTab = serde_json::from_str("\"resolved\"").unwrap();
        assert_eq!(tab, ConflictTab::Resolved);
        let cats: SyncCategories = serde_json::from_str("{}").unwrap();
        assert_eq!(cats, SyncCategories::default());
        let prefs: prefs::DevicePrefs = serde_json::from_str("{}").unwrap();
        assert!(!prefs.enabled && prefs.categories.settings && !prefs.categories.chats);
    }

    /// A backup never holds sync's own state (§6.1): a megabyte of random
    /// bytes in `<data_local>/kage/sync/` doesn't grow the encrypted full
    /// backup. (Other tests may write small extension data meanwhile, so
    /// the check allows for that.)
    #[test]
    fn a_backup_never_contains_sync_state() {
        use rand::RngCore;
        let config = crate::config::Config::default();
        let size = || {
            crate::config_export::export(
                &config,
                crate::config_export::ExportMode::Encrypted,
                Some("backup passphrase"),
                &[],
            )
            .unwrap()
            .len()
        };
        let before = size();
        let dir = crate::paths::data_local_dir().unwrap().join("kage/sync");
        std::fs::create_dir_all(&dir).unwrap();
        let mut noise = vec![0u8; 1 << 20];
        rand::rng().fill_bytes(&mut noise);
        std::fs::write(dir.join("replica.db"), &noise).unwrap();
        std::fs::write(dir.join("key.bin"), b"{\"v\":1,\"keys\":{}}").unwrap();
        let after = size();
        assert!(after < before + (512 << 10), "{before} -> {after}");
        let _ = std::fs::remove_file(dir.join("replica.db"));
        let _ = std::fs::remove_file(dir.join("key.bin"));
    }

    #[test]
    fn sync_is_offered_in_debug_and_dev_builds_only() {
        assert_eq!(
            sync_available(),
            cfg!(debug_assertions) || RELEASED || env!("CARGO_PKG_VERSION").contains("+dev.")
        );
        // A handle in a build without sync never starts a thread.
        if !sync_available() {
            let h = SyncHandle::pending();
            h.start(|| anyhow::bail!("never called"), Duration::ZERO);
            assert!(!h.started());
        }
    }
}
