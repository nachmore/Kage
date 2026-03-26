// OS-specific functionality abstraction
// This module provides a unified interface for platform-specific operations

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

// Re-export platform-specific implementations with a common interface
// Note: These are available for advanced use cases, but most code should use
// the cross-platform functions exported below
#[cfg(target_os = "windows")]
#[allow(unused)]
pub use windows as platform;

#[cfg(target_os = "macos")]
#[allow(unused)]
pub use macos as platform;

#[cfg(target_os = "linux")]
#[allow(unused)]
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
pub use process::{kill_process, configure_process_spawn};
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
    #[cfg(target_os = "windows")]
    { crate::os::windows::clipboard::simulate_paste_impl(); }

    #[cfg(not(target_os = "windows"))]
    { /* TODO: implement for macOS/Linux */ }
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
