//! What a cloud sync app does with a file, without reading it
//! (docs/design/SYNC.md §3, §11.4): placeholders, download requests,
//! pinning, and which app a folder belongs to.
//!
//! Per OS:
//! - macOS: a dataless file (`SF_DATALESS`, File Provider and newer
//!   iCloud) or an older iCloud `.name.icloud` stub is a placeholder;
//!   downloads are requested through `NSFileManager`; nothing pins.
//! - Windows: a Cloud Files placeholder has a recall attribute
//!   (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `…_ON_OPEN`, `…_OFFLINE`);
//!   pinning (`CfSetPinState`, else the pinned attribute) keeps the folder
//!   downloaded and asks for a file's download.
//! - Linux: no placeholders; nothing to request or pin.
//!
//! Provider detection and suggestions use known locations and the apps'
//! own info files only (Dropbox's `info.json`, Syncthing's `config.xml`,
//! the OneDrive environment variables). They never scan the disk.

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
    /// Another app on the OS's cloud files support: macOS's File Provider,
    /// Windows' Cloud Files.
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
    crate::os::platform::cloud_files::system_impl()
}

// --- Shared by the per-OS implementations ----------------------------------------

/// Ancestors checked for Syncthing's folder marker.
const MAX_MARKER_DEPTH: usize = 32;
/// The most of an app's info file worth reading.
const MAX_INFO_FILE: u64 = 1024 * 1024;

/// Whether `path` is inside a folder Syncthing syncs: Syncthing marks each
/// one with a `.stfolder` directory (or file, in old versions).
pub(crate) fn in_syncthing_folder(path: &Path) -> bool {
    path.ancestors()
        .take(MAX_MARKER_DEPTH)
        .any(|a| std::fs::symlink_metadata(a.join(".stfolder")).is_ok())
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_INFO_FILE {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Dropbox's `info.json`: `{ "personal": { "path": … }, "business": { "path":
/// … } }`.
pub(crate) fn dropbox_roots(info_json: &Path) -> Vec<SuggestedFolder> {
    let Some(text) = read_small(info_json) else {
        return Vec::new();
    };
    parse_dropbox_info(&text)
}

pub(crate) fn parse_dropbox_info(text: &str) -> Vec<SuggestedFolder> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, label) in [("personal", "Dropbox"), ("business", "Dropbox (Business)")] {
        if let Some(p) = json
            .get(key)
            .and_then(|a| a.get("path"))
            .and_then(|p| p.as_str())
        {
            out.push(SuggestedFolder {
                provider: Provider::Dropbox,
                label: label.to_string(),
                path: PathBuf::from(p),
            });
        }
    }
    out
}

/// The folders in Syncthing's `config.xml`: `<folder id=… label=… path=…>`.
pub(crate) fn syncthing_roots(config_xml: &Path) -> Vec<SuggestedFolder> {
    let Some(text) = read_small(config_xml) else {
        return Vec::new();
    };
    parse_syncthing_config(&text)
}

pub(crate) fn parse_syncthing_config(text: &str) -> Vec<SuggestedFolder> {
    let mut out = Vec::new();
    for tag in text.split("<folder ").skip(1) {
        let Some(open) = tag.split('>').next() else {
            continue;
        };
        let attr = |name: &str| {
            let needle = format!("{}=\"", name);
            let start = open.find(&needle)? + needle.len();
            let end = open[start..].find('"')? + start;
            Some(xml_unescape(&open[start..end]))
        };
        let Some(path) = attr("path").filter(|p| !p.is_empty()) else {
            continue;
        };
        let path = match path.strip_prefix("~/") {
            Some(rest) => match crate::paths::home_dir() {
                Some(home) => home.join(rest),
                None => continue,
            },
            None => PathBuf::from(path),
        };
        let label = attr("label")
            .filter(|l| !l.is_empty())
            .or_else(|| attr("id"))
            .unwrap_or_default();
        out.push(SuggestedFolder {
            provider: Provider::Syncthing,
            label: format!("Syncthing · {}", label),
            path,
        });
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The provider of the deepest root that holds `path`.
pub(crate) fn provider_of(path: &Path, roots: &[SuggestedFolder]) -> Option<Provider> {
    roots
        .iter()
        .filter(|r| path.starts_with(&r.path))
        .max_by_key(|r| r.path.components().count())
        .map(|r| r.provider)
}

/// The roots that exist as directories, each once.
pub(crate) fn existing(mut roots: Vec<SuggestedFolder>) -> Vec<SuggestedFolder> {
    roots.retain(|r| r.path.is_dir());
    let mut seen = std::collections::HashSet::new();
    roots.retain(|r| seen.insert(r.path.clone()));
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropbox_info_lists_personal_and_business() {
        let roots = parse_dropbox_info(
            r#"{"personal":{"path":"/home/u/Dropbox","host":1},"business":{"path":"/home/u/Dropbox (Work)"}}"#,
        );
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].path, PathBuf::from("/home/u/Dropbox"));
        assert_eq!(roots[1].provider, Provider::Dropbox);
        assert!(parse_dropbox_info("not json").is_empty());
        assert!(parse_dropbox_info("{}").is_empty());
    }

    #[test]
    fn syncthing_config_lists_folders() {
        let xml = r#"<configuration version="37">
            <folder id="abcd-1234" label="Notes &amp; more" path="/home/u/Notes" type="sendreceive">
              <device id="X"></device>
            </folder>
            <folder id="photos" label="" path="/data/photos" type="receiveonly"></folder>
            <folder id="nopath" label="x"></folder>
        </configuration>"#;
        let roots = parse_syncthing_config(xml);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].label, "Syncthing · Notes & more");
        assert_eq!(roots[0].path, PathBuf::from("/home/u/Notes"));
        assert_eq!(roots[1].label, "Syncthing · photos");
        assert_eq!(roots[1].provider, Provider::Syncthing);
    }

    #[test]
    fn the_deepest_root_wins() {
        let roots = vec![
            SuggestedFolder {
                provider: Provider::Network,
                label: "share".into(),
                path: PathBuf::from("/mnt/share"),
            },
            SuggestedFolder {
                provider: Provider::Dropbox,
                label: "Dropbox".into(),
                path: PathBuf::from("/mnt/share/Dropbox"),
            },
        ];
        assert_eq!(
            provider_of(Path::new("/mnt/share/Dropbox/a"), &roots),
            Some(Provider::Dropbox)
        );
        assert_eq!(
            provider_of(Path::new("/mnt/share/b"), &roots),
            Some(Provider::Network)
        );
        assert_eq!(provider_of(Path::new("/mnt/sharex"), &roots), None);
    }

    #[test]
    fn a_syncthing_marker_is_found_above() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("synced/a/b");
        std::fs::create_dir_all(&deep).unwrap();
        // Unless the temp folder itself is synced, nothing is found yet.
        assert_eq!(in_syncthing_folder(&deep), in_syncthing_folder(tmp.path()));
        std::fs::create_dir_all(tmp.path().join("synced/.stfolder")).unwrap();
        assert!(in_syncthing_folder(&deep));
    }

    #[test]
    fn existing_keeps_directories_once() {
        let tmp = tempfile::tempdir().unwrap();
        let r = |p: &Path| SuggestedFolder {
            provider: Provider::Other,
            label: String::new(),
            path: p.to_path_buf(),
        };
        let kept = existing(vec![
            r(tmp.path()),
            r(tmp.path()),
            r(&tmp.path().join("missing")),
        ]);
        assert_eq!(kept.len(), 1);
    }
}
