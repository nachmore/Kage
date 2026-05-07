// OS-specific functionality abstraction
// This module provides a unified interface for platform-specific operations

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

// Pattern A dispatch: each cross-platform module forwards to
// `crate::os::platform::<mod>::<fn>_impl(...)`. The platform alias
// resolves at compile time so there's no runtime cost; the rest of
// the codebase never imports the platform-specific submodule directly
// (with one exception: main.rs's `/capture-hotkey` helper-process CLI
// dispatch, which is a Windows-only entry point not a runtime API).
#[cfg(target_os = "windows")]
pub use windows as platform;

#[cfg(target_os = "macos")]
pub use macos as platform;

#[cfg(target_os = "linux")]
pub use linux as platform;

// Common types and traits
pub mod cursor;
pub mod launcher;
pub mod process;
pub mod shell;
pub mod user;
pub mod clipboard;
pub mod clipboard_history;
pub mod file_search;
pub mod calendar;
pub mod startup;
pub mod hotkey;
pub mod icon;
pub mod window_list;
pub mod power;
#[allow(dead_code)] // Consumed by the kage-computer-control-mcp binary
pub mod accessibility;

// Re-export common functionality
pub use cursor::get_cursor_position;
pub use launcher::{scan_applications, launch_application};
pub use process::{kill_process, configure_process_spawn, install_kill_on_exit_job};
pub use shell::{open_url, open_path, reveal_in_file_manager, open_in_editor};
pub use user::get_user_profile;
#[allow(unused_imports)]
pub use clipboard::{read_clipboard, capture_selection, begin_selection_capture, finish_selection_capture, SelectionCaptureToken};
#[allow(unused)]
pub use clipboard::write_clipboard;
pub use startup::{get_startup_enabled, set_startup_enabled};

/// Simulate Ctrl+V paste keystroke to the foreground window.
#[allow(unused)]
pub fn simulate_paste() {
    crate::os::platform::clipboard::simulate_paste_impl();
}

pub use clipboard_history::get_clipboard_history;
pub use file_search::search_files;
pub use calendar::get_upcoming_events;
pub use calendar::get_events_for_date;
pub use hotkey::{capture_hotkey, cancel_hotkey_capture};
#[allow(unused)]
pub use hotkey::CapturedHotkey;
pub use icon::extract_icon_base64;
pub use window_list::{list_windows, focus_window, get_app_icon};

/// Detect whether the OS is using dark mode.
/// Returns true for dark, false for light.
pub fn is_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        // Read AppsUseLightTheme from the registry (0 = dark, 1 = light)
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize") {
            Ok(key) => {
                let val: u32 = key.get_value("AppsUseLightTheme").unwrap_or(1);
                val == 0
            }
            Err(_) => false,
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: `defaults read -g AppleInterfaceStyle` returns "Dark" in dark mode
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().eq_ignore_ascii_case("dark"))
            .unwrap_or(false)
    }

    #[cfg(target_os = "linux")]
    {
        // GNOME/GTK: check gsettings color-scheme
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("dark"))
            .unwrap_or(false)
    }
}

/// Get the system fonts directory.
/// Windows: %WINDIR%\Fonts, macOS/Linux: dirs::font_dir()
pub fn fonts_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("WINDIR")
            .ok()
            .map(|w| std::path::PathBuf::from(w).join("Fonts"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        dirs::font_dir()
    }
}

/// Configure a Command to hide the console window on Windows (no-op on other platforms).
pub fn configure_no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000) // CREATE_NO_WINDOW
    }

    #[cfg(not(target_os = "windows"))]
    { cmd }
}

/// Launch an installer/update package appropriate for the current platform.
/// Windows: runs NSIS installer with /S (silent). macOS: opens .dmg. Linux: chmod +x and run.
pub fn run_installer(path: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(path)
            .arg("/S")
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to run installer: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to open installer: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        std::process::Command::new(path)
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to run installer: {}", e))?;
    }

    Ok(())
}

/// Set the current thread's name/description so it shows up in debuggers
/// and in the thread dump diagnostic command. No-op on non-Windows.
#[allow(unused)]
pub fn set_current_thread_name(_name: &str) {
    #[cfg(target_os = "windows")]
    crate::os::windows::process::set_thread_name(_name);
}
