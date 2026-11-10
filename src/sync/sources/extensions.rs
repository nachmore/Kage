//! The extension source (docs/design/SYNC.md §4.5, §12): what is
//! installed, extension settings, and extension data in keys, lists and
//! collections mode.
//!
//! | Namespace | Record | Field `v` holds |
//! |---|---|---|
//! | `installs` | `<kind>:<id>` | `{ version, source, store }` |
//! | `cfg.ext` | `<id>/<field>` | the setting (§12.1) |
//! | `ext.<id>.k` | storage key | the key's JSON value (§12.4) |
//! | `ext.<id>.l.<key>` | element id, and `~rest` | the element, or the rest of the object (§12.5) |
//! | `ext.<id>.c.<collection>` | record id | the manifest's structure, not `v` (§12.6) |
//!
//! What syncs where is decided in [`Source::prepare`], which also runs the
//! per-window bases the engine's own capture doesn't know about:
//! - **Keys**: a save from a window that read the key before a remote
//!   change arrived is based on what that window read (`reads`), so the
//!   remote value becomes a conflict instead of being erased.
//! - **Lists**: a save deletes only the elements the saving window had
//!   seen (`ext_view`). Elements it never saw are written back, at the end.
//! - **Collections**: the extension pushes, pulls and acks; `ext_base`
//!   holds what it last saw, so a push over a stale copy is a conflict.
//! - **Not here**: an extension that isn't installed, has another origin,
//!   or a setting its manifest doesn't let sync, is pinned (`*`) so the
//!   engine neither captures nor projects it. Uninstalling therefore
//!   deletes nothing elsewhere, and reinstalling brings it back.
//!
//! The source never installs or removes code. A theme from a store this
//! computer uses is installed through [`ExtensionHost`].

use super::{
    is_unchanged, unchanged, CaptureCtx, CaptureResult, LocalRecord, LocalState, LocalWrite,
    NsSpec, Prepared, ProjectResult, Source,
};
use crate::config::Config;
use crate::config_export::sensitive::{is_host_managed, looks_like_secret, Plain, Rules};
use crate::error::{AppError, ErrorKind};
use crate::extensions::{InstalledItem, SyncCollectionDecl, SyncDecl, SyncListDecl, SyncScopeDecl};
use crate::sync::canonical::{canonical, hash, same};
use crate::sync::engine::{eff_cell, CommitError, ConfigPort};
use crate::sync::merge::is_visible;
use crate::sync::replica::Tx;
use crate::sync::split::{self, WHOLE};
use crate::sync::types::{
    Category, DefaultScope, Origin, OsKind, PinReason, Record, Scope, Stamp, Structure,
};
use crate::sync::{
    ChangeFrom, ExtChange, ExtDevice, ExtReply, ExtRequest, ExtSyncStatus, PullResult, PushResult,
    RejectCode, Rejected,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The source's id, for `sync::notify`.
pub const SOURCE_ID: &str = "extensions";
pub const INSTALLS_NS: &str = "installs";
pub const SETTINGS_NS: &str = "cfg.ext";
/// Where an extension's origin cell lives: record `ext.<id>`, field
/// `origin` (§12.3).
pub const STATE_NS: &str = "sync.ns_state";
/// A lists-mode key's non-list fields.
pub const REST_ID: &str = "~rest";
/// The one field of installs, settings, keys and lists records.
const FIELD: &str = WHOLE;

// §12.7 limits.
pub const MAX_KEY_BYTES: u64 = 1 << 20;
pub const MAX_ELEMENT_BYTES: usize = 64 << 10;
pub const MAX_ELEMENTS: usize = 20_000;
pub const MAX_LIST_FILE_BYTES: u64 = 8 << 20;
pub const MAX_PUSH_RECORDS: usize = 500;
pub const MAX_PUSH_BYTES: usize = 4 << 20;
pub const PUSH_PER_MINUTE: f64 = 5_000.0;
pub const MAX_PULL: usize = 500;
pub const MAX_EXTENSION_BYTES: u64 = 128 << 20;

/// What the app does for the source. WP-S4 wires it.
pub trait ExtensionHost: Send + Sync {
    /// Install store theme `id` from the store whose URL [`store_origin`]
    /// names `store` (§4.5). Returns at once.
    fn install_store_theme(&self, id: &str, store: &str);
    /// These installed extensions' grants have no `origin`: work it out
    /// from the stores' catalogs and save it (§4.5). Returns at once.
    fn resolve_origins(&self, ids: &[String]);
}

/// `store:<16 hex of SHA-256 of the store URL>`, the origin of an item
/// installed from `url` (§4.5). A trailing `/` doesn't count.
pub fn store_origin(url: &str) -> String {
    let h = Sha256::digest(url.trim_end_matches('/').as_bytes());
    format!("store:{}", hex::encode(&h[..8]))
}

/// The namespaces of extension `ext`'s data.
pub fn keys_ns(ext: &str) -> String {
    format!("ext.{ext}.k")
}

pub fn list_ns(ext: &str, key: &str) -> String {
    format!("ext.{ext}.l.{key}")
}

pub fn collection_ns(ext: &str, coll: &str) -> String {
    format!("ext.{ext}.c.{coll}")
}

fn origin_record(ext: &str) -> String {
    format!("ext.{ext}")
}

/// Meta keys of an extension's data sync.
fn meta_data_on(ext: &str) -> String {
    format!("extsync.on.{ext}")
}

fn meta_initial_done(ext: &str) -> String {
    format!("extsync.initial.{ext}")
}

fn meta_bytes(ext: &str) -> String {
    format!("extsync.bytes.{ext}")
}

const META_PLAN: &str = "extsync.plan";

/// The data switch of `ext` was set here (`sync_set_extension_data`):
/// remember the user's own choice, which a sideloaded extension needs
/// before its data syncs, and have the extension push everything once
/// when it is switched on (§12.3, §12.6).
pub(crate) fn note_data_switch(tx: &mut Tx<'_>, ext: &str, on: bool) -> anyhow::Result<()> {
    tx.delete_meta(&meta_initial_done(ext))?;
    if on {
        tx.set_meta(&meta_data_on(ext), "1")
    } else {
        tx.delete_meta(&meta_data_on(ext))
    }
}

/// A record whose one field `v` holds `v`.
fn rec_of(v: Value) -> LocalRecord {
    let mut fields = Map::new();
    fields.insert(FIELD.to_string(), v);
    LocalRecord {
        fields,
        avail: None,
    }
}

fn spec(
    ns: String,
    schema: u32,
    structure: Structure,
    large: bool,
    label: Option<String>,
) -> NsSpec {
    NsSpec {
        ns,
        category: Category::Extensions,
        schema,
        echo: true,
        structure,
        default_scope: DefaultScope::Shared,
        large,
        label_field: label,
    }
}

/// The structure of a collection's records.
pub fn collection_structure(d: &SyncCollectionDecl) -> Structure {
    Structure {
        whole: d.whole,
        together: d.together.clone(),
        sets: d.sets.clone(),
        maps: d.maps.clone(),
        local: d.local.clone(),
        per_os: Vec::new(),
    }
}

/// `installs`' record id of an installed item.
pub fn install_id(item: &InstalledItem) -> String {
    format!("{}:{}", item.manifest.kind, item.manifest.id)
}

/// `installs`' value of an installed item, from its grant's origin; `None`
/// while the origin isn't known.
fn install_value(item: &InstalledItem, config: &Config) -> Option<Value> {
    let origin = config
        .extension_grants
        .get(&item.manifest.id)?
        .origin
        .clone()?;
    let (source, store) = match origin.strip_prefix("store:") {
        Some(h) => ("store", Value::String(h.to_string())),
        None if origin == "bundled" => ("bundled", Value::Null),
        None => ("sideload", Value::Null),
    };
    Some(serde_json::json!({
        "version": item.manifest.version,
        "source": source,
        "store": store,
    }))
}

/// Whether a record id is a valid collection record id:
/// `^[A-Za-z0-9_.:-]{1,128}$`.
pub fn valid_record_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

/// Whether a list element id is one sync can carry.
fn valid_element_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.starts_with('~')
}

// --- What syncs here ------------------------------------------------------------

/// Which extensions' settings and data sync on this computer, decided in
/// [`Source::prepare`] from the installed manifests, the grants, the data
/// switches and the origin cells.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Plan {
    /// Extensions whose data syncs here, with their data namespaces.
    data: BTreeMap<String, Vec<String>>,
    /// Extensions whose settings sync here.
    settings: BTreeSet<String>,
    /// `<id>/<field>` settings that default per OS.
    per_os: BTreeSet<String>,
}

/// Whether setting `field` of `ext` may sync, by the installed manifest
/// (§12.1): not a credential or scratch field, and either a plain value a
/// settings-only backup may copy or marked `shared`/`os`, and not marked
/// `machine`/`never`.
pub fn setting_syncs(
    rules: &Rules,
    item: Option<&InstalledItem>,
    ext: &str,
    field: &str,
    value: &Value,
) -> bool {
    if is_host_managed(field)
        || rules.is_sensitive_setting(ext, field)
        || rules.is_sensitive_data(ext, field)
        || looks_like_secret(field)
    {
        return false;
    }
    let mark = item.and_then(|i| {
        i.manifest
            .config_field_decls()
            .into_iter()
            .find(|d| d.field == field)
            .and_then(|d| d.sync)
    });
    match mark {
        Some(SyncScopeDecl::Machine | SyncScopeDecl::Never) => false,
        Some(SyncScopeDecl::Shared | SyncScopeDecl::Os) => true,
        None => rules.plain_setting(ext, field, value) == Plain::Include,
    }
}

