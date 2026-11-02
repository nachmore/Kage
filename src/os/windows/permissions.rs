//! Windows has no per-app privacy grants for the APIs Kage uses (UI
//! Automation, SendInput, window titles all work unprompted), so every
//! permission is "not applicable" and there's nothing to repair.

use crate::os::permissions::{Permission, PermissionStatus, RepairError};

pub fn status_impl(_permission: Permission) -> PermissionStatus {
    PermissionStatus::NotApplicable
}

pub fn repair_impl(
    _permission: Permission,
    _bundle_id: &str,
) -> Result<PermissionStatus, RepairError> {
    Err(RepairError::Unsupported)
}
