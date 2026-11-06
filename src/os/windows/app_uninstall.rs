// Windows app uninstall: launch the app's own interactive UninstallString,
// or open Settings → Installed apps (see `os::app_uninstall`).

use crate::os::app_uninstall::{
    expand_env_vars, has_silent_flag, match_uninstall_entry, normalize_win_path, own_install_root,
    product_install_dirs, split_command_line, UninstallEntry, UninstallError, UninstallOutcome,
};
use crate::os::platform::file_ops::{shell_execute, with_com_sta};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use windows::Win32::UI::Shell::SEE_MASK_NOASYNC;

const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const INSTALLED_APPS_URI: &str = "ms-settings:appsfeatures";
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_ELEVATION_REQUIRED: i32 = 740;

pub fn is_protected_app_impl(_bundle: &Path) -> bool {
    false
}

pub fn bundle_identifier_impl(_bundle: &Path) -> Option<String> {
    None
}

/// Every product's own install folder from the Uninstall keys.
pub fn installed_product_dirs_impl() -> Vec<PathBuf> {
    product_install_dirs(&read_uninstall_entries(), &generic_dirs())
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

pub fn uninstall_impl(path: &Path, name: &str) -> Result<UninstallOutcome, UninstallError> {
    let raw = path.to_string_lossy();
    // Store/MSIX apps: Settings owns their removal.
    if raw.to_lowercase().starts_with(r"shell:appsfolder\") {
        return open_installed_apps();
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    let target: Option<PathBuf> = match ext.as_deref() {
        Some("lnk") => resolve_shortcut_target(path),
        Some("exe") => Some(path.to_path_buf()),
        _ => return Err(UninstallError::Unsupported),
    };
    let own_root = own_install_root().map(|p| normalize_win_path(&p.to_string_lossy()));
    let inside_own_root = |p: &str| {
        let p = normalize_win_path(p);
        own_root
            .as_deref()
            .is_some_and(|root| p == root || p.starts_with(&format!("{root}\\")))
    };
    let target_str = target.as_ref().map(|t| t.to_string_lossy().into_owned());
    if target_str.as_deref().is_some_and(inside_own_root) {
        return Err(UninstallError::IsKage);
    }

    let entries = read_uninstall_entries();
    let Some(entry) = match_uninstall_entry(name, target_str.as_deref(), &entries, &generic_dirs())
    else {
        log::info!("[uninstall] no confident Uninstall entry for {name:?}; opening Settings");
        return open_installed_apps();
    };
    let display = entry.display_name.as_deref().unwrap_or_default();
    if display.trim().eq_ignore_ascii_case("kage")
        || entry
            .install_location
            .as_deref()
            .is_some_and(inside_own_root)
    {
        return Err(UninstallError::IsKage);
    }
    // Already `%VAR%`-expanded by `read_uninstall_entries`.
    let command = entry.uninstall_string.clone().unwrap_or_default();
    let exists = |p: &str| Path::new(p).is_file() || Path::new(&format!("{p}.exe")).is_file();
    let Some((program, params)) = split_command_line(&command, exists) else {
        return open_installed_apps();
    };
    if has_silent_flag(&params) {
        log::warn!(
            "[uninstall] {display:?}'s UninstallString runs unattended; opening Settings instead"
        );
        return open_installed_apps();
    }
    log::info!("[uninstall] launching {display:?}'s uninstaller: {program}");
    launch_uninstaller(&program, &params)?;
    Ok(UninstallOutcome::UninstallerLaunched)
}

fn open_installed_apps() -> Result<UninstallOutcome, UninstallError> {
    crate::os::open_url(INSTALLED_APPS_URI).map_err(|e| UninstallError::Failed(e.to_string()))?;
    Ok(UninstallOutcome::SettingsOpened)
}

/// Run the uninstaller outside Kage's kill-on-exit Job Object (so quitting
/// Kage doesn't kill it). One whose manifest demands elevation fails
/// `CreateProcess` with ERROR_ELEVATION_REQUIRED; the shell then shows the
/// normal UAC prompt for it. Kage never elevates anything itself.
fn launch_uninstaller(program: &str, params: &str) -> Result<(), UninstallError> {
    use std::os::windows::process::CommandExt;
    let build = |breakaway: bool| {
        let mut cmd = std::process::Command::new(program);
        if !params.is_empty() {
            cmd.raw_arg(params);
        }
        if breakaway {
            crate::os::configure_breakaway_from_job(&mut cmd);
        }
        cmd
    };
    let mut result = build(true).spawn();
    // Kage itself may run inside a job that forbids breakaway.
    if matches!(&result, Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED)) {
        result = build(false).spawn();
    }
    match result {
        Ok(_child) => Ok(()),
        Err(e) if e.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) => {
            let program = program.to_string();
            let params = params.to_string();
            let launched = with_com_sta(move || {
                let params = (!params.is_empty()).then(|| OsStr::new(&params));
                shell_execute(None, OsStr::new(&program), params, SEE_MASK_NOASYNC)
            })
            .map_err(|e| UninstallError::Failed(e.to_string()))?
            .map_err(|e| UninstallError::Failed(e.to_string()))?;
            if !launched {
                log::info!("[uninstall] UAC prompt declined");
            }
            Ok(())
        }
        Err(e) => Err(UninstallError::Failed(e.to_string())),
    }
}