/// Every collections, keys and lists namespace of a sync block.
fn data_namespaces(ext: &str, d: &SyncDecl) -> Vec<String> {
    let mut out = vec![keys_ns(ext)];
    out.extend(d.lists.keys().map(|k| list_ns(ext, k)));
    out.extend(d.collections.keys().map(|c| collection_ns(ext, c)));
    out
}

// --- The journal of windows' reads and saves -----------------------------------------

/// What a window read or saved of a key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    /// Keys mode: the value's hash.
    Hash([u8; 32]),
    /// Lists mode: the element ids.
    Ids(BTreeSet<String>),
    /// The key was absent, or deleted.
    Gone,
}

/// One thing a window did with a synced key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Load {
        window: String,
        ext: String,
        key: String,
        seen: Seen,
    },
    Save {
        window: String,
        ext: String,
        key: String,
        seen: Seen,
    },
    Forget {
        window: String,
    },
}

#[derive(Default)]
struct JournalInner {
    /// What syncs, by extension (set by the source each prepare).
    decls: HashMap<String, SyncDecl>,
    /// In the order they happened.
    events: Vec<Event>,
}

/// Which window loaded and saved which synced key, from the storage
/// commands to the engine thread (§12.4, §12.5), in order. Commands never
/// touch the replica; [`Source::prepare`] turns this into `reads` and
/// `ext_view` rows and into the bases of the captures.
#[derive(Default)]
pub struct Journal {
    inner: Mutex<JournalInner>,
}

/// Most events the journal keeps between two prepares; past it the
/// oldest go (a lost save of a lists key only keeps its elements).
const MAX_JOURNAL_EVENTS: usize = 8192;

impl Journal {
    fn push(j: &mut JournalInner, e: Event) {
        if j.events.len() >= MAX_JOURNAL_EVENTS {
            j.events.remove(0);
        }
        j.events.push(e);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JournalInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn seen_of(d: &SyncDecl, key: &str, data: Option<&str>) -> Option<Seen> {
        if let Some(ld) = d.lists.get(key) {
            return Some(match data {
                None => Seen::Gone,
                Some(text) => match serde_json::from_str::<Value>(text) {
                    Ok(v) => Seen::Ids(list_ids(&v, ld)),
                    Err(_) => Seen::Ids(BTreeSet::new()),
                },
            });
        }
        if d.key_syncs(key) {
            return Some(match data.map(serde_json::from_str::<Value>) {
                Some(Ok(v)) => Seen::Hash(hash(&v)),
                _ => Seen::Gone,
            });
        }
        None
    }

    /// Window `window` loaded `key` of `ext` (`None`: it had no value).
    pub fn note_load(&self, ext: &str, key: &str, window: &str, data: Option<&str>) {
        let mut j = self.lock();
        let Some(seen) = j.decls.get(ext).and_then(|d| Self::seen_of(d, key, data)) else {
            return;
        };
        let e = Event::Load {
            window: window.to_string(),
            ext: ext.to_string(),
            key: key.to_string(),
            seen,
        };
        // A load like the one just before it adds nothing.
        if j.events.last() != Some(&e) {
            Self::push(&mut j, e);
        }
    }

    /// Window `window` saved `key` of `ext` (`None`: deleted it).
    pub fn note_save(&self, ext: &str, key: &str, window: &str, data: Option<&str>) {
        let mut j = self.lock();
        let Some(seen) = j.decls.get(ext).and_then(|d| Self::seen_of(d, key, data)) else {
            return;
        };
        Self::push(
            &mut j,
            Event::Save {
                window: window.to_string(),
                ext: ext.to_string(),
                key: key.to_string(),
                seen,
            },
        );
    }

    /// A window was destroyed: its `reads` and `ext_view` rows go.
    pub fn forget_window(&self, window: &str) {
        let mut j = self.lock();
        if j.decls.is_empty() {
            return;
        }
        Self::push(
            &mut j,
            Event::Forget {
                window: window.to_string(),
            },
        );
    }

    fn set_decls(&self, decls: &BTreeMap<String, SyncDecl>) {
        let mut j = self.lock();
        j.decls = decls.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        j.events.retain(|e| match e {
            Event::Load { ext, .. } | Event::Save { ext, .. } => decls.contains_key(ext),
            Event::Forget { .. } => true,
        });
    }

    fn take(&self) -> Vec<Event> {
        std::mem::take(&mut self.lock().events)
    }

    fn has_saves(&self, ext: &str, key: &str) -> bool {
        self.lock()
            .events
            .iter()
            .any(|e| matches!(e, Event::Save { ext: x, key: k, .. } if x == ext && k == key))
    }
}

/// The saves of one keys-mode key in one journal batch.
#[derive(Default)]
struct KeySaves {
    /// What the last saving window had read: the stamp and value hash.
    base: Option<(Stamp, [u8; 32])>,
    savers: Vec<String>,
}

/// The app's journal: the storage commands write it, the app's extension
/// source reads it.
pub fn journal() -> Arc<Journal> {
    static J: std::sync::LazyLock<Arc<Journal>> = std::sync::LazyLock::new(Default::default);
    J.clone()
}

// --- Files ----------------------------------------------------------------------------

/// A data file's JSON value and the hash of its bytes, if it is readable
/// within `max` bytes.
enum FileRead {
    Missing,
    /// Too large, not JSON, or unreadable: leave it alone.
    Unusable,
    Value(Value, [u8; 32]),
}

fn read_json(path: &Path, max: u64) -> FileRead {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FileRead::Missing,
        Err(_) => return FileRead::Unusable,
        Ok(m) if m.len() > max || !m.is_file() => return FileRead::Unusable,
        Ok(_) => {}
    }
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileRead::Missing,
        Err(_) => FileRead::Unusable,
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => FileRead::Value(v, Sha256::digest(&bytes).into()),
            Err(_) => FileRead::Unusable,
        },
    }
}

/// The hash of a file's bytes, `None` when it doesn't exist.
fn file_hash(path: &Path) -> Option<[u8; 32]> {
    std::fs::read(path).ok().map(|b| Sha256::digest(&b).into())
}

/// The storage keys an extension has files for.
fn data_keys(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let key = name.strip_suffix(".json")?;
            crate::extensions::validate_data_key(key)
                .is_ok()
                .then(|| key.to_string())
        })
        .collect();
    out.sort();
    out
}

/// Write a data file atomically and owner-only, under its key lock, if it
/// still holds what `expect` says (`None`: don't check). `None` value:
/// remove it. Returns false when the file changed underneath.
fn write_data(
    root: &Path,
    ext: &str,
    key: &str,
    expect: Option<Option<[u8; 32]>>,
    value: Option<&Value>,
) -> anyhow::Result<bool> {
    let lock = crate::extensions::data_key_lock(ext, key);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let path = crate::extensions::resolve_extension_data_path(root, ext, key)?;
    if let Some(expect) = expect {
        if file_hash(&path) != expect {
            return Ok(false);
        }
    }
    match value {
        Some(v) => crate::atomic_write::write(
            &path,
            serde_json::to_string(v)?,
            crate::atomic_write::Perms::Private,
        )?,
        None => match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        },
    }
    Ok(true)
}

/// A lists-mode file, split.
#[derive(Debug, Clone, Default)]
struct ListFile {
    /// Elements in file order, by id.
    elements: Vec<(String, Value)>,
    /// The other top-level fields (`None` when the file is an array).
    rest: Option<Map<String, Value>>,
    /// Elements are too large or too many: they stay out of sync for now.
    unusable: bool,
}

/// The element ids of a list value.
fn list_ids(v: &Value, ld: &SyncListDecl) -> BTreeSet<String> {
    split_list(v, ld)
        .map(|l| l.elements.into_iter().map(|(id, _)| id).collect())
        .unwrap_or_default()
}

fn split_list(v: &Value, ld: &SyncListDecl) -> Option<ListFile> {
    let (array, rest) = if ld.array.is_empty() {
        (v.as_array()?, None)
    } else {
        let o = v.as_object()?;
        let mut rest = o.clone();
        rest.remove(&ld.array);
        let array = match o.get(&ld.array) {
            Some(Value::Array(a)) => a,
            None => {
                return Some(ListFile {
                    elements: Vec::new(),
                    rest: Some(rest),
                    unusable: false,
                })
            }
            Some(_) => return None,
        };
        (array, Some(rest))
    };
    let mut out = ListFile {
        rest,
        ..ListFile::default()
    };
    let mut seen = BTreeSet::new();
    for e in array {
        let Some(id) = e.get(&ld.id).and_then(Value::as_str) else {
            out.unusable = true;
            continue;
        };
        if !valid_element_id(id)
            || !seen.insert(id.to_string())
            || canonical(e).len() > MAX_ELEMENT_BYTES
        {
            out.unusable = true;
            continue;
        }
        out.elements.push((id.to_string(), e.clone()));
    }
    if out.elements.len() > MAX_ELEMENTS {
        out.unusable = true;
    }
    Some(out)
}

/// Put a list back together: `elements` in order, in the `array` field.
fn join_list(ld: &SyncListDecl, rest: Option<&Map<String, Value>>, elements: Vec<Value>) -> Value {
    if ld.array.is_empty() {
        return Value::Array(elements);
    }
    let mut o = rest.cloned().unwrap_or_default();
    o.insert(ld.array.clone(), Value::Array(elements));
    Value::Object(o)
}

/// A file's size and modification time.
type FileSig = (u64, Option<std::time::SystemTime>);

// --- The source -------------------------------------------------------------------------

