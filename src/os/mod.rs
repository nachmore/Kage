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

// accessibility + launcher moved to kage-core (shared with the MCP
// sidecar); re-exported so `crate::os::accessibility::...` paths and the
// platform-dispatch shape stay unchanged for app code.
pub use kage_core::os::accessibility;
pub use kage_core::os::launcher;

// Common types and traits
pub mod calendar;
pub mod clipboard;
pub mod clipboard_history;
pub mod cursor;
pub mod diagnostics;
pub mod file_search;
pub mod hotkey;
pub mod icon;
pub mod power;
pub mod process;
pub mod shell;
pub mod startup;
pub mod user;
pub mod window_list;

// Re-export common functionality
#[allow(unused)]
pub use clipboard::write_clipboard;
#[allow(unused_imports)]
pub use clipboard::{
    begin_selection_capture, capture_selection, finish_selection_capture, read_clipboard,
    SelectionCaptureToken,
};
pub use cursor::get_cursor_position;
pub use launcher::{launch_application, scan_applications};
pub use process::{
    cleanup_stale_processes, configure_process_spawn, install_kill_on_exit_job, kill_process,
    release_kill_on_exit_job,
};
pub use shell::{open_in_editor, open_path, open_url, reveal_in_file_manager};
pub use startup::{get_startup_enabled, migrate_startup_mechanism, set_startup_enabled};
pub use user::get_user_profile;

/// Simulate Ctrl+V paste keystroke to the foreground window.
#[allow(unused)]
pub fn simulate_paste() {
    crate::os::platform::clipboard::simulate_paste_impl();
}

pub use calendar::get_events_for_date;
pub use calendar::get_upcoming_events;
pub use clipboard_history::get_clipboard_history;
pub use file_search::search_files;
#[allow(unused)]
pub use hotkey::CapturedHotkey;
pub use hotkey::{cancel_hotkey_capture, capture_hotkey};
pub use icon::extract_icon_base64;
pub use window_list::{focus_window, get_app_icon, list_windows};

/// Detect whether the OS is using dark mode.
/// Returns true for dark, false for light.
pub fn is_dark_mode() -> bool {
    #[cfg(target_os = "windows")]
    {
        // Read AppsUseLightTheme from the registry (0 = dark, 1 = light)
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        {
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
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .eq_ignore_ascii_case("dark")
            })
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

// fonts_dir + configure_no_window live in kage-core (folder_tools and
// script_runner need them); re-exported to keep `crate::os::` callers as-is.
pub use kage_core::os::{configure_no_window, fonts_dir};

/// Spawn this command outside the parent's Job Object on Windows (no-op
/// elsewhere). Used for the restart helper and `execute_shortcut` so the
/// child survives the parent process exit / job-close. See
/// `os::install_kill_on_exit_job` for the matching teardown semantics.
pub fn configure_breakaway_from_job(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x01000000) // CREATE_BREAKAWAY_FROM_JOB
    }

    #[cfg(not(target_os = "windows"))]
    {
        cmd
    }
}

/// Set the current thread's name/description so it shows up in debuggers
/// and in the thread dump diagnostic command. No-op on non-Windows.
#[allow(unused)]
pub fn set_current_thread_name(_name: &str) {
    #[cfg(target_os = "windows")]
    crate::os::windows::process::set_thread_name(_name);
}
