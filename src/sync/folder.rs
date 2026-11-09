//! The layout inside the sync folder (docs/design/SYNC.md §5): the strict
//! file classifier, the descriptor `kage-sync.json`, key files, and what a
//! folder the user picked holds.
//!
//! Every call goes through [`SyncFs`], so the folder is reached only on the
//! I/O pool, and a test can stand in for it.

use super::crypto::{device_text, is_id_text, KeyFile};
use super::fsio::{self, SyncFs, READ_TIMEOUT};
use super::types::{DeviceId, READER_VERSION};
use crate::os::cloud_files::{CloudFiles, Provider};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Component, Path, PathBuf};

/// The directory Kage creates in the folder the user picked.
pub const ROOT_NAME: &str = "Kage Sync";
/// The descriptor, written once at setup or Start over.
pub const DESCRIPTOR: &str = "kage-sync.json";
/// Plain text for whoever opens the folder, written once.
pub const README: &str = "README.txt";
pub const KEYS_DIR: &str = "keys";
pub const DEVICES_DIR: &str = "devices";
pub const SHARDS_DIR: &str = "shards";
pub const BLOBS_DIR: &str = "blobs";
pub const ARCHIVE_DIR: &str = "archive";
/// The most a descriptor or key file may hold.
const DESCRIPTOR_MAX: u64 = 64 * 1024;
const KEY_FILE_MAX: u64 = 16 * 1024;
/// Device directories `inspect` looks into.
const MAX_DEVICE_DIRS: usize = 64;
/// The i18n prefix of `FolderInfo::reason`.
const REASON_PREFIX: &str = "settings.sync.folder.reason.";

/// `<chosen>/Kage Sync`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `Kage Sync` inside the folder the user picked.
    pub fn in_chosen(chosen: &Path) -> Self {
        Self::new(chosen.join(ROOT_NAME))
    }

    /// The layout for a folder the user picked: the folder itself when it
    /// already is a sync folder (it holds `kage-sync.json`, a copy of it,
    /// or a placeholder for it), else `Kage Sync` inside it.
    pub fn resolve(fs: &dyn SyncFs, chosen: &Path) -> Self {
        match fs.list(chosen) {
            Ok(entries)
                if entries
                    .iter()
                    .any(|e| !e.is_dir && names_descriptor(&e.name)) =>
            {
                Self::new(chosen)
            }
            _ => Self::in_chosen(chosen),
        }
    }

    pub fn descriptor(&self) -> PathBuf {
        self.root.join(DESCRIPTOR)
    }

    pub fn readme(&self) -> PathBuf {
        self.root.join(README)
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.root.join(KEYS_DIR)
    }

    pub fn devices_dir(&self) -> PathBuf {
        self.root.join(DEVICES_DIR)
    }

    pub fn device_dir(&self, dev: DeviceId) -> PathBuf {
        self.devices_dir().join(device_text(dev))
    }

    pub fn shards_dir(&self, dev: DeviceId) -> PathBuf {
        self.device_dir(dev).join(SHARDS_DIR)
    }

    pub fn blobs_dir(&self, dev: DeviceId) -> PathBuf {
        self.device_dir(dev).join(BLOBS_DIR)
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.root.join(ARCHIVE_DIR)
    }
}

/// `head-000000000042.kage`.
pub fn head_name(seq: u64) -> String {
    format!("head-{:012}.kage", seq)
}

/// `<32 hex>.kage` for a content name.
pub fn data_name(content_name: &str) -> String {
    format!("{}.kage", content_name)
}

/// `e0001-<dev>-0001.json`.
pub fn key_file_name(epoch: u32, dev: DeviceId, n: u32) -> String {
    format!("e{:04}-{}-{:04}.json", epoch, device_text(dev), n)
}

/// The directories names are classified in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirKind {
    Root,
    Keys,
    Device,
    Shards,
    Blobs,
}

/// What a file name is (§5.2). Only `Head`, `Data` and `KeyFile` are read
/// as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileClass {
    Head(u64),
    /// A shard or blob, or `kage-sync.json` in the root.
    Data,
    KeyFile {
        epoch: u32,
        dev: String,
        n: u32,
    },
    OwnTemp,
    /// An iCloud placeholder for the inner name, which is a data name or a
    /// copy of one.
    ICloudStub(String),
    /// A sync app's conflicted or numbered copy of `of`.
    Duplicate {
        of: String,
    },
    ToolTemp,
    OsJunk,
    Unknown,
}

fn all_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `head-(\d{12})` at the start of `s`: the sequence number and the rest.
fn head_stem(s: &str) -> Option<(u64, &str)> {
    let rest = s.strip_prefix("head-")?;
    let digits = rest.get(..12)?;
    if !all_digits(digits) {
        return None;
    }
    Some((digits.parse().ok()?, &rest[12..]))
}

/// `[0-9a-f]{32}` at the start of `s`, and the rest.
fn hex_stem(s: &str) -> Option<&str> {
    let stem = s.get(..32)?;
    is_lower_hex(stem).then(|| &s[32..])
}

/// Characters in a key file name's stem: `e0001-<16>-0001`.
const KEY_STEM_LEN: usize = 27;

/// `e(\d{4})-([0-9a-z]{16})-(\d{4})` at the start of `s`, and the rest.
fn key_stem(s: &str) -> Option<((u32, &str, u32), &str)> {
    let stem = s.get(..KEY_STEM_LEN)?;
    let b = stem.as_bytes();
    if !stem.is_ascii() || b[0] != b'e' || b[5] != b'-' || b[22] != b'-' {
        return None;
    }
    let (epoch, dev, n) = (&stem[1..5], &stem[6..22], &stem[23..]);
    if !all_digits(epoch)
        || !all_digits(n)
        || !dev
            .bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
    {
        return None;
    }
    Some((
        (epoch.parse().ok()?, dev, n.parse().ok()?),
        &s[KEY_STEM_LEN..],
    ))
}