/// Installs, extension settings and extension data.
pub struct ExtensionsSource {
    port: Arc<dyn ConfigPort>,
    /// `<config>/kage/extension-data`.
    data_dir: PathBuf,
    host: Option<Arc<dyn ExtensionHost>>,
    journal: Arc<Journal>,
    plan: Plan,
    /// The sync blocks of the extensions whose data syncs here.
    decls: BTreeMap<String, SyncDecl>,
    /// The bytes' hash of each data file `read_local` read, so `project`
    /// doesn't write over a save that came in between.
    seen: HashMap<(String, String), Option<[u8; 32]>>,
    /// What a window read of a key, by hash (keys mode).
    read_hashes: HashMap<(String, String, String), [u8; 32]>,
    /// Size and modification time of each lists file, to tell when one
    /// changed without a save through Kage.
    list_sigs: HashMap<(String, String), Option<FileSig>>,
    /// Record ids of list keys whose file is unusable, reported unchanged.
    frozen_lists: HashMap<(String, String), BTreeSet<String>>,
    /// Push tokens per extension: `(tokens, at)`.
    rate: HashMap<String, (f64, i64)>,
    themes_asked: BTreeSet<String>,
    origins_asked: BTreeSet<String>,
}

impl ExtensionsSource {
    /// The app's source, on the app's journal.
    pub fn new(
        port: Arc<dyn ConfigPort>,
        data_dir: PathBuf,
        host: Option<Arc<dyn ExtensionHost>>,
    ) -> Self {
        Self::with_journal(port, data_dir, host, journal())
    }

    /// A source on its own journal (one per simulated computer in tests).
    pub fn with_journal(
        port: Arc<dyn ConfigPort>,
        data_dir: PathBuf,
        host: Option<Arc<dyn ExtensionHost>>,
        journal: Arc<Journal>,
    ) -> Self {
        Self {
            port,
            data_dir,
            host,
            journal,
            plan: Plan::default(),
            decls: BTreeMap::new(),
            seen: HashMap::new(),
            read_hashes: HashMap::new(),
            list_sigs: HashMap::new(),
            frozen_lists: HashMap::new(),
            rate: HashMap::new(),
            themes_asked: BTreeSet::new(),
            origins_asked: BTreeSet::new(),
        }
    }

    fn data_path(&self, ext: &str, key: &str) -> PathBuf {
        self.data_dir.join(ext).join(format!("{key}.json"))
    }

    fn list_sig(&self, ext: &str, key: &str) -> Option<FileSig> {
        std::fs::metadata(self.data_path(ext, key))
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
    }

    /// Decide what syncs here (see [`Plan`]). Also lists installed
    /// extensions whose grant has no origin yet.
    fn compute_plan(
        &self,
        ctx: &dyn CaptureCtx,
        config: &Config,
        installed: &[InstalledItem],
    ) -> anyhow::Result<(Plan, BTreeMap<String, SyncDecl>, Vec<String>)> {
        let prefs = ctx.prefs();
        let mut plan = Plan::default();
        let mut decls = BTreeMap::new();
        let mut unknown = Vec::new();
        if !prefs.category_on(Category::Extensions) {
            return Ok((plan, decls, unknown));
        }
        for item in installed.iter().filter(|i| i.manifest.kind == "extension") {
            let ext = &item.manifest.id;
            let Some(grant) = config.extension_grants.get(ext) else {
                continue;
            };
            let Some(origin) = grant.origin.clone() else {
                unknown.push(ext.clone());
                continue;
            };
            let cell = origin_cell(ctx, ext)?;
            let switched_on = ctx.meta(&meta_data_on(ext))?.is_some();
            let sideload = origin == "sideload";
            let same = cell.as_deref().is_none_or(|c| c == origin) && (!sideload || switched_on);
            if !same {
                continue;
            }
            plan.settings.insert(ext.clone());
            for d in item.manifest.config_field_decls() {
                if d.sync == Some(SyncScopeDecl::Os) {
                    plan.per_os.insert(format!("{ext}/{}", d.field));
                }
            }
            let granted = |cap: &str| grant.granted.iter().any(|g| g == cap);
            let enabled = config.extension_states.get(ext).copied().unwrap_or(true);
            let Some(decl) = item.manifest.sync_decl() else {
                continue;
            };
            if granted("sync") && granted("storage") && enabled && !prefs.ext_data_is_off(ext) {
                plan.data.insert(ext.clone(), data_namespaces(ext, &decl));
                decls.insert(ext.clone(), decl);
            }
        }
        Ok((plan, decls, unknown))
    }

