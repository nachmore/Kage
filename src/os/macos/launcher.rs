// macOS application launcher

use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::os::launcher::AppInfo;

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = Vec::new();

    let applications_dir = PathBuf::from("/Applications");
    if applications_dir.exists() {
        if let Ok(entries) = fs::read_dir(&applications_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("app") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        let icon_path = path.to_string_lossy().to_string();
                        apps.push(AppInfo {
                            name: name.to_string(),
                            path: path.clone(),
                            icon_path: Some(icon_path),
                            emoji_icon: None,
                            icon_data: None,
                        });
                    }
                }
            }
        }
    }

    // Add System Settings pages (macOS Ventura+)
    let settings: Vec<(&str, &str, &str)> = vec![
        ("System Settings", "x-apple.systempreferences:", "⚙️"),
        (
            "Settings: Wi-Fi",
            "x-apple.systempreferences:com.apple.wifi-settings-extension",
            "📶",
        ),
        (
            "Settings: Bluetooth",
            "x-apple.systempreferences:com.apple.BluetoothSettings",
            "🔵",
        ),
        (
            "Settings: Network",
            "x-apple.systempreferences:com.apple.Network-Settings.extension",
            "🌐",
        ),
        (
            "Settings: Sound",
            "x-apple.systempreferences:com.apple.Sound-Settings.extension",
            "🔊",
        ),
        (
            "Settings: Display",
            "x-apple.systempreferences:com.apple.Displays-Settings.extension",
            "🖥️",
        ),
        (
            "Settings: Wallpaper",
            "x-apple.systempreferences:com.apple.Wallpaper-Settings.extension",
            "🖼️",
        ),
        (
            "Settings: Notifications",
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
            "🔔",
        ),
        (
            "Settings: Keyboard",
            "x-apple.systempreferences:com.apple.Keyboard-Settings.extension",
            "⌨️",
        ),
        (
            "Settings: Trackpad",
            "x-apple.systempreferences:com.apple.Trackpad-Settings.extension",
            "🖱️",
        ),
        (
            "Settings: Mouse",
            "x-apple.systempreferences:com.apple.Mouse-Settings.extension",
            "🖱️",
        ),
        (
            "Settings: Printers & Scanners",
            "x-apple.systempreferences:com.apple.Print-Scan-Settings.extension",
            "🖨️",
        ),
        (
            "Settings: Battery",
            "x-apple.systempreferences:com.apple.Battery-Settings.extension",
            "🔋",
        ),
        (
            "Settings: Privacy & Security",
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension",
            "🛡️",
        ),
        (
            "Settings: General",
            "x-apple.systempreferences:com.apple.General-Settings.extension",
            "⚙️",
        ),
        (
            "Settings: Accessibility",
            "x-apple.systempreferences:com.apple.Accessibility-Settings.extension",
            "♿",
        ),
        (
            "Settings: Users & Groups",
            "x-apple.systempreferences:com.apple.Users-Groups-Settings.extension",
            "👥",
        ),
        (
            "Settings: Software Update",
            "x-apple.systempreferences:com.apple.Software-Update-Settings.extension",
            "🔄",
        ),
    ];

    for (name, uri, emoji) in settings {
        apps.push(AppInfo {
            name: name.to_string(),
            path: PathBuf::from(uri),
            icon_path: None,
            emoji_icon: Some(emoji.to_string()),
            icon_data: None,
        });
    }

    Ok(apps)
}

pub fn launch_application_impl(path: &PathBuf) -> Result<()> {
    let path_str = path.to_str().unwrap_or("");
    if path_str.contains(':') && !path_str.starts_with('/') {
        // URI-based launch (x-apple.systempreferences:, etc.)
        info!("Launching URI: {}", path_str);
        Command::new("open")
            .arg(path_str)
            .spawn()
            .context("Failed to launch URI")?;
    } else {
        info!("Launching macOS application at {:?}", path);
        Command::new("open")
            .arg(path)
            .spawn()
            .context("Failed to launch application")?;
    }
    Ok(())
}

/// Launch by name. Dispatches by input shape:
///   - URI (`scheme://...` or `x-apple.systempreferences:...`) → `open <uri>`
///   - Absolute path (starts with `/`)                         → `open <path>`
///   - Bare name ("Calculator", "Safari", "chrome")            → `open -a <name>`
///
/// `open -a` asks LaunchServices to resolve the name, so display names and
/// bundle basenames both work.
pub fn shell_launch_impl(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("shell_launch called with empty name");
    }

    // Treat anything starting with an RFC 3986 scheme (`letter followed by
    // letters/digits/+-. then `:`) as a URI — covers `http://`, `mailto:`,
    // `x-apple.systempreferences:`, `spotify:track:...`, etc. Display names
    // never contain a colon, so this heuristic is safe.
    let is_uri = looks_like_uri(name);
    let is_path = name.starts_with('/');

    let status = if is_uri || is_path {
        info!("shell_launch_impl: open '{}'", name);
        Command::new("open")
            .arg(name)
            .status()
            .context("`open` failed")?
    } else {
        info!("shell_launch_impl: open -a '{}'", name);
        Command::new("open")
            .args(["-a", name])
            .status()
            .context("`open -a` failed")?
    };

    if !status.success() {
        anyhow::bail!("Failed to launch '{}': `open` exited with {}", name, status);
    }
    Ok(())
}

/// True if `s` starts with an RFC 3986 URI scheme — `ALPHA *( ALPHA / DIGIT /
/// "+" / "-" / "." ) ":"`. Used to distinguish URIs like `mailto:`,
/// `x-apple.systempreferences:`, and `https://` from bare display names.
fn looks_like_uri(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    // .skip(1) guarantees at least one byte before any colon, satisfying the
    // RFC-3986 requirement that a scheme has ≥1 character.
    for &b in bytes.iter().skip(1) {
        if b == b':' {
            return true;
        }
        let ok = b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.';
        if !ok {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::looks_like_uri;

    #[test]
    fn recognizes_common_schemes() {
        assert!(looks_like_uri("https://example.com"));
        assert!(looks_like_uri("http://example.com"));
        assert!(looks_like_uri("mailto:foo@bar.com"));
        assert!(looks_like_uri("x-apple.systempreferences:com.apple.foo"));
        assert!(looks_like_uri("spotify:track:abc123"));
        assert!(looks_like_uri("file:///tmp/x"));
    }

    #[test]
    fn rejects_display_names_and_paths() {
        assert!(!looks_like_uri("Safari"));
        assert!(!looks_like_uri("Google Chrome"));
        assert!(!looks_like_uri("/Applications/Safari.app"));
        assert!(!looks_like_uri("123abc:foo")); // must start with ALPHA
        assert!(!looks_like_uri(":nope")); // empty scheme
        assert!(!looks_like_uri("")); // empty
        assert!(!looks_like_uri("no colon here"));
    }
}