/// The file a Start Menu shortcut points at, via `IShellLinkW`.
fn resolve_shortcut_target(lnk: &Path) -> Option<PathBuf> {
    let lnk = lnk.to_path_buf();
    with_com_sta(move || unsafe {
        use windows::core::{Interface, HSTRING};
        use windows::Win32::System::Com::{
            CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
        };
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    })
    .ok()
    .flatten()
}

/// Every Uninstall subkey: HKLM's 64- and 32-bit views, then HKCU.
fn read_uninstall_entries() -> Vec<UninstallEntry> {
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };
    use winreg::RegKey;

    let views = [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, KEY_READ),
    ];
    let mut out = Vec::new();
    for (hive, access) in views {
        let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(UNINSTALL_KEY, access) else {
            continue;
        };
        for name in root.enum_keys().flatten() {
            let Ok(sub) = root.open_subkey_with_flags(&name, access) else {
                continue;
            };
            let text = |v: &str| {
                sub.get_value::<String, _>(v)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            let flag = |v: &str| sub.get_value::<u32, _>(v).ok() == Some(1);
            // REG_EXPAND_SZ values carry `%VAR%`s: expand them once, here,
            // so locations compare with real paths and the command runs.
            let expanded =
                |v: &str| text(v).map(|s| expand_env_vars(&s, |name| std::env::var(name).ok()));
            out.push(UninstallEntry {
                display_name: text("DisplayName"),
                uninstall_string: expanded("UninstallString"),
                install_location: expanded("InstallLocation"),
                display_icon: expanded("DisplayIcon"),
                system_component: flag("SystemComponent"),
                no_remove: flag("NoRemove"),
                parent_key_name: text("ParentKeyName"),
                release_type: text("ReleaseType"),
            });
        }
    }
    out
}

/// Shared folders many products install under (normalised).
fn generic_dirs() -> Vec<String> {
    let mut dirs: Vec<String> = [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "CommonProgramFiles",
        "CommonProgramFiles(x86)",
        "SystemRoot",
        "windir",
        "ProgramData",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "PUBLIC",
    ]
    .iter()
    .filter_map(|v| std::env::var(v).ok())
    .collect();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(format!(r"{local}\Programs"));
    }
    if let Ok(root) = std::env::var("SystemRoot") {
        dirs.push(format!(r"{root}\System32"));
        dirs.push(format!(r"{root}\SysWOW64"));
    }
    dirs.iter().map(|d| normalize_win_path(d)).collect()
}