    fn installed_item<'a>(installed: &'a [InstalledItem], ext: &str) -> Option<&'a InstalledItem> {
        installed
            .iter()
            .find(|i| i.manifest.kind == "extension" && i.manifest.id == ext)
    }

    /// Settings records that don't apply here are pinned whole, so the
    /// engine leaves them alone; ones that apply again are projected.
    fn pin_settings(
        &self,
        ctx: &mut dyn CaptureCtx,
        rules: &Rules,
        installed: &[InstalledItem],
        out: &mut Prepared,
    ) -> anyhow::Result<()> {
        let os = ctx.os();
        for r in ctx.records(SETTINGS_NS)? {
            let Some((ext, field)) = r.id.split_once('/') else {
                continue;
            };
            let item = Self::installed_item(installed, ext);
            let value = eff_cell(&r, FIELD, os)
                .map(|c| c.value.clone())
                .unwrap_or(Value::Null);
            let reason = if item.is_none() {
                Some(PinReason::NotInstalled)
            } else if !self.plan.settings.contains(ext) {
                Some(PinReason::OriginMismatch)
            } else if !value.is_null() && !setting_syncs(rules, item, ext, field, &value) {
                Some(PinReason::ThisComputer)
            } else {
                None
            };
            set_away(ctx, SETTINGS_NS, &r.id, reason, out)?;
        }
        Ok(())
    }

    /// Keys of an extension whose data syncs here that its manifest here
    /// doesn't list (or calls sensitive) are pinned whole.
    fn pin_keys(
        &self,
        ctx: &mut dyn CaptureCtx,
        rules: &Rules,
        out: &mut Prepared,
    ) -> anyhow::Result<()> {
        for (ext, decl) in &self.decls {
            let ns = keys_ns(ext);
            for r in ctx.records(&ns)? {
                let ok = decl.key_syncs(&r.id) && !rules.is_sensitive_data(ext, &r.id);
                set_away(
                    ctx,
                    &ns,
                    &r.id,
                    (!ok).then_some(PinReason::ThisComputer),
                    out,
                )?;
            }
        }
        Ok(())
    }

    /// Install records of items not installed here are pinned whole; a
    /// store theme from a store this computer uses is installed.
    fn pin_installs(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        config: &Config,
        installed: &[InstalledItem],
        out: &mut Prepared,
    ) -> anyhow::Result<()> {
        let here: BTreeMap<String, &InstalledItem> =
            installed.iter().map(|i| (install_id(i), i)).collect();
        let stores = local_store_origins(config);
        let me = ctx.device();
        let absent = hash(&Value::Null);
        for r in ctx.records(INSTALLS_NS)? {
            let is_here = here.contains_key(&r.id);
            // Shown here before and gone now: it was uninstalled here, and
            // the engine's capture records the removal (§4.5).
            let removed_here = !is_here
                && is_visible(&r)
                && ctx
                    .projected_field(INSTALLS_NS, &r.id, FIELD)?
                    .is_some_and(|p| p.0 != absent);
            if !removed_here {
                set_away(
                    ctx,
                    INSTALLS_NS,
                    &r.id,
                    (!is_here).then_some(PinReason::NotInstalled),
                    out,
                )?;
            }
            if is_here && !is_visible(&r) && r.del.as_ref().is_some_and(|t| t.stamp.dev == me) {
                // Removed here, now installed here again: it is back on
                // this computer, whatever version it had.
                if let Some(v) = here.get(&r.id).and_then(|i| install_value(i, config)) {
                    ctx.write_cell(INSTALLS_NS, &r.id, FIELD, v)?;
                    point_projected(ctx, INSTALLS_NS, &r.id)?;
                }
            }
            if is_here || !is_visible(&r) {
                continue;
            }
            let Some(v) = eff_cell(&r, FIELD, ctx.os()).map(|c| c.value.clone()) else {
                continue;
            };
            let Some(theme) = r.id.strip_prefix("theme:") else {
                continue;
            };
            let store = v
                .get("store")
                .and_then(Value::as_str)
                .map(|h| format!("store:{h}"));
            let excluded = ctx
                .prefs()
                .install_exclusions
                .iter()
                .any(|e| e.ends_with(&r.id));
            if let (Some(store), Some(host)) = (store, self.host.as_ref()) {
                if v.get("source").and_then(Value::as_str) == Some("store")
                    && stores.contains(&store)
                    && !excluded
                    && self.themes_asked.insert(theme.to_string())
                {
                    log::info!("sync: installing the synced theme {theme} from its store");
                    host.install_store_theme(theme, &store);
                }
            }
        }
        Ok(())
    }

    /// Act on extensions whose data started or stopped syncing here since
    /// the last prepare, and write the origin cells this computer is the
    /// first to publish.
    fn transitions(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        out: &mut Prepared,
        config: &Config,
    ) -> anyhow::Result<()> {
        let before: Plan = ctx
            .meta(META_PLAN)?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        for ext in self.plan.settings.iter().chain(self.plan.data.keys()) {
            if origin_cell(ctx, ext)?.is_none() {
                if let Some(o) = config
                    .extension_grants
                    .get(ext)
                    .and_then(|g| g.origin.clone())
                {
                    ctx.write_cell(STATE_NS, &origin_record(ext), "origin", Value::String(o))?;
                }
            }
        }
        for (ext, namespaces) in &self.plan.data {
            if before.data.get(ext) == Some(namespaces) {
                continue;
            }
            log::info!("sync: {ext}'s data syncs on this computer now");
            ctx.set_meta(&meta_initial_done(ext), None)?;
            for ns in namespaces {
                if let Some((_, coll)) = crate::sync::engine::collection_of(ns) {
                    // The extension pulls everything the folder has: what
                    // it saw before doesn't count any more.
                    for r in ctx.records(ns)? {
                        ctx.store()
                            .set_ext_base(ext, coll, &r.id, &BTreeMap::new())?;
                        if is_visible(&r) {
                            ctx.store().feed_push(ext, coll, &r.id)?;
                        }
                    }
                    ctx.sync_changes(ext, coll, "enabled");
                } else {
                    out.project.insert((ns.clone(), String::new()));
                }
            }
            ctx.sync_changes(ext, "", "enabled");
        }
        for (ext, namespaces) in &before.data {
            let stays = self.plan.data.get(ext);
            for ns in namespaces {
                if stays.is_some_and(|s| s.contains(ns))
                    || crate::sync::engine::collection_of(ns).is_some()
                {
                    continue;
                }
                // What the files showed no longer counts: a reinstall
                // starts from the synced values, not from deletes.
                for r in ctx.records(ns)? {
                    ctx.set_projected(ns, &r.id, FIELD, None)?;
                }
            }
        }
        if before != self.plan {
            ctx.set_meta(META_PLAN, Some(&serde_json::to_string(&self.plan)?))?;
        }
        Ok(())
    }

    /// Turn the journal into `reads` and `ext_view` rows, and capture the
    /// saves with the saving windows' bases.
    fn apply_journal(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        origin: Origin,
        out: &mut Prepared,
    ) -> anyhow::Result<()> {
        // Events in the order they happened: each save is judged against
        // what its window had seen at that moment. The files hold the last
        // save, so each key is captured once, after the loop.
        let mut key_saves: BTreeMap<(String, String), KeySaves> = BTreeMap::new();
        let mut list_deletes: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for event in self.journal.take() {
            match event {
                Event::Forget { window } => {
                    ctx.store().forget_window(&window)?;
                    self.read_hashes.retain(|(w, _, _), _| *w != window);
                }
                Event::Load {
                    window,
                    ext,
                    key,
                    seen,
                } => {
                    let Some(decl) = self.decls.get(&ext) else {
                        continue;
                    };
                    if decl.lists.contains_key(&key) {
                        let ids = match &seen {
                            Seen::Ids(ids) => ids.clone(),
                            _ => BTreeSet::new(),
                        };
                        ctx.store().set_ext_view(&window, &ext, &key, &ids)?;
                        continue;
                    }
                    let stamp = match &seen {
                        Seen::Hash(h) => stamp_of_hash(ctx, &keys_ns(&ext), &key, h)?,
                        _ => None,
                    };
                    ctx.store().set_read_stamp(&window, &ext, &key, stamp)?;
                    match seen {
                        Seen::Hash(h) => self.read_hashes.insert((window, ext, key), h),
                        _ => self.read_hashes.remove(&(window, ext, key)),
                    };
                }
                Event::Save {
                    window,
                    ext,
                    key,
                    seen,
                } => {
                    let Some(decl) = self.decls.get(&ext) else {
                        continue;
                    };
                    if decl.lists.contains_key(&key) {
                        let saved = match seen {
                            Seen::Ids(ids) => ids,
                            _ => BTreeSet::new(),
                        };
                        let view = ctx.store().ext_view(&window, &ext, &key)?;
                        let deleted = list_deletes.entry((ext.clone(), key.clone())).or_default();
                        deleted.extend(view.difference(&saved).cloned());
                        deleted.retain(|id| !saved.contains(id));
                        ctx.store().set_ext_view(&window, &ext, &key, &saved)?;
                        continue;
                    }
                    let base = ctx.store().read_stamp(&window, &ext, &key)?.map(|s| {
                        let h = self
                            .read_hashes
                            .get(&(window.clone(), ext.clone(), key.clone()))
                            .copied()
                            .unwrap_or([0; 32]);
                        (s, h)
                    });
                    let e = key_saves.entry((ext, key)).or_default();
                    e.base = base;
                    e.savers.push(window);
                }
            }
        }
        for ((ext, key), saves) in key_saves {
            self.key_saved(ctx, &ext, &key, &saves, origin, out)?;
        }
        for ((ext, key), deleted) in list_deletes {
            let Some(ld) = self
                .decls
                .get(&ext)
                .and_then(|d| d.lists.get(&key))
                .cloned()
            else {
                continue;
            };
            self.list_saved(ctx, &ext, &key, &ld, deleted, origin, out)?;
        }
        Ok(())
    }

    /// A keys-mode key was saved: capture it against the last saving
    /// window's base (§12.4).
    fn key_saved(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        ext: &str,
        key: &str,
        saves: &KeySaves,
        origin: Origin,
        out: &mut Prepared,
    ) -> anyhow::Result<()> {
        let ns = keys_ns(ext);
        let path = self.data_path(ext, key);
        let file = read_json(&path, MAX_KEY_BYTES);
        if matches!(file, FileRead::Unusable) {
            return Ok(());
        }
        let shown = ctx.projected_field(&ns, key, FIELD)?;
        if let Some((rs, h)) = saves.base {
            if shown.and_then(|p| p.1) != Some(rs) {
                // The window saved over what it read, which is older than
                // what the file showed since.
                ctx.set_projected(&ns, key, FIELD, Some((h, Some(rs), Some(Scope::All))))?;
            }
        }
        let value = match file {
            FileRead::Value(v, _) => Some(v),
            _ => None,
        };
        let r = ctx.capture(&ns, key, value.clone().map(rec_of).as_ref(), origin, None)?;
        if r == CaptureResult::Written {
            out.written += 1;
        }
        if let (CaptureResult::Unchanged, Some(v)) = (r, value.as_ref()) {
            // The window saved what it read: the newer value stays, and
            // the file gets it back.
            if let Some(m) = ctx
                .merged(&ns, key)?
                .and_then(|m| m.fields.get(FIELD).cloned())
            {
                if !same(&m, v)
                    && write_data(&self.data_dir, ext, key, Some(file_hash(&path)), Some(&m))?
                {
                    point_projected(ctx, &ns, key)?;
                    ctx.data_changed(ext, key);
                }
            }
        }
        // Every saving window has now seen what the file holds.
        let stamp = ctx.projected_field(&ns, key, FIELD)?.and_then(|p| p.1);
        let now_hash = match read_json(&path, MAX_KEY_BYTES) {
            FileRead::Value(v, _) => Some(hash(&v)),
            _ => None,
        };
        for w in &saves.savers {
            ctx.store().set_read_stamp(w, ext, key, stamp)?;
            let k = (w.clone(), ext.to_string(), key.to_string());
            match now_hash {
                Some(h) => self.read_hashes.insert(k, h),
                None => self.read_hashes.remove(&k),
            };
        }
        Ok(())
    }

    /// A lists-mode key was saved: delete only what the saving windows had
    /// seen and dropped (`deleted`), keep what they hadn't (§12.5).
    #[allow(clippy::too_many_arguments)]
    fn list_saved(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        ext: &str,
        key: &str,
        ld: &SyncListDecl,
        mut deleted: BTreeSet<String>,
        origin: Origin,
        out: &mut Prepared,
    ) -> anyhow::Result<()> {
        let ns = list_ns(ext, key);
        let path = self.data_path(ext, key);
        let file = match read_json(&path, MAX_LIST_FILE_BYTES) {
            FileRead::Unusable => return Ok(()),
            FileRead::Missing => None,
            FileRead::Value(v, _) => match split_list(&v, ld) {
                Some(l) if !l.unusable => Some(l),
                _ => return Ok(()),
            },
        };
        let in_file: BTreeSet<String> = file
            .as_ref()
            .map(|l| l.elements.iter().map(|(id, _)| id.clone()).collect())
            .unwrap_or_default();
        deleted.retain(|id| !in_file.contains(id));
        for id in &deleted {
            let Some(r) = ctx.record(&ns, id)? else {
                continue;
            };
            if !is_visible(&r) {
                continue;
            }
            if ctx.projected_field(&ns, id, FIELD)?.is_none() {
                // Shown to the window, if not yet recorded as shown here.
                point_projected(ctx, &ns, id)?;
            }
            if ctx.capture(&ns, id, None, origin, None)? == CaptureResult::Written {
                out.written += 1;
            }
        }
        self.restore_unseen(ctx, ext, key, ld, file, &deleted)?;
        self.list_sigs
            .insert((ext.to_string(), key.to_string()), self.list_sig(ext, key));
        Ok(())
    }

    /// Write back the synced elements a lists file lacks that no window
    /// deleted: at the end, oldest change first.
    fn restore_unseen(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        ext: &str,
        key: &str,
        ld: &SyncListDecl,
        file: Option<ListFile>,
        deleted: &BTreeSet<String>,
    ) -> anyhow::Result<()> {
        let ns = list_ns(ext, key);
        let path = self.data_path(ext, key);
        let in_file: BTreeSet<String> = file
            .as_ref()
            .map(|l| l.elements.iter().map(|(id, _)| id.clone()).collect())
            .unwrap_or_default();
        let mut missing: Vec<(Stamp, String, Value)> = Vec::new();
        let mut rest = file.as_ref().and_then(|l| l.rest.clone());
        for r in ctx.records(&ns)? {
            if !is_visible(&r) || in_file.contains(&r.id) || deleted.contains(&r.id) {
                continue;
            }
            let Some(m) = ctx
                .merged(&ns, &r.id)?
                .and_then(|m| m.fields.get(FIELD).cloned())
            else {
                continue;
            };
            if r.id == REST_ID {
                if file.is_none() && !ld.array.is_empty() {
                    rest = m.as_object().cloned();
                }
                continue;
            }
            let at = r.max_stamp().unwrap_or(Stamp::new(0, ctx.device()));
            missing.push((at, r.id.clone(), m));
        }
        if missing.is_empty() {
            return Ok(());
        }
        missing.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut elements: Vec<Value> = file
            .map(|l| l.elements.into_iter().map(|(_, e)| e).collect())
            .unwrap_or_default();
        let added: Vec<String> = missing.iter().map(|(_, id, _)| id.clone()).collect();
        elements.extend(missing.into_iter().map(|(_, _, e)| e));
        let value = join_list(ld, rest.as_ref(), elements);
        if !write_data(
            &self.data_dir,
            ext,
            key,
            Some(file_hash(&path)),
            Some(&value),
        )? {
            return Ok(());
        }
        log::info!(
            "sync: kept {} element(s) of {ext}/{key} the saving window hadn't seen",
            added.len()
        );
        for id in added.iter().chain(std::iter::once(&REST_ID.to_string())) {
            if ctx.record(&ns, id)?.is_some() {
                point_projected(ctx, &ns, id)?;
            }
        }
        self.list_sigs
            .insert((ext.to_string(), key.to_string()), self.list_sig(ext, key));
        ctx.data_changed(ext, key);
        Ok(())
    }

    /// Lists files that changed without a save through Kage (a restart
    /// between a save and its capture, an edit by hand) lose no synced
    /// element: missing ones come back.
    fn check_lists(&mut self, ctx: &mut dyn CaptureCtx) -> anyhow::Result<()> {
        let decls = self.decls.clone();
        for (ext, decl) in &decls {
            for (key, ld) in &decl.lists {
                let k = (ext.clone(), key.clone());
                let sig = self.list_sig(ext, key);
                if self.list_sigs.get(&k) == Some(&sig) || self.journal.has_saves(ext, key) {
                    continue;
                }
                self.list_sigs.insert(k.clone(), sig);
                let file = match read_json(&self.data_path(ext, key), MAX_LIST_FILE_BYTES) {
                    FileRead::Unusable => None,
                    FileRead::Missing => Some(None),
                    FileRead::Value(v, _) => match split_list(&v, ld) {
                        Some(l) if !l.unusable => Some(Some(l)),
                        _ => None,
                    },
                };
                match file {
                    Some(file) => {
                        self.frozen_lists.remove(&k);
                        self.restore_unseen(ctx, ext, key, ld, file, &BTreeSet::new())?;
                    }
                    None => {
                        // Unusable: every synced element stays as it is.
                        let ids = ctx
                            .records(&list_ns(ext, key))?
                            .into_iter()
                            .map(|r| r.id)
                            .collect();
                        log::warn!("sync: {ext}/{key} is over the lists-mode limits; it doesn't sync for now");
                        self.frozen_lists.insert(k, ids);
                    }
                }
            }
        }
        Ok(())
    }

    // --- Collections (§12.6) ---------------------------------------------------------

    fn ext_status(&mut self, ctx: &mut dyn CaptureCtx, ext: &str) -> anyhow::Result<ExtSyncStatus> {
        let on = self.decls.contains_key(ext);
        let me = ctx.device();
        let used = used_bytes(ctx, ext, &self.decls)?;
        let paused = ctx.paused() || used > MAX_EXTENSION_BYTES;
        let mut devices = vec![ExtDevice {
            tag: ctx.keys().ext_device_tag(ext, me),
            os: ctx.os(),
            last_seen_at: ctx.now(),
            this_device: true,
        }];
        for d in ctx.devices()? {
            if d.dev == me || d.state == "retired" {
                continue;
            }
            devices.push(ExtDevice {
                tag: ctx.keys().ext_device_tag(ext, d.dev),
                os: d.os.unwrap_or(ctx.os()),
                last_seen_at: d.last_seen_ms,
                this_device: false,
            });
        }
        Ok(ExtSyncStatus {
            available: on,
            paused,
            needs_initial_push: on && ctx.meta(&meta_initial_done(ext))?.is_none(),
            device_tag: ctx.keys().ext_device_tag(ext, me),
            devices,
            used_bytes: used,
            max_bytes: MAX_EXTENSION_BYTES,
            last_sync_at: ctx.last_sync_at(),
        })
    }

    /// Take `n` push tokens of `ext`; the ones there aren't are refused.
    fn take_tokens(&mut self, ext: &str, n: usize, now: i64) -> usize {
        let (tokens, at) = self
            .rate
            .entry(ext.to_string())
            .or_insert((PUSH_PER_MINUTE, now));
        let refill = (now - *at).max(0) as f64 / 60_000.0 * PUSH_PER_MINUTE;
        *tokens = (*tokens + refill).min(PUSH_PER_MINUTE);
        *at = now;
        let granted = (*tokens as usize).min(n);
        *tokens -= granted as f64;
        granted
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        ext: &str,
        collection: &str,
        records: &[Value],
        deleted: &[String],
        initial: bool,
    ) -> anyhow::Result<PushResult> {
        let mut res = PushResult::default();
        let Some(decl) = self.decls.get(ext).cloned() else {
            res.off = true;
            return Ok(res);
        };
        let reject_all = |res: &mut PushResult, code: RejectCode| {
            for r in records {
                let id = r.get("id").and_then(Value::as_str).unwrap_or_default();
                res.rejected.push(Rejected {
                    id: id.to_string(),
                    code,
                });
            }
            for id in deleted {
                res.rejected.push(Rejected {
                    id: id.clone(),
                    code,
                });
            }
        };
        let Some(cd) = decl.collections.get(collection).cloned() else {
            reject_all(&mut res, RejectCode::BadCollection);
            return Ok(res);
        };
        let ns = collection_ns(ext, collection);
        let newer = ctx
            .meta(&format!("schema.{ns}"))?
            .and_then(|v| v.parse::<u32>().ok())
            .is_some_and(|seen| seen > decl.schema);
        if newer {
            reject_all(&mut res, RejectCode::NewerSchema);
            return Ok(res);
        }
        let mut used = used_bytes(ctx, ext, &self.decls)?;
        if used > MAX_EXTENSION_BYTES {
            reject_all(&mut res, RejectCode::Quota);
            return Ok(res);
        }
        let now = ctx.now();
        let mut tokens = self.take_tokens(ext, records.len() + deleted.len(), now);
        let structure = collection_structure(&cd);
        let mut feed_again = false;
        for raw in records {
            let id = raw
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let code = check_record(raw, &cd);
            if let Some(code) = code {
                res.rejected.push(Rejected { id, code });
                continue;
            }
            if tokens == 0 {
                res.rejected.push(Rejected {
                    id,
                    code: RejectCode::RateLimited,
                });
                continue;
            }
            tokens -= 1;
            let mut fields = raw.as_object().cloned().unwrap_or_default();
            fields.remove("id");
            fields.retain(|k, _| !cd.local.contains(k));
            let size = canonical(&Value::Object(fields.clone())).len() as u64;
            let before = ctx.merged(&ns, &id)?;
            let before_size = before.as_ref().map_or(0, |m| {
                canonical(&Value::Object(m.fields.clone())).len() as u64
            });
            if used + size > MAX_EXTENSION_BYTES + before_size {
                res.rejected.push(Rejected {
                    id,
                    code: RejectCode::Quota,
                });
                continue;
            }
            let local = LocalRecord {
                fields: fields.clone(),
                avail: None,
            };
            if before
                .as_ref()
                .is_some_and(|m| same_cells(&m.fields, &fields, &structure))
            {
                // Equal to the merged value: nothing to store, and the
                // extension now holds the current cells.
                set_base_to_current(ctx, &ns, ext, collection, &id)?;
                res.unchanged += 1;
                continue;
            }
            let initial_ms = initial.then(|| updated_at(raw).unwrap_or(0).min(now));
            let names = cell_names_of(ctx, &ns, &id, &fields, &structure)?;
            load_base(ctx, &ns, ext, collection, &id, &names)?;
            let r = ctx.capture(&ns, &id, Some(&local), Origin::User, initial_ms)?;
            save_base(ctx, &ns, ext, collection, &id, &names)?;
            match r {
                CaptureResult::Rejected(code) => {
                    let code = match code {
                        "too_large" => RejectCode::TooLarge,
                        "sensitive_field" => RejectCode::SensitiveField,
                        _ => RejectCode::BadCollection,
                    };
                    res.rejected.push(Rejected { id, code });
                    continue;
                }
                CaptureResult::Written => res.accepted += 1,
                CaptureResult::Unchanged => res.unchanged += 1,
            }
            used = (used + size).saturating_sub(before_size);
            let after = ctx.merged(&ns, &id)?;
            let kept_remote = after
                .as_ref()
                .is_none_or(|m| !same_cells(&m.fields, &fields, &structure));
            if kept_remote {
                if let Some(ms) = initial_ms {
                    // The folder's copy was newer: the extension still holds
                    // its own, so the record must reach it through pull.
                    mark_initial_losers(ctx, &ns, ext, collection, &id, &fields, &structure, ms)?;
                }
                ctx.store().feed_push(ext, collection, &id)?;
                feed_again = true;
            }
        }
        for id in deleted {
            if !valid_record_id(id) {
                res.rejected.push(Rejected {
                    id: id.clone(),
                    code: RejectCode::BadId,
                });
                continue;
            }
            if tokens == 0 {
                res.rejected.push(Rejected {
                    id: id.clone(),
                    code: RejectCode::RateLimited,
                });
                continue;
            }
            tokens -= 1;
            let Some(r) = ctx.record(&ns, id)? else {
                res.unchanged += 1;
                continue;
            };
            if !is_visible(&r) {
                res.unchanged += 1;
                continue;
            }
            let before_size = ctx
                .merged(&ns, id)?
                .map_or(0, |m| canonical(&Value::Object(m.fields)).len() as u64);
            let names = cell_names_of(ctx, &ns, id, &Map::new(), &structure)?;
            load_base(ctx, &ns, ext, collection, id, &names)?;
            if names
                .iter()
                .all(|n| matches!(ctx.projected_field(&ns, id, n), Ok(None)))
            {
                // The extension deletes what it holds; with no base kept
                // (a rebuilt replica), that is the current record.
                point_projected_all(ctx, &ns, id, &names)?;
            }
            let w = ctx.capture(&ns, id, None, Origin::User, None)?;
            save_base(ctx, &ns, ext, collection, id, &names)?;
            if w == CaptureResult::Written {
                res.accepted += 1;
                used = used.saturating_sub(before_size);
            } else {
                res.unchanged += 1;
            }
        }
        ctx.set_meta(&meta_bytes(ext), Some(&used.to_string()))?;
        if feed_again {
            ctx.sync_changes(ext, collection, "remote");
        }
        Ok(res)
    }

    fn pull(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        ext: &str,
        max: usize,
    ) -> anyhow::Result<PullResult> {
        let Some(decl) = self.decls.get(ext).cloned() else {
            return Ok(PullResult {
                cursor: "0".into(),
                more: false,
                changes: Vec::new(),
            });
        };
        let max = max.clamp(1, MAX_PULL);
        let mut out = PullResult::default();
        let mut after = 0u64;
        // Rows the extension already holds are skipped, but count towards
        // the cursor, so an ack clears them.
        'pages: loop {
            let rows = ctx.store().feed_page(ext, after, 256)?;
            if rows.is_empty() {
                break;
            }
            for (seq, coll, id) in &rows {
                if out.changes.len() == max {
                    out.more = true;
                    break 'pages;
                }
                after = *seq;
                let ns = collection_ns(ext, coll);
                let Some(cd) = decl.collections.get(coll) else {
                    continue;
                };
                let newer = ctx
                    .meta(&format!("schema.{ns}"))?
                    .and_then(|v| v.parse::<u32>().ok())
                    .is_some_and(|seen| seen > decl.schema);
                if newer {
                    continue;
                }
                let Some(r) = ctx.record(&ns, id)? else {
                    continue;
                };
                let merged = ctx.merged(&ns, id)?;
                let mut base = ctx.store().ext_base(ext, coll, id)?;
                // Absent values: `null`, or `false` for a removed set
                // element. A plain `false` field is a value.
                let (null, no) = (hash(&Value::Null), hash(&Value::Bool(false)));
                base.retain(|n, (_, h)| *h != null && !(*h == no && split::is_union(n)));
                let structure = collection_structure(cd);
                let held = match &merged {
                    Some(m) => {
                        let cells = split::split(&m.fields, &structure);
                        cells.len() == base.len()
                            && cells
                                .iter()
                                .all(|(n, v)| base.get(n).is_some_and(|(_, h)| *h == hash(v)))
                    }
                    None => base.is_empty(),
                };
                if held {
                    continue;
                }
                let (stamp, wall) = newest(&r);
                let record = merged.map(|m| {
                    let mut f = m.fields;
                    f.insert("id".into(), Value::String(id.clone()));
                    Value::Object(f)
                });
                out.changes.push(ExtChange {
                    collection: coll.clone(),
                    id: id.clone(),
                    record,
                    from: ChangeFrom {
                        device: stamp
                            .map(|s| ctx.keys().ext_device_tag(ext, s.dev))
                            .unwrap_or_default(),
                        at: wall,
                    },
                });
            }
        }
        out.cursor = after.to_string();
        Ok(out)
    }

    fn ack(&mut self, ctx: &mut dyn CaptureCtx, ext: &str, cursor: &str) -> Result<(), AppError> {
        let upto: u64 = cursor
            .parse()
            .map_err(|_| AppError::keyed(ErrorKind::InvalidInput, "errors.sync.bad_cursor", &[]))?;
        let decl = self.decls.get(ext).cloned();
        ack_rows(ctx, ext, upto, decl.as_ref()).map_err(crate::sync::engine::internal)
    }
}