/// A strict data name for `dir`.
fn classify_data(dir: DirKind, name: &str) -> Option<FileClass> {
    match dir {
        DirKind::Device => {
            let (seq, rest) = head_stem(name)?;
            (rest == ".kage").then_some(FileClass::Head(seq))
        }
        DirKind::Shards | DirKind::Blobs => (hex_stem(name)? == ".kage").then_some(FileClass::Data),
        DirKind::Keys => {
            let ((epoch, dev, n), rest) = key_stem(name)?;
            (rest == ".json").then(|| FileClass::KeyFile {
                epoch,
                dev: dev.to_string(),
                n,
            })
        }
        DirKind::Root => (name == DESCRIPTOR).then_some(FileClass::Data),
    }
}

/// A sync app's copy of a data name for `dir`: the name's stem, extra
/// text, and the name's own extension (§5.2).
fn classify_duplicate(dir: DirKind, name: &str) -> Option<FileClass> {
    let (stem, rest, ext) = match dir {
        DirKind::Device => {
            let (_, rest) = head_stem(name)?;
            (&name[..17], rest, ".kage")
        }
        DirKind::Shards | DirKind::Blobs => {
            let rest = hex_stem(name)?;
            (&name[..32], rest, ".kage")
        }
        DirKind::Keys => {
            let (_, rest) = key_stem(name)?;
            (&name[..KEY_STEM_LEN], rest, ".json")
        }
        DirKind::Root => ("kage-sync", name.strip_prefix("kage-sync")?, ".json"),
    };
    let extra = rest.strip_suffix(ext)?;
    (!extra.is_empty()).then(|| FileClass::Duplicate {
        of: format!("{}{}", stem, ext),
    })
}

fn is_tool_temp(name: &str) -> bool {
    let between = |pre: &str, suf: &str| {
        name.strip_prefix(pre)
            .and_then(|r| r.strip_suffix(suf))
            .is_some_and(|mid| !mid.is_empty())
    };
    between(".syncthing.", ".tmp")
        || between("~syncthing~", ".tmp")
        || name.starts_with(".~")
        || name.starts_with("~$")
}

fn is_own_temp(name: &str) -> bool {
    name.strip_prefix("~kage-")
        .and_then(|r| r.strip_suffix(".tmp"))
        .is_some_and(|hex| hex.len() == 16 && is_lower_hex(hex))
}

/// Classify a name found in a directory of kind `dir`.
pub fn classify(dir: DirKind, name: &str) -> FileClass {
    if let Some(class) = classify_data(dir, name) {
        return class;
    }
    if is_own_temp(name) {
        return FileClass::OwnTemp;
    }
    if crate::config_export::sensitive::is_os_junk(name) {
        return FileClass::OsJunk;
    }
    if is_tool_temp(name) {
        return FileClass::ToolTemp;
    }
    if let Some(inner) = name
        .strip_prefix('.')
        .and_then(|r| r.strip_suffix(".icloud"))
        .filter(|inner| !inner.is_empty())
    {
        return match classify(dir, inner) {
            FileClass::Head(_)
            | FileClass::Data
            | FileClass::KeyFile { .. }
            | FileClass::Duplicate { .. } => FileClass::ICloudStub(inner.to_string()),
            _ => FileClass::Unknown,
        };
    }
    classify_duplicate(dir, name).unwrap_or(FileClass::Unknown)
}

/// Whether a root entry is the descriptor, a copy of it, or a placeholder
/// for either.
fn names_descriptor(name: &str) -> bool {
    match classify(DirKind::Root, name) {
        FileClass::Data => true,
        FileClass::Duplicate { of } => of == DESCRIPTOR,
        FileClass::ICloudStub(inner) => names_descriptor(&inner),
        _ => false,
    }
}

/// `kage-sync.json`: plain JSON, written once at setup or Start over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub kage_sync: u32,
    pub folder_id: String,
    pub created_at: String,
    pub min_reader: u32,
}

impl Descriptor {
    fn valid(&self) -> bool {
        self.kage_sync >= 1
            && self.min_reader >= 1
            && is_id_text(&self.folder_id)
            && chrono::DateTime::parse_from_rfc3339(&self.created_at).is_ok()
    }

    /// Of two descriptors made at the same moment, the earlier wins; ties
    /// go to the smaller folder id (§5.3).
    fn wins_over(&self, other: &Self) -> bool {
        let at = |d: &Self| chrono::DateTime::parse_from_rfc3339(&d.created_at).ok();
        (at(self), &self.folder_id) < (at(other), &other.folder_id)
    }

    /// Whether this build can read the folder.
    pub fn readable_here(&self) -> bool {
        self.min_reader <= READER_VERSION
    }
}

/// The folder has the file, but it isn't downloaded or readable yet: an
/// iCloud placeholder, a read that timed out, half a download. Never the
/// same as "missing" (R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotReady;

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the sync folder's descriptor isn't downloaded or readable yet")
    }
}

impl std::error::Error for NotReady {}

/// Whether an error from this module is [`NotReady`].
pub fn is_not_ready(e: &anyhow::Error) -> bool {
    e.is::<NotReady>()
}

/// The descriptor, or `None` when the folder has none. With two (a sync
/// app's copy after two computers set up at once), the winner of §5.3.
/// `Err(NotReady)` when one exists but can't be read yet.
pub fn read_descriptor(fs: &dyn SyncFs, l: &Layout) -> anyhow::Result<Option<Descriptor>> {
    let entries = match fs.list(&l.root) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut names = Vec::new();
    let mut placeholder = false;
    for e in entries.iter().filter(|e| !e.is_dir) {
        match classify(DirKind::Root, &e.name) {
            FileClass::Data => names.push(e.name.clone()),
            FileClass::Duplicate { of } if of == DESCRIPTOR => names.push(e.name.clone()),
            FileClass::ICloudStub(inner) if names_descriptor(&inner) => placeholder = true,
            _ => {}
        }
    }
    if names.is_empty() {
        return if placeholder {
            Err(NotReady.into())
        } else {
            Ok(None)
        };
    }
    let mut best: Option<Descriptor> = None;
    for name in &names {
        let bytes = match fs.read(&l.root.join(name), DESCRIPTOR_MAX, READ_TIMEOUT) {
            Ok(b) => b,
            Err(e) => {
                log::info!("The sync folder descriptor isn't readable yet: {}", e);
                return Err(NotReady.into());
            }
        };
        match serde_json::from_slice::<Descriptor>(&bytes) {
            Ok(d) if d.valid() => {
                if best.as_ref().is_none_or(|b| d.wins_over(b)) {
                    best = Some(d);
                }
            }
            _ => log::warn!("Ignoring an unreadable sync folder descriptor ({})", name),
        }
    }
    best.map(Some).ok_or_else(|| NotReady.into())
}

