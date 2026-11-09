//! Windows cloud folders (docs/design/SYNC.md §3).
//!
//! - Placeholders: Cloud Files (OneDrive, Dropbox, iCloud for Windows) mark
//!   a file whose content isn't local with a recall attribute. Read with
//!   `GetFileAttributesW`, which never opens the file.
//! - Pinning: `CfSetPinState(PINNED)` keeps a file or a whole folder
//!   downloaded ("Always keep on this device"), and the sync app fetches
//!   what it pinned in the background. Where that fails, the pinned file
//!   attribute (what `attrib +P` sets). A failure is reported and tolerated.
//! - Downloads: pinning the one file.
//! - Providers: the OneDrive environment variables, Dropbox's `info.json`,
//!   iCloud Drive's folder, Syncthing's `config.xml`, the Cloud Files sync
//!   root's provider name, network paths and drives, and Google Drive's
//!   drive label.

use crate::os::cloud_files::{
    dropbox_roots, existing, in_syncthing_folder, provider_of, syncthing_roots, CloudFiles,
    Provider, Residency, SuggestedFolder,
};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::CloudFilters::{
    CfGetSyncRootInfoByPath, CfSetPinState, CF_PIN_STATE_PINNED, CF_SET_PIN_FLAGS,
    CF_SET_PIN_FLAG_NONE, CF_SET_PIN_FLAG_RECURSE, CF_SYNC_ROOT_INFO_PROVIDER,
    CF_SYNC_ROOT_PROVIDER_INFO,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDriveTypeW, GetFileAttributesW, GetLogicalDrives, GetVolumeInformationW,
    SetFileAttributesW, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_PINNED,
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_UNPINNED,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
};

/// `GetDriveTypeW` answers.
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

struct WinCloudFiles;

pub fn system_impl() -> Arc<dyn CloudFiles> {
    Arc::new(WinCloudFiles)
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn attributes(path: &Path) -> Option<u32> {
    let w = wide(path);
    // SAFETY: `w` is a NUL-terminated UTF-16 path that outlives the call.
    let a = unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) };
    (a != INVALID_FILE_ATTRIBUTES).then_some(a)
}

pub(crate) fn residency_from_attributes(a: u32) -> Residency {
    let recall = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0
        | FILE_ATTRIBUTE_RECALL_ON_OPEN.0
        | FILE_ATTRIBUTE_OFFLINE.0;
    if a & recall != 0 {
        Residency::Placeholder
    } else {
        Residency::Local
    }
}

/// An open handle, closed on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened and nothing else closes.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Open `path` (a file or directory) for pinning. Reading attributes and
/// the right to read data never downloads anything.
fn open_for_pin(path: &Path) -> windows::core::Result<Handle> {
    let w = wide(path);
    // SAFETY: `w` outlives the call; the returned handle is owned by `Handle`.
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            FILE_READ_DATA.0 | FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }?;
    Ok(Handle(h))
}

/// Pin `path`, and with `recurse` everything in it.
fn pin(path: &Path, recurse: bool) -> anyhow::Result<()> {
    let flags: CF_SET_PIN_FLAGS = if recurse {
        CF_SET_PIN_FLAG_RECURSE
    } else {
        CF_SET_PIN_FLAG_NONE
    };
    let cf = open_for_pin(path).and_then(|h| {
        // SAFETY: `h` is an open handle for the whole call; no OVERLAPPED.
        unsafe { CfSetPinState(h.0, CF_PIN_STATE_PINNED, flags, None) }
    });
    let cf_err = match cf {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    // Not a Cloud Files sync root, or an older client: the attribute.
    let Some(a) = attributes(path) else {
        anyhow::bail!("pinning failed: {}", cf_err);
    };
    let next = (a | FILE_ATTRIBUTE_PINNED.0) & !FILE_ATTRIBUTE_UNPINNED.0;
    if next == a {
        return Ok(());
    }
    let w = wide(path);
    // SAFETY: `w` is a NUL-terminated UTF-16 path that outlives the call.
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(next)) }
        .map_err(|e| anyhow::anyhow!("pinning failed: {}; {}", cf_err, e))
}

/// The Cloud Files provider name of the sync root holding `path`.
fn sync_root_provider(path: &Path) -> Option<String> {
    let w = wide(path);
    let mut info = CF_SYNC_ROOT_PROVIDER_INFO::default();
    // SAFETY: `info` is a buffer of the size passed; `w` outlives the call.
    unsafe {
        CfGetSyncRootInfoByPath(
            PCWSTR(w.as_ptr()),
            CF_SYNC_ROOT_INFO_PROVIDER,
            &mut info as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<CF_SYNC_ROOT_PROVIDER_INFO>() as u32,
            None,
        )
    }
    .ok()?;
    let end = info
        .ProviderName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.ProviderName.len());
    Some(String::from_utf16_lossy(&info.ProviderName[..end]))
}

pub(crate) fn provider_from_name(name: &str) -> Provider {
    let n = name.to_ascii_lowercase();
    if n.contains("onedrive") {
        Provider::OneDrive
    } else if n.contains("dropbox") {
        Provider::Dropbox
    } else if n.contains("icloud") {
        Provider::ICloud
    } else if n.contains("google") {
        Provider::GoogleDrive
    } else {
        Provider::FileProvider
    }
}

/// The root of `path`'s volume (`C:\`), or `None` for a UNC path.
fn volume_root(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    let b = s.as_bytes();
    (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic())
        .then(|| format!("{}:\\", b[0] as char))
}

