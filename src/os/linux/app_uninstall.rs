// Linux: uninstalling from the launcher isn't offered — the distribution's
// package manager (apt, dnf, flatpak, snap, …) owns app removal, and there's
// no single correct way to drive it. See `os::app_uninstall`.

use crate::os::app_uninstall::{UninstallError, UninstallOutcome};
use std::path::Path;

pub fn uninstall_impl(_path: &Path, _name: &str) -> Result<UninstallOutcome, UninstallError> {
    Err(UninstallError::Unsupported)
}

pub fn is_protected_app_impl(_bundle: &Path) -> bool {
    false
}

pub fn bundle_identifier_impl(_bundle: &Path) -> Option<String> {
    None
}
