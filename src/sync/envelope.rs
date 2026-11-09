//! The envelope of every `.kage` file (docs/design/SYNC.md §5.4):
//! AES-256-GCM, with the folder, device, kind and name in the
//! authenticated data, so a planted, edited or moved file fails to open.
//!
//! C0 skeleton: nothing seals or opens yet. WP-S2 writes the format and
//! its golden files.

use super::crypto::Keys;
use super::types::DeviceId;

/// The kind of a data file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Head,
    Shard,
    Blob,
}

/// What a file is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aad<'a> {
    pub folder_id: &'a str,
    pub device: DeviceId,
    pub kind: FileKind,
    pub name: &'a str,
}

/// Why a file didn't open. `Truncated` (short or still downloading) is
/// retried; `BadTag` means changed or planted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    Truncated,
    BadTag,
    UnknownEpoch(u32),
    TooLarge,
    NameMismatch,
}

/// Seal `plain` under the keys of `epoch`.
pub fn seal(keys: &Keys, epoch: u32, aad: &Aad<'_>, plain: &[u8]) -> Vec<u8> {
    let _ = (keys, epoch, aad, plain);
    Vec::new()
}

/// Open a sealed file. `keys` looks up the keys of the file's epoch.
pub fn open(
    keys: &dyn Fn(u32) -> Option<Keys>,
    aad: &Aad<'_>,
    bytes: &[u8],
) -> Result<Vec<u8>, OpenError> {
    let _ = (keys, aad, bytes);
    Err(OpenError::Truncated)
}
