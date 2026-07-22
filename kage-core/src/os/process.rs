//! Minimal process helpers shared by the app and the MCP sidecar.
//!
//! The app crate's `os::process` has the full surface (job objects,
//! signal handlers, stale-process cleanup); only the pieces kage-core's
//! own modules need live here. The app delegates to these so there is a
//! single implementation.

use std::process::Command;

/// Return the executable name for a PID, or None if the process doesn't
/// exist or can't be queried.
pub fn get_process_name(pid: u32) -> Option<String> {
    crate::os::platform::process::get_process_name_impl(pid)
}

/// Spawn a process detached from the parent's Job Object (Windows) so it
/// survives when the parent exits. Plain spawn on other platforms. Use
/// for user-facing launches (apps, URLs, explorer) — NOT for internal
/// children that should die with the parent.
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    crate::os::platform::process::spawn_detached_impl(cmd)
}
