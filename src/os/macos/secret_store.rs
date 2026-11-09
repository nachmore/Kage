//! macOS: a generic password in the login keychain, never synchronizable,
//! so iCloud Keychain doesn't copy it (docs/design/SYNC.md §11.4).
//!
//! The keychain ties an item's access list to the app's code signature.
//! `keychain_is_reliable_impl` therefore says yes only for a build with a
//! team identifier in its signing information (Developer ID); an
//! ad-hoc-signed build changes signature on every update, so macOS would
//! ask for the keychain again after each one.

use crate::os::secret_store::{SecretBackend, SecretError};
use security_framework::passwords::{
    delete_generic_password_options, generic_password, set_generic_password_options,
    PasswordOptions,
};
use std::sync::OnceLock;
use zeroize::Zeroizing;

pub const BACKEND: SecretBackend = SecretBackend::Keychain;

/// `errSecItemNotFound`.
const ITEM_NOT_FOUND: i32 = -25300;
/// `errSecUserCanceled`, `errSecAuthFailed`.
const DENIED: [i32; 2] = [-128, -25293];
/// `errSecNotAvailable`, `errSecNoSuchKeychain`, `errSecInteractionNotAllowed`,
/// `errSecMissingEntitlement`.
const UNAVAILABLE: [i32; 4] = [-25291, -25294, -25308, -34018];

fn options(service: &str, account: &str) -> PasswordOptions {
    let mut o = PasswordOptions::new_generic_password(service, account);
    // Only the store that never leaves this Mac.
    o.set_access_synchronized(Some(false));
    o
}

fn map(e: security_framework::base::Error) -> anyhow::Error {
    let code = e.code();
    let kind = if DENIED.contains(&code) {
        Some(SecretError::Denied)
    } else if UNAVAILABLE.contains(&code) {
        Some(SecretError::Unavailable)
    } else {
        None
    };
    match kind {
        Some(k) => anyhow::Error::new(k).context(format!("keychain status {}", code)),
        None => anyhow::anyhow!("keychain status {}", code),
    }
}

pub fn get_impl(service: &str, account: &str) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
    match generic_password(options(service, account)) {
        Ok(v) => Ok(Some(Zeroizing::new(v))),
        Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
        Err(e) => Err(map(e)),
    }
}

pub fn set_impl(service: &str, account: &str, secret: &[u8]) -> anyhow::Result<()> {
    set_generic_password_options(secret, options(service, account)).map_err(map)
}

pub fn delete_impl(service: &str, account: &str) -> anyhow::Result<()> {
    match delete_generic_password_options(options(service, account)) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
        Err(e) => Err(map(e)),
    }
}

pub fn keychain_is_reliable_impl() -> bool {
    static RELIABLE: OnceLock<bool> = OnceLock::new();
    *RELIABLE.get_or_init(|| {
        let reliable = team_identifier().is_some();
        log::info!(
            "Sync key store: this build {} a team identifier",
            if reliable { "has" } else { "doesn't have" }
        );
        reliable
    })
}

/// The team identifier in this process's own signing information. `None`
/// for an ad-hoc or unsigned build.
fn team_identifier() -> Option<String> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use security_framework::os::macos::code_signing::{Flags, SecCode};

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecCodeCopySigningInformation(
            code: *const std::ffi::c_void,
            flags: u32,
            information: *mut CFDictionaryRef,
        ) -> i32;
        static kSecCodeInfoTeamIdentifier: CFStringRef;
    }
    /// `kSecCSSigningInformation`.
    const SIGNING_INFORMATION: u32 = 1 << 1;

    let code = SecCode::for_self(Flags::NONE).ok()?;
    let mut info: CFDictionaryRef = std::ptr::null();
    // SAFETY: `code` is a live SecCode (a SecStaticCode subtype) for the
    // whole call; on success `info` is a dictionary we own (create rule).
    let status = unsafe {
        SecCodeCopySigningInformation(
            code.as_concrete_TypeRef() as *const std::ffi::c_void,
            SIGNING_INFORMATION,
            &mut info,
        )
    };
    if status != 0 || info.is_null() {
        return None;
    }
    // SAFETY: owned by us per the create rule; the key is a constant.
    let dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(info) };
    let key = unsafe { CFString::wrap_under_get_rule(kSecCodeInfoTeamIdentifier) };
    let team = dict.find(&key)?.downcast::<CFString>()?.to_string();
    (!team.is_empty()).then_some(team)
}