/// Ack every feed row of `ext` up to `upto`: the extension now holds those
/// records' current values (§12.6), and its initial push is done.
fn ack_rows(
    ctx: &mut dyn CaptureCtx,
    ext: &str,
    upto: u64,
    decl: Option<&SyncDecl>,
) -> anyhow::Result<()> {
    let mut after = 0u64;
    'pages: loop {
        let rows = ctx.store().feed_page(ext, after, 1000)?;
        if rows.is_empty() {
            break;
        }
        for (seq, coll, id) in &rows {
            if *seq > upto {
                break 'pages;
            }
            after = *seq;
            if decl.is_some_and(|d| d.collections.contains_key(coll)) {
                set_base_to_current(ctx, &collection_ns(ext, coll), ext, coll, id)?;
            }
        }
    }
    ctx.store().feed_ack(ext, upto)?;
    if decl.is_some() {
        ctx.set_meta(&meta_initial_done(ext), Some("1"))?;
    }
    Ok(())
}

/// Set or lift a whole-record pin of this source's (`reason`), so the
/// engine leaves a record that doesn't apply here alone. A lifted pin's
/// record is projected.
fn set_away(
    ctx: &mut dyn CaptureCtx,
    ns: &str,
    id: &str,
    reason: Option<PinReason>,
    out: &mut Prepared,
) -> anyhow::Result<()> {
    let mine = |r: PinReason| {
        matches!(
            r,
            PinReason::NotInstalled | PinReason::OriginMismatch | PinReason::ThisComputer
        )
    };
    let cur = ctx.pin(ns, id, "*")?;
    match (reason, cur) {
        (Some(re), None) => {
            ctx.set_pin(ns, id, "*", Some((Value::Null, re)))?;
            ctx.set_projected(ns, id, FIELD, None)?;
        }
        (Some(re), Some((v, old))) if old != re && mine(old) => {
            ctx.set_pin(ns, id, "*", Some((v, re)))?;
        }
        (None, Some((_, old))) if mine(old) => {
            ctx.set_pin(ns, id, "*", None)?;
            out.project.insert((ns.to_string(), id.to_string()));
        }
        _ => {}
    }
    Ok(())
}

