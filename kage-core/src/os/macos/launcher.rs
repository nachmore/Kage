// macOS application launcher

use anyhow::{Context, Result};
use log::info;
use std::collections::{HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::os::launcher::{AppInfo, WatchRoot};

/// Directories scanned for `.app` bundles, in priority order: a
/// user-installed copy in /Applications wins over the built-in one in
/// /System/Applications (Catalina+ home of Calculator, Notes, …).
fn application_dirs() -> Vec<PathBuf> {
    let mut out = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("Applications"));
    }
    out
}

/// How many folder levels below an application dir the scan looks for
/// apps. Plenty of vendors install into a folder of their own —
/// `/Applications/Cisco/Cisco Secure Client.app` (depth 2), Adobe,
/// Setapp, Chrome/Edge web apps in `~/Applications/Chrome Apps.localized/`
/// — and Unity Hub goes as deep as
/// `/Applications/Unity/Hub/Editor/<version>/Unity.app` (depth 5).
const MAX_DEPTH: usize = 5;

/// Upper bound on plain folders visited under one application dir, so a
/// vendor that drops a huge non-bundle tree into /Applications can't make
/// the scan crawl.
const MAX_DIRS_PER_ROOT: usize = 2_000;

/// Directory extensions that mark a bundle or package. The scan never
/// looks inside one: apps embedded in other apps (helpers, login items)
/// aren't things a user launches, and bundle contents are the bulk of
/// /Applications. Compared case-insensitively.
const BUNDLE_EXTENSIONS: &[&str] = &[
    "app",
    "appex",
    "bundle",
    "component",
    "driver",
    "framework",
    "kext",
    "mdimporter",
    "mpkg",
    "pkg",
    "plugin",
    "prefpane",
    "qlgenerator",
    "saver",
    "systemextension",
    "vst",
    "vst3",
    "xpc",
];

fn has_extension(name: &OsStr, ext: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn is_bundle_name(name: &OsStr) -> bool {
    BUNDLE_EXTENSIONS.iter().any(|ext| has_extension(name, ext))
}

/// `.app` bundles under `root`, breadth-first (so a shallower copy wins
/// name collisions) and in name order within a folder. Descends only into
/// plain folders — never bundles, hidden folders, symlinks or anything in
/// `skip` (the other application dirs, which are scanned at their own
/// priority) — at most `max_depth` levels down.
fn find_app_bundles(root: &Path, skip: &[PathBuf], max_depth: usize) -> Vec<PathBuf> {
    let mut apps = Vec::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), 1usize)]);
    let mut visited = 0usize;

    while let Some((dir, depth)) = queue.pop_front() {
        // Missing dirs (no ~/Applications, pre-Catalina layout) and
        // unreadable ones are normal.
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if has_extension(&name, "app") {
                // Symlinked apps count (some installers link into
                // /Applications); what they point at is irrelevant here.
                apps.push(path);
                continue;
            }
            if depth >= max_depth || visited >= MAX_DIRS_PER_ROOT || is_bundle_name(&name) {
                continue;
            }
            // `file_type` doesn't follow symlinks, so a link back up the
            // tree can't loop the walk.
            if entry.file_type().is_ok_and(|t| t.is_dir()) && !skip.contains(&path) {
                visited += 1;
                queue.push_back((path, depth + 1));
            }
        }
    }
    apps
}

/// Each application dir, watched recursively so a vendor-folder install
/// is seen too. A dir nested in another one (`/Applications/Utilities`)
/// is already covered by its parent's watch.
pub fn watch_roots_impl() -> Vec<WatchRoot> {
    let dirs = application_dirs();
    dirs.iter()
        .filter(|d| !dirs.iter().any(|other| other != *d && d.starts_with(other)))
        .map(|path| WatchRoot {
            path: path.clone(),
            recursive: true,
        })
        .collect()
}

/// A change inside a bundle (an app updating its own `Contents/`) can't
/// add or remove an app; a change at or above bundle level can.
pub fn affects_app_list_impl(path: &Path) -> bool {
    !path
        .parent()
        .is_some_and(|parent| parent.iter().any(is_bundle_name))
}

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let dirs = application_dirs();

    for dir in &dirs {
        let skip: Vec<PathBuf> = dirs.iter().filter(|d| *d != dir).cloned().collect();
        for path in find_app_bundles(dir, &skip, MAX_DEPTH) {
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !seen.insert(name.to_string()) {
                continue;
            }
            let name = name.to_string();
            let icon_path = path.to_string_lossy().to_string();
            apps.push(AppInfo {
                name,
                path,
                icon_path: Some(icon_path),
                emoji_icon: None,
                icon_data: None,
            });
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
        // "System Settings" also exists as /System/Applications/System
        // Settings.app — keep the real bundle (it has an icon).
        if !seen.insert(name.to_string()) {
            continue;
        }
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
    use super::{affects_app_list_impl, find_app_bundles, watch_roots_impl};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn names(apps: &[PathBuf], root: &Path) -> Vec<String> {
        apps.iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn finds_apps_in_vendor_folders_but_not_inside_bundles() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for dir in [
            "Safari.app/Contents/MacOS",
            "Xcode.app/Contents/Applications/Instruments.app",
            "Cisco/Cisco Secure Client.app/Contents",
            "Cisco/Uninstall Cisco Secure Client.app",
            "Chrome Apps.localized/Gmail.app",
            "Python 3.12/IDLE.app",
            "Unity/Hub/Editor/2022.3.1f1/Unity.app",
            "Too/Deep/For/The/Scan/Deep.app",
            "Some.framework/Versions/A/Helper.app",
            ".hidden/Secret.app",
            "Utilities/Terminal.app",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::os::unix::fs::symlink(root, root.join("Loop")).unwrap();

        let skip = vec![root.join("Utilities")];
        let found = names(&find_app_bundles(root, &skip, 5), root);
        assert_eq!(
            found,
            [
                "Safari.app",
                "Xcode.app",
                "Chrome Apps.localized/Gmail.app",
                "Cisco/Cisco Secure Client.app",
                "Cisco/Uninstall Cisco Secure Client.app",
                "Python 3.12/IDLE.app",
                "Unity/Hub/Editor/2022.3.1f1/Unity.app",
            ]
        );
    }

    #[test]
    fn missing_root_yields_nothing() {
        assert!(find_app_bundles(Path::new("/definitely/not/here"), &[], 5).is_empty());
    }

    #[test]
    fn bundle_internal_changes_do_not_affect_the_app_list() {
        assert!(affects_app_list_impl(Path::new("/Applications/Foo.app")));
        assert!(affects_app_list_impl(Path::new(
            "/Applications/Cisco/Foo.app"
        )));
        assert!(affects_app_list_impl(Path::new("/Applications/Cisco")));
        assert!(!affects_app_list_impl(Path::new(
            "/Applications/Google Chrome.app/Contents/Frameworks/x.dylib"
        )));
        assert!(!affects_app_list_impl(Path::new(
            "/Applications/Xcode.app/Contents/Applications/Instruments.app"
        )));
        assert!(!affects_app_list_impl(Path::new(
            "/Applications/Foo.APP/Contents/Info.plist"
        )));
    }

    #[test]
    fn nested_application_dirs_are_not_watched_twice() {
        let roots = watch_roots_impl();
        assert!(roots.iter().all(|r| r.recursive));
        assert!(roots.iter().any(|r| r.path == Path::new("/Applications")));
        assert!(!roots
            .iter()
            .any(|r| r.path == Path::new("/Applications/Utilities")));
    }

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
