//! Whether a computer-control tool that injects input (a click, a key, a
//! typed string, an accessibility action) would act on Kage's own windows.
//!
//! Kage's windows hold the buttons that approve what the agent may do
//! ("Run once", "Always allow …" on a command outside the agent's sandbox,
//! the tool-permission prompt). Those must only ever be pressed by the
//! user, so the sidecar's input tools refuse to act on a Kage window.
//! Read-only tools (UI trees, element text, window lists) still work.
//!
//! Kage is recognised by its process: the executable is named `kage`
//! (`kage.exe`), or on macOS the bundle identifier is [`KAGE_BUNDLE_ID`].
//! Best-effort by nature — see docs/SECURITY_MODEL.md.

/// Kage's macOS bundle identifier (`identifier` in tauri.conf.json).
pub const KAGE_BUNDLE_ID: &str = "com.kage.launcher";

/// What the tool tells the agent when it refuses.
pub const REFUSAL: &str = "Refused: that would act on one of Kage's own windows. \
Computer-control tools can't click, type or press keys in Kage — its approval \
buttons may only be pressed by the user. Bring the app you want to control to \
the front first (for example with launch_app, or click_element on one of its \
elements), then try again.";

/// Whether `path_or_name` (a full executable path or a bare name) is the
/// Kage app's executable: `kage` / `kage.exe`, any case. The sidecar
/// (`kage-computer-control-mcp`) and other helpers are not.
pub fn is_kage_executable(path_or_name: &str) -> bool {
    let base = path_or_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path_or_name)
        .trim();
    let stem = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    stem.eq_ignore_ascii_case("kage")
}

/// Whether process `pid` is the Kage app.
pub fn is_kage_process(pid: u32) -> bool {
    pid != 0 && crate::os::platform::kage_target::is_kage_process_impl(pid)
}

/// Whether keyboard input sent now would reach a Kage window (Kage is the
/// active app, or holds keyboard focus — a key panel included).
pub fn keyboard_goes_to_kage() -> bool {
    crate::os::platform::kage_target::keyboard_target_pids_impl()
        .into_iter()
        .any(is_kage_process)
}

/// Whether a click at screen point (`x`, `y`) would land on a Kage window.
pub fn point_is_on_kage(x: i32, y: i32) -> bool {
    crate::os::platform::kage_target::window_owners_at_impl(x, y)
        .into_iter()
        .any(is_kage_process)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_kage_executable_only() {
        for yes in [
            "kage",
            "Kage",
            "kage.exe",
            "KAGE.EXE",
            "/Applications/Kage.app/Contents/MacOS/kage",
            r"C:\Program Files\Kage\kage.exe",
            "/home/me/src/kage/target/debug/kage",
        ] {
            assert!(is_kage_executable(yes), "{yes}");
        }
        for no in [
            "kage-computer-control-mcp",
            r"C:\Kage\kage-computer-control-mcp.exe",
            "kiro-cli",
            "Kage Helper",
            "kagent",
            "/Applications/Kage.app/Contents/MacOS/kage-calendar-helper",
            "",
        ] {
            assert!(!is_kage_executable(no), "{no}");
        }
    }

    #[test]
    fn pid_zero_is_never_kage() {
        assert!(!is_kage_process(0));
    }

    #[test]
    fn this_test_process_is_not_kage() {
        assert!(!is_kage_process(std::process::id()));
    }
}
