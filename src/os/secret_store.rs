//! The OS secret store: macOS Keychain, Windows Credential Manager, Linux
//! Secret Service (docs/design/SYNC.md §11.4). Entries never roam: not
//! synchronizable on macOS, `CRED_PERSIST_LOCAL_MACHINE` on Windows.
//!
//! C0 skeleton: every call fails. WP-S2 adds the per-OS implementations
//! behind this module.

use zeroize::Zeroizing;

const NOT_BUILT: &str = "the OS secret store isn't built yet";

/// The secret stored under `service` / `account`, or `None`.
pub fn get(service: &str, account: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
    let _ = (service, account);
    anyhow::bail!(NOT_BUILT)
}

/// Store `secret` under `service` / `account`, replacing any earlier one.
pub fn set(service: &str, account: &str, secret: &[u8]) -> anyhow::Result<()> {
    let _ = (service, account, secret);
    anyhow::bail!(NOT_BUILT)
}

/// Remove the secret under `service` / `account`. Removing nothing is fine.
pub fn delete(service: &str, account: &str) -> anyhow::Result<()> {
    let _ = (service, account);
    anyhow::bail!(NOT_BUILT)
}

/// Whether the keychain keeps working across updates. macOS: only with a
/// stable Developer ID signature; ad-hoc-signed builds change signature on
/// every update. True elsewhere once implemented.
pub fn keychain_is_reliable() -> bool {
    false
}
