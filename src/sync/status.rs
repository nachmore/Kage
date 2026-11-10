//! What the engine reports to the frontend (docs/design/SYNC.md §19.4,
//! §19.5): the status, scope views, conflict summaries, reviews, devices
//! and the join preview. Every type here is serialised as the commands
//! return it. The `impl SyncEngine` block at the end builds them, and
//! carries the scope actions (§9.3) and the conflicts-list actions (§10.4).
//!
//! Labels are `{ key, params }`; the window renders them in its language.
//! User text in params (computer and item names) is never translated.

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
    /// Extensions whose data sync is off on this computer, so Settings →
    /// Sync → Extensions can show each data switch (§13.3).
    #[serde(default)]
    pub ext_data_off: Vec<String>,
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

// --- Building the answers ------------------------------------------------------

use super::catalog::{self, Rule, CFG_NS};
use super::conflicts;
use super::engine::{eff_cell, internal, SyncEngine};
use super::merge::is_visible;
use super::sources::config::FIELD;
use super::types::{FieldKey, LocalKey, Record, Scope, ScopeAction, Stamp};
use super::{ConflictAction, ConflictTab};
use crate::error::{AppError, ErrorKind};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

/// Longest preview text.
const PREVIEW_CHARS: usize = 200;
/// A computer whose newest head is older than this is "stale".
const STALE_MS: i64 = 7 * 86_400_000;

/// What the status line shows, refreshed after every tick and request
/// (the status itself is built without touching the replica).
#[derive(Debug, Clone, Default)]
pub(crate) struct StatusCache {
    pub devices: Vec<DeviceView>,
    pub conflicts_open: u32,
    pub held: u32,
    pub reviews: u32,
    pub pending_local: u32,
    pub folder: Option<FolderView>,
    pub undo_join_until: Option<String>,
    pub problems: Vec<Problem>,
    /// Installs on other computers that aren't here, and removals (§4.5).
    pub suggestions: u32,
}

fn problem(kind: &str, params: &[(&str, String)]) -> Problem {
    Problem {
        kind: kind.to_string(),
        key: format!("sync.problem.{kind}"),
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    }
}

/// A value as short text for a preview.
pub(crate) fn preview(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    if s.chars().count() > PREVIEW_CHARS {
        let mut t: String = s.chars().take(PREVIEW_CHARS - 1).collect();
        t.push('…');
        t
    } else {
        s
    }
}

/// What a catalog path names: a record and one of its cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathTarget {
    /// A synced cell: `(ns, id, cell name)`.
    Cell(String, String, String),
    /// A list item as a whole (`avail` and `*` pins).
    Item(String, String),
    /// A row that never leaves this computer, and its reason key.
    ThisComputer(&'static str),
    Never,
}

/// Read a path of `sync_get_scopes`: a setting (`ui.theme`), a list item
/// (`shortcuts/<sync_id>`) or one field of one (`shortcuts/<sync_id>/path`).
pub(crate) fn parse_path(path: &str) -> PathTarget {
    if catalog::cfg_row(path).is_some() {
        return PathTarget::Cell(CFG_NS.into(), path.into(), FIELD.into());
    }
    let mut parts = path.splitn(3, '/');
    let head = parts.next().unwrap_or_default();
    if let Some(l) = catalog::list_at(head) {
        return match (parts.next(), parts.next()) {
            (Some(id), None) if !id.is_empty() => PathTarget::Item(l.ns.into(), id.into()),
            (Some(id), Some(f)) if !id.is_empty() && super::split::valid_field_name(f) => {
                PathTarget::Cell(l.ns.into(), id.into(), f.into())
            }
            _ => PathTarget::Never,
        };
    }
    match catalog::row_for(path).map(|r| r.rule) {
        Some(Rule::ThisComputer(why)) => PathTarget::ThisComputer(why.reason_key()),
        _ => PathTarget::Never,
    }
}

/// The reason key for a value held on this computer.
fn held_reason_key(r: PinReason) -> &'static str {
    match r {
        PinReason::HeldBack => "settings.sync_scope.reason.looks_like_key",
        PinReason::Lossy | PinReason::InvalidHere | PinReason::NewerSchema => {
            "settings.sync_scope.reason.not_this_version"
        }
        PinReason::NeedsApproval | PinReason::Consent => {
            "settings.sync_scope.reason.needs_approval"
        }
        PinReason::NotInstalled | PinReason::OriginMismatch => {
            "settings.sync_scope.reason.not_installed"
        }
        PinReason::ThisComputer => "settings.sync_scope.this_computer",
    }
}

