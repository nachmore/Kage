// Windows app uninstall: Windows Installer's own `msiexec /x`, or the
// app's own plain uninstaller, or Settings → Installed apps — the allowlist
// in `os::app_uninstall::plan_uninstall`.

use crate::os::app_uninstall::{
    expand_env_vars, match_uninstall_entry, normalize_win_path, own_install, plan_uninstall,
    product_install_dirs, UninstallEntry, UninstallError, UninstallOutcome, UninstallPlan, WinFs,
};
use crate::os::file_ops::Preflight;
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
    product_install_dirs(&read_uninstall_entries(), &win_fs())
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

/// No file is removed by Kage here — an uninstaller is started, or
/// Settings opened — so there's nothing for `_preflight` to guard.
pub fn uninstall_impl(
    path: &Path,
    name: &str,
    _preflight: Preflight,
) -> Result<UninstallOutcome, UninstallError> {
    let raw = path.to_string_lossy();
    // Store/MSIX apps: Settings owns their removal.
    if raw.to_lowercase().starts_with(r"shell:appsfolder\") {
        return open_installed_apps();
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    let (target, shortcut_args): (Option<PathBuf>, bool) = match ext.as_deref() {
        Some("lnk") => match resolve_shortcut(path) {
            Some((target, args)) => (Some(target), args),
            None => (None, false),
        },
        Some("exe") => (Some(path.to_path_buf()), false),
        _ => return Err(UninstallError::Unsupported),
    };
    let fs = win_fs();
    let own = own_install();
    let normalized = |paths: Vec<PathBuf>| -> Vec<String> {
        paths
            .iter()
            .map(|p| normalize_win_path(&p.to_string_lossy()))
            .collect()
    };
    let own_paths = normalized(own.paths());
    let target_str = target.as_ref().map(|t| t.to_string_lossy().into_owned());
    if target_str
        .as_deref()
        .is_some_and(|t| fs.is_within_any(t, &own_paths))
    {
        return Err(UninstallError::IsKage);
    }

    let entries = read_uninstall_entries();
    let Some(matched) =
        match_uninstall_entry(name, target_str.as_deref(), shortcut_args, &entries, &fs)
    else {
        log::info!("[uninstall] no confident Uninstall entry for {name:?}; opening Settings");
        return open_installed_apps();
    };
    let entry = matched.entry;
    let display = entry.display_name.as_deref().unwrap_or_default();
    let own_root = normalized(own.root().map(Path::to_path_buf).into_iter().collect());
    let in_own_root = |p: &str| fs.is_within_any(p, &own_root);
    if display
        .trim()
        .eq_ignore_ascii_case(crate::launcher_actions::KAGE_PRODUCT_NAME)
        || entry.install_location.as_deref().is_some_and(in_own_root)
        || matched.install_dir.as_deref().is_some_and(in_own_root)
    {
        return Err(UninstallError::IsKage);
    }

    match plan_uninstall(&matched, &fs, |p| Path::new(p).is_file()) {
        UninstallPlan::Msi { product_code } => {
            let Some(msiexec) = msiexec_path() else {
                log::warn!("[uninstall] msiexec.exe not found; opening Settings");
                return open_installed_apps();
            };
            log::info!("[uninstall] {display:?}: msiexec /x {product_code}");
            launch_uninstaller(&msiexec, &["/x", product_code.as_str()])?;
        }
        UninstallPlan::Run { program, arg } => {
            log::info!("[uninstall] launching {display:?}'s uninstaller: {program}");
            let args: Vec<&str> = arg.as_deref().into_iter().collect();
            launch_uninstaller(&program, &args)?;
        }
        UninstallPlan::Settings => {
            log::info!(
                "[uninstall] {display:?}'s UninstallString isn't on the allowlist, or doesn't resolve inside its own folder; opening Settings"
            );
            return open_installed_apps();
        }
    }
    Ok(UninstallOutcome::UninstallerLaunched)
}

fn open_installed_apps() -> Result<UninstallOutcome, UninstallError> {
    crate::os::open_url(INSTALLED_APPS_URI).map_err(|e| UninstallError::Failed(e.to_string()))?;
    Ok(UninstallOutcome::SettingsOpened)
}

/// `%SystemRoot%\System32\msiexec.exe`, never a PATH lookup.
fn msiexec_path() -> Option<String> {
    let root = std::env::var("SystemRoot").ok()?;
    let path = Path::new(&root).join("System32").join("msiexec.exe");
    (path.is_absolute() && path.is_file()).then(|| path.to_string_lossy().into_owned())
}

/// Run the uninstaller outside Kage's kill-on-exit Job Object (so quitting
/// Kage doesn't kill it). One whose manifest demands elevation fails
/// `CreateProcess` with ERROR_ELEVATION_REQUIRED; the shell then shows the
/// normal UAC prompt for it. Kage never elevates anything itself. `args`
/// come from the allowlist: no spaces or quotes to escape.
fn launch_uninstaller(program: &str, args: &[&str]) -> Result<(), UninstallError> {
    let build = |breakaway: bool| {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
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
            let params = args.join(" ");
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

/// The file a Start Menu shortcut points at, via `IShellLinkW`, and
/// whether it passes that file arguments (a launcher starting something
/// else — see `match_uninstall_entry`). Arguments that can't be read
/// count as present.
fn resolve_shortcut(lnk: &Path) -> Option<(PathBuf, bool)> {
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
        if len == 0 {
            return None;
        }
        let target = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
        let mut args = [0u16; 1024];
        let has_args = match link.GetArguments(&mut args) {
            Ok(()) => args
                .iter()
                .take_while(|&&c| c != 0)
                .any(|&c| !char::from_u32(u32::from(c)).is_some_and(char::is_whitespace)),
            Err(_) => true,
        };
        Some((target, has_args))
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
            // so locations compare with real paths. One that stays
            // unexpanded is never a usable path (`is_absolute_win_path`).
            let expanded =
                |v: &str| text(v).map(|s| expand_env_vars(&s, |name| std::env::var(name).ok()));
            out.push(UninstallEntry {
                key_name: name.clone(),
                display_name: text("DisplayName"),
                uninstall_string: expanded("UninstallString"),
                install_location: expanded("InstallLocation"),
                display_icon: expanded("DisplayIcon"),
                windows_installer: flag("WindowsInstaller"),
                system_component: flag("SystemComponent"),
                no_remove: flag("NoRemove"),
                parent_key_name: text("ParentKeyName"),
                release_type: text("ReleaseType"),
            });
        }
    }
    out
}

/// This machine's shared folders, resolved through `canonicalize` (links,
/// junctions, 8.3 names, trailing dots and spaces).
fn win_fs() -> WinFs {
    WinFs::new(shared_dirs(), |p| {
        std::fs::canonicalize(p)
            .ok()
            .map(|c| c.to_string_lossy().into_owned())
    })
}

/// This machine's shared folders (normalised) — never one product's own
/// folder (see `WinFs::is_shared`; drive and share roots always are).
fn shared_dirs() -> Vec<String> {
    let env = |v: &str| std::env::var(v).ok().filter(|s| !s.trim().is_empty());
    let mut dirs: Vec<String> = [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "CommonProgramFiles",
        "CommonProgramFiles(x86)",
        "CommonProgramW6432",
        "SystemRoot",
        "windir",
        "ProgramData",
        "ALLUSERSPROFILE",
        "PUBLIC",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "TEMP",
        "TMP",
    ]
    .iter()
    .filter_map(|v| env(v))
    .collect();
    if let Some(root) = env("SystemRoot") {
        for sub in ["System32", "SysWOW64", "Installer", "Temp", "WinSxS"] {
            dirs.push(format!(r"{root}\{sub}"));
        }
    }
    if let Some(data) = env("ProgramData") {
        dirs.push(format!(r"{data}\Package Cache"));
        dirs.push(format!(r"{data}\Microsoft\Windows\Start Menu\Programs"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(pf) = env(var) {
            dirs.push(format!(r"{pf}\WindowsApps"));
        }
    }
    if let Some(profile) = env("USERPROFILE") {
        // The folder holding every user's profile (`C:\Users`).
        if let Some(parent) = Path::new(&profile).parent() {
            dirs.push(parent.to_string_lossy().into_owned());
        }
        for sub in ["AppData", r"AppData\LocalLow"] {
            dirs.push(format!(r"{profile}\{sub}"));
        }
    }
    if let Some(local) = env("LOCALAPPDATA") {
        for sub in [
            "Programs",
            "Package Cache",
            "Temp",
            r"Microsoft\WindowsApps",
        ] {
            dirs.push(format!(r"{local}\{sub}"));
        }
    }
    if let Some(public) = env("PUBLIC") {
        if let Some(parent) = Path::new(&public).parent() {
            dirs.push(parent.to_string_lossy().into_owned());
        }
    }
    // Desktop, Documents, Downloads, … wherever they're redirected.
    dirs.extend(
        crate::commands::input::well_known_dirs()
            .iter()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    let mut out: Vec<String> = dirs.iter().map(|d| normalize_win_path(d)).collect();
    out.sort();
    out.dedup();
    out
}
