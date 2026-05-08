// Windows application launcher

use anyhow::Result;
use log::{info, warn};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use winreg::enums::*;
use winreg::RegKey;

use crate::os::launcher::AppInfo;

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = HashMap::new();

    // Scan Start Menu shortcuts (.lnk files)
    if let Some(start_menu) = dirs::data_dir() {
        let start_menu_path = start_menu.join("Microsoft\\Windows\\Start Menu\\Programs");
        if start_menu_path.exists() {
            scan_directory_for_shortcuts(&start_menu_path, &mut apps)?;
        }
    }
    let common_start_menu =
        PathBuf::from("C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs");
    if common_start_menu.exists() {
        scan_directory_for_shortcuts(&common_start_menu, &mut apps)?;
    }

    // Scan registry for installed desktop applications
    scan_registry_apps(&mut apps)?;

    // Scan UWP/Store packages
    scan_uwp_packages(&mut apps);

    // Add Windows Settings pages (URI-based, not packages)
    add_settings_pages(&mut apps);

    Ok(apps.into_values().collect())
}

fn scan_directory_for_shortcuts(dir: &PathBuf, apps: &mut HashMap<String, AppInfo>) -> Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                scan_directory_for_shortcuts(&path, apps)?;
            } else if path.extension().and_then(|s| s.to_str()) == Some("lnk") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    let icon_path = path.to_string_lossy().to_string();
                    let key = name.to_lowercase();
                    apps.entry(key).or_insert_with(|| AppInfo {
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
    Ok(())
}

fn scan_registry_apps(apps: &mut HashMap<String, AppInfo>) -> Result<()> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(uninstall_key) =
        hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall")
    {
        for subkey_name in uninstall_key.enum_keys().filter_map(|k| k.ok()) {
            if let Ok(subkey) = uninstall_key.open_subkey(&subkey_name) {
                if let Ok(display_name) = subkey.get_value::<String, _>("DisplayName") {
                    if let Ok(install_location) = subkey.get_value::<String, _>("InstallLocation") {
                        let install_path = PathBuf::from(&install_location);
                        if install_path.exists() {
                            if let Ok(entries) = fs::read_dir(&install_path) {
                                for entry in entries.filter_map(|e| e.ok()) {
                                    let path = entry.path();
                                    if path.extension().and_then(|s| s.to_str()) == Some("exe") {
                                        let icon_path = path.to_string_lossy().to_string();
                                        let key = display_name.to_lowercase();
                                        apps.entry(key).or_insert_with(|| AppInfo {
                                            name: display_name.clone(),
                                            path: path.clone(),
                                            icon_path: Some(icon_path),
                                            emoji_icon: None,
                                            icon_data: None,
                                        });
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Scan installed UWP/Store packages for launchable apps
fn scan_uwp_packages(apps: &mut HashMap<String, AppInfo>) {
    use windows::core::HSTRING;
    use windows::Management::Deployment::PackageManager;
    use windows_collections::IVectorView;

    let pm = match PackageManager::new() {
        Ok(pm) => pm,
        Err(e) => {
            warn!("Failed to create PackageManager: {}", e);
            return;
        }
    };

    // FindPackagesByUserSecurityId with empty string = current user (no admin needed)
    let packages = match pm.FindPackagesByUserSecurityId(&HSTRING::new()) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to enumerate UWP packages: {}", e);
            return;
        }
    };

    for package in packages {
        // Skip framework/resource packages — they're not launchable apps
        if package.IsFramework().unwrap_or(true) {
            continue;
        }
        if package.IsResourcePackage().unwrap_or(false) {
            continue;
        }

        // Get app list entries (launchable apps within the package)
        let async_op = match package.GetAppListEntriesAsync() {
            Ok(op) => op,
            Err(_) => continue,
        };
        let entries: IVectorView<windows::ApplicationModel::Core::AppListEntry> =
            match async_op.join() {
                Ok(e) => e,
                Err(_) => continue,
            };

        for entry in &entries {
            let di = match entry.DisplayInfo() {
                Ok(d) => d,
                Err(_) => continue,
            };

            let name: String = match di.DisplayName() {
                Ok(n) => n.to_string(),
                Err(_) => continue,
            };

            if name.is_empty() || name.starts_with("ms-resource:") {
                continue;
            }

            let key = name.to_lowercase();
            if apps.contains_key(&key) {
                continue;
            }

            let aumid: String = match entry.AppUserModelId() {
                Ok(id) => id.to_string(),
                Err(_) => continue,
            };

            let icon_data = get_uwp_icon_base64(&package);

            apps.insert(
                key,
                AppInfo {
                    name,
                    path: PathBuf::from(format!("shell:AppsFolder\\{}", aumid)),
                    icon_path: None,
                    emoji_icon: None,
                    icon_data,
                },
            );
        }
    }
}

/// Try to extract a UWP app's icon as a base64 data URI
fn get_uwp_icon_base64(package: &windows::ApplicationModel::Package) -> Option<String> {
    // Get the logo URI from the package
    let logo_uri = package.Logo().ok()?;
    let logo_path_raw = logo_uri.Path().ok()?.to_string();

    // The URI path is URL-encoded and already absolute (e.g., /C:/Program%20Files/...)
    // Decode %20 etc. and strip leading slash on Windows
    let decoded = logo_path_raw.replace("%20", " ").replace("%23", "#");
    let logo_path = decoded.strip_prefix('/').unwrap_or(&decoded);

    // If the path already includes a scale suffix and exists, use it directly
    let base_path = PathBuf::from(logo_path);
    let icon_path = if base_path.exists() {
        base_path
    } else {
        // Try finding a scale variant
        match find_best_scale_icon(&base_path) {
            Some(p) => p,
            None => {
                return None;
            }
        }
    };

    // Read and encode as base64
    use base64::Engine;
    let data = fs::read(&icon_path).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);

    // Detect MIME type from extension
    let ext = icon_path.extension()?.to_str()?.to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        _ => "image/png",
    };

    Some(format!("data:{};base64,{}", mime, b64))
}

/// Find the best scale variant of a UWP icon (prefer scale-200, then 150, 100, etc.)
fn find_best_scale_icon(base_path: &std::path::Path) -> Option<PathBuf> {
    // If the exact file exists, use it
    if base_path.exists() {
        return Some(base_path.to_path_buf());
    }

    let stem = base_path.file_stem()?.to_str()?;
    let ext = base_path.extension()?.to_str()?;
    let parent = base_path.parent()?;

    // Try common scale suffixes in preference order
    let scales = [
        "scale-200",
        "scale-150",
        "scale-100",
        "scale-400",
        "scale-125",
    ];
    for scale in &scales {
        let candidate = parent.join(format!("{}.{}.{}", stem, scale, ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Try targetsize variants
    let sizes = [
        "targetsize-48",
        "targetsize-64",
        "targetsize-32",
        "targetsize-256",
    ];
    for size in &sizes {
        let candidate = parent.join(format!("{}.{}.{}", stem, size, ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Try without any suffix but with different extensions
    for alt_ext in &["png", "jpg", "svg"] {
        let candidate = parent.join(format!("{}.{}", stem, alt_ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

/// Add Windows Settings pages (URI-based, not discoverable as packages)
fn add_settings_pages(apps: &mut HashMap<String, AppInfo>) {
    let pages: Vec<(&str, &str, &str)> = vec![
        // Settings — System
        ("Settings: Display", "ms-settings:display", "🖥️"),
        ("Settings: Sound", "ms-settings:sound", "🔊"),
        ("Settings: Notifications", "ms-settings:notifications", "🔔"),
        ("Settings: Power & Battery", "ms-settings:powersleep", "🔋"),
        ("Settings: Storage", "ms-settings:storagesense", "💾"),
        ("Settings: Multitasking", "ms-settings:multitasking", "🪟"),
        ("Settings: About", "ms-settings:about", "ℹ️"),
        (
            "Settings: Remote Desktop",
            "ms-settings:remotedesktop",
            "🖥️",
        ),
        ("Settings: Clipboard", "ms-settings:clipboard", "📋"),
        // Settings — Network
        ("Settings: Network", "ms-settings:network", "🌐"),
        ("Settings: Wi-Fi", "ms-settings:network-wifi", "📶"),
        ("Settings: VPN", "ms-settings:network-vpn", "🔒"),
        ("Settings: Proxy", "ms-settings:network-proxy", "🔀"),
        ("Settings: Ethernet", "ms-settings:network-ethernet", "🔌"),
        (
            "Settings: Mobile Hotspot",
            "ms-settings:network-mobilehotspot",
            "📱",
        ),
        // Settings — Personalization
        (
            "Settings: Personalization",
            "ms-settings:personalization",
            "🎨",
        ),
        (
            "Settings: Background",
            "ms-settings:personalization-background",
            "🖼️",
        ),
        ("Settings: Colors", "ms-settings:colors", "🎨"),
        ("Settings: Themes", "ms-settings:themes", "🎭"),
        ("Settings: Lock Screen", "ms-settings:lockscreen", "🔐"),
        ("Settings: Taskbar", "ms-settings:taskbar", "📌"),
        (
            "Settings: Start Menu",
            "ms-settings:personalization-start",
            "▶️",
        ),
        ("Settings: Fonts", "ms-settings:fonts", "🔤"),
        // Settings — Apps
        (
            "Settings: Apps & Features",
            "ms-settings:appsfeatures",
            "📦",
        ),
        ("Settings: Default Apps", "ms-settings:defaultapps", "📱"),
        ("Settings: Startup Apps", "ms-settings:startupapps", "🚀"),
        // Settings — Accounts
        ("Settings: Accounts", "ms-settings:accounts", "👤"),
        (
            "Settings: Sign-in Options",
            "ms-settings:signinoptions",
            "🔑",
        ),
        (
            "Settings: Email & Accounts",
            "ms-settings:emailandaccounts",
            "📧",
        ),
        // Settings — Time & Language
        ("Settings: Date & Time", "ms-settings:dateandtime", "📅"),
        (
            "Settings: Language & Region",
            "ms-settings:regionlanguage",
            "🌍",
        ),
        ("Settings: Typing", "ms-settings:typing", "⌨️"),
        // Settings — Privacy & Security
        ("Settings: Privacy", "ms-settings:privacy", "🛡️"),
        (
            "Settings: Windows Security",
            "ms-settings:windowsdefender",
            "🛡️",
        ),
        ("Settings: For Developers", "ms-settings:developers", "👨‍💻"),
        // Settings — Windows Update
        (
            "Settings: Windows Update",
            "ms-settings:windowsupdate",
            "🔄",
        ),
        (
            "Settings: Update History",
            "ms-settings:windowsupdate-history",
            "📜",
        ),
        // Settings — Bluetooth & Devices
        ("Settings: Bluetooth", "ms-settings:bluetooth", "🔵"),
        (
            "Settings: Printers & Scanners",
            "ms-settings:printers",
            "🖨️",
        ),
        ("Settings: Mouse", "ms-settings:mousetouchpad", "🖱️"),
        ("Settings: Camera", "ms-settings:camera", "📷"),
        ("Settings: USB", "ms-settings:usb", "🔌"),
        // Settings — Accessibility
        ("Settings: Accessibility", "ms-settings:easeofaccess", "♿"),
        (
            "Settings: Text Size",
            "ms-settings:easeofaccess-display",
            "🔍",
        ),
        (
            "Settings: Magnifier",
            "ms-settings:easeofaccess-magnifier",
            "🔎",
        ),
        (
            "Settings: Narrator",
            "ms-settings:easeofaccess-narrator",
            "🗣️",
        ),
        (
            "Settings: Keyboard",
            "ms-settings:easeofaccess-keyboard",
            "⌨️",
        ),
        // Settings — Gaming
        ("Settings: Game Mode", "ms-settings:gaming-gamemode", "🎮"),
        (
            "Settings: Xbox Game Bar",
            "ms-settings:gaming-gamebar",
            "🎮",
        ),
    ];

    for (name, uri, emoji) in pages {
        let key = name.to_lowercase();
        apps.entry(key).or_insert_with(|| AppInfo {
            name: name.to_string(),
            path: PathBuf::from(uri),
            icon_path: None,
            emoji_icon: Some(emoji.to_string()),
            icon_data: None,
        });
    }

    // Bluetooth SVG icon override
    if let Some(bt) = apps.get_mut("settings: bluetooth") {
        bt.icon_data = Some("data:image/svg+xml;base64,PHN2ZyB2aWV3Qm94PSIwIDAgMjQgMjQiIGZpbGw9Im5vbmUiIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyI+PHBhdGggZD0iTTcgMTdMMTcgN0wxMiAyVjIyTDE3IDE3TDcgNyIgc3Ryb2tlPSJ3aGl0ZSIgc3Ryb2tlLXdpZHRoPSIyIiBzdHJva2UtbGluZWNhcD0icm91bmQiIHN0cm9rZS1saW5lam9pbj0icm91bmQiLz48L3N2Zz4=".to_string());
        bt.emoji_icon = None;
    }
}

pub fn launch_application_impl(path: &PathBuf) -> Result<()> {
    use super::process::spawn_detached_impl;
    use std::process::Command;

    let path_str = path.to_str().unwrap_or("");
    info!(
        "launch_application_impl: path={:?} path_str='{}'",
        path, path_str
    );

    if path_str.is_empty() {
        anyhow::bail!("Empty path passed to launch_application_impl");
    }

    info!("Launching (detached): '{}'", path_str);
    spawn_detached_impl(Command::new("cmd").args(["/C", "start", "", path_str]))
        .map_err(|e| anyhow::anyhow!("Failed to launch '{}': {}", path_str, e))?;

    info!("Launch succeeded for '{}'", path_str);
    Ok(())
}

/// Launch by name via `ShellExecuteW` — delegates name resolution to the
/// Windows shell (PATH, App Paths registry, associations, AUMID for UWP
/// apps). Handles program names, paths, URIs, and "program args" strings.
pub fn shell_launch_impl(name: &str) -> Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // Detect if this is a file path (contains backslash, or drive letter
    // like C:/). Paths should not be split on the first space.
    let is_path =
        name.contains('\\') || (name.len() >= 3 && name.as_bytes().get(1) == Some(&b':'));
    let (file_str, params_str) = if name.contains(' ') && !is_path {
        let mut parts = name.splitn(2, ' ');
        let prog = parts.next().unwrap_or(name);
        let args = parts.next().unwrap_or("");
        (prog, args)
    } else {
        (name, "")
    };

    let op = HSTRING::from("open");
    let file = HSTRING::from(file_str);

    info!(
        "shell_launch_impl: file='{}' params='{}'",
        file_str, params_str
    );

    let result = unsafe {
        if params_str.is_empty() {
            ShellExecuteW(
                None,
                &op,
                &file,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        } else {
            let params = HSTRING::from(params_str);
            ShellExecuteW(None, &op, &file, &params, PCWSTR::null(), SW_SHOWNORMAL)
        }
    };

    // ShellExecuteW returns a HINSTANCE cast from an int — values > 32 mean
    // success, 0..=32 are error codes.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        anyhow::bail!(
            "ShellExecuteW failed with code {} for '{}'",
            result.0 as usize,
            name
        );
    }
}