impl SyncEngine {
    /// The status line and everything Settings → Sync shows at a glance.
    pub(crate) fn build_status(&self) -> SyncStatus {
        let c = &self.rt.cache;
        let now = self.env.clock.now_ms();
        let state = if self.other_engine {
            SyncState::OtherEngine
        } else if !self.prefs.enabled {
            SyncState::Off
        } else if self.rt.join.is_some() {
            SyncState::SettingUp
        } else if self.core.is_none() {
            SyncState::FolderMissing
        } else if self.rt.blocked == Some(SyncState::Removed) {
            SyncState::Removed
        } else if self.core_locked() || self.rt.newer_epoch.is_some() {
            SyncState::Locked
        } else if self.prefs.paused_at(now) {
            SyncState::Paused
        } else if let Some(b) = self.rt.blocked {
            b
        } else if self.rt.disk_full {
            SyncState::DiskFull
        } else if self.rt.write_failed {
            SyncState::WriteFailed
        } else if self.rt.placeholder_ticks >= 3 {
            SyncState::Placeholders
        } else if self.rt.waiting_files > 0 {
            SyncState::Waiting
        } else if self.has_own_dirty() || self.rt.busy {
            SyncState::Syncing
        } else {
            SyncState::UpToDate
        };
        SyncStatus {
            state,
            folder: c.folder.clone(),
            last_sync_at: self.rt.last_ingest_ms,
            last_publish_at: self.rt.last_publish_ms,
            paused_until: self
                .prefs
                .paused_until
                .clone()
                .filter(|_| self.prefs.paused_at(now)),
            pending_local: c.pending_local,
            waiting_files: self.rt.waiting_files,
            categories: self.prefs.categories,
            devices: c.devices.clone(),
            conflicts_open: c.conflicts_open,
            held: c.held,
            reviews: c.reviews,
            suggestions: c.suggestions,
            problems: c.problems.clone(),
            key_store: self
                .prefs
                .key_store
                .as_deref()
                .and_then(KeyStoreKind::from_wire)
                .or(self.prefs.enabled.then(|| self.env.keys.kind())),
            undo_join_until: c.undo_join_until.clone(),
            used_bytes: self.rt.used_bytes,
            ext_data_off: self.prefs.ext_data_off.clone(),
        }
    }

    /// Refresh [`StatusCache`] from the replica.
    pub(crate) fn refresh_status_cache(&mut self) {
        if let Err(e) = self.refresh_status_cache_inner() {
            log::info!("sync: couldn't refresh the status: {e:#}");
        }
    }

    fn refresh_status_cache_inner(&mut self) -> anyhow::Result<()> {
        let now = self.env.clock.now_ms();
        let os = self.env.os;
        let mut cache = StatusCache {
            pending_local: u32::from(self.has_own_dirty()),
            ..StatusCache::default()
        };
        if let Some(folder) = self.prefs.folder.as_deref().filter(|_| self.prefs.enabled) {
            let p = std::path::Path::new(folder);
            let parent = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            cache.folder = Some(FolderView {
                display: format!("{parent} › {}", super::folder::ROOT_NAME),
                provider: self.env.cloud.detect_provider(p),
            });
        }
        let mut problems = self.rt.problems.clone();
        for (dev, n) in &self.rt.unreadable {
            problems.push(problem(
                "unreadable",
                &[("count", n.to_string()), ("computer", dev.to_string())],
            ));
        }
        if !self.rt.frozen.is_empty() {
            problems.push(problem(
                "churn",
                &[("count", self.rt.frozen.len().to_string())],
            ));
        }
        if self.rt.duplicates > 0 {
            problems.push(problem(
                "duplicates",
                &[("count", self.rt.duplicates.to_string())],
            ));
        }
        if self.rt.reset_notice {
            problems.push(problem("config_reset", &[]));
        }
        let my_name = self.prefs.device_name.clone().unwrap_or_default();
        let version = self.env.kage_version.clone();
        let joined = self.prefs.joined_at.clone();
        let last_pub = self.rt.last_publish_ms.unwrap_or(0);
        let installed = self.env.config.installed();
        let (_, local_cfg) = self.env.config.snapshot();
        let exclusions = self.prefs.install_exclusions.clone();
        let Some(core) = self.core.as_mut() else {
            cache.problems = problems;
            self.rt.cache = cache;
            return Ok(());
        };
        let me = core.me;
        let names: HashMap<String, String> = core.replica.tx(|tx| {
            let rows = tx.devices()?;
            let mut names = HashMap::new();
            cache.devices.push(DeviceView {
                id: me.to_string(),
                name: my_name.clone(),
                os,
                kage_version: version.clone(),
                last_seen_at: last_pub,
                this: true,
                state: DeviceState::Ok,
            });
            for d in rows {
                if d.dev == me {
                    continue;
                }
                names.insert(d.dev.to_string(), d.name.clone());
                let state = if d.state == "retired" || d.state == "removed" {
                    DeviceState::Retired
                } else if d.state == "needs_update" || d.min_reader > super::types::READER_VERSION {
                    DeviceState::NeedsUpdate
                } else if now - d.last_seen_ms >= super::gc::ACTIVE_MS {
                    DeviceState::Inactive
                } else if now - d.last_seen_ms >= STALE_MS {
                    DeviceState::Stale
                } else {
                    DeviceState::Ok
                };
                cache.devices.push(DeviceView {
                    id: d.dev.to_string(),
                    name: d.name,
                    os: d.os.unwrap_or(os),
                    kage_version: d.kage_version,
                    last_seen_at: d.last_seen_ms,
                    this: false,
                    state,
                });
            }
            for r in tx.records(conflicts::NS)? {
                if !is_visible(&r) || conflicts::state_of(&r) != Some(ConflictState::Open) {
                    continue;
                }
                let generated = conflicts::body_of(&r).is_some_and(|b| {
                    b.ns == super::sources::steering::NS && b.rid == super::sources::steering::AUTO
                });
                if !generated {
                    cache.conflicts_open += 1;
                }
            }
            let mut held_back = 0u32;
            let mut not_here = 0u32;
            let mut runnable: BTreeSet<(String, String)> = BTreeSet::new();
            let mut consent = 0u32;
            for (k, _, reason) in tx.pins(None)? {
                match reason {
                    PinReason::HeldBack => held_back += 1,
                    PinReason::Lossy | PinReason::InvalidHere | PinReason::NewerSchema => {
                        not_here += 1
                    }
                    PinReason::NeedsApproval => {
                        let key = if k.ns == CFG_NS {
                            None
                        } else {
                            Some(super::guards::guard_key(&k.ns, &k.id))
                        };
                        if let Some(key) = key {
                            if tx.meta(&format!("held.{key}"))?.is_some() {
                                runnable.insert((k.ns, k.id));
                            }
                        }
                    }
                    PinReason::Consent => consent += 1,
                    _ => {}
                }
            }
            let holds = super::guards::holds(tx)?.len() as u32;
            cache.held = held_back + not_here;
            cache.reviews = runnable.len() as u32 + consent + holds;
            let (suggest, remove) = super::sources::extensions::install_reviews(
                tx,
                &installed,
                &local_cfg,
                &exclusions,
                &|_| String::new(),
                os,
            )?;
            cache.suggestions = (suggest.len() + remove.len()) as u32;
            if held_back > 0 {
                problems.push(problem("held_back", &[("count", held_back.to_string())]));
            }
            if not_here > 0 {
                problems.push(problem(
                    "not_this_version",
                    &[("count", not_here.to_string())],
                ));
            }
            if tx.meta("join_backup")?.is_some() {
                if let Some(at) = joined
                    .as_deref()
                    .and_then(|j| chrono::DateTime::parse_from_rfc3339(j).ok())
                {
                    let until = at.timestamp_millis() + super::membership::UNDO_JOIN_MS;
                    if until > now {
                        cache.undo_join_until = chrono::DateTime::from_timestamp_millis(until)
                            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
                    }
                }
            }
            Ok(names)
        })?;
        // Unreadable files name the computer, not its id.
        for p in problems.iter_mut() {
            if let Some(c) = p.params.get_mut("computer") {
                if let Some(n) = names.get(c.as_str()) {
                    *c = n.clone();
                }
            }
        }
        cache.problems = problems;
        self.rt.cache = cache;
        Ok(())
    }