/// Create `Kage Sync/` with this computer's first key file, `README.txt`
/// and, last, the descriptor, each written so it can't replace a file.
/// If the descriptor exists (another computer got there first), this
/// computer's key file is removed again and the error is `AlreadyExists`.
pub fn create_folder(
    fs: &dyn SyncFs,
    l: &Layout,
    d: &Descriptor,
    readme: &str,
    key: &KeyFile,
    me: DeviceId,
) -> anyhow::Result<()> {
    anyhow::ensure!(d.valid(), "invalid sync folder descriptor");
    fs.create_dir_all(&l.keys_dir())?;
    fs.create_dir_all(&l.shards_dir(me))?;
    fs.create_dir_all(&l.blobs_dir(me))?;
    let key_name = write_key_file(fs, l, key, me)?;
    match fs.create_new(&l.readme(), readme.as_bytes()) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => log::warn!("Couldn't write the sync folder README: {}", e),
    }
    let mut json = serde_json::to_vec(d)?;
    json.push(b'\n');
    if let Err(e) = fs.create_new(&l.descriptor(), &json) {
        if let Err(rm) = fs.remove(&l.keys_dir().join(&key_name)) {
            log::warn!("Couldn't remove this computer's new key file: {}", rm);
        }
        return Err(e.into());
    }
    Ok(())
}

/// Write `kf` as this computer's next key file of its epoch (Change
/// passphrase writes another). Returns its name.
pub fn write_key_file(
    fs: &dyn SyncFs,
    l: &Layout,
    kf: &KeyFile,
    me: DeviceId,
) -> anyhow::Result<String> {
    let dir = l.keys_dir();
    let mine = device_text(me);
    let mut n = match fs.list(&dir) {
        Ok(entries) => entries
            .iter()
            .filter_map(|e| match classify(DirKind::Keys, &e.name) {
                FileClass::KeyFile { epoch, dev, n } if epoch == kf.epoch && dev == mine => Some(n),
                _ => None,
            })
            .max()
            .unwrap_or(0),
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e.into()),
    };
    let mut json = serde_json::to_vec_pretty(kf)?;
    json.push(b'\n');
    loop {
        n += 1;
        anyhow::ensure!(n <= 9999, "too many key files for this computer");
        let name = key_file_name(kf.epoch, me, n);
        match fs.write_once(&dir, &name, &json) {
            Ok(()) => return Ok(name),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Every key file in `keys/`, with its parsed name, ordered by epoch,
/// device and number. Files that don't read or parse yet are left out;
/// so is one whose content names another epoch than its file name.
pub fn list_key_files(fs: &dyn SyncFs, l: &Layout) -> anyhow::Result<Vec<(FileClass, KeyFile)>> {
    let dir = l.keys_dir();
    let entries = match fs.list(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for e in entries.iter().filter(|e| !e.is_dir) {
        let class = classify(DirKind::Keys, &e.name);
        let FileClass::KeyFile { epoch, .. } = &class else {
            continue;
        };
        let kf = match fs.read(&dir.join(&e.name), KEY_FILE_MAX, READ_TIMEOUT) {
            Ok(bytes) => serde_json::from_slice::<KeyFile>(&bytes),
            Err(err) => {
                log::debug!("Key file {} isn't readable yet: {}", e.name, err);
                continue;
            }
        };
        match kf {
            Ok(kf) if kf.epoch == *epoch => out.push((class, kf)),
            _ => log::warn!(
                "Ignoring key file {}: it doesn't parse or match its name",
                e.name
            ),
        }
    }
    out.sort_by(|(a, _), (b, _)| key_order(a).cmp(&key_order(b)));
    Ok(out)
}

fn key_order(c: &FileClass) -> (u32, &str, u32) {
    match c {
        FileClass::KeyFile { epoch, dev, n } => (*epoch, dev.as_str(), *n),
        _ => (0, "", 0),
    }
}

/// What `sync_inspect_folder` found in a folder the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderKind {
    /// Kage will create `Kage Sync` here.
    New,
    /// Kage sync from other computers is here; this one joins.
    Existing,
    /// Kage can't tell whether the folder syncs. A note, not a block.
    UnknownSync,
    /// Inside Kage's own folders, a drive root, a temp folder, read-only.
    NotUsable,
    /// The folder needs a newer Kage.
    NewerFormat,
}

/// The answer of `sync_inspect_folder`. Computer names are encrypted, so
/// only their count is known before the passphrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderInfo {
    pub kind: FolderKind,
    pub provider: Provider,
    pub computer_count: u32,
    /// For `not_usable`: the i18n key of the reason
    /// (`settings.sync.folder.reason.*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Folders a sync folder can't be in: Kage's own, and temp folders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    pub kage_dirs: Vec<PathBuf>,
    pub temp_dirs: Vec<PathBuf>,
}

impl Places {
    /// This computer's: `<config>/kage`, `<data_local>/kage`, `<data>/kage`,
    /// `<cache>/kage`, and the temp folders, as the OS resolves them.
    pub fn here() -> Self {
        let kage_dirs = [
            crate::paths::config_dir(),
            crate::paths::data_local_dir(),
            crate::paths::data_dir(),
            crate::paths::cache_dir(),
        ]
        .into_iter()
        .flatten()
        .map(|d| resolved(&d.join("kage")))
        .collect();
        let mut temp_dirs = vec![resolved(&std::env::temp_dir())];
        for t in [
            "/tmp",
            "/var/tmp",
            "/private/tmp",
            "/private/var/tmp",
            "/private/var/folders",
            "/dev/shm",
        ] {
            temp_dirs.push(PathBuf::from(t));
        }
        if let Some(root) = std::env::var_os("SystemRoot") {
            temp_dirs.push(resolved(&PathBuf::from(root).join("Temp")));
        }
        temp_dirs.sort();
        temp_dirs.dedup();
        Self {
            kage_dirs,
            temp_dirs,
        }
    }
}

/// The path with links and `..` resolved where it exists, else as given.
fn resolved(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| lexical(p))
}