/// The origin cell of `ext`, if one was published.
fn origin_cell(ctx: &dyn CaptureCtx, ext: &str) -> anyhow::Result<Option<String>> {
    Ok(ctx
        .record(STATE_NS, &origin_record(ext))?
        .and_then(|r| r.cell("origin", Scope::All).cloned())
        .and_then(|c| c.value.as_str().map(str::to_string)))
}

/// The origins of the stores this computer installs from.
fn local_store_origins(config: &Config) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(u) = config.store_url.as_deref().filter(|u| !u.is_empty()) {
        out.insert(store_origin(u));
    }
    for u in crate::commands::extensions::default_store_urls() {
        out.insert(store_origin(u));
    }
    for s in &config.store_sources {
        if s.enabled && !s.url.is_empty() {
            out.insert(store_origin(&s.url));
        }
    }
    out
}

/// The stamp of the cell whose value hashes to `h`: what the file showed,
/// or the current value.
fn stamp_of_hash(
    ctx: &dyn CaptureCtx,
    ns: &str,
    id: &str,
    h: &[u8; 32],
) -> anyhow::Result<Option<Stamp>> {
    if let Some((ph, Some(s), _)) = ctx.projected_field(ns, id, FIELD)? {
        if ph == *h {
            return Ok(Some(s));
        }
    }
    Ok(ctx
        .record(ns, id)?
        .and_then(|r| eff_cell(&r, FIELD, ctx.os()).cloned())
        .filter(|c| hash(&c.value) == *h)
        .map(|c| c.stamp))
}

/// Record that the file now shows record `id`'s current value of `v`.
fn point_projected(ctx: &mut dyn CaptureCtx, ns: &str, id: &str) -> anyhow::Result<()> {
    let names: BTreeSet<String> = [FIELD.to_string()].into();
    point_projected_all(ctx, ns, id, &names)
}

fn point_projected_all(
    ctx: &mut dyn CaptureCtx,
    ns: &str,
    id: &str,
    names: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let Some(r) = ctx.record(ns, id)? else {
        return Ok(());
    };
    let os = ctx.os();
    for name in names {
        if let Some(c) = eff_cell(&r, name, os) {
            ctx.set_projected(
                ns,
                id,
                name,
                Some((hash(&c.value), Some(c.stamp), Some(Scope::All))),
            )?;
        }
    }
    Ok(())
}

/// The cell names a capture of a collection record looks at.
fn cell_names_of(
    ctx: &dyn CaptureCtx,
    ns: &str,
    id: &str,
    fields: &Map<String, Value>,
    s: &Structure,
) -> anyhow::Result<BTreeSet<String>> {
    let mut names: BTreeSet<String> = split::split(fields, s).into_keys().collect();
    if let Some(r) = ctx.record(ns, id)? {
        names.extend(r.cells.keys().map(|k| k.name.clone()));
    }
    Ok(names)
}

/// Put the extension's base of a record where the engine's capture reads
/// its base: the `projected` rows.
fn load_base(
    ctx: &mut dyn CaptureCtx,
    ns: &str,
    ext: &str,
    coll: &str,
    id: &str,
    names: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let base = ctx.store().ext_base(ext, coll, id)?;
    let all: BTreeSet<&String> = names.iter().chain(base.keys()).collect();
    for name in all {
        let p = base
            .get(name)
            .map(|(s, h)| (*h, Some(*s), Some(Scope::All)));
        ctx.set_projected(ns, id, name, p)?;
    }
    Ok(())
}

