//! macOS cloud folders (docs/design/SYNC.md §3).
//!
//! - Placeholders: a dataless file (`SF_DATALESS` in `st_flags`), which File
//!   Provider apps and newer iCloud Drive leave under the real name, or an
//!   older iCloud `.name.icloud` stub. Both are seen with `lstat`, which
//!   never downloads.
//! - Downloads: `NSFileManager startDownloadingUbiquitousItemAtURL:`, which
//!   returns at once.
//! - Pinning: macOS has no API for it; Settings suggests "Keep Downloaded".
//! - Providers: `~/Library/Mobile Documents` (iCloud Drive),
//!   `~/Library/CloudStorage/<App>-<account>` (File Provider apps), the
//!   Dropbox and Syncthing info files, network mounts (`statfs`), and
//!   iCloud's own answer for Desktop & Documents.

use crate::os::cloud_files::{
    dropbox_roots, existing, in_syncthing_folder, provider_of, syncthing_roots, CloudFiles,
    Provider, Residency, SuggestedFolder,
};
use std::os::macos::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `SF_DATALESS` from `<sys/stat.h>`: the file's content isn't on disk.
const SF_DATALESS: u32 = 0x4000_0000;
/// `MNT_LOCAL` from `<sys/mount.h>`.
const MNT_LOCAL: u32 = 0x0000_1000;

struct MacCloudFiles;

pub fn system_impl() -> Arc<dyn CloudFiles> {
    Arc::new(MacCloudFiles)
}

/// `.name.icloud` next to `path`.
fn icloud_stub(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    Some(path.with_file_name(format!(".{}.icloud", name)))
}

/// What a file's `st_flags` say about its content.
pub(crate) fn residency_from_flags(st_flags: u32) -> Residency {
    if st_flags & SF_DATALESS != 0 {
        Residency::Placeholder
    } else {
        Residency::Local
    }
}

pub(crate) fn residency_of(path: &Path) -> Residency {
    match std::fs::symlink_metadata(path) {
        Ok(m) => residency_from_flags(m.st_flags()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match icloud_stub(path).map(|s| std::fs::symlink_metadata(s).is_ok()) {
                Some(true) => Residency::Placeholder,
                _ => Residency::Unknown,
            }
        }
        Err(_) => Residency::Unknown,
    }
}

fn home() -> Option<PathBuf> {
    crate::paths::home_dir()
}

/// File Provider roots under `~/Library/CloudStorage`, by their folder
/// names (`Dropbox`, `OneDrive-Personal`, `GoogleDrive-<account>`, …).
fn cloud_storage_roots() -> Vec<SuggestedFolder> {
    let Some(dir) = home().map(|h| h.join("Library/CloudStorage")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        out.push(classify_cloud_storage(&name, &path));
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// What a `~/Library/CloudStorage` entry is, and how to label it.
fn classify_cloud_storage(name: &str, path: &Path) -> SuggestedFolder {
    let suffix = |prefix: &str| {
        name.strip_prefix(prefix)
            .map(|r| r.trim_start_matches('-').to_string())
    };
    if let Some(rest) = suffix("Dropbox") {
        return SuggestedFolder {
            provider: Provider::Dropbox,
            label: if rest.is_empty() {
                "Dropbox".into()
            } else {
                format!("Dropbox ({})", rest)
            },
            path: path.to_path_buf(),
        };
    }
    if let Some(rest) = suffix("OneDrive") {
        return SuggestedFolder {
            provider: Provider::OneDrive,
            label: if rest.is_empty() || rest == "Personal" {
                "OneDrive".into()
            } else {
                format!("OneDrive - {}", rest)
            },
            path: path.to_path_buf(),
        };
    }
    if let Some(rest) = suffix("GoogleDrive") {
        let my_drive = path.join("My Drive");
        return SuggestedFolder {
            provider: Provider::GoogleDrive,
            label: if rest.is_empty() {
                "Google Drive".into()
            } else {
                format!("Google Drive ({})", rest)
            },
            path: if my_drive.is_dir() {
                my_drive
            } else {
                path.to_path_buf()
            },
        };
    }
    SuggestedFolder {
        provider: Provider::FileProvider,
        label: name.replace('-', " "),
        path: path.to_path_buf(),
    }
}

fn known_roots() -> Vec<SuggestedFolder> {
    let mut roots = Vec::new();
    if let Some(h) = home() {
        roots.push(SuggestedFolder {
            provider: Provider::ICloud,
            label: "iCloud Drive".into(),
            path: h.join("Library/Mobile Documents/com~apple~CloudDocs"),
        });
    }
    roots.extend(cloud_storage_roots());
    if let Some(h) = home() {
        roots.extend(dropbox_roots(&h.join(".dropbox/info.json")));
        roots.extend(syncthing_roots(
            &h.join("Library/Application Support/Syncthing/config.xml"),
        ));
    }
    // Google Drive's older virtual drive.
    roots.push(SuggestedFolder {
        provider: Provider::GoogleDrive,
        label: "Google Drive".into(),
        path: PathBuf::from("/Volumes/GoogleDrive/My Drive"),
    });
    roots
}

/// Whether the volume holding `path` (or its nearest existing ancestor) is
/// a network mount.
fn on_network_volume(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(existing) = path.ancestors().find(|a| a.exists()) else {
        return false;
    };
    let Ok(c) = std::ffi::CString::new(existing.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid C string; `st` is written by the call.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    st.f_flags & MNT_LOCAL == 0
}

/// iCloud's own answer, for Desktop & Documents kept in iCloud.
fn is_ubiquitous(path: &Path) -> bool {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let Some(text) = path.to_str() else {
        return false;
    };
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(text));
        NSFileManager::defaultManager().isUbiquitousItemAtURL(&url)
    })
}

