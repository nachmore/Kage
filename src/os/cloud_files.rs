//! What a cloud sync app does with a file, without reading it
//! (docs/design/SYNC.md §3, §11.4): placeholders, download requests,
//! pinning, and which app a folder belongs to.
//!
//! C0 skeleton: `system()` knows no provider and sees no placeholders.
//! WP-S2 adds the per-OS implementations behind this module.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Whether a file's content is on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Residency {
    Local,
    /// A placeholder: reading it would block on a download.
    Placeholder,
    Unknown,
}

/// The sync app a folder belongs to.
#[allow(clippy::enum_variant_names)] // `FileProvider` is macOS's name for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    #[serde(rename = "icloud")]
    ICloud,
    #[serde(rename = "dropbox")]
    Dropbox,
    #[serde(rename = "onedrive")]
    OneDrive,
    #[serde(rename = "google_drive")]
    GoogleDrive,
    #[serde(rename = "syncthing")]
    Syncthing,
    /// Another app on macOS's File Provider.
    #[serde(rename = "file_provider")]
    FileProvider,
    #[serde(rename = "network")]
    Network,
    #[serde(rename = "other")]
    Other,
}

/// A detected sync root, offered as a quick pick in setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedFolder {
    pub provider: Provider,
    pub label: String,
    pub path: PathBuf,
}

/// The OS side of cloud folders.
pub trait CloudFiles: Send + Sync {
    /// Never reads content.
    fn residency(&self, path: &Path) -> Residency;
    /// Ask the sync app to download `path`. Returns at once.
    fn request_download(&self, path: &Path);
    /// Keep `dir` downloaded (Windows Cloud Files). A no-op elsewhere.
    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()>;
    fn detect_provider(&self, path: &Path) -> Provider;
    /// Known sync roots, from known locations and the apps' own info files.
    /// Never scans the disk.
    fn suggest_folders(&self) -> Vec<SuggestedFolder>;
}

/// The cloud files support of this OS.
pub fn system() -> Arc<dyn CloudFiles> {
    Arc::new(NoCloudFiles)
}

/// Knows nothing: until WP-S2, every file is `Unknown`.
struct NoCloudFiles;

impl CloudFiles for NoCloudFiles {
    fn residency(&self, path: &Path) -> Residency {
        let _ = path;
        Residency::Unknown
    }

    fn request_download(&self, path: &Path) {
        let _ = path;
    }

    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()> {
        let _ = dir;
        Ok(())
    }

    fn detect_provider(&self, path: &Path) -> Provider {
        let _ = path;
        Provider::Other
    }

    fn suggest_folders(&self) -> Vec<SuggestedFolder> {
        Vec::new()
    }
}