/// Move the `projected` rows a capture left back into `ext_base`.
fn save_base(
    ctx: &mut dyn CaptureCtx,
    ns: &str,
    ext: &str,
    coll: &str,
    id: &str,
    names: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let mut all = names.clone();
    all.extend(ctx.store().ext_base(ext, coll, id)?.into_keys());
    if let Some(r) = ctx.record(ns, id)? {
        all.extend(r.cells.keys().map(|k| k.name.clone()));
    }
    let mut base = BTreeMap::new();
    for name in all {
        if let Some((h, Some(s), _)) = ctx.projected_field(ns, id, &name)? {
            base.insert(name.clone(), (s, h));
        }
        ctx.set_projected(ns, id, &name, None)?;
    }
    ctx.store().set_ext_base(ext, coll, id, &base)
}

/// The extension now holds a record's current value: its base is the
/// current cells (never moved back past a newer one, §12.6).
fn set_base_to_current(
    ctx: &mut dyn CaptureCtx,
    ns: &str,
    ext: &str,
    coll: &str,
    id: &str,
) -> anyhow::Result<()> {
    let Some(r) = ctx.record(ns, id)? else {
        return ctx.store().set_ext_base(ext, coll, id, &BTreeMap::new());
    };
    if !is_visible(&r) {
        return ctx.store().set_ext_base(ext, coll, id, &BTreeMap::new());
    }
    let mut base = ctx.store().ext_base(ext, coll, id)?;
    let os = ctx.os();
    let names: BTreeSet<String> = r.cells.keys().map(|k| k.name.clone()).collect();
    base.retain(|n, _| names.contains(n));
    for name in names {
        let Some(c) = eff_cell(&r, &name, os) else {
            continue;
        };
        if c.value.is_null() || (c.value == Value::Bool(false) && split::is_union(&name)) {
            base.remove(&name);
            continue;
        }
        match base.get(&name) {
            Some((s, _)) if *s > c.stamp => {}
            _ => {
                base.insert(name, (c.stamp, hash(&c.value)));
            }
        }
    }
    ctx.store().set_ext_base(ext, coll, id, &base)
}

/// After an initial push the folder's copy won: the extension's base for
/// the fields it lost is its own value at its own stamp, so pull delivers
/// the winner and a later edit is concurrent with it.
#[allow(clippy::too_many_arguments)]
fn mark_initial_losers(
    ctx: &mut dyn CaptureCtx,
    ns: &str,
    ext: &str,
    coll: &str,
    id: &str,
    fields: &Map<String, Value>,
    s: &Structure,
    ms: i64,
) -> anyhow::Result<()> {
    let mine = split::split(fields, s);
    let Some(r) = ctx.record(ns, id)? else {
        return Ok(());
    };
    let os = ctx.os();
    let own = Stamp::new(crate::sync::hlc::physical(ms), ctx.device());
    let mut base = ctx.store().ext_base(ext, coll, id)?;
    for (name, v) in &mine {
        let winner = eff_cell(&r, name, os);
        if winner.is_some_and(|c| !same(&c.value, v)) {
            base.insert(name.clone(), (own, hash(v)));
        }
    }
    ctx.store().set_ext_base(ext, coll, id, &base)
}

/// Whether two records split into the same cells.
fn same_cells(a: &Map<String, Value>, b: &Map<String, Value>, s: &Structure) -> bool {
    let ca = split::split(a, s);
    let cb = split::split(b, s);
    ca.len() == cb.len()
        && ca
            .iter()
            .all(|(k, v)| cb.get(k).is_some_and(|w| same(v, w)))
}

/// A record's newest stamp and wall time (its tombstone, if newer).
fn newest(r: &Record) -> (Option<Stamp>, i64) {
    let mut best: Option<(Stamp, i64)> = r.cells.values().map(|c| (c.stamp, c.wall_ms)).max();
    if let Some(t) = &r.del {
        if best.is_none_or(|(s, _)| t.stamp > s) {
            best = Some((t.stamp, t.wall_ms));
        }
    }
    (best.map(|b| b.0), best.map_or(0, |b| b.1))
}