/// `.` and `..` removed without touching the disk.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Look at a folder the user picked.
pub fn inspect(fs: &dyn SyncFs, cloud: &dyn CloudFiles, chosen: &Path) -> FolderInfo {
    inspect_in(fs, cloud, &resolved(chosen), &Places::here())
}

/// [`inspect`] against given places, so tests can use temp folders.
pub fn inspect_in(
    fs: &dyn SyncFs,
    cloud: &dyn CloudFiles,
    chosen: &Path,
    places: &Places,
) -> FolderInfo {
    let chosen = lexical(chosen);
    // A relative or drive-relative path (`C:notes`) names nothing the user
    // picked. (`has_root`, not `is_absolute`, so tests' `/cloud/…` paths in
    // memory work on Windows too.)
    let rooted = chosen.has_root();
    let provider = if rooted {
        cloud.detect_provider(&chosen)
    } else {
        Provider::Other
    };
    let not_usable = |why: &str| FolderInfo {
        kind: FolderKind::NotUsable,
        provider,
        computer_count: 0,
        reason: Some(format!("{}{}", REASON_PREFIX, why)),
    };
    if !rooted {
        return not_usable("missing");
    }
    if chosen.parent().is_none() {
        return not_usable("drive_root");
    }
    let inside = |dirs: &[PathBuf]| dirs.iter().any(|d| chosen.starts_with(d));
    if inside(&places.kage_dirs) {
        return not_usable("kage_folder");
    }
    if inside(&places.temp_dirs) {
        return not_usable("temp");
    }
    let entries = match fs.list(&chosen) {
        Ok(e) => e,
        Err(e) => {
            return not_usable(match e.kind() {
                io::ErrorKind::NotFound => "missing",
                io::ErrorKind::NotADirectory => "not_a_folder",
                io::ErrorKind::PermissionDenied => "denied",
                io::ErrorKind::TimedOut => "not_responding",
                _ => "unreadable",
            })
        }
    };
    if inside_a_sync_folder(fs, &chosen) {
        return not_usable("inside_sync");
    }
    let picked_root = entries
        .iter()
        .any(|e| !e.is_dir && names_descriptor(&e.name));
    let layout = if picked_root {
        Layout::new(&chosen)
    } else {
        match entries.iter().find(|e| e.name == ROOT_NAME) {
            Some(e) if !e.is_dir => return not_usable("not_a_folder"),
            Some(_) => Layout::in_chosen(&chosen),
            None => return new_folder(fs, &chosen, provider, &not_usable),
        }
    };
    match read_descriptor(fs, &layout) {
        Ok(Some(d)) if !d.readable_here() => FolderInfo {
            kind: FolderKind::NewerFormat,
            provider,
            computer_count: count_computers(fs, &layout),
            reason: None,
        },
        Ok(Some(_)) => {
            if !writable(fs, &layout.root) {
                return not_usable("read_only");
            }
            FolderInfo {
                kind: FolderKind::Existing,
                provider,
                computer_count: count_computers(fs, &layout),
                reason: None,
            }
        }
        // `Kage Sync` without a descriptor: an empty one can be set up
        // again; one with computers in it is missing its descriptor.
        Ok(None) if count_computers(fs, &layout) > 0 => not_usable("incomplete"),
        Ok(None) => new_folder(fs, &layout.root, provider, &not_usable),
        Err(e) if is_not_ready(&e) => {
            cloud.request_download(&layout.descriptor());
            not_usable("not_downloaded")
        }
        Err(e) => {
            log::info!("Couldn't read the picked sync folder: {}", e);
            not_usable("unreadable")
        }
    }
}

/// A folder where Kage would create `Kage Sync`.
fn new_folder(
    fs: &dyn SyncFs,
    dir: &Path,
    provider: Provider,
    not_usable: &dyn Fn(&str) -> FolderInfo,
) -> FolderInfo {
    if !writable(fs, dir) {
        return not_usable("read_only");
    }
    FolderInfo {
        kind: if provider == Provider::Other {
            FolderKind::UnknownSync
        } else {
            FolderKind::New
        },
        provider,
        computer_count: 0,
        reason: None,
    }
}

/// Whether a picked folder is a directory inside a sync folder's own
/// structure (`Kage Sync/devices`, a device directory, …).
fn inside_a_sync_folder(fs: &dyn SyncFs, chosen: &Path) -> bool {
    let mut cur = chosen;
    let mut depth = 0;
    while let Some(name) = cur.file_name().and_then(|n| n.to_str()) {
        let structural = matches!(
            name,
            KEYS_DIR | DEVICES_DIR | SHARDS_DIR | BLOBS_DIR | ARCHIVE_DIR
        ) || is_id_text(name);
        if !structural || depth > 4 {
            break;
        }
        let Some(parent) = cur.parent() else {
            break;
        };
        cur = parent;
        depth += 1;
    }
    depth > 0
        && fs.list(cur).is_ok_and(|entries| {
            entries
                .iter()
                .any(|e| !e.is_dir && names_descriptor(&e.name))
        })
}

/// Whether Kage can create a file in `dir`: a temp it removes again.
fn writable(fs: &dyn SyncFs, dir: &Path) -> bool {
    let probe = dir.join(fsio::temp_name());
    match fs.create_new(&probe, b"") {
        Ok(()) => {
            if let Err(e) = fs.remove(&probe) {
                log::warn!("Couldn't remove the sync folder write probe: {}", e);
            }
            true
        }
        Err(e) => !matches!(
            e.kind(),
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
        ),
    }
}