fn is_unc(path: &Path) -> bool {
    path.to_str().is_some_and(|s| {
        (s.starts_with(r"\\") && !s.starts_with(r"\\?\")) || s.starts_with(r"\\?\UNC\")
    })
}

fn drive_type(root: &str) -> u32 {
    let w = wide_str(root);
    // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) }
}

fn volume_label(root: &str) -> Option<String> {
    let w = wide_str(root);
    let mut label = [0u16; 261];
    // SAFETY: `w` and `label` outlive the call.
    unsafe { GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, None) }
        .ok()?;
    let end = label.iter().position(|&c| c == 0).unwrap_or(label.len());
    Some(String::from_utf16_lossy(&label[..end]))
}

/// Google Drive for desktop mounts a fixed drive labelled "Google Drive".
/// Remote and removable drives aren't asked: they can stall.
fn google_drive_roots() -> Vec<SuggestedFolder> {
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        if drive_type(&root) != DRIVE_FIXED {
            continue;
        }
        if volume_label(&root).as_deref() == Some("Google Drive") {
            out.push(SuggestedFolder {
                provider: Provider::GoogleDrive,
                label: "Google Drive".into(),
                path: PathBuf::from(&root).join("My Drive"),
            });
        }
    }
    out
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn known_roots() -> Vec<SuggestedFolder> {
    let mut roots = Vec::new();
    let label_of = |p: &Path, fallback: &str| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| fallback.to_string())
    };
    if let Some(p) = env_dir("OneDriveConsumer") {
        roots.push(SuggestedFolder {
            provider: Provider::OneDrive,
            label: "OneDrive".into(),
            path: p,
        });
    }
    if let Some(p) = env_dir("OneDriveCommercial") {
        roots.push(SuggestedFolder {
            provider: Provider::OneDrive,
            label: label_of(&p, "OneDrive"),
            path: p,
        });
    }
    if let Some(p) = env_dir("OneDrive") {
        roots.push(SuggestedFolder {
            provider: Provider::OneDrive,
            label: label_of(&p, "OneDrive"),
            path: p,
        });
    }
    for dir in [crate::paths::config_dir(), crate::paths::data_local_dir()]
        .into_iter()
        .flatten()
    {
        roots.extend(dropbox_roots(&dir.join("Dropbox/info.json")));
    }
    if let Some(h) = crate::paths::home_dir() {
        roots.push(SuggestedFolder {
            provider: Provider::ICloud,
            label: "iCloud Drive".into(),
            path: h.join("iCloudDrive"),
        });
    }
    if let Some(d) = crate::paths::data_local_dir() {
        roots.extend(syncthing_roots(&d.join("Syncthing/config.xml")));
    }
    roots
}

impl CloudFiles for WinCloudFiles {
    fn residency(&self, path: &Path) -> Residency {
        match attributes(path) {
            Some(a) => residency_from_attributes(a),
            None => Residency::Unknown,
        }
    }

    fn request_download(&self, path: &Path) {
        if let Err(e) = pin(path, false) {
            log::debug!("Sync folder: download request refused: {}", e);
        }
    }

    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()> {
        pin(dir, true)
    }

    fn detect_provider(&self, path: &Path) -> Provider {
        if let Some(p) = provider_of(path, &known_roots()) {
            return p;
        }
        if let Some(name) = sync_root_provider(path) {
            return provider_from_name(&name);
        }
        if in_syncthing_folder(path) {
            return Provider::Syncthing;
        }
        if is_unc(path) {
            return Provider::Network;
        }
        if let Some(root) = volume_root(path) {
            match drive_type(&root) {
                DRIVE_REMOTE => return Provider::Network,
                DRIVE_FIXED if volume_label(&root).as_deref() == Some("Google Drive") => {
                    return Provider::GoogleDrive
                }
                _ => {}
            }
        }
        Provider::Other
    }

    fn suggest_folders(&self) -> Vec<SuggestedFolder> {
        let mut roots = known_roots();
        roots.extend(google_drive_roots());
        existing(roots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_attributes_mark_placeholders() {
        assert_eq!(residency_from_attributes(0x20), Residency::Local);
        assert_eq!(
            residency_from_attributes(0x20 | FILE_ATTRIBUTE_PINNED.0),
            Residency::Local
        );
        for a in [
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0,
            FILE_ATTRIBUTE_RECALL_ON_OPEN.0,
            FILE_ATTRIBUTE_OFFLINE.0,
        ] {
            assert_eq!(residency_from_attributes(0x20 | a), Residency::Placeholder);
        }
    }

    #[test]
    fn a_local_file_is_local() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.kage");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(WinCloudFiles.residency(&f), Residency::Local);
        assert_eq!(
            WinCloudFiles.residency(&tmp.path().join("missing")),
            Residency::Unknown
        );
    }

    #[test]
    fn provider_names_and_paths() {
        assert_eq!(provider_from_name("OneDrive"), Provider::OneDrive);
        assert_eq!(provider_from_name("Dropbox"), Provider::Dropbox);
        assert_eq!(provider_from_name("iCloud Drive"), Provider::ICloud);
        assert_eq!(provider_from_name("Box"), Provider::FileProvider);
        assert_eq!(
            volume_root(Path::new(r"C:\Users\a")).as_deref(),
            Some(r"C:\")
        );
        assert_eq!(volume_root(Path::new(r"\\?\D:\x")).as_deref(), Some(r"D:\"));
        assert!(is_unc(Path::new(r"\\server\share\x")));
        assert!(is_unc(Path::new(r"\\?\UNC\server\share")));
        assert!(!is_unc(Path::new(r"\\?\C:\x")));
    }
}
