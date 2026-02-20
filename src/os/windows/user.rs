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

/// Get the current user's SID as a string using Win32 API
fn get_current_user_sid() -> Option<String> {
    use std::process::Command;

    // Use whoami /user to get the SID (simplest cross-compatible approach)
    let output = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .ok()?;

    let text = String::from_utf8_lossy(&output.stdout);
    // Output format: "DOMAIN\user","S-1-5-..."
    let parts: Vec<&str> = text.trim().split(',').collect();
    if parts.len() >= 2 {
        let sid = parts[1].trim().trim_matches('"');
        if sid.starts_with("S-1-") {
            return Some(sid.to_string());
        }
    }

    None
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
