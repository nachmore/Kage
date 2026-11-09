//! Windows: a generic credential in Credential Manager, persisted with
//! `CRED_PERSIST_LOCAL_MACHINE` (docs/design/SYNC.md §11.4). Never
//! `CRED_PERSIST_ENTERPRISE`, which roams with a roaming profile.
//!
//! The target name is `<service>/<account>` (`Kage Sync/<folder id>`), the
//! user name is the account, and the blob is the secret.

use crate::os::secret_store::{SecretBackend, SecretError};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_NOT_FOUND, ERROR_NO_SUCH_LOGON_SESSION, WIN32_ERROR,
};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};
use zeroize::Zeroizing;

pub const BACKEND: SecretBackend = SecretBackend::CredentialManager;

/// Credential Manager's limit on a blob.
const MAX_BLOB: usize = 5 * 512;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn target(service: &str, account: &str) -> Vec<u16> {
    wide(&format!("{}/{}", service, account))
}

fn is(e: &windows::core::Error, code: WIN32_ERROR) -> bool {
    e.code() == code.to_hresult()
}

fn map(e: windows::core::Error) -> anyhow::Error {
    let kind = if is(&e, ERROR_ACCESS_DENIED) {
        Some(SecretError::Denied)
    } else if is(&e, ERROR_NO_SUCH_LOGON_SESSION) {
        Some(SecretError::Unavailable)
    } else {
        None
    };
    let detail = format!("Credential Manager error {:#010x}", e.code().0);
    match kind {
        Some(k) => anyhow::Error::new(k).context(detail),
        None => anyhow::anyhow!(detail),
    }
}

pub fn get_impl(service: &str, account: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
    let t = target(service, account);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `t` is a NUL-terminated UTF-16 string that outlives the call;
    // on success `cred` points at a buffer the OS allocated for us.
    match unsafe { CredReadW(PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, None, &mut cred) } {
        Ok(()) => {
            // SAFETY: `cred` is valid until `CredFree`; the blob is
            // `CredentialBlobSize` bytes at `CredentialBlob`.
            let secret = unsafe {
                let c = &*cred;
                let len = c.CredentialBlobSize as usize;
                let out = if c.CredentialBlob.is_null() || len == 0 {
                    Vec::new()
                } else {
                    let bytes = std::slice::from_raw_parts(c.CredentialBlob, len).to_vec();
                    std::ptr::write_bytes(c.CredentialBlob, 0, len);
                    bytes
                };
                CredFree(cred as *const std::ffi::c_void);
                out
            };
            Ok(Some(Zeroizing::new(secret)))
        }
        Err(e) if is(&e, ERROR_NOT_FOUND) => Ok(None),
        Err(e) => Err(map(e)),
    }
}

pub fn set_impl(service: &str, account: &str, secret: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(secret.len() <= MAX_BLOB, "the secret is too large");
    let mut t = target(service, account);
    let mut user = wide(account);
    let mut blob = Zeroizing::new(secret.to_vec());
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(t.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` points at a buffer that outlives the
    // call; the OS copies what it keeps.
    unsafe { CredWriteW(&cred, 0) }.map_err(map)
}

pub fn delete_impl(service: &str, account: &str) -> anyhow::Result<()> {
    let t = target(service, account);
    // SAFETY: `t` is a NUL-terminated UTF-16 string that outlives the call.
    match unsafe { CredDeleteW(PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        Err(e) if is(&e, ERROR_NOT_FOUND) => Ok(()),
        Err(e) => Err(map(e)),
    }
}

/// Credential Manager doesn't depend on the code signature.
pub fn keychain_is_reliable_impl() -> bool {
    true
}
