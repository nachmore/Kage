//! File operations behind the launcher's result menu: put files on the
//! clipboard the way the OS file manager does (so Finder / Explorer can
//! paste them), move items to the Trash / Recycle Bin, and the
//! Windows-only shell verbs (run as administrator, Properties).
//!
//! Support differs per OS, and the menu only offers what works here:
//!
//! | operation            | macOS            | Windows                     | Linux                  |
//! |----------------------|------------------|-----------------------------|------------------------|
//! | copy file            | NSPasteboard URL | CF_HDROP + DropEffect=copy  | —                      |
//! | cut file             | — (no file cut)  | CF_HDROP + DropEffect=move  | —                      |
//! | move to trash        | NSFileManager    | SHFileOperation + ALLOWUNDO | gio / trash-cli / kio  |
//! | run as administrator | —                | ShellExecuteEx "runas"      | —                      |
//! | properties           | —                | ShellExecuteEx "properties" | —                      |
//!
//! Linux has no file-clipboard format every file manager reads (GNOME
//! uses `x-special/gnome-copied-files`, KDE `text/uri-list` plus its own
//! cut marker) and `xclip` can only serve one of them, so copy and cut
//! are left out there rather than half-working.

use std::path::{Path, PathBuf};

/// Why a trash request failed.
#[derive(Debug)]
pub enum TrashError {
    /// The OS refused: root-owned or SIP-protected item, read-only volume.
    PermissionDenied(String),
    /// No trash implementation available (Linux without gio/trash-cli/kio).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Unavailable,
    Other(String),
}

impl std::fmt::Display for TrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrashError::PermissionDenied(s) => write!(f, "permission denied: {s}"),
            TrashError::Unavailable => write!(f, "no trash implementation available"),
            TrashError::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Put `paths` on the clipboard as files. With `cut`, pasting in the file
/// manager moves them (Windows only; macOS has no file cut).
pub fn copy_files_to_clipboard(paths: &[PathBuf], cut: bool) -> anyhow::Result<()> {
    if paths.is_empty() {
        anyhow::bail!("no files to copy");
    }
    crate::os::platform::file_ops::copy_files_to_clipboard_impl(paths, cut)
}

/// Move `path` to the Trash / Recycle Bin, where the OS can restore it.
pub fn move_to_trash(path: &Path) -> Result<(), TrashError> {
    crate::os::platform::file_ops::move_to_trash_impl(path)
}

/// Whether `move_to_trash` can work at all on this machine.
pub fn trash_available() -> bool {
    crate::os::platform::file_ops::trash_available_impl()
}

/// Launch `path` (an .exe or shortcut) elevated, through the normal UAC
/// prompt. Returns `Ok(false)` when the user declined the prompt.
pub fn run_as_admin(path: &Path) -> anyhow::Result<bool> {
    crate::os::platform::file_ops::run_as_admin_impl(path)
}

/// Open the shell's Properties dialog for `path`. Windows: call this on a
/// thread with a message loop (the Tauri main thread) — the property
/// sheet belongs to the caller's apartment.
pub fn show_properties(path: &Path) -> anyhow::Result<()> {
    crate::os::platform::file_ops::show_properties_impl(path)
}

/// Which file a path names, independent of how it was spelled: (device,
/// inode) on Unix, (volume serial, file index) on Windows. Taken of the
/// item itself, never of a link's target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId {
    pub volume: u64,
    pub index: u64,
}

/// The identity of the item at `path` (a link is identified as the link),
/// or `None` if it can't be read.
pub fn file_id(path: &Path) -> Option<FileId> {
    crate::os::platform::file_ops::file_id_impl(path)
}

/// Whether `meta` (from `symlink_metadata`) is a link the OS would follow:
/// a symlink, or on Windows any reparse point (junctions included).
pub fn is_link(meta: &std::fs::Metadata) -> bool {
    crate::os::platform::file_ops::is_link_impl(meta)
}

/// The final component of `path` as the directory stores it — Windows
/// resolves 8.3 aliases (`DOCUME~1`) and drops trailing dots and spaces
/// (`Documents.`) when opening a path, so the name typed may not be the
/// item's name. Doesn't follow a link. `None` elsewhere, or on failure.
pub fn on_disk_name(path: &Path) -> Option<std::ffi::OsString> {
    crate::os::platform::file_ops::on_disk_name_impl(path)
}

/// `DROPEFFECT_COPY` / `DROPEFFECT_MOVE` for the clipboard's
/// "Preferred DropEffect" format, which tells Explorer whether a paste
/// copies or moves.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn preferred_drop_effect(cut: bool) -> u32 {
    if cut {
        2
    } else {
        1
    }
}

/// The `CF_HDROP` clipboard payload for `paths`: a 20-byte `DROPFILES`
/// header (`pFiles` = 20, no point, `fNC` = 0, `fWide` = 1) followed by
/// each path as NUL-terminated UTF-16LE and one more NUL ending the list.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn build_hdrop_payload(paths: &[&str]) -> Vec<u8> {
    const HEADER: u32 = 20;
    let mut out = Vec::with_capacity(HEADER as usize + 2);
    out.extend_from_slice(&HEADER.to_le_bytes()); // pFiles
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0i32.to_le_bytes()); // fNC
    out.extend_from_slice(&1i32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_list(bytes: &[u8]) -> Vec<String> {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let mut out = Vec::new();
        let mut cur = Vec::new();
        for u in units {
            if u == 0 {
                if cur.is_empty() {
                    break;
                }
                out.push(String::from_utf16(&cur).unwrap());
                cur.clear();
            } else {
                cur.push(u);
            }
        }
        out
    }

    #[test]
    fn hdrop_header_points_at_a_wide_file_list() {
        let payload = build_hdrop_payload(&[r"C:\Users\me\report.pdf"]);
        assert_eq!(u32::from_le_bytes(payload[0..4].try_into().unwrap()), 20);
        assert_eq!(i32::from_le_bytes(payload[12..16].try_into().unwrap()), 0);
        assert_eq!(i32::from_le_bytes(payload[16..20].try_into().unwrap()), 1);
        assert_eq!(utf16_list(&payload[20..]), vec![r"C:\Users\me\report.pdf"]);
    }

    #[test]
    fn hdrop_list_is_double_nul_terminated() {
        let payload = build_hdrop_payload(&[r"C:\a.txt", r"D:\b — ü.txt"]);
        assert!(payload.ends_with(&[0, 0, 0, 0]));
        assert_eq!(
            utf16_list(&payload[20..]),
            vec![r"C:\a.txt", r"D:\b — ü.txt"]
        );
        // header + "C:\a.txt\0" + "D:\b — ü.txt\0" + "\0", two bytes a unit
        let units = 8 + 1 + 12 + 1 + 1;
        assert_eq!(payload.len(), 20 + units * 2);
    }

    #[test]
    fn drop_effect_distinguishes_cut_from_copy() {
        assert_eq!(preferred_drop_effect(false), 1);
        assert_eq!(preferred_drop_effect(true), 2);
    }
}