    /// `sync_list_devices`.
    pub(crate) fn list_devices(&mut self) -> Result<Vec<DeviceView>, AppError> {
        self.require_on()?;
        self.refresh_status_cache();
        Ok(self.rt.cache.devices.clone())
    }

    /// The name of the computer that wrote `stamp`.
    fn computer_of(&self, names: &HashMap<String, String>, s: &Stamp) -> String {
        let me = self.core.as_ref().map(|c| c.me);
        if Some(s.dev) == me {
            return self.prefs.device_name.clone().unwrap_or_default();
        }
        names.get(&s.dev.to_string()).cloned().unwrap_or_default()
    }

    fn device_names(&mut self) -> anyhow::Result<HashMap<String, String>> {
        let Some(core) = self.core.as_mut() else {
            return Ok(HashMap::new());
        };
        core.replica.tx(|tx| {
            Ok(tx
                .devices()?
                .into_iter()
                .map(|d| (d.dev.to_string(), d.name))
                .collect())
        })
    }

    // --- Scopes (§9.3, §9.4) -------------------------------------------------------

    /// `sync_get_scopes`: every catalog path Settings shows, or `paths`.
    pub(crate) fn get_scopes(
        &mut self,
        paths: Option<Vec<String>>,
    ) -> Result<HashMap<String, ScopeView>, AppError> {
        self.require_on()?;
        let paths = match paths {
            Some(p) => p,
            None => self.all_scope_paths().map_err(internal)?,
        };
        let mut out = HashMap::new();
        for p in paths {
            let v = self.scope_view(&p).map_err(internal)?;
            out.insert(p, v);
        }
        Ok(out)
    }

    fn all_scope_paths(&mut self) -> anyhow::Result<Vec<String>> {
        let mut out: Vec<String> = catalog::SYNC_FIELDS
            .iter()
            .filter(|r| {
                !matches!(
                    r.rule,
                    Rule::Never | Rule::List(_) | Rule::ExtensionSettings
                )
            })
            .filter(|r| r.path != "extension_states.*")
            .map(|r| r.path.to_string())
            .collect();
        let core = self.core.as_mut().expect("on");
        let more: Vec<String> = core.replica.tx(|tx| {
            let mut more = Vec::new();
            for r in tx.records(CFG_NS)? {
                if r.id.starts_with(super::sources::config::EXTENSION_STATE) {
                    more.push(r.id);
                }
            }
            for l in catalog::SYNC_LISTS {
                for r in tx.records(l.ns)? {
                    if is_visible(&r) {
                        more.push(format!("{}/{}", l.path, r.id));
                    }
                }
            }
            Ok(more)
        })?;
        out.extend(more);
        Ok(out)
    }

