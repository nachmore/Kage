// Linux file operations for launcher results. Trash goes through the
// desktop's own freedesktop.org Trash implementation (`gio trash`, then
// trash-cli, then KDE's kioclient), so the file manager can restore it.
// File copy/cut aren't offered on Linux — see `os::file_ops`.

use crate::os::file_ops::{ClipboardFilesError, Preflight, TrashError};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Trash tools in order of preference, with the arguments that go before
/// the path.
const TRASH_TOOLS: &[(&str, &[&str])] = &[
    ("gio", &["trash", "--"]),
    ("trash-put", &["--"]),
    ("kioclient6", &["move"]),
    ("kioclient5", &["move"]),
];

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

pub fn copy_files_to_clipboard_impl(
    _paths: &[PathBuf],
    _cut: bool,
    _preflight: Option<Preflight>,
) -> Result<(), ClipboardFilesError> {
    Err(anyhow::anyhow!("copying files to the clipboard isn't supported on Linux").into())
}

/// `preflight` runs right before the trash tool is started. The tool is a
/// separate process handed the path, so the move itself is path-based
/// (see `os::file_ops`).
pub fn move_to_trash_impl(path: &Path, preflight: Preflight) -> Result<(), TrashError> {
    let Some((program, args)) = TRASH_TOOLS.iter().find(|(p, _)| on_path(p)) else {
        return Err(TrashError::Unavailable);
    };
    preflight().map_err(TrashError::Refused)?;
    let mut cmd = Command::new(program);
    cmd.args(*args).arg(path);
    if program.starts_with("kioclient") {
        cmd.arg("trash:/");
    }
    let output = cmd
        .output()
        .map_err(|e| TrashError::Other(format!("{program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    log::warn!("[file_ops] {program} failed ({}): {stderr}", output.status);
    let lower = stderr.to_lowercase();
    if lower.contains("permission denied") || lower.contains("not permitted") {
        Err(TrashError::PermissionDenied(stderr))
    } else {
        Err(TrashError::Other(stderr))
    }
}

pub fn trash_available_impl() -> bool {
    TRASH_TOOLS.iter().any(|(p, _)| on_path(p))
}

pub fn run_as_admin_impl(_path: &Path) -> anyhow::Result<bool> {
    anyhow::bail!("run as administrator is Windows-only")
}

pub fn show_properties_impl(_path: &Path) -> anyhow::Result<()> {
    anyhow::bail!("the Properties dialog is Windows-only")
}

pub fn file_id_impl(path: &Path) -> Option<crate::os::file_ops::FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(crate::os::file_ops::FileId {
        volume: meta.dev(),
        index: meta.ino(),
    })
}

pub fn is_link_impl(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

pub fn on_disk_name_impl(_path: &Path) -> Option<std::ffi::OsString> {
    None
}