impl CloudFiles for MacCloudFiles {
    fn residency(&self, path: &Path) -> Residency {
        residency_of(path)
    }

    fn request_download(&self, path: &Path) {
        use objc2_foundation::{NSFileManager, NSString, NSURL};
        let Some(text) = path.to_str() else {
            return;
        };
        objc2::rc::autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(text));
            if let Err(e) =
                NSFileManager::defaultManager().startDownloadingUbiquitousItemAtURL_error(&url)
            {
                log::debug!("Sync folder: download request refused (code {})", e.code());
            }
        });
    }

    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()> {
        let _ = dir;
        Ok(())
    }

    fn detect_provider(&self, path: &Path) -> Provider {
        if let Some(p) = provider_of(path, &known_roots()) {
            return p;
        }
        if let Some(h) = home() {
            if path.starts_with(h.join("Library/Mobile Documents")) {
                return Provider::ICloud;
            }
        }
        if in_syncthing_folder(path) {
            return Provider::Syncthing;
        }
        if on_network_volume(path) {
            return Provider::Network;
        }
        if is_ubiquitous(path) {
            return Provider::ICloud;
        }
        Provider::Other
    }

    fn suggest_folders(&self) -> Vec<SuggestedFolder> {
        existing(known_roots())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_storage_names_map_to_providers() {
        let p = Path::new("/x");
        let c = |n: &str| classify_cloud_storage(n, &p.join(n));
        assert_eq!(c("Dropbox").provider, Provider::Dropbox);
        assert_eq!(c("Dropbox").label, "Dropbox");
        assert_eq!(c("Dropbox-Personal").label, "Dropbox (Personal)");
        assert_eq!(c("OneDrive-Personal").label, "OneDrive");
        assert_eq!(c("OneDrive-Contoso").label, "OneDrive - Contoso");
        assert_eq!(c("OneDrive-Contoso").provider, Provider::OneDrive);
        assert_eq!(
            c("GoogleDrive-a@example.com").provider,
            Provider::GoogleDrive
        );
        assert_eq!(c("Box-Box").provider, Provider::FileProvider);
        assert_eq!(c("Box-Box").label, "Box Box");
    }

    #[test]
    fn local_files_and_icloud_stubs() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("head-000000000001.kage");
        assert_eq!(residency_of(&f), Residency::Unknown);
        std::fs::write(tmp.path().join(".head-000000000001.kage.icloud"), b"stub").unwrap();
        assert_eq!(residency_of(&f), Residency::Placeholder);
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(residency_of(&f), Residency::Local);
    }

    /// A dataless file (File Provider, newer iCloud) keeps its name; only
    /// its flags say so. Real ones are in the manual matrix (§18.7).
    #[test]
    fn dataless_flags_mark_placeholders() {
        assert_eq!(residency_from_flags(0), Residency::Local);
        // UF_HIDDEN, UF_COMPRESSED: still local.
        assert_eq!(residency_from_flags(0x8000 | 0x20), Residency::Local);
        assert_eq!(residency_from_flags(SF_DATALESS), Residency::Placeholder);
        assert_eq!(
            residency_from_flags(SF_DATALESS | 0x20),
            Residency::Placeholder
        );
    }

    #[test]
    fn a_local_temp_folder_is_not_a_network_volume() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!on_network_volume(tmp.path()));
        assert!(!on_network_volume(&tmp.path().join("missing/deeper")));
    }
}