    /// One path's where-it-applies state.
    pub(crate) fn scope_view(&mut self, path: &str) -> anyhow::Result<ScopeView> {
        let os = self.env.os;
        let not_synced = |rule: ScopeRule, reason: Option<&str>| ScopeView {
            synced: false,
            rule,
            applies: Applies::NotSynced,
            reason_key: reason.map(str::to_string),
            held: None,
            values: BTreeMap::new(),
            others: Vec::new(),
        };
        let (ns, id, field) = match parse_path(path) {
            PathTarget::ThisComputer(reason) => {
                return Ok(not_synced(ScopeRule::ThisComputer, Some(reason)))
            }
            PathTarget::Never => return Ok(not_synced(ScopeRule::Never, None)),
            PathTarget::Item(ns, id) => {
                let core = self.core.as_mut().expect("on");
                return core.replica.tx(|tx| {
                    let rec = tx.record(&ns, &id)?;
                    let star = tx.pin(&LocalKey::new(&ns, &id, "*"))?;
                    let avail = rec
                        .as_ref()
                        .and_then(|r| r.cell(super::split::AVAIL, Scope::All))
                        .and_then(|c| c.value.as_str().map(str::to_string))
                        .unwrap_or_else(|| "all".into());
                    let applies = if star.is_some() {
                        Applies::ThisComputer
                    } else if avail == "all" {
                        Applies::All
                    } else {
                        Applies::Os
                    };
                    let mut values = BTreeMap::new();
                    values.insert("all".to_string(), Value::String(avail));
                    Ok(ScopeView {
                        synced: star.is_none(),
                        rule: ScopeRule::Shared,
                        applies,
                        reason_key: None,
                        held: None,
                        values,
                        others: Vec::new(),
                    })
                });
            }
            PathTarget::Cell(ns, id, f) => (ns, id, f),
        };
        let default = self
            .specs
            .get(&ns)
            .map_or(super::types::DefaultScope::Shared, |(_, s)| {
                super::engine::default_scope(s, &id, &field)
            });
        let core = self.core.as_mut().expect("on");
        core.replica.tx(|tx| {
            let rec = tx
                .record(&ns, &id)?
                .unwrap_or_else(|| Record::new(ns.clone(), id.clone()));
            let pin = tx.pin(&LocalKey::new(&ns, &id, &field))?;
            let mut values = BTreeMap::new();
            for scope in [
                Scope::All,
                Scope::Os(OsKind::Macos),
                Scope::Os(OsKind::Windows),
                Scope::Os(OsKind::Linux),
            ] {
                if let Some(c) = rec.cell(&field, scope).filter(|c| !c.value.is_null()) {
                    values.insert(scope.as_str().to_string(), c.value.clone());
                }
            }
            let here = rec
                .cell(&field, Scope::Os(os))
                .filter(|c| !c.value.is_null());
            let applies = match &pin {
                Some((_, PinReason::ThisComputer)) => Applies::ThisComputer,
                Some(_) => Applies::NotSynced,
                None if here.is_some() => Applies::Os,
                None => Applies::All,
            };
            let mine = here
                .or_else(|| rec.cell(&field, Scope::All))
                .map(|c| c.value.clone());
            let others = OsKind::ALL
                .into_iter()
                .filter(|o| *o != os)
                .filter_map(|o| {
                    let c = rec
                        .cell(&field, Scope::Os(o))
                        .filter(|c| !c.value.is_null())?;
                    (Some(&c.value) != mine.as_ref()).then(|| OtherOs {
                        os: o,
                        preview: preview(&c.value),
                    })
                })
                .collect();
            Ok(ScopeView {
                synced: !matches!(pin, Some((_, PinReason::ThisComputer))),
                rule: match default {
                    super::types::DefaultScope::PerOs => ScopeRule::PerOs,
                    super::types::DefaultScope::Shared => ScopeRule::Shared,
                },
                applies,
                reason_key: pin
                    .as_ref()
                    .filter(|(_, r)| *r != PinReason::ThisComputer)
                    .map(|(_, r)| held_reason_key(*r).to_string()),
                held: pin
                    .as_ref()
                    .map(|(_, r)| *r)
                    .filter(|r| *r != PinReason::ThisComputer),
                values,
                others,
            })
        })
    }