/// Computers with a head in the folder (placeholders and copies count).
fn count_computers(fs: &dyn SyncFs, l: &Layout) -> u32 {
    let Ok(devices) = fs.list(&l.devices_dir()) else {
        return 0;
    };
    let mut count = 0;
    for d in devices
        .iter()
        .filter(|d| d.is_dir && is_id_text(&d.name))
        .take(MAX_DEVICE_DIRS)
    {
        let Ok(files) = fs.list(&l.devices_dir().join(&d.name)) else {
            continue;
        };
        let has_head = files
            .iter()
            .any(|f| match classify(DirKind::Device, &f.name) {
                FileClass::Head(_) | FileClass::Duplicate { .. } => true,
                FileClass::ICloudStub(inner) => {
                    matches!(classify(DirKind::Device, &inner), FileClass::Head(_))
                }
                _ => false,
            });
        if has_head {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::crypto::{parse_device_text, unwrap_dek, wrap_dek_with, KdfCost};
    use crate::sync::testing::{FakeCloudFiles, Fault, FaultyFs, FsOp};

    const FOLDER: &str = "5qg2vbn8h1xk0d3m";
    const CHEAP: KdfCost = KdfCost {
        m_kib: 64,
        t: 1,
        p: 1,
    };

    fn me() -> DeviceId {
        parse_device_text("k7m2p9xqa3rt5vwe").unwrap()
    }

    fn descriptor(folder_id: &str, created_at: &str) -> Descriptor {
        Descriptor {
            kage_sync: 1,
            folder_id: folder_id.into(),
            created_at: created_at.into(),
            min_reader: 1,
        }
    }

    fn dup(of: &str) -> FileClass {
        FileClass::Duplicate { of: of.into() }
    }

    #[test]
    fn strict_names_classify_as_data() {
        use DirKind::*;
        assert_eq!(
            classify(Device, "head-000000000042.kage"),
            FileClass::Head(42)
        );
        assert_eq!(
            classify(Device, "head-999999999999.kage"),
            FileClass::Head(999_999_999_999)
        );
        let hex = "0123456789abcdef0123456789abcdef.kage";
        assert_eq!(classify(Shards, hex), FileClass::Data);
        assert_eq!(classify(Blobs, hex), FileClass::Data);
        assert_eq!(
            classify(Keys, "e0001-k7m2p9xqa3rt5vwe-0002.json"),
            FileClass::KeyFile {
                epoch: 1,
                dev: "k7m2p9xqa3rt5vwe".into(),
                n: 2
            }
        );
        assert_eq!(classify(Root, "kage-sync.json"), FileClass::Data);
        // Strict: the right place, lowercase, exact lengths.
        for (dir, name) in [
            (Shards, "head-000000000042.kage"),
            (Device, hex),
            (Root, hex),
            (Keys, "kage-sync.json"),
            (Device, "head-00000000042.kage"),
            (Device, "HEAD-000000000042.kage"),
            (Device, "head-000000000042.KAGE"),
            (Shards, "0123456789ABCDEF0123456789abcdef.kage"),
            (Shards, "0123456789abcdef0123456789abcde.kage"),
            (Keys, "e001-k7m2p9xqa3rt5vwe-0001.json"),
            (Keys, "e0001-K7M2P9XQA3RT5VWE-0001.json"),
            (Keys, "e0001-k7m2p9xqa3rt5vw-0001.json"),
            (Root, "README.txt"),
            (Root, "Kage-Sync.json"),
            (Device, "notes.txt"),
            (Device, ""),
        ] {
            assert_eq!(
                classify(dir, name),
                FileClass::Unknown,
                "{:?} {:?}",
                dir,
                name
            );
        }
        // Names that aren't ASCII never match, and never panic.
        for name in [
            "e\u{e9}001-k7m2p9xqa3rt5vwe-0001.json",
            "e0001-k7m2p9xqa3rt5v\u{e9}-0001.json",
            "head-00000000004\u{e9}.kage",
            "\u{65e5}\u{672c}\u{8a9e}.kage",
            "0123456789abcdef0123456789abcde\u{e9}.kage",
            ".\u{e9}.icloud",
        ] {
            for dir in [Root, Keys, Device, Shards, Blobs] {
                assert_eq!(
                    classify(dir, name),
                    FileClass::Unknown,
                    "{:?} {:?}",
                    dir,
                    name
                );
            }
        }
    }

    #[test]
    fn every_provider_duplicate_pattern() {
        use DirKind::*;
        let head = "head-000000000042.kage";
        for name in [
            "head-000000000042 (Work laptop's conflicted copy 2026-11-09).kage",
            "head-000000000042 (Copie en conflit de Work laptop 2026-11-09).kage",
            "head-000000000042 2.kage",
            "head-000000000042 (1).kage",
            "head-000000000042-DESKTOP-ABC123.kage",
            "head-000000000042.sync-conflict-20261109-100000-ABCDEFG.kage",
            "head-000000000042 (Case Conflict).kage",
        ] {
            assert_eq!(classify(Device, name), dup(head), "{}", name);
        }
        let hex = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            classify(Shards, &format!("{} (1).kage", hex)),
            dup(&format!("{}.kage", hex))
        );
        assert_eq!(
            classify(
                Blobs,
                &format!("{}.sync-conflict-20261109-100000-X.kage", hex)
            ),
            dup(&format!("{}.kage", hex))
        );
        assert_eq!(
            classify(Root, "kage-sync (Home Mac's conflicted copy).json"),
            dup("kage-sync.json")
        );
        assert_eq!(classify(Root, "kage-sync 2.json"), dup("kage-sync.json"));
        assert_eq!(
            classify(Keys, "e0001-k7m2p9xqa3rt5vwe-0001-LAPTOP.json"),
            dup("e0001-k7m2p9xqa3rt5vwe-0001.json")
        );
        // Extra text right after the stem is a copy too (the §5.2 pattern):
        // Kage never writes a 13-digit head.
        assert_eq!(
            classify(Device, "head-0000000000042.kage"),
            dup("head-000000000004.kage")
        );
        // The extension must be the name's own, in the name's own directory.
        assert_eq!(
            classify(Device, "head-000000000042 (1).json"),
            FileClass::Unknown
        );
        assert_eq!(classify(Root, "kage-sync (1).kage"), FileClass::Unknown);
        assert_eq!(
            classify(Shards, "head-000000000042 (1).kage"),
            FileClass::Unknown
        );
        assert_eq!(classify(Root, "kage-sync.json.bak"), FileClass::Unknown);
    }

    #[test]
    fn icloud_stubs_temps_and_junk() {
        use DirKind::*;
        assert_eq!(
            classify(Device, ".head-000000000042.kage.icloud"),
            FileClass::ICloudStub("head-000000000042.kage".into())
        );
        assert_eq!(
            classify(Shards, ".0123456789abcdef0123456789abcdef.kage.icloud"),
            FileClass::ICloudStub("0123456789abcdef0123456789abcdef.kage".into())
        );
        assert_eq!(
            classify(Root, ".kage-sync.json.icloud"),
            FileClass::ICloudStub("kage-sync.json".into())
        );
        assert_eq!(
            classify(Device, ".head-000000000042 2.kage.icloud"),
            FileClass::ICloudStub("head-000000000042 2.kage".into())
        );
        // A stub for something that isn't ours is just unknown.
        assert_eq!(classify(Device, ".notes.txt.icloud"), FileClass::Unknown);
        assert_eq!(classify(Device, "..icloud"), FileClass::Unknown);

        assert_eq!(
            classify(Shards, "~kage-0123456789abcdef.tmp"),
            FileClass::OwnTemp
        );
        assert_eq!(
            classify(Root, "~kage-0123456789abcdef.tmp"),
            FileClass::OwnTemp
        );
        assert_eq!(
            classify(Shards, "~kage-0123456789abcde.tmp"),
            FileClass::Unknown
        );
        for name in [
            ".syncthing.head-000000000042.kage.tmp",
            "~syncthing~head-000000000042.kage.tmp",
            ".~lock.kage-sync.json#",
            "~$document.docx",
        ] {
            assert_eq!(classify(Device, name), FileClass::ToolTemp, "{}", name);
        }
        for name in [
            ".DS_Store",
            "Thumbs.db",
            "desktop.ini",
            "._head-000000000042.kage",
            "Icon\r",
        ] {
            assert_eq!(classify(Device, name), FileClass::OsJunk, "{:?}", name);
        }
        // Dropbox's own temp names and anything else stay unknown: never
        // read, never deleted.
        for name in [".dropbox", ".dropbox.attr", "notes.kage", "~kage-x.tmp"] {
            assert_eq!(classify(Device, name), FileClass::Unknown, "{}", name);
        }
    }

    #[test]
    fn names_have_the_documented_shapes() {
        assert_eq!(head_name(42), "head-000000000042.kage");
        assert_eq!(classify(DirKind::Device, &head_name(7)), FileClass::Head(7));
        assert_eq!(
            key_file_name(1, me(), 1),
            "e0001-k7m2p9xqa3rt5vwe-0001.json"
        );
        let l = Layout::in_chosen(Path::new("/cloud/Dropbox"));
        assert_eq!(l.root, PathBuf::from("/cloud/Dropbox/Kage Sync"));
        assert_eq!(
            l.shards_dir(me()),
            PathBuf::from("/cloud/Dropbox/Kage Sync/devices/k7m2p9xqa3rt5vwe/shards")
        );
        // The deepest path below the chosen folder is about 75 characters.
        let deepest = l
            .blobs_dir(me())
            .join(data_name(&"0".repeat(32)))
            .strip_prefix("/cloud/Dropbox/")
            .unwrap()
            .to_string_lossy()
            .len();
        assert!(deepest <= 80, "{}", deepest);
    }

    fn created(fs: &FaultyFs) -> (Layout, KeyFile) {
        let l = Layout::in_chosen(Path::new("/cloud/Dropbox"));
        fs.create_dir_all(Path::new("/cloud/Dropbox")).unwrap();
        let kf = wrap_dek_with("passphrase 123", &[3; 32], FOLDER, 1, CHEAP).unwrap();
        create_folder(
            fs,
            &l,
            &descriptor(FOLDER, "2026-11-09T10:00:00Z"),
            "Kage Sync\n",
            &kf,
            me(),
        )
        .unwrap();
        (l, kf)
    }

    #[test]
    fn create_then_read_back() {
        let fs = FaultyFs::new();
        let (l, kf) = created(&fs);
        assert_eq!(
            read_descriptor(&fs, &l).unwrap(),
            Some(descriptor(FOLDER, "2026-11-09T10:00:00Z"))
        );
        let json: serde_json::Value =
            serde_json::from_slice(&fs.get(l.descriptor()).unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kage_sync": 1, "folder_id": FOLDER,
                                "created_at": "2026-11-09T10:00:00Z", "min_reader": 1 })
        );
        assert_eq!(fs.get(l.readme()).unwrap(), b"Kage Sync\n");
        assert!(fs.exists(l.shards_dir(me())) && fs.exists(l.blobs_dir(me())));
        let keys = list_key_files(&fs, &l).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0].0,
            FileClass::KeyFile {
                epoch: 1,
                dev: "k7m2p9xqa3rt5vwe".into(),
                n: 1
            }
        );
        assert_eq!(keys[0].1, kf);
        assert_eq!(
            &unwrap_dek("passphrase 123", &keys[0].1, FOLDER).unwrap()[..],
            &[3; 32]
        );
        // Change passphrase: the next key file of the epoch.
        let kf2 = wrap_dek_with("another one!", &[3; 32], FOLDER, 1, CHEAP).unwrap();
        assert_eq!(
            write_key_file(&fs, &l, &kf2, me()).unwrap(),
            "e0001-k7m2p9xqa3rt5vwe-0002.json"
        );
        assert_eq!(list_key_files(&fs, &l).unwrap().len(), 2);
    }

    #[test]
    fn create_refuses_an_existing_folder_and_leaves_no_key() {
        let fs = FaultyFs::new();
        let (l, _) = created(&fs);
        let other = parse_device_text("a9x3h6t1qq8m0ve2").unwrap();
        let kf = wrap_dek_with("passphrase 456", &[4; 32], "aaaaaaaaaaaaaaaa", 1, CHEAP).unwrap();
        let err = create_folder(
            &fs,
            &l,
            &descriptor("aaaaaaaaaaaaaaaa", "2026-11-09T10:00:01Z"),
            "x",
            &kf,
            other,
        )
        .unwrap_err();
        assert_eq!(
            err.downcast_ref::<io::Error>().map(|e| e.kind()),
            Some(io::ErrorKind::AlreadyExists)
        );
        // Only the first computer's key file; the README is unchanged.
        assert_eq!(list_key_files(&fs, &l).unwrap().len(), 1);
        assert_eq!(fs.get(l.readme()).unwrap(), b"Kage Sync\n");
        assert_eq!(read_descriptor(&fs, &l).unwrap().unwrap().folder_id, FOLDER);
    }

    #[test]
    fn two_descriptors_the_earliest_wins() {
        let fs = FaultyFs::new();
        let l = Layout::new("/cloud/Kage Sync");
        let put = |name: &str, d: &Descriptor| {
            fs.put(l.root.join(name), serde_json::to_vec(d).unwrap());
        };
        put(
            "kage-sync.json",
            &descriptor("bbbbbbbbbbbbbbbb", "2026-11-09T10:00:05Z"),
        );
        put(
            "kage-sync (Home Mac's conflicted copy).json",
            &descriptor("cccccccccccccccc", "2026-11-09T10:00:01Z"),
        );
        assert_eq!(
            read_descriptor(&fs, &l).unwrap().unwrap().folder_id,
            "cccccccccccccccc"
        );
        // A tie: the smaller folder id.
        put(
            "kage-sync 2.json",
            &descriptor("aaaaaaaaaaaaaaaa", "2026-11-09T10:00:01Z"),
        );
        assert_eq!(
            read_descriptor(&fs, &l).unwrap().unwrap().folder_id,
            "aaaaaaaaaaaaaaaa"
        );
        // Garbage in a copy is ignored when another one parses.
        fs.put(l.root.join("kage-sync (1).json"), b"{ half".to_vec());
        assert_eq!(
            read_descriptor(&fs, &l).unwrap().unwrap().folder_id,
            "aaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn a_descriptor_that_isnt_ready_is_never_missing() {
        let fs = FaultyFs::new();
        let l = Layout::new("/cloud/Kage Sync");
        assert_eq!(read_descriptor(&fs, &l).unwrap(), None, "no folder at all");
        fs.create_dir_all(&l.root).unwrap();
        assert_eq!(read_descriptor(&fs, &l).unwrap(), None, "no descriptor");
        // An iCloud placeholder.
        fs.put(l.root.join(".kage-sync.json.icloud"), b"stub".to_vec());
        assert!(is_not_ready(&read_descriptor(&fs, &l).unwrap_err()));
        // Half downloaded.
        fs.put(l.descriptor(), b"{\"kage_sync\":1,".to_vec());
        assert!(is_not_ready(&read_descriptor(&fs, &l).unwrap_err()));
        // A read that hangs.
        fs.put(
            l.descriptor(),
            serde_json::to_vec(&descriptor(FOLDER, "2026-11-09T10:00:00Z")).unwrap(),
        );
        fs.inject(FsOp::Read, l.descriptor(), Fault::Hang);
        assert!(is_not_ready(&read_descriptor(&fs, &l).unwrap_err()));
        fs.clear_faults();
        assert!(read_descriptor(&fs, &l).unwrap().is_some());
        // Invalid ids or times don't count as a descriptor.
        fs.put(
            l.descriptor(),
            serde_json::to_vec(&descriptor("../../etc", "2026-11-09T10:00:00Z")).unwrap(),
        );
        assert!(is_not_ready(&read_descriptor(&fs, &l).unwrap_err()));
    }

    #[test]
    fn key_files_that_dont_match_their_names_are_skipped() {
        let fs = FaultyFs::new();
        let (l, kf) = created(&fs);
        let mut moved = kf.clone();
        moved.epoch = 2;
        fs.put(
            l.keys_dir().join("e0001-a9x3h6t1qq8m0ve2-0001.json"),
            serde_json::to_vec(&moved).unwrap(),
        );
        fs.put(
            l.keys_dir().join("e0001-a9x3h6t1qq8m0ve3-0001.json"),
            b"junk".to_vec(),
        );
        fs.put(
            l.keys_dir().join("e0001-k7m2p9xqa3rt5vwe-0001 (1).json"),
            serde_json::to_vec(&kf).unwrap(),
        );
        fs.put(l.keys_dir().join("notes.txt"), b"x".to_vec());
        let keys = list_key_files(&fs, &l).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(
            list_key_files(&fs, &Layout::new("/nowhere")).unwrap().len(),
            0
        );
    }

    fn places() -> Places {
        Places {
            kage_dirs: vec![PathBuf::from("/home/u/.config/kage")],
            temp_dirs: vec![PathBuf::from("/tmp")],
        }
    }

    fn reason(info: &FolderInfo) -> &str {
        info.reason
            .as_deref()
            .and_then(|r| r.strip_prefix(REASON_PREFIX))
            .unwrap_or("")
    }

    #[test]
    fn inspect_new_unknown_and_not_usable_folders() {
        let fs = FaultyFs::new();
        let cloud = FakeCloudFiles::new();
        fs.create_dir_all(Path::new("/home/u/Dropbox")).unwrap();
        cloud.set_provider(Provider::Dropbox);
        let info = inspect_in(&fs, &cloud, Path::new("/home/u/Dropbox"), &places());
        assert_eq!(info.kind, FolderKind::New);
        assert_eq!(info.provider, Provider::Dropbox);
        assert_eq!(info.reason, None);
        // The write probe left nothing behind.
        assert!(fs.files_under("/home/u/Dropbox").is_empty());

        cloud.set_provider(Provider::Other);
        assert_eq!(
            inspect_in(&fs, &cloud, Path::new("/home/u/Dropbox"), &places()).kind,
            FolderKind::UnknownSync
        );
        let not_usable = |p: &str| {
            let info = inspect_in(&fs, &cloud, Path::new(p), &places());
            assert_eq!(info.kind, FolderKind::NotUsable, "{}", p);
            reason(&info).to_string()
        };
        assert_eq!(not_usable("/"), "drive_root");
        assert_eq!(not_usable("relative/path"), "missing");
        assert_eq!(not_usable("/home/u/.config/kage/sub"), "kage_folder");
        assert_eq!(not_usable("/home/u/.config/kage"), "kage_folder");
        assert_eq!(not_usable("/tmp/x"), "temp");
        assert_eq!(not_usable("/home/u/missing"), "missing");
        assert_eq!(not_usable("/home/u/Dropbox/../.config/kage"), "kage_folder");
        fs.put("/home/u/file.txt", b"x".to_vec());
        assert_eq!(not_usable("/home/u/file.txt"), "not_a_folder");
        fs.inject(FsOp::CreateNew, "/home/u/Dropbox", Fault::ReadOnly);
        assert_eq!(not_usable("/home/u/Dropbox"), "read_only");
        fs.clear_faults();
        fs.inject(FsOp::List, "/home/u/Dropbox", Fault::Denied);
        assert_eq!(not_usable("/home/u/Dropbox"), "denied");
        fs.clear_faults();
        fs.inject(FsOp::List, "/home/u/Dropbox", Fault::Hang);
        assert_eq!(not_usable("/home/u/Dropbox"), "not_responding");
        fs.clear_faults();
        // Every reason has an English string.
        for r in [
            "missing",
            "not_a_folder",
            "drive_root",
            "kage_folder",
            "temp",
            "read_only",
            "denied",
            "unreadable",
            "not_responding",
            "inside_sync",
            "not_downloaded",
            "incomplete",
        ] {
            let key = format!("{}{}", REASON_PREFIX, r);
            assert_ne!(crate::i18n::translate_in("en", &key, &[]), key, "{}", key);
        }
    }

    #[test]
    fn inspect_an_existing_folder() {
        let fs = FaultyFs::new();
        let cloud = FakeCloudFiles::new();
        let (l, _) = created(&fs);
        let chosen = Path::new("/cloud/Dropbox");
        // Created but nobody has published yet.
        let info = inspect_in(&fs, &cloud, chosen, &places());
        assert_eq!(info.kind, FolderKind::Existing);
        assert_eq!(info.computer_count, 0);
        fs.put(l.device_dir(me()).join(head_name(1)), b"h".to_vec());
        let other = l.devices_dir().join("a9x3h6t1qq8m0ve2");
        fs.put(
            other.join(".head-000000000003.kage.icloud"),
            b"stub".to_vec(),
        );
        // Not a computer: no head.
        fs.create_dir_all(&l.devices_dir().join("zzzzzzzzzzzzzzzz/shards"))
            .unwrap();
        let info = inspect_in(&fs, &cloud, chosen, &places());
        assert_eq!((info.kind, info.computer_count), (FolderKind::Existing, 2));
        // Picking `Kage Sync` itself works too.
        let info = inspect_in(&fs, &cloud, &l.root, &places());
        assert_eq!((info.kind, info.computer_count), (FolderKind::Existing, 2));
        assert_eq!(Layout::resolve(&fs, &l.root), l);
        assert_eq!(Layout::resolve(&fs, chosen), l);
        // A folder inside it doesn't.
        assert_eq!(
            reason(&inspect_in(&fs, &cloud, &l.devices_dir(), &places())),
            "inside_sync"
        );
        assert_eq!(
            reason(&inspect_in(&fs, &cloud, &l.shards_dir(me()), &places())),
            "inside_sync"
        );
        // Read-only: can't join.
        fs.inject(FsOp::CreateNew, &l.root, Fault::Denied);
        assert_eq!(
            reason(&inspect_in(&fs, &cloud, chosen, &places())),
            "read_only"
        );
        fs.clear_faults();
        // A newer format.
        let mut d = descriptor(FOLDER, "2026-11-09T10:00:00Z");
        d.min_reader = READER_VERSION + 1;
        fs.put(l.descriptor(), serde_json::to_vec(&d).unwrap());
        let info = inspect_in(&fs, &cloud, chosen, &places());
        assert_eq!(
            (info.kind, info.computer_count),
            (FolderKind::NewerFormat, 2)
        );
    }

    #[test]
    fn inspect_a_folder_still_downloading_or_incomplete() {
        let fs = FaultyFs::new();
        let cloud = FakeCloudFiles::new();
        let l = Layout::in_chosen(Path::new("/cloud/iCloud"));
        fs.put(l.root.join(".kage-sync.json.icloud"), b"stub".to_vec());
        let info = inspect_in(&fs, &cloud, Path::new("/cloud/iCloud"), &places());
        assert_eq!(reason(&info), "not_downloaded");
        assert_eq!(cloud.download_requests(), [l.descriptor()]);
        // `Kage Sync` with computers but no descriptor.
        fs.delete(l.root.join(".kage-sync.json.icloud"));
        fs.put(
            l.devices_dir().join("a9x3h6t1qq8m0ve2").join(head_name(1)),
            b"h".to_vec(),
        );
        let info = inspect_in(&fs, &cloud, Path::new("/cloud/iCloud"), &places());
        assert_eq!(reason(&info), "incomplete");
        // An empty `Kage Sync` can be set up again.
        fs.delete(l.devices_dir());
        cloud.set_provider(Provider::ICloud);
        let info = inspect_in(&fs, &cloud, Path::new("/cloud/iCloud"), &places());
        assert_eq!(info.kind, FolderKind::New);
        // `Kage Sync` that is a file.
        fs.delete(&l.root);
        fs.put(&l.root, b"x".to_vec());
        assert_eq!(
            reason(&inspect_in(
                &fs,
                &cloud,
                Path::new("/cloud/iCloud"),
                &places()
            )),
            "not_a_folder"
        );
    }

    #[test]
    fn folder_info_serialises_for_the_frontend() {
        let json = serde_json::to_value(FolderInfo {
            kind: FolderKind::UnknownSync,
            provider: Provider::GoogleDrive,
            computer_count: 0,
            reason: None,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "unknown_sync", "provider": "google_drive", "computer_count": 0 })
        );
    }

    #[test]
    fn places_here_include_kage_and_temp_dirs() {
        let p = Places::here();
        assert!(!p.kage_dirs.is_empty());
        assert!(p.kage_dirs.iter().all(|d| d.ends_with("kage")));
        assert!(p.temp_dirs.contains(&PathBuf::from("/tmp")));
    }
}