/// A pushed record's `updatedAt`, in ms: a number, or RFC 3339 text.
fn updated_at(raw: &Value) -> Option<i64> {
    let v = raw.get("updatedAt")?;
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    if let Some(f) = v.as_f64() {
        return Some(f as i64);
    }
    chrono::DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Why one pushed record is refused, before anything is stored.
pub fn check_record(raw: &Value, cd: &SyncCollectionDecl) -> Option<RejectCode> {
    let Some(o) = raw.as_object() else {
        return Some(RejectCode::BadId);
    };
    let id = o.get("id").and_then(Value::as_str).unwrap_or_default();
    if !valid_record_id(id) {
        return Some(RejectCode::BadId);
    }
    for (k, v) in o {
        if k == "id" || cd.local.contains(k) {
            continue;
        }
        if crate::extensions::is_sensitive_name(k) {
            return Some(RejectCode::SensitiveField);
        }
        if !crate::extensions::valid_sync_field(k)
            || (cd.sets.contains(k) && !v.is_array())
            || (cd.maps.contains(k) && !v.is_object())
        {
            return Some(RejectCode::BadCollection);
        }
    }
    let mut fields = o.clone();
    fields.remove("id");
    fields.retain(|k, _| !cd.local.contains(k));
    if canonical(&Value::Object(fields)).len() as u64 > cd.max_bytes {
        return Some(RejectCode::TooLarge);
    }
    None
}

/// The bytes an extension's synced collections take: kept in meta, worked
/// out once when missing.
fn used_bytes(
    ctx: &dyn CaptureCtx,
    ext: &str,
    decls: &BTreeMap<String, SyncDecl>,
) -> anyhow::Result<u64> {
    if let Some(n) = ctx.meta(&meta_bytes(ext))?.and_then(|v| v.parse().ok()) {
        return Ok(n);
    }
    let Some(d) = decls.get(ext) else {
        return Ok(0);
    };
    let mut total = 0u64;
    for coll in d.collections.keys() {
        let ns = collection_ns(ext, coll);
        for r in ctx.records(&ns)? {
            if is_visible(&r) {
                if let Some(m) = ctx.merged(&ns, &r.id)? {
                    total += canonical(&Value::Object(m.fields)).len() as u64;
                }
            }
        }
    }
    Ok(total)
}

impl Source for ExtensionsSource {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    fn specs(&self) -> Vec<NsSpec> {
        let mut out = vec![
            spec(
                INSTALLS_NS.to_string(),
                1,
                Structure::default(),
                false,
                None,
            ),
            spec(
                SETTINGS_NS.to_string(),
                1,
                Structure {
                    per_os: self.plan.per_os.iter().map(|p| format!("@{p}")).collect(),
                    ..Structure::default()
                },
                false,
                None,
            ),
        ];
        for (ext, d) in &self.decls {
            out.push(spec(
                keys_ns(ext),
                d.schema,
                Structure::default(),
                true,
                None,
            ));
            for key in d.lists.keys() {
                out.push(spec(
                    list_ns(ext, key),
                    d.schema,
                    Structure::default(),
                    false,
                    None,
                ));
            }
            for (coll, cd) in &d.collections {
                out.push(spec(
                    collection_ns(ext, coll),
                    d.schema,
                    collection_structure(cd),
                    cd.large,
                    cd.label.clone(),
                ));
            }
        }
        out
    }

    fn read_local(&mut self) -> anyhow::Result<LocalState> {
        let (_, config) = self.port.snapshot();
        let installed = self.port.installed();
        let rules = Rules::new(&installed);
        let mut st = LocalState::default();
        for item in &installed {
            if let Some(v) = install_value(item, &config) {
                st.put(INSTALLS_NS, &install_id(item), rec_of(v));
            }
        }
        for ext in &self.plan.settings {
            let Some(obj) = config.extensions.get(ext).and_then(Value::as_object) else {
                continue;
            };
            let item = Self::installed_item(&installed, ext);
            for (field, v) in obj {
                if !field.contains('/') && setting_syncs(&rules, item, ext, field, v) {
                    st.put(SETTINGS_NS, &format!("{ext}/{field}"), rec_of(v.clone()));
                }
            }
        }
        self.seen.clear();
        for (ext, decl) in &self.decls {
            let dir = self.data_dir.join(ext);
            for key in data_keys(&dir) {
                if !decl.key_syncs(&key) || rules.is_sensitive_data(ext, &key) {
                    continue;
                }
                let path = dir.join(format!("{key}.json"));
                match read_json(&path, MAX_KEY_BYTES) {
                    FileRead::Value(v, h) => {
                        st.put(&keys_ns(ext), &key, rec_of(v));
                        self.seen.insert((ext.clone(), key.clone()), Some(h));
                    }
                    FileRead::Unusable => st.put(&keys_ns(ext), &key, rec_of(unchanged())),
                    FileRead::Missing => {}
                }
            }
            for (key, ld) in &decl.lists {
                if rules.is_sensitive_data(ext, key) {
                    continue;
                }
                let ns = list_ns(ext, key);
                let path = self.data_path(ext, key);
                if let Some(ids) = self.frozen_lists.get(&(ext.clone(), key.clone())) {
                    for id in ids {
                        st.put(&ns, id, rec_of(unchanged()));
                    }
                    continue;
                }
                match read_json(&path, MAX_LIST_FILE_BYTES) {
                    FileRead::Value(v, h) => {
                        self.seen.insert((ext.clone(), key.clone()), Some(h));
                        let Some(l) = split_list(&v, ld) else {
                            continue;
                        };
                        for (id, e) in l.elements {
                            st.put(&ns, &id, rec_of(if l.unusable { unchanged() } else { e }));
                        }
                        if let Some(rest) = l.rest {
                            st.put(&ns, REST_ID, rec_of(Value::Object(rest)));
                        }
                    }
                    FileRead::Missing => {
                        self.seen.insert((ext.clone(), key.clone()), None);
                    }
                    FileRead::Unusable => {}
                }
            }
        }
        Ok(st)
    }

    fn project(
        &mut self,
        writes: Vec<LocalWrite>,
        origin: Origin,
    ) -> anyhow::Result<ProjectResult> {
        let mut settings: Vec<&LocalWrite> = Vec::new();
        let mut keys: Vec<&LocalWrite> = Vec::new();
        let mut lists: BTreeMap<(String, String), Vec<&LocalWrite>> = BTreeMap::new();
        for w in &writes {
            if w.ns == SETTINGS_NS {
                settings.push(w);
            } else if let Some(ext) = w.ns.strip_prefix("ext.").and_then(|r| r.strip_suffix(".k")) {
                if self.decls.contains_key(ext) {
                    keys.push(w);
                }
            } else if let Some((ext, key)) = list_of(&w.ns) {
                if self.decls.contains_key(ext) {
                    lists
                        .entry((ext.to_string(), key.to_string()))
                        .or_default()
                        .push(w);
                }
            }
            // `installs`: nothing is installed or removed by sync, except
            // store themes (prepare).
        }
        let (rev, mut config) = self.port.snapshot();
        if !settings.is_empty() {
            for w in &settings {
                let Some((ext, field)) = w.id.split_once('/') else {
                    continue;
                };
                let entry = config
                    .extensions
                    .entry(ext.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                if !entry.is_object() {
                    *entry = Value::Object(Map::new());
                }
                let obj = entry.as_object_mut().expect("an object");
                match w
                    .value
                    .as_ref()
                    .and_then(|r| r.fields.get(FIELD))
                    .filter(|v| !v.is_null())
                {
                    Some(v) => {
                        obj.insert(field.to_string(), v.clone());
                    }
                    None => {
                        obj.remove(field);
                    }
                }
            }
            match self.port.commit(rev, config.clone(), origin) {
                Ok(_) => {}
                Err(CommitError::Stale) => return Ok(ProjectResult::Stale),
                Err(CommitError::Suspended) => {
                    anyhow::bail!("config.json is from a newer build; saves are suspended")
                }
                Err(CommitError::Io(e)) => return Err(e),
            }
        }
        for w in &keys {
            let ext =
                w.ns.strip_prefix("ext.")
                    .and_then(|r| r.strip_suffix(".k"))
                    .unwrap_or_default();
            let expect = self.seen.get(&(ext.to_string(), w.id.clone())).copied();
            let value = w
                .value
                .as_ref()
                .and_then(|r| r.fields.get(FIELD))
                .filter(|v| !is_unchanged(v));
            if !write_data(&self.data_dir, ext, &w.id, Some(expect.flatten()), value)? {
                return Ok(ProjectResult::Stale);
            }
        }
        for ((ext, key), ws) in &lists {
            let Some(ld) = self.decls.get(ext).and_then(|d| d.lists.get(key)).cloned() else {
                continue;
            };
            let path = self.data_path(ext, key);
            let expect = self
                .seen
                .get(&(ext.clone(), key.clone()))
                .copied()
                .flatten();
            let cur = match read_json(&path, MAX_LIST_FILE_BYTES) {
                FileRead::Value(v, _) => split_list(&v, &ld).unwrap_or_default(),
                FileRead::Missing => ListFile::default(),
                FileRead::Unusable => continue,
            };
            let mut elements = cur.elements;
            let mut rest = cur.rest;
            let mut appended: Vec<(String, Value)> = Vec::new();
            for w in ws {
                let v = w
                    .value
                    .as_ref()
                    .and_then(|r| r.fields.get(FIELD))
                    .filter(|v| !is_unchanged(v))
                    .cloned();
                if w.id == REST_ID {
                    rest = v
                        .and_then(|v| v.as_object().cloned())
                        .or(if ld.array.is_empty() {
                            None
                        } else {
                            Some(Map::new())
                        });
                    continue;
                }
                match v {
                    Some(e) => match elements.iter_mut().find(|(id, _)| *id == w.id) {
                        Some(slot) => slot.1 = e,
                        None => appended.push((w.id.clone(), e)),
                    },
                    None => elements.retain(|(id, _)| *id != w.id),
                }
            }
            appended.sort_by(|a, b| a.0.cmp(&b.0));
            elements.extend(appended);
            let value = join_list(
                &ld,
                rest.as_ref(),
                elements.into_iter().map(|(_, e)| e).collect(),
            );
            if !write_data(&self.data_dir, ext, key, Some(expect), Some(&value))? {
                return Ok(ProjectResult::Stale);
            }
            self.list_sigs
                .insert((ext.clone(), key.clone()), self.list_sig(ext, key));
        }
        let read_back = self.read_local()?;
        Ok(ProjectResult::Applied { read_back })
    }

    fn request(
        &mut self,
        ctx: &mut dyn CaptureCtx,
        req: &ExtRequest,
    ) -> Option<Result<ExtReply, AppError>> {
        let internal = crate::sync::engine::internal;
        Some(match req {
            ExtRequest::Status { ext } => self
                .ext_status(ctx, ext)
                .map(ExtReply::Status)
                .map_err(internal),
            ExtRequest::Push {
                ext,
                collection,
                records,
                deleted,
                initial,
            } => self
                .push(ctx, ext, collection, records, deleted, *initial)
                .map(ExtReply::Push)
                .map_err(internal),
            ExtRequest::Pull { ext, max } => self
                .pull(ctx, ext, *max)
                .map(ExtReply::Pull)
                .map_err(internal),
            ExtRequest::Ack { ext, cursor } => self.ack(ctx, ext, cursor).map(|()| ExtReply::Ack),
        })
    }

    fn prepare(&mut self, ctx: &mut dyn CaptureCtx, origin: Origin) -> anyhow::Result<Prepared> {
        let (_, config) = self.port.snapshot();
        let installed = self.port.installed();
        let rules = Rules::new(&installed);
        let mut out = Prepared::default();
        let (plan, decls, unknown) = self.compute_plan(ctx, &config, &installed)?;
        if let Some(host) = &self.host {
            let ask: Vec<String> = unknown
                .into_iter()
                .filter(|e| self.origins_asked.insert(e.clone()))
                .collect();
            if !ask.is_empty() {
                host.resolve_origins(&ask);
            }
        }
        if plan != self.plan || decls != self.decls {
            self.plan = plan;
            self.decls = decls;
            self.journal.set_decls(&self.decls);
            self.list_sigs.clear();
            self.frozen_lists.clear();
            out.specs_changed = true;
            return Ok(out);
        }
        self.transitions(ctx, &mut out, &config)?;
        self.pin_installs(ctx, &config, &installed, &mut out)?;
        self.pin_settings(ctx, &rules, &installed, &mut out)?;
        self.pin_keys(ctx, &rules, &mut out)?;
        self.apply_journal(ctx, origin, &mut out)?;
        self.check_lists(ctx)?;
        Ok(out)
    }
}

/// The extension and key of a lists-mode namespace.
fn list_of(ns: &str) -> Option<(&str, &str)> {
    let rest = ns.strip_prefix("ext.")?;
    let (ext, rest) = rest.split_once('.')?;
    let key = rest.strip_prefix("l.")?;
    (!key.is_empty()).then_some((ext, key))
}

// --- Reviews: suggestions and removals (§4.5) ---------------------------------------------

/// The installs reviews: items on other computers that aren't here
/// (themes from a store this computer uses install by themselves), and
/// items removed elsewhere that are still here. Excluded ones
/// (`install_exclusions`, `<kind>:<id>` as the review's kind and id) stay
/// away.
pub(crate) fn install_reviews(
    tx: &Tx<'_>,
    installed: &[InstalledItem],
    config: &Config,
    exclusions: &[String],
    name_of: &dyn Fn(crate::sync::types::DeviceId) -> String,
    os: OsKind,
) -> anyhow::Result<(
    Vec<crate::sync::status::ReviewItem>,
    Vec<crate::sync::status::ReviewItem>,
)> {
    use crate::sync::status::ReviewItem;
    let here: BTreeSet<String> = installed.iter().map(install_id).collect();
    let stores = local_store_origins(config);
    let excluded = |kind: &str, id: &str| exclusions.iter().any(|e| *e == format!("{kind}:{id}"));
    let mut suggestions = Vec::new();
    let mut removals = Vec::new();
    for r in tx.records(INSTALLS_NS)? {
        let Some((kind, id)) = r.id.split_once(':') else {
            continue;
        };
        let (stamp, _) = newest(&r);
        let computer = stamp.map(|s| name_of(s.dev));
        let mut params = BTreeMap::new();
        params.insert("name".to_string(), id.to_string());
        params.insert("kind".to_string(), kind.to_string());
        if is_visible(&r) && !here.contains(&r.id) {
            if excluded("suggestion", &r.id) {
                continue;
            }
            let v = eff_cell(&r, FIELD, os)
                .map(|c| c.value.clone())
                .unwrap_or(Value::Null);
            let source = v
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("sideload");
            let store = v
                .get("store")
                .and_then(Value::as_str)
                .map(|h| format!("store:{h}"));
            if kind == "theme" && source == "store" && store.is_some_and(|s| stores.contains(&s)) {
                continue;
            }
            let label_key = if source == "store" {
                "sync.review.suggestion"
            } else {
                "sync.review.suggestion_sideload"
            };
            suggestions.push(ReviewItem {
                kind: "suggestion".into(),
                id: r.id.clone(),
                label_key: label_key.into(),
                label_params: params,
                computer,
                detail: Some(source.to_string()),
                count: None,
            });
        } else if !is_visible(&r) && here.contains(&r.id) && !excluded("removal", &r.id) {
            removals.push(ReviewItem {
                kind: "removal".into(),
                id: r.id.clone(),
                label_key: "sync.review.removal".into(),
                label_params: params,
                computer,
                detail: None,
                count: None,
            });
        }
    }
    Ok((suggestions, removals))
}

#[cfg(test)]
mod tests;