    /// `sync_set_scope`: one action of §9.3.
    pub(crate) fn set_scope(
        &mut self,
        path: &str,
        action: ScopeAction,
    ) -> Result<ScopeView, AppError> {
        self.require_on()?;
        if self.core_locked() {
            return Err(super::unavailable());
        }
        let bad = || AppError::keyed(ErrorKind::InvalidInput, "errors.sync.no_such_item", &[]);
        let (ns, id, field) = match parse_path(path) {
            PathTarget::Cell(ns, id, f) => (ns, id, Some(f)),
            PathTarget::Item(ns, id) => (ns, id, None),
            _ => return Err(bad()),
        };
        let item_action = matches!(
            action,
            ScopeAction::AvailAll | ScopeAction::AvailOs | ScopeAction::AvailThisComputer
        );
        if item_action != field.is_none() {
            return Err(bad());
        }
        let Some((si, _)) = self.specs.get(&ns).cloned() else {
            return Err(bad());
        };
        // Capture first, so the value on this computer is the one in hand.
        let local = self.sources[si].read_local().map_err(internal)?;
        self.capture_state(si, &local, super::types::Origin::User)
            .map_err(internal)?;
        let local_rec = local.get(&ns, &id).cloned();
        let now = self.now();
        let os = self.env.os;
        let core = self.core.as_mut().expect("on");
        let keys = core.cur_keys().cloned().ok_or_else(super::unavailable)?;
        let me = core.me;
        let super::engine::Core { replica, hlc, .. } = core;
        replica
            .tx(|tx| {
                let mut rec = tx
                    .record(&ns, &id)?
                    .unwrap_or_else(|| Record::new(ns.clone(), id.clone()));
                let write =
                    |rec: &mut Record, hlc: &mut super::hlc::Hlc, fk: FieldKey, v: Value| {
                        let cur = rec.cells.get(&fk).cloned();
                        let floor = rec.max_stamp().map_or(0, |s| s.hlc);
                        let stamp = Stamp::new(hlc.tick(now, floor), me);
                        rec.cells.insert(
                            fk,
                            super::types::Cell {
                                value: v,
                                stamp,
                                wall_ms: now,
                                anc: super::merge::ancestors(cur.as_ref(), me),
                            },
                        );
                    };
                match (&field, action) {
                    (Some(f), ScopeAction::ThisComputer) => {
                        let cur = current_value(&rec, f, os, local_rec.as_ref());
                        tx.set_pin(
                            &LocalKey::new(&ns, &id, f),
                            Some((cur, PinReason::ThisComputer)),
                        )?;
                        return Ok(());
                    }
                    (Some(f), ScopeAction::StopThisComputer) => {
                        tx.set_pin(&LocalKey::new(&ns, &id, f), None)?;
                        return Ok(());
                    }
                    (Some(f), a) => {
                        let cur = current_value(&rec, f, os, local_rec.as_ref());
                        for (fk, v) in super::scope::scope_writes(&rec, f, os, a, &cur) {
                            write(&mut rec, hlc, fk, v);
                        }
                        tx.set_pin(&LocalKey::new(&ns, &id, f), None)?;
                    }
                    (None, ScopeAction::AvailThisComputer) => {
                        let fields = local_rec
                            .as_ref()
                            .map(|l| Value::Object(l.fields.clone()))
                            .unwrap_or(Value::Null);
                        tx.set_pin(
                            &LocalKey::new(&ns, &id, "*"),
                            Some((fields, PinReason::ThisComputer)),
                        )?;
                        if is_visible(&rec) && !rec.cells.is_empty() {
                            let floor = rec.max_stamp().map_or(0, |s| s.hlc);
                            rec.del = Some(super::types::Tomb {
                                stamp: Stamp::new(hlc.tick(now, floor), me),
                                wall_ms: now,
                                anc: rec.newest_cell_stamps(super::merge::MAX_ANCESTORS),
                            });
                        }
                        super::engine::drop_projected(tx, &ns, &id)?;
                    }
                    (None, a) => {
                        for (fk, v) in super::scope::scope_writes(
                            &rec,
                            super::split::AVAIL,
                            os,
                            a,
                            &Value::Null,
                        ) {
                            write(&mut rec, hlc, fk, v);
                        }
                    }
                }
                let b = tx
                    .bucket_of(&ns, &id)?
                    .unwrap_or_else(|| keys.bucket(&ns, &id, 1 << 16));
                tx.write(&rec, b)?;
                tx.set_meta("hlc_last", &hlc.last().to_string())
            })
            .map_err(internal)?;
        self.rt.first_own_ms.get_or_insert(now);
        self.rt.last_own_ms = Some(now);
        let set: BTreeSet<(String, String)> = [(ns.clone(), id.clone())].into();
        self.project(&set, false).map_err(internal)?;
        let action_name = serde_json::to_value(action)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        self.env
            .events
            .track("sync_scope_changed", vec![("action", action_name)]);
        self.scope_view(path).map_err(internal)
    }

    // --- Conflicts (§10) ---------------------------------------------------------------

