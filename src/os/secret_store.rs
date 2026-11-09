//! The OS secret store: macOS Keychain, Windows Credential Manager, Linux
//! Secret Service (docs/design/SYNC.md §11.4). Entries never roam: not
//! synchronizable on macOS, `CRED_PERSIST_LOCAL_MACHINE` on Windows.
//!
//! Nothing here touches a real key store while user dirs are isolated
//! (tests, the mock-app sweep, a scratch profile): every call fails with
//! [`SecretError::Isolated`] before reaching the OS.

use std::sync::atomic::{AtomicUsize, Ordering};
use zeroize::Zeroizing;

/// Which OS store this build uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretBackend {
    Keychain,
    CredentialManager,
    SecretService,
}

/// Why the store refused. Carried inside the `anyhow::Error` the calls
/// return; find it with `downcast_ref`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretError {
    /// The user or the OS denied access to the entry.
    Denied,
    /// The store isn't there, is locked, or didn't answer.
    Unavailable,
    /// User dirs are isolated: the real store is never used.
    Isolated,
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Denied => "access to the key store was denied",
            Self::Unavailable => "the key store isn't available",
            Self::Isolated => "the OS key store isn't used while user dirs are isolated",
        })
    }
}

impl std::error::Error for SecretError {}

/// Calls that reached the OS store in this process.
static OS_CALLS: AtomicUsize = AtomicUsize::new(0);

/// How many calls reached the OS store in this process (tests assert none
/// do while isolated).
pub fn os_calls() -> usize {
    OS_CALLS.load(Ordering::SeqCst)
}

/// Whether the real store is off limits: unit tests of this crate, and any
/// process whose user dirs are isolated.
pub fn is_off_limits() -> bool {
    cfg!(test) || crate::paths::is_isolated()
}

fn enter() -> anyhow::Result<()> {
    if is_off_limits() {
        return Err(SecretError::Isolated.into());
    }
    OS_CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// The OS store of this build.
pub fn backend() -> SecretBackend {
    crate::os::platform::secret_store::BACKEND
}

/// The secret stored under `service` / `account`, or `None`.
pub fn get(service: &str, account: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
    enter()?;
    crate::os::platform::secret_store::get_impl(service, account)
}

/// Store `secret` under `service` / `account`, replacing any earlier one.
pub fn set(service: &str, account: &str, secret: &[u8]) -> anyhow::Result<()> {
    enter()?;
    crate::os::platform::secret_store::set_impl(service, account, secret)
}

/// Remove the secret under `service` / `account`. Removing nothing is fine.
pub fn delete(service: &str, account: &str) -> anyhow::Result<()> {
    enter()?;
    crate::os::platform::secret_store::delete_impl(service, account)
}

/// Whether the store keeps working across updates and answers:
/// - macOS: only with a stable signature (a team identifier in this
///   build's signing information). Ad-hoc-signed builds change signature
///   on every update, and macOS would ask for the keychain again each time.
/// - Windows: always.
/// - Linux: the Secret Service answered within 2 seconds.
///
/// False while user dirs are isolated. Call it off the launch path: on
/// Linux it waits for D-Bus.
pub fn keychain_is_reliable() -> bool {
    if enter().is_err() {
        return false;
    }
    crate::os::platform::secret_store::keychain_is_reliable_impl()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_reaches_the_os_store_in_tests() {
        let before = os_calls();
        assert!(is_off_limits());
        for err in [
            get("Kage Sync", "aaaaaaaaaaaaaaaa").unwrap_err(),
            set("Kage Sync", "aaaaaaaaaaaaaaaa", b"x").unwrap_err(),
            delete("Kage Sync", "aaaaaaaaaaaaaaaa").unwrap_err(),
        ] {
            assert_eq!(
                err.downcast_ref::<SecretError>(),
                Some(&SecretError::Isolated)
            );
        }
        assert!(!keychain_is_reliable());
        assert_eq!(os_calls(), before);
        assert_eq!(os_calls(), 0);
    }
}
