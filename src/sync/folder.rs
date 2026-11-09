//! The layout inside the sync folder (docs/design/SYNC.md §5): the strict
//! file classifier, the descriptor `kage-sync.json`, key files, and what a
//! folder the user picked holds.
//!
//! C0 skeleton: every name is `Unknown` and no folder is usable. WP-S2
//! writes the bodies.

use super::crypto::KeyFile;
use super::fsio::SyncFs;
use super::types::DeviceId;
use super::NOT_BUILT;
use crate::os::cloud_files::{CloudFiles, Provider};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `<chosen>/Kage Sync`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
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
    Data,
    KeyFile {
        epoch: u32,
        dev: String,
        n: u32,
    },
    OwnTemp,
    /// An iCloud placeholder for the inner name.
    ICloudStub(String),
    /// A sync app's conflicted or numbered copy of `of`.
    Duplicate {
        of: String,
    },
    ToolTemp,
    OsJunk,
    Unknown,
}

/// Classify a name found in a directory of kind `dir`.
pub fn classify(dir: DirKind, name: &str) -> FileClass {
    let _ = (dir, name);
    FileClass::Unknown
}

/// `kage-sync.json`: plain JSON, written once at setup or Start over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub kage_sync: u32,
    pub folder_id: String,
    pub created_at: String,
    pub min_reader: u32,
}

/// The descriptor, or `None` when the folder has none.
pub fn read_descriptor(fs: &dyn SyncFs, l: &Layout) -> anyhow::Result<Option<Descriptor>> {
    let _ = (fs, l);
    Ok(None)
}

/// Create `Kage Sync/` with its descriptor, `README.txt` and this
/// computer's first key file.
pub fn create_folder(
    fs: &dyn SyncFs,
    l: &Layout,
    d: &Descriptor,
    readme: &str,
    key: &KeyFile,
    me: DeviceId,
) -> anyhow::Result<()> {
    let _ = (fs, l, d, readme, key, me);
    anyhow::bail!(NOT_BUILT)
}

/// Every key file in `keys/`, with its parsed name.
pub fn list_key_files(fs: &dyn SyncFs, l: &Layout) -> anyhow::Result<Vec<(FileClass, KeyFile)>> {
    let _ = (fs, l);
    Ok(Vec::new())
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
    /// For `not_usable`: the i18n key of the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Look at a folder the user picked.
pub fn inspect(fs: &dyn SyncFs, cloud: &dyn CloudFiles, chosen: &Path) -> FolderInfo {
    let _ = (fs, cloud, chosen);
    FolderInfo {
        kind: FolderKind::NotUsable,
        provider: Provider::Other,
        computer_count: 0,
        reason: None,
    }
}
