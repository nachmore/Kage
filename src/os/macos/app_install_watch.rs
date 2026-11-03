//! macOS needs nothing beyond the filesystem watcher in
//! `os::app_install_watch` (every app source is a directory), but decides
//! which of its events count.

use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::EventKind;
use std::path::Path;

pub fn start_extra_impl(_on_change: crate::os::app_install_watch::ChangeCallback) {}

/// The application dirs are watched recursively so vendor-folder installs
/// are seen, which also delivers every write a program makes into a plain
/// folder there (MAMP/XAMPP keep logs and databases under
/// /Applications/<vendor>/). Only changes the scan can observe count:
///
/// - anything at an `.app` path — including metadata, which is the only
///   event a Finder/DMG drag-copy raises at the bundle when it finishes;
///   the copy's own writes are inside the bundle and dropped;
/// - a folder created or removed (a vendor folder can hold apps);
/// - a rename of something that is now a folder, or of an
///   extension-less entry that vanished (a vendor folder moved to the
///   Trash) — FSEvents renames don't say whether the item was a folder;
///
/// and never anything inside a bundle or a hidden folder below `root`
/// (`launcher::affects_app_list`).
pub fn is_relevant_change_impl(kind: &EventKind, root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if !kage_core::os::launcher::affects_app_list(relative) {
        return false;
    }
    let is_app = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    if is_app {
        return true;
    }
    match kind {
        EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder) => true,
        EventKind::Modify(ModifyKind::Name(_)) => {
            path.is_dir() || (!path.exists() && path.extension().is_none())
        }
        _ => false,
    }
}
