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
//! |                      |                  | (paths under MAX_PATH only) |                        |
//! | run as administrator | —                | ShellExecuteEx "runas"      | —                      |
//! | properties           | —                | ShellExecuteEx "properties" | —                      |
//!
//! Linux has no file-clipboard format every file manager reads (GNOME
//! uses `x-special/gnome-copied-files`, KDE `text/uri-list` plus its own
//! cut marker) and `xclip` can only serve one of them, so copy and cut
//! are left out there rather than half-working.
//!
//! # The last-moment check
//!
//! The destructive entry points (`move_to_trash`, a Cut through
//! `copy_files_to_clipboard`) take a `Preflight`: the caller's re-check
//! that the path still names the item it vetted (same canonical path, same
//! file identity — see `launcher_actions::Resolved::verify_unchanged`).
//! Each platform runs it **on the thread that makes the destructive call,
//! immediately before it** — never on one thread with the call handed to
//! another afterwards — and an `Err` aborts with nothing done.
//!
//! What remains is inherent to the OS APIs, which take paths, not handles:
//!
//! - The check and the call are still two steps. Something that swaps the
//!   item (or a folder on its path) in the instant between them wins; the
//!   check narrows that window to microseconds, it can't close it.
//! - A Cut is pasted later, by the file manager, from the `CF_HDROP` path
//!   list — path-based by design. Kage checks the item when it publishes
//!   the list; what the path names when the user pastes is the file
//!   manager's to resolve.
//! - Linux trashes through `gio` / `trash-cli` / `kioclient`, separate
//!   processes that are handed the path.
//! - File identity is (volume, file index). On FAT and exFAT volumes
//!   there is no stable file index — it is derived from where the
//!   directory entry sits — so a replacement that reuses the same entry
//!   can carry the same "identity"; and on ReFS (Windows Dev Drives) the
//!   64-bit index Windows reports isn't guaranteed unique. The
//!   canonical-path half of the check still applies there.

use crate::error::AppError;
use std::path::{Path, PathBuf};

/// The caller's last check before a destructive call, run on the calling
/// thread right before it (see the module docs). `Err` refuses.
pub type Preflight<'a> = &'a (dyn Fn() -> Result<(), AppError> + Sync);

/// Why a trash request failed.
#[derive(Debug)]
pub enum TrashError {
    /// The OS refused: root-owned or SIP-protected item, read-only volume.
    PermissionDenied(String),
    /// No trash implementation available (Linux without gio/trash-cli/kio).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Unavailable,
    /// Windows: the path is too long for the Recycle Bin API (`MAX_PATH`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    PathTooLong,
    /// The preflight refused: the item isn't the one that was checked.
    Refused(AppError),
    Other(String),
}

impl std::fmt::Display for TrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrashError::PermissionDenied(s) => write!(f, "permission denied: {s}"),
            TrashError::Unavailable => write!(f, "no trash implementation available"),
            TrashError::PathTooLong => write!(f, "path too long for the Recycle Bin"),
            TrashError::Refused(e) => write!(f, "refused: {e}"),
            TrashError::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Why putting files on the clipboard failed.
#[derive(Debug)]
pub enum ClipboardFilesError {
    /// The preflight refused: nothing was published. (Only a Cut has
    /// one, and only Windows cuts files.)
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Refused(AppError),
    Failed(anyhow::Error),
}

impl std::fmt::Display for ClipboardFilesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardFilesError::Refused(e) => write!(f, "refused: {e}"),
            ClipboardFilesError::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl From<anyhow::Error> for ClipboardFilesError {
    fn from(e: anyhow::Error) -> Self {
        ClipboardFilesError::Failed(e)
    }
}

/// Put `paths` on the clipboard as files. With `cut`, pasting in the file
/// manager moves them (Windows only; macOS has no file cut). `preflight`
/// runs right before the clipboard is replaced, with it already open (see
/// the module docs); a Cut must pass one.
pub fn copy_files_to_clipboard(
    paths: &[PathBuf],
    cut: bool,
    preflight: Option<Preflight>,
) -> Result<(), ClipboardFilesError> {
    if paths.is_empty() {
        return Err(anyhow::anyhow!("no files to copy").into());
    }
    if cut && preflight.is_none() {
        return Err(anyhow::anyhow!("a cut needs a preflight check").into());
    }
    crate::os::platform::file_ops::copy_files_to_clipboard_impl(paths, cut, preflight)
}

/// Move `path` to the Trash / Recycle Bin, where the OS can restore it.
/// `preflight` runs on the thread that trashes it, right before (see the
/// module docs).
pub fn move_to_trash(path: &Path, preflight: Preflight) -> Result<(), TrashError> {
    crate::os::platform::file_ops::move_to_trash_impl(path, preflight)
}

/// The longest path (UTF-16 units) the Trash can take on this OS: Windows'
/// `SHFileOperationW` is limited to `MAX_PATH` (260, the terminating NUL
/// included). `None`: no limit Kage knows of.
pub fn trash_path_limit() -> Option<usize> {
    cfg!(target_os = "windows").then_some(WINDOWS_MAX_PATH - 1)
}

/// `MAX_PATH`, the terminating NUL included.
pub const WINDOWS_MAX_PATH: usize = 260;

/// Whether `path` is longer than `limit` UTF-16 units.
pub fn exceeds_path_limit(path: &Path, limit: Option<usize>) -> bool {
    limit.is_some_and(|max| path.as_os_str().to_string_lossy().encode_utf16().count() > max)
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
    fn path_limits_count_utf16_units() {
        let at = |n: usize| PathBuf::from(format!("C:\\{}", "a".repeat(n - 3)));
        let limit = Some(WINDOWS_MAX_PATH - 1);
        assert!(!exceeds_path_limit(&at(259), limit));
        assert!(exceeds_path_limit(&at(260), limit));
        // An astral character is two UTF-16 units.
        let astral = PathBuf::from(format!("C:\\{}\u{1F600}", "a".repeat(255)));
        assert!(exceeds_path_limit(&astral, limit));
        assert!(!exceeds_path_limit(&at(10_000), None));
    }

    #[test]
    fn a_cut_without_a_preflight_is_refused() {
        let err = copy_files_to_clipboard(&[PathBuf::from("/tmp/x")], true, None).unwrap_err();
        assert!(matches!(err, ClipboardFilesError::Failed(_)), "{err}");
    }

    #[test]
    fn drop_effect_distinguishes_cut_from_copy() {
        assert_eq!(preferred_drop_effect(false), 1);
        assert_eq!(preferred_drop_effect(true), 2);
    }
}
