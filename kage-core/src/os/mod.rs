// OS-specific functionality abstraction (kage-core subset).
//
// Same Pattern-A dispatch as the app crate: each cross-platform module
// forwards to `crate::os::platform::<mod>::...`, where `platform` is a
// compile-time alias for the current OS's submodule. Only the modules
// the MCP sidecar needs live here — the app-only OS surface (clipboard,
// hotkeys, tray icons, …) stays in the `kage` crate.

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub use windows as platform;

#[cfg(target_os = "macos")]
pub use macos as platform;

#[cfg(target_os = "linux")]
pub use linux as platform;

pub mod accessibility;
pub mod input;
pub mod launcher;
pub mod process;

/// Configure a Command to hide the console window on Windows (no-op on other platforms).
pub fn configure_no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000) // CREATE_NO_WINDOW
    }

    #[cfg(not(target_os = "windows"))]
    {
        cmd
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
