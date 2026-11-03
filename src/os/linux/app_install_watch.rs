//! Linux needs nothing beyond the filesystem watcher in
//! `os::app_install_watch`: every app source the launcher scans is a
//! directory of `.desktop` files.

use notify::event::ModifyKind;
use notify::EventKind;
use std::path::Path;

pub fn start_extra_impl(_on_change: crate::os::app_install_watch::ChangeCallback) {}

/// The `applications/` dirs are watched non-recursively and hold only
/// desktop entries, so any content change can matter (a `.desktop` edit
/// can flip `NoDisplay`); permission/timestamp changes can't.
pub fn is_relevant_change_impl(kind: &EventKind, _root: &Path, _path: &Path) -> bool {
    !matches!(kind, EventKind::Modify(ModifyKind::Metadata(_)))
}
