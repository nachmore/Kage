// Windows user avatar lookup

/// Find the user's profile picture on Windows.
/// 1. Try the registry (AccountPicture\Users\{SID}) — works on Windows 10/11
/// 2. Fall back to scanning the AppData AccountPictures directory
pub fn get_avatar_path_impl(_username: &str) -> Option<String> {
    // Try registry first (most reliable on modern Windows)
    if let Some(path) = get_avatar_from_registry() {
        log::info!("[USER] Avatar found via registry: {}", path);
        return Some(path);
    }
    log::info!("[USER] No avatar from registry, trying AppData fallback");

    // Fallback: scan AppData AccountPictures directory
    let result = get_avatar_from_appdata();
    log::info!("[USER] AppData fallback result: {:?}", result);
    result
}

/// Read the current user's account picture from the registry.
/// The pictures are stored under:
/// HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users\{SID}
/// with values like Image1080, Image448, Image240, etc.
fn get_avatar_from_registry() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    // Get current user's SID
    let sid = get_current_user_sid()?;
    log::info!("[USER] Current user SID: {}", sid);

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key_path = format!(
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AccountPicture\\Users\\{}",
        sid
    );
    log::info!("[USER] Registry key path: {}", key_path);

    let key = match hklm.open_subkey(&key_path) {
        Ok(k) => k,
        Err(e) => {
            log::info!("[USER] Failed to open registry key: {}", e);
            return None;
        }
    };

    // Try sizes from largest to smallest
    let sizes = [
        "Image1080",
        "Image448",
        "Image424",
        "Image240",
        "Image208",
        "Image192",
        "Image96",
        "Image64",
        "Image48",
        "Image40",
        "Image32",
    ];

    for size in &sizes {
        if let Ok(path) = key.get_value::<String, _>(size) {
            let p = std::path::Path::new(&path);
            log::info!("[USER] Registry {} = {} (exists: {})", size, path, p.exists());
            if p.exists() {
                return Some(path);
            }
        }
    }

    log::info!("[USER] No valid image found in registry");
    None
}

/// Get the current user's SID as a string using Win32 API (no subprocess).
fn get_current_user_sid() -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        // Open the current process token
        let mut token_handle = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle).is_err() {
            return None;
        }

        // Query token for user SID — first call to get buffer size
        let mut return_length = 0u32;
        let _ = GetTokenInformation(
            token_handle,
            TokenUser,
            None,
            0,
            &mut return_length,
        );

        if return_length == 0 {
            let _ = CloseHandle(token_handle);
            return None;
        }

        // Allocate buffer and get the actual data
        let mut buffer = vec![0u8; return_length as usize];
        let result = GetTokenInformation(
            token_handle,
            TokenUser,
            Some(buffer.as_mut_ptr() as *mut _),
            return_length,
            &mut return_length,
        );

        let _ = CloseHandle(token_handle);

        if result.is_err() {
            return None;
        }

        // Extract the SID from TOKEN_USER
        let token_user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let sid = token_user.User.Sid;

        // Convert SID to string
        let mut sid_string = windows::core::PWSTR::null();
        if ConvertSidToStringSidW(sid, &mut sid_string).is_err() {
            return None;
        }

        let sid_str = sid_string.to_string().ok();

        // Free the string allocated by ConvertSidToStringSidW
        windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(sid_string.0 as *mut _)));

        sid_str
    }
}

/// Fallback: scan the AppData AccountPictures directory
fn get_avatar_from_appdata() -> Option<String> {
    let home = dirs::home_dir()?;
    let pictures_dir = home
        .join("AppData")
        .join("Roaming")
        .join("Microsoft")
        .join("Windows")
        .join("AccountPictures");

    if !pictures_dir.exists() {
        return None;
    }

    let entries = std::fs::read_dir(&pictures_dir).ok()?;
    let mut best: Option<(u64, std::path::PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "png" || ext == "jpg" || ext == "bmp" {
            if let Ok(meta) = std::fs::metadata(&path) {
                let size = meta.len();
                if best.as_ref().map_or(true, |(s, _)| size > *s) {
                    best = Some((size, path));
                }
            }
        }
    }

    best.and_then(|(_, path)| path.to_str().map(|s| s.to_string()))
}
