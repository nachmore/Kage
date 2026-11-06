// Linux: uninstalling from the launcher isn't offered — the distribution's
// package manager (apt, dnf, flatpak, snap, …) owns app removal, and there's
// no single correct way to drive it. See `os::app_uninstall`.

use crate::os::app_uninstall::{UninstallError, UninstallOutcome};
use crate::os::file_ops::Preflight;
use std::path::Path;

pub fn uninstall_impl(
    _path: &Path,
    _name: &str,
    _preflight: Preflight,
) -> Result<UninstallOutcome, UninstallError> {
    Err(UninstallError::Unsupported)
}

pub fn is_protected_app_impl(_bundle: &Path) -> bool {
    false
}

pub fn bundle_identifier_impl(_bundle: &Path) -> Option<String> {
    None
}

/// Nothing records install folders here (macOS apps are their bundles).
pub fn installed_product_dirs_impl() -> Vec<std::path::PathBuf> {
    Vec::new()
}