    /// `sync_list_conflicts`. "Recently deleted" lists tombstoned records
    /// whose cells are still kept; their ids are `d|<ns>|<id>`.
    pub(crate) fn list_conflicts(
        &mut self,
        tab: ConflictTab,
    ) -> Result<Vec<ConflictSummary>, AppError> {
        self.require_on()?;
        let names = self.device_names().map_err(internal)?;
        let os = self.env.os;
        let core = self.core.as_mut().expect("on");
        let mut rows: Vec<(Record, Option<Record>)> = Vec::new();
        let mut deleted: Vec<Record> = Vec::new();
        let group = core
            .replica
            .tx(|tx| {
                if tab == ConflictTab::Deleted {
                    for ns in tx.namespaces()? {
                        if ns.starts_with("sync.") {
                            continue;
                        }
                        for r in tx.records(&ns)? {
                            if r.del.is_some() && !is_visible(&r) && !r.cells.is_empty() {
                                deleted.push(r);
                            }
                        }
                    }
                } else {
                    for r in tx.records(conflicts::NS)? {
                        if !is_visible(&r) {
                            continue;
                        }
                        let Some(b) = conflicts::body_of(&r) else {
                            continue;
                        };
                        let target = tx.record(&b.ns, &b.rid)?;
                        rows.push((r, target));
                    }
                }
                tx.meta("join_group")
            })
            .map_err(internal)?;
        let mut out = Vec::new();
        for r in deleted {
            let Some(d) = r.del.clone() else { continue };
            let (label_key, label_params) = record_label(&r.ns, &r.id, Some(&r), None, os);
            out.push(ConflictSummary {
                id: format!("d|{}|{}", r.ns, r.id),
                kind: ConflictKind::EditVsDelete,
                category: catalog::category_of(&r.ns, &r.id).unwrap_or(Category::Extensions),
                label_key,
                label_params,
                loser: LoserView {
                    computer: self.computer_of(&names, &d.stamp),
                    at: d.wall_ms,
                    deleted: true,
                    preview: String::new(),
                },
                winner: None,
                state: ConflictState::Open,
                group: None,
            });
        }
        for (r, target) in rows {
            let Some(s) = self.summary(&r, target.as_ref(), &names, group.as_deref(), os) else {
                continue;
            };
            let want = match tab {
                ConflictTab::Replaced => s.state == ConflictState::Open,
                ConflictTab::Resolved => s.state != ConflictState::Open,
                ConflictTab::Deleted => false,
            };
            if want {
                out.push(s);
            }
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.loser.at));
        Ok(out)
    }

    fn summary(
        &self,
        r: &Record,
        target: Option<&Record>,
        names: &HashMap<String, String>,
        join_group: Option<&str>,
        os: OsKind,
    ) -> Option<ConflictSummary> {
        let b = conflicts::body_of(r)?;
        let state = conflicts::state_of(r).unwrap_or(ConflictState::Open);
        let (label_key, label_params) =
            record_label(&b.ns, &b.rid, target, b.loser.value.as_ref(), os);
        let group = match b.kind {
            ConflictKind::ReplacedOnJoin => Some(join_group.unwrap_or("join").to_string()),
            ConflictKind::RestoreBackup => Some("restore".to_string()),
            ConflictKind::DeletedWhileAway => Some("away".to_string()),
            _ if b.ns == super::sources::steering::NS
                && b.rid == super::sources::steering::AUTO =>
            {
                Some("generated".to_string())
            }
            _ => None,
        };
        Some(ConflictSummary {
            id: r.id.clone(),
            kind: b.kind,
            category: catalog::category_of(&b.ns, &b.rid).unwrap_or(Category::Extensions),
            label_key,
            label_params,
            loser: LoserView {
                computer: self.computer_of(names, &b.loser.stamp),
                at: b.loser.wall_ms,
                deleted: b.loser.deleted,
                preview: b
                    .loser
                    .value
                    .as_ref()
                    .filter(|_| b.field.is_some())
                    .map(preview)
                    .unwrap_or_default(),
            },
            winner: b.winner.as_ref().map(|w| WinnerView {
                computer: self.computer_of(names, &w.stamp),
                at: w.wall_ms,
            }),
            state,
            group,
        })
    }

    /// `sync_conflict_detail`: the summary and the fields side by side.
    pub(crate) fn conflict_detail(&mut self, id: &str) -> Result<ConflictDetail, AppError> {
        self.require_on()?;
        let no_such =
            || AppError::keyed(ErrorKind::InvalidInput, "errors.sync.no_such_conflict", &[]);
        let names = self.device_names().map_err(internal)?;
        let os = self.env.os;
        let core = self.core.as_mut().expect("on");
        let found = core
            .replica
            .tx(|tx| {
                let group = tx.meta("join_group")?;
                let Some(r) = tx.record(conflicts::NS, id)? else {
                    return Ok(None);
                };
                let target = match conflicts::body_of(&r) {
                    Some(b) => tx.record(&b.ns, &b.rid)?,
                    None => None,
                };
                Ok(Some((r, target, group)))
            })
            .map_err(internal)?;
        let Some((r, target, group)) = found else {
            return Err(no_such());
        };
        let summary = self
            .summary(&r, target.as_ref(), &names, group.as_deref(), os)
            .ok_or_else(no_such)?;
        let b = conflicts::body_of(&r).ok_or_else(no_such)?;
        let now_of = |name: &str| {
            target
                .as_ref()
                .and_then(|t| eff_cell(t, name, os))
                .map(|c| preview(&c.value))
        };
        let mut fields = Vec::new();
        match (&b.field, &b.loser.value) {
            (Some(fk), v) => fields.push(FieldDiff {
                field: fk.name.clone(),
                now: now_of(&fk.name),
                replaced: v.as_ref().map(preview),
            }),
            (None, Some(v)) => {
                if let Ok(lost) = Record::from_json(&b.ns, v) {
                    let cell_names: BTreeSet<String> =
                        lost.cells.keys().map(|k| k.name.clone()).collect();
                    for n in cell_names {
                        fields.push(FieldDiff {
                            field: n.clone(),
                            now: now_of(&n),
                            replaced: eff_cell(&lost, &n, os).map(|c| preview(&c.value)),
                        });
                    }
                }
            }
            (None, None) => {}
        }
        Ok(ConflictDetail { summary, fields })
    }

    /// `sync_resolve_conflict`.
    pub(crate) fn resolve_conflict(
        &mut self,
        id: &str,
        action: ConflictAction,
    ) -> Result<(), AppError> {
        self.require_on()?;
        if self.core_locked() {
            return Err(super::unavailable());
        }
        let now = self.now();
        let core = self.core.as_mut().expect("on");
        let keys = core.cur_keys().cloned().ok_or_else(super::unavailable)?;
        let name_key = *keys.name_key();
        let me = core.me;
        let bucket = move |ns: &str, id: &str| keys.bucket(ns, id, 1 << 16);
        let super::engine::Core { replica, hlc, .. } = core;
        let mut category = None;
        let touched: Vec<(String, String)> = replica
            .tx(|tx| {
                let out = if let Some(rest) = id.strip_prefix("d|") {
                    // Recently deleted → Restore.
                    let Some((ns, rid)) = rest.split_once('|') else {
                        anyhow::bail!("no such item");
                    };
                    category = catalog::category_of(ns, rid);
                    if conflicts::restore_record(tx, ns, rid, now, me, hlc)? {
                        vec![(ns.to_string(), rid.to_string())]
                    } else {
                        Vec::new()
                    }
                } else {
                    if let Some(b) = tx
                        .record(conflicts::NS, id)?
                        .and_then(|r| conflicts::body_of(&r))
                    {
                        category = catalog::category_of(&b.ns, &b.rid);
                    }
                    conflicts::resolve(tx, &name_key, id, action, now, me, hlc, &bucket)?
                };
                tx.set_meta("hlc_last", &hlc.last().to_string())?;
                Ok(out)
            })
            .map_err(|e| {
                log::info!("sync: resolving a conflict failed: {e:#}");
                AppError::keyed(ErrorKind::InvalidInput, "errors.sync.no_such_conflict", &[])
            })?;
        self.rt.first_own_ms.get_or_insert(now);
        self.rt.last_own_ms = Some(now);
        let set: BTreeSet<(String, String)> = touched.into_iter().collect();
        self.project(&set, false).map_err(internal)?;
        let action_name = match action {
            ConflictAction::Restore => "restore",
            ConflictAction::Dismiss | ConflictAction::DeleteAnyway => "dismiss",
        };
        let cat = match category {
            Some(Category::Automations) => "automations",
            Some(Category::Extensions) => "extensions",
            Some(Category::Chats) => "chats",
            _ => "settings",
        };
        self.env.events.track(
            "sync_conflict_action",
            vec![
                ("action", action_name.to_string()),
                ("category", cat.to_string()),
            ],
        );
        Ok(())
    }

    /// `sync_dismiss_all`: every open entry, or those of one group (`join`
    /// is every join's entries).
    pub(crate) fn dismiss_all(&mut self, group: Option<&str>) -> Result<(), AppError> {
        let open = self.list_conflicts(ConflictTab::Replaced)?;
        for c in open {
            let in_group = match group {
                None => true,
                Some("join") => c.kind == ConflictKind::ReplacedOnJoin,
                Some(g) => c.group.as_deref() == Some(g),
            };
            if in_group {
                self.resolve_conflict(&c.id, ConflictAction::Dismiss)?;
            }
        }
        Ok(())
    }

    // --- Reviews (§8.10, §13.3) -----------------------------------------------------------

    /// `sync_list_reviews`.
    pub(crate) fn list_reviews(&mut self) -> Result<Reviews, AppError> {
        self.require_on()?;
        let names = self.device_names().map_err(internal)?;
        let os = self.env.os;
        let me = self.core.as_ref().map(|c| c.me);
        let my_name = self.prefs.device_name.clone().unwrap_or_default();
        let installed = self.env.config.installed();
        let (_, local_cfg) = self.env.config.snapshot();
        let exclusions = self.prefs.install_exclusions.clone();
        let core = self.core.as_mut().expect("on");
        core.replica
            .tx(|tx| {
                let mut out = Reviews::default();
                let name_of = |d: super::types::DeviceId| {
                    if Some(d) == me {
                        my_name.clone()
                    } else {
                        names.get(&d.to_string()).cloned().unwrap_or_default()
                    }
                };
                (out.suggestions, out.removals) = super::sources::extensions::install_reviews(
                    tx,
                    &installed,
                    &local_cfg,
                    &exclusions,
                    &name_of,
                    os,
                )?;
                let mut seen = BTreeSet::new();
                for (k, _, reason) in tx.pins(None)? {
                    match reason {
                        PinReason::NeedsApproval => {
                            // A held item, or the active connection waiting
                            // for the connection it points to.
                            let (ns, id) = if k.ns != CFG_NS {
                                (k.ns.clone(), k.id.clone())
                            } else if k.id == "acp.active_connection_id" {
                                let target = tx.record(CFG_NS, &k.id)?.and_then(|r| {
                                    eff_cell(&r, FIELD, os)
                                        .and_then(|c| c.value.as_str().map(str::to_string))
                                });
                                let Some(target) = target else { continue };
                                ("list.connections".to_string(), target)
                            } else {
                                continue;
                            };
                            let key = super::guards::guard_key(&ns, &id);
                            if !seen.insert(key.clone())
                                || tx.meta(&format!("held.{key}"))?.is_none()
                            {
                                continue;
                            }
                            let Some(rec) = tx.record(&ns, &id)? else {
                                continue;
                            };
                            let Some(spec) =
                                catalog::list_spec(&ns).map(super::sources::lists::spec_of)
                            else {
                                continue;
                            };
                            let eff = super::engine::effective_record(
                                &spec,
                                &rec,
                                None,
                                &BTreeMap::new(),
                                os,
                            );
                            let detail = super::guards::runnable_of(&ns, &eff.fields)
                                .map(|v| runnable_text(&ns, &v));
                            let (label_key, label_params) =
                                record_label(&ns, &id, Some(&rec), None, os);
                            let by = rec.cells.values().map(|c| c.stamp).max();
                            out.runnables.push(ReviewItem {
                                kind: "runnable".into(),
                                id: key,
                                label_key,
                                label_params,
                                computer: by.map(|s| {
                                    if Some(s.dev) == me {
                                        my_name.clone()
                                    } else {
                                        names.get(&s.dev.to_string()).cloned().unwrap_or_default()
                                    }
                                }),
                                detail,
                                count: None,
                            });
                        }
                        PinReason::Consent => {
                            let ext =
                                k.id.strip_prefix(super::sources::config::EXTENSION_STATE)
                                    .unwrap_or(&k.id)
                                    .to_string();
                            let mut params = BTreeMap::new();
                            params.insert("extension".to_string(), ext.clone());
                            out.approvals.push(ReviewItem {
                                kind: "consent".into(),
                                id: ext,
                                label_key: "sync.review.consent".into(),
                                label_params: params,
                                ..ReviewItem::default()
                            });
                        }
                        _ => {}
                    }
                }
                for (ns, h) in super::guards::holds(tx)? {
                    let mut params = BTreeMap::new();
                    params.insert("count".to_string(), h.count.to_string());
                    params.insert("what".to_string(), ns.clone());
                    let (kind, key) = if h.outgoing {
                        ("mass_delete_out", "sync.review.mass_delete_out")
                    } else {
                        ("mass_delete", "sync.review.mass_delete")
                    };
                    out.mass_changes.push(ReviewItem {
                        kind: kind.into(),
                        id: ns,
                        label_key: key.into(),
                        label_params: params,
                        computer: h.from.clone(),
                        detail: None,
                        count: Some(h.count as u64),
                    });
                }
                Ok(out)
            })
            .map_err(internal)
    }
}

