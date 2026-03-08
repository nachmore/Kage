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
pub mod startup;
pub mod hotkey;
pub mod icon;

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
pub use hotkey::{capture_hotkey, cancel_hotkey_capture};
#[allow(unused)]
pub use hotkey::CapturedHotkey;
pub use icon::extract_icon_base64;
