// macOS user avatar lookup

use std::path::PathBuf;
use std::process::Command;

/// Find the user's profile picture on macOS.
///
/// Strategy (first match wins):
/// 1. `dscl . -read /Users/<username> Picture` — returns a file path stored in Directory Services
/// 2. Extract `JPEGPhoto` binary data from Directory Services → write to cache file
/// 3. Check legacy cache location `/Library/Caches/com.apple.user<uid>pictureCache.tiff`
/// 4. Freedesktop fallback `~/.face`
pub fn get_avatar_path_impl(username: &str) -> Option<String> {
    // 1. Try the Picture attribute (a file path stored in the user record)
    if let Some(path) = get_picture_path_from_dscl(username) {
        log::info!("[USER] Avatar found via dscl Picture attribute: {}", path);
        return Some(path);
    }

    // 2. Try extracting the embedded JPEGPhoto data
    if let Some(path) = extract_jpeg_photo_from_dscl(username) {
        log::info!("[USER] Avatar extracted from dscl JPEGPhoto: {}", path);
        return Some(path);
    }

    // 3. Legacy cache location (older macOS versions)
    if let Some(path) = get_avatar_from_cache() {
        log::info!("[USER] Avatar found in system cache: {}", path);
        return Some(path);
    }

    // 4. Freedesktop fallback (~/.face)
    if let Some(home) = dirs::home_dir() {
        let face = home.join(".face");
        if face.exists() {
            return face.to_str().map(|s| s.to_string());
        }
    }

    log::info!("[USER] No avatar found for user '{}'", username);
    None
}

/// Read the `Picture` attribute from Directory Services — this is a file path.
fn get_picture_path_from_dscl(username: &str) -> Option<String> {
    let output = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", username), "Picture"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Output format: "Picture:\n /path/to/image.png" or "Picture: /path/to/image.png"
    for line in stdout.lines() {
        let trimmed = line.trim();
        // Skip the "Picture:" label line
        if trimmed == "Picture:" || trimmed.is_empty() {
            continue;
        }
        // Strip "Picture: " prefix if on same line
        let path_str = trimmed.strip_prefix("Picture: ").unwrap_or(trimmed);
        let path = PathBuf::from(path_str);
        if path.exists() {
            return path.to_str().map(|s| s.to_string());
        }
    }

    None
}

/// Extract the raw JPEG data from the `JPEGPhoto` attribute and write it to a cache file.
/// Returns the path to the cached JPEG file.
fn extract_jpeg_photo_from_dscl(username: &str) -> Option<String> {
    // dscl outputs the JPEGPhoto as a hex dump after the first line
    let output = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{}", username), "JPEGPhoto"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    // First line is "JPEGPhoto:" header, remaining lines are hex data
    if lines.len() < 2 {
        return None;
    }

    // Collect all hex data (skip the header line, concatenate remaining)
    let hex_data: String = lines[1..]
        .iter()
        .flat_map(|line| line.trim().chars())
        .filter(|c| c.is_ascii_hexdigit())
        .collect();

    if hex_data.len() < 10 {
        return None;
    }

    // Decode hex to bytes
    let bytes = hex_to_bytes(&hex_data)?;

    // Write to a cache file in the app's cache directory
    let cache_dir = dirs::cache_dir()?.join("com.kage.app");
    std::fs::create_dir_all(&cache_dir).ok()?;
    let cache_path = cache_dir.join("user-avatar.jpg");
    std::fs::write(&cache_path, &bytes).ok()?;

    cache_path.to_str().map(|s| s.to_string())
}

/// Check the legacy system cache location for the user picture.
fn get_avatar_from_cache() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let cache_path = PathBuf::from(format!(
        "/Library/Caches/com.apple.user{}pictureCache.tiff",
        uid
    ));
    if cache_path.exists() {
        return cache_path.to_str().map(|s| s.to_string());
    }
    None
}

/// Decode a hex string into bytes.
fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    let chars: Vec<char> = hex.chars().collect();
    if chars.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(chars.len() / 2);
    for pair in chars.chunks(2) {
        let hi = pair[0].to_digit(16)?;
        let lo = pair[1].to_digit(16)?;
        bytes.push((hi * 16 + lo) as u8);
    }
    Some(bytes)
}
