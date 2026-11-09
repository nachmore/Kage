//! Linux cloud folders (docs/design/SYNC.md §3). Linux sync apps keep
//! whole files on disk, so there are no placeholders to see, request or
//! pin. Providers: Dropbox's `info.json`, the OneDrive client's config,
//! Syncthing's `config.xml` and folder marker, and the mount's filesystem
//! type (network filesystems, Google Drive FUSE mounts).

use crate::os::cloud_files::{
    dropbox_roots, existing, in_syncthing_folder, provider_of, syncthing_roots, CloudFiles,
    Provider, Residency, SuggestedFolder,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct LinuxCloudFiles;

pub fn system_impl() -> Arc<dyn CloudFiles> {
    Arc::new(LinuxCloudFiles)
}

fn home() -> Option<PathBuf> {
    crate::paths::home_dir()
}

/// The OneDrive client's `sync_dir` (default `~/OneDrive`), if it is set up.
fn onedrive_root() -> Option<SuggestedFolder> {
    let h = home()?;
    let config_dir = h.join(".config/onedrive");
    if !config_dir.is_dir() {
        return None;
    }
    let configured = std::fs::read_to_string(config_dir.join("config"))
        .ok()
        .and_then(|text| parse_onedrive_sync_dir(&text));
    let path = match configured {
        Some(p) => match p.strip_prefix("~/") {
            Some(rest) => h.join(rest),
            None => PathBuf::from(p),
        },
        None => h.join("OneDrive"),
    };
    Some(SuggestedFolder {
        provider: Provider::OneDrive,
        label: "OneDrive".into(),
        path,
    })
}

/// `sync_dir = "…"` in the OneDrive client's config.
pub(crate) fn parse_onedrive_sync_dir(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        (key.trim() == "sync_dir").then(|| value.trim().trim_matches('"').to_string())
    })
}

fn known_roots() -> Vec<SuggestedFolder> {
    let mut roots = Vec::new();
    if let Some(h) = home() {
        roots.extend(dropbox_roots(&h.join(".dropbox/info.json")));
        for config in [
            ".local/state/syncthing/config.xml",
            ".config/syncthing/config.xml",
        ] {
            roots.extend(syncthing_roots(&h.join(config)));
        }
    }
    roots.extend(onedrive_root());
    roots
}

/// The filesystem type of the mount holding `path`, from
/// `/proc/self/mountinfo` (the longest mount point that holds it).
fn mount_type(path: &Path) -> Option<String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    parse_mount_type(&text, &path)
}

pub(crate) fn parse_mount_type(mountinfo: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        // id parent major:minor root mount-point options [optional…] - type source super
        let mut halves = line.splitn(2, " - ");
        let (Some(left), Some(right)) = (halves.next(), halves.next()) else {
            continue;
        };
        let Some(mount_point) = left.split(' ').nth(4) else {
            continue;
        };
        let mount_point = unescape_mount(mount_point);
        let Some(fs_type) = right.split(' ').next() else {
            continue;
        };
        if !path.starts_with(&mount_point) {
            continue;
        }
        let depth = Path::new(&mount_point).components().count();
        if best.as_ref().is_none_or(|(d, _)| depth >= *d) {
            best = Some((depth, fs_type.to_string()));
        }
    }
    best.map(|(_, t)| t)
}

/// Mount points escape space, tab, newline and backslash as octal.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 4 <= b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 4])
                .ok()
                .and_then(|o| u8::from_str_radix(o, 8).ok())
            {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn provider_from_mount(fs_type: &str, path: &Path) -> Option<Provider> {
    match fs_type {
        "fuse.google-drive-ocamlfuse" => Some(Provider::GoogleDrive),
        "fuse.gvfsd-fuse" if path.to_str().is_some_and(|p| p.contains("google-drive:")) => {
            Some(Provider::GoogleDrive)
        }
        "nfs" | "nfs4" | "cifs" | "smb3" | "smbfs" | "9p" | "afs" | "ceph" | "glusterfs"
        | "fuse.sshfs" | "fuse.rclone" | "davfs" | "fuse.davfs2" | "fuse.gvfsd-fuse" => {
            Some(Provider::Network)
        }
        _ => None,
    }
}

impl CloudFiles for LinuxCloudFiles {
    fn residency(&self, path: &Path) -> Residency {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Residency::Local,
            Err(_) => Residency::Unknown,
        }
    }

    fn request_download(&self, path: &Path) {
        let _ = path;
    }

    fn pin_folder(&self, dir: &Path) -> anyhow::Result<()> {
        let _ = dir;
        Ok(())
    }

    fn detect_provider(&self, path: &Path) -> Provider {
        if let Some(p) = provider_of(path, &known_roots()) {
            return p;
        }
        if in_syncthing_folder(path) {
            return Provider::Syncthing;
        }
        mount_type(path)
            .and_then(|t| provider_from_mount(&t, path))
            .unwrap_or(Provider::Other)
    }

    fn suggest_folders(&self) -> Vec<SuggestedFolder> {
        existing(known_roots())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_gives_the_deepest_mount() {
        let info = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
40 22 0:44 / /mnt/my\\040share rw,relatime - cifs //server/share rw
41 22 0:45 / /home/u/gdrive rw - fuse.google-drive-ocamlfuse gdfs rw
";
        assert_eq!(
            parse_mount_type(info, Path::new("/home/u/notes")).as_deref(),
            Some("ext4")
        );
        assert_eq!(
            parse_mount_type(info, Path::new("/mnt/my share/Kage")).as_deref(),
            Some("cifs")
        );
        assert_eq!(
            provider_from_mount("cifs", Path::new("/mnt/x")),
            Some(Provider::Network)
        );
        assert_eq!(
            provider_from_mount("fuse.google-drive-ocamlfuse", Path::new("/home/u/gdrive")),
            Some(Provider::GoogleDrive)
        );
        assert_eq!(provider_from_mount("ext4", Path::new("/")), None);
    }

    #[test]
    fn onedrive_config_sync_dir() {
        assert_eq!(
            parse_onedrive_sync_dir("# sync_dir = \"/x\"\nsync_dir = \"~/Cloud/OneDrive\"\n")
                .as_deref(),
            Some("~/Cloud/OneDrive")
        );
        assert_eq!(parse_onedrive_sync_dir("skip_file = \"~*\""), None);
    }
}