/// The value of a cell on this computer: the cells, else the local file's.
fn current_value(
    rec: &Record,
    field: &str,
    os: OsKind,
    local: Option<&super::sources::LocalRecord>,
) -> Value {
    if let Some(c) = eff_cell(rec, field, os).filter(|c| !c.value.is_null()) {
        return c.value.clone();
    }
    local
        .and_then(|l| l.fields.get(field))
        .cloned()
        .unwrap_or(Value::Null)
}

/// What a runnable runs, as text for the review dialog.
fn runnable_text(ns: &str, v: &Value) -> String {
    match ns {
        "list.connections" => v
            .get("spawn_command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "list.internal_sites" => v
            .get("script")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => serde_json::to_string_pretty(v).unwrap_or_default(),
    }
}

/// The label of a record: the catalog's key for a setting, the item's
/// name for a list item, a document's own key for steering.
fn record_label(
    ns: &str,
    id: &str,
    rec: Option<&Record>,
    loser: Option<&Value>,
    os: OsKind,
) -> (String, BTreeMap<String, String>) {
    let mut params = BTreeMap::new();
    if ns == CFG_NS {
        if let Some(ext) = id.strip_prefix(super::sources::config::EXTENSION_STATE) {
            params.insert("extension".to_string(), ext.to_string());
        }
        return match catalog::label_key(id) {
            Some(k) => (k.to_string(), params),
            None => {
                params.insert("path".to_string(), id.to_string());
                ("sync.label.setting".to_string(), params)
            }
        };
    }
    if ns == super::sources::steering::NS {
        return (format!("sync.label.steering.{id}"), params);
    }
    let name_field = catalog::label_field(ns).unwrap_or("name");
    let name = rec
        .and_then(|r| eff_cell(r, name_field, os))
        .and_then(|c| c.value.as_str().map(str::to_string))
        .or_else(|| loser.and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default();
    params.insert("name".to_string(), name);
    if let Some(l) = catalog::list_spec(ns) {
        params.insert(
            "list".to_string(),
            catalog::label_key(l.path).unwrap_or_default().to_string(),
        );
        return ("sync.conflicts.label.item".to_string(), params);
    }
    ("sync.conflicts.label.record".to_string(), params)
}

#[cfg(test)]
mod build_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_parse_into_cells_items_and_reasons() {
        assert_eq!(
            parse_path("ui.theme"),
            PathTarget::Cell("cfg".into(), "ui.theme".into(), "v".into())
        );
        assert_eq!(
            parse_path("shortcuts/s1"),
            PathTarget::Item("list.shortcuts".into(), "s1".into())
        );
        assert_eq!(
            parse_path("acp.connections/c1/mode"),
            PathTarget::Cell("list.connections".into(), "c1".into(), "mode".into())
        );
        assert!(matches!(
            parse_path("system.auto_start"),
            PathTarget::ThisComputer(_)
        ));
        assert!(matches!(
            parse_path("tool_permissions.trust_all"),
            PathTarget::ThisComputer(_)
        ));
        assert_eq!(parse_path("ui.chat_window_x"), PathTarget::Never);
        assert_eq!(parse_path("shortcuts/s1/bad@name"), PathTarget::Never);
    }

    #[test]
    fn status_names_the_extensions_whose_data_is_off_here() {
        let mut s = SyncStatus::unavailable();
        s.ext_data_off = vec!["mindmap".into()];
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["ext_data_off"], json!(["mindmap"]));
        // A status without the field (an older build) still reads.
        let mut older = v.clone();
        older.as_object_mut().unwrap().remove("ext_data_off");
        let back: SyncStatus = serde_json::from_value(older).unwrap();
        assert!(back.ext_data_off.is_empty());
    }

    #[test]
    fn previews_are_short_text() {
        assert_eq!(preview(&json!("dark")), "dark");
        assert_eq!(preview(&json!(14)), "14");
        let long = "x".repeat(500);
        assert_eq!(preview(&json!(long)).chars().count(), PREVIEW_CHARS);
    }
}
