//! Uninstalling an app from the launcher's result menu — always after the
//! user confirmed in Kage, and always through the OS's own mechanism:
//!
//! - **macOS:** move the `.app` bundle to the Trash. Bundles on the system
//!   volume (`/System`, which is where `/Applications/Safari.app` really
//!   lives) or carrying the SIP `restricted` flag are refused. If macOS
//!   denies the move (root-owned or App Store bundles), the bundle is
//!   revealed in Finder so the user can remove it there.
//! - **Windows:** run the app's own interactive `UninstallString` from the
//!   HKLM / HKCU Uninstall keys, matched to the app with evidence (see
//!   `match_uninstall_entry`). Never `QuietUninstallString`, never an entry
//!   carrying silent switches, and elevation only through the OS prompt.
//!   Store/MSIX apps, and anything without a confident match, go to
//!   Settings → Installed apps instead.
//! - **Linux:** not offered — package managers own app removal and there's
//!   no one correct way to drive them.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallOutcome {
    /// macOS: the bundle is in the Trash.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    MovedToTrash,
    /// Windows: the app's own uninstaller is running.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    UninstallerLaunched,
    /// Windows: Settings → Installed apps is open.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    SettingsOpened,
    /// macOS: moving it was denied; it's selected in Finder instead.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    RevealedForManualRemoval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallError {
    /// Part of the OS (system volume / SIP) — never removed from here.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Protected,
    /// The target is Kage itself.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    IsKage,
    /// macOS: the app is running; Finder refuses that too.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    AppRunning,
    /// Not something this OS can uninstall from the launcher.
    Unsupported,
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Failed(String),
}

/// Uninstall the launcher app `name` found at `path` (a macOS bundle, a
/// Windows Start Menu shortcut / exe / `shell:AppsFolder\…` id). Blocking:
/// run it off the async runtime.
pub fn uninstall(path: &Path, name: &str) -> Result<UninstallOutcome, UninstallError> {
    crate::os::platform::app_uninstall::uninstall_impl(path, name)
}

/// Whether `bundle` must never be removed from the launcher (macOS: on the
/// system volume or SIP-restricted). Always `false` elsewhere.
pub fn is_protected_app(bundle: &Path) -> bool {
    crate::os::platform::app_uninstall::is_protected_app_impl(bundle)
}

/// The folders the OS records installed products in — Windows: each
/// Uninstall entry's `InstallLocation`, shared folders and drive roots
/// left out. Empty elsewhere. Blocking: reads the registry.
pub fn installed_product_dirs() -> Vec<PathBuf> {
    crate::os::platform::app_uninstall::installed_product_dirs_impl()
}

/// The app's bundle identifier (macOS `CFBundleIdentifier`), if any.
pub fn bundle_identifier(bundle: &Path) -> Option<String> {
    crate::os::platform::app_uninstall::bundle_identifier_impl(bundle)
}

/// The `.app` bundle the running Kage executable lives in (macOS), or the
/// executable's own directory elsewhere — what "Kage itself" means when
/// comparing paths.
pub fn own_install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    if cfg!(target_os = "macos") {
        if let Some(bundle) = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
        {
            return Some(bundle.to_path_buf());
        }
    }
    exe.parent().map(Path::to_path_buf)
}

/// macOS: a canonical bundle path under `/System` (the sealed system
/// volume, cryptex-hosted Safari included), or one carrying the SIP
/// `restricted` flag, belongs to the OS.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn mac_bundle_is_protected(canonical: &Path, sip_restricted: bool) -> bool {
    sip_restricted || canonical.starts_with("/System")
}

// ---------------------------------------------------------------------
// Windows Uninstall-key matching. Pure so it's tested on every OS; only
// the Windows implementation calls it.
// ---------------------------------------------------------------------

/// One subkey of a `…\CurrentVersion\Uninstall` key, as read from the
/// registry (empty strings already mapped to `None`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub struct UninstallEntry {
    pub display_name: Option<String>,
    pub uninstall_string: Option<String>,
    pub install_location: Option<String>,
    pub display_icon: Option<String>,
    pub system_component: bool,
    pub no_remove: bool,
    pub parent_key_name: Option<String>,
    pub release_type: Option<String>,
}

/// Lower-cased, forward-slash-free, no trailing separator, quotes and a
/// `,<icon index>` suffix stripped — how Windows paths are compared.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn normalize_win_path(raw: &str) -> String {
    let mut s = raw.trim().trim_matches('"').trim().to_string();
    // DisplayIcon is often `C:\App\app.exe,0`.
    if let Some(idx) = s.rfind(',') {
        if s[idx + 1..]
            .trim()
            .trim_start_matches('-')
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            s.truncate(idx);
        }
    }
    let s = s.trim().trim_matches('"').replace('/', "\\").to_lowercase();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    s.trim_end_matches('\\').to_string()
}

/// Whether `dir` (normalised) is a shared folder that many products live
/// under — a match "inside" it says nothing about which product an exe
/// belongs to. `generic` holds the machine's shared folders (Program Files,
/// Windows, AppData, …), also normalised.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn is_generic_install_dir(dir: &str, generic: &[String]) -> bool {
    // A drive root (`c:`) or a single folder below it (`c:\tools`).
    let depth = dir.split('\\').filter(|c| !c.is_empty()).count();
    depth <= 2 || generic.iter().any(|g| g == dir)
}

/// Each entry's `InstallLocation` that names one product's own folder
/// (normalised, deduplicated) — every entry, removable or not: they're
/// all installed software. Shared folders (`generic`, drive roots and
/// one-level folders) are left out, so a sloppy `InstallLocation` of
/// `C:\` can't claim the whole drive.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn product_install_dirs(entries: &[UninstallEntry], generic: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for loc in entries
        .iter()
        .filter_map(|e| e.install_location.as_deref())
        .map(normalize_win_path)
    {
        if !loc.is_empty() && !is_generic_install_dir(&loc, generic) && !out.contains(&loc) {
            out.push(loc);
        }
    }
    out
}

fn eligible(e: &UninstallEntry) -> bool {
    let is_update = e.release_type.as_deref().is_some_and(|t| {
        let t = t.to_lowercase();
        t.contains("update") || t.contains("hotfix") || t.contains("service pack")
    });
    e.display_name.is_some()
        && e.uninstall_string.is_some()
        && !e.system_component
        && !e.no_remove
        && e.parent_key_name.is_none()
        && !is_update
}

/// Distinctive words (3+ letters/digits) of a product name.
fn name_tokens(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_string)
        .collect()
}

/// The program an `UninstallString` runs, for locating the product (not
/// for launching it — `split_command_line` does that): a quoted program,
/// else everything up to the first `.exe`, else the first word.
fn uninstaller_program(cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    if let Some(quoted) = cmd.strip_prefix('"') {
        return quoted.split_once('"').map(|(p, _)| p.to_string());
    }
    if let Some(idx) = cmd.to_lowercase().find(".exe") {
        return Some(cmd[..idx + 4].to_string());
    }
    cmd.split_whitespace().next().map(str::to_string)
}

/// The folder part of a normalised Windows path.
fn win_parent(path: &str) -> Option<&str> {
    path.rsplit_once('\\').map(|(dir, _)| dir)
}

/// Where an entry says its product lives: `InstallLocation`, the folder
/// of `DisplayIcon` and the folder of the uninstaller — normalised, with
/// shared folders (Program Files, System32, …) dropped, since a product
/// "inside" one of those says nothing about which product it is.
fn entry_locations(e: &UninstallEntry, generic: &[String]) -> Vec<String> {
    let icon = e.display_icon.as_deref().map(normalize_win_path);
    let uninstaller = e
        .uninstall_string
        .as_deref()
        .and_then(uninstaller_program)
        .map(|p| normalize_win_path(&p));
    let mut out: Vec<String> = Vec::new();
    let candidates = [
        e.install_location.as_deref().map(normalize_win_path),
        icon.as_deref().and_then(win_parent).map(str::to_string),
        uninstaller
            .as_deref()
            .and_then(win_parent)
            .map(str::to_string),
    ];
    for loc in candidates.into_iter().flatten() {
        if loc.contains('\\') && !is_generic_install_dir(&loc, generic) && !out.contains(&loc) {
            out.push(loc);
        }
    }
    out
}

/// Pick the Uninstall entry for the app called `app_name` whose executable
/// is `target` (a shortcut's target, or the exe itself). Returns `None`
/// unless exactly one eligible entry matches with evidence — the caller
/// then sends the user to Settings rather than guess.
///
/// With the executable known, only the executable's location counts:
///
/// 1. *Strong:* `DisplayIcon` is the target exe, or `InstallLocation` is
///    the exe's own folder.
/// 2. *Weaker, used only without a strong match:* the exe sits inside one
///    of the entry's locations (`entry_locations`: install location, icon
///    folder, uninstaller folder) **and** the names are equal or share a
///    distinctive word.
///
/// A name alone never decides (a portable copy can share an installed
/// product's name), and an entry whose `InstallLocation` leaves the exe
/// out is contradicted and never matches. Without a known executable (a
/// shortcut that couldn't be resolved), only an exact `DisplayName` match
/// counts.
///
/// Shared folders (`generic`, drive roots) never count as a location, and
/// duplicates (the same product listed in the 32- and 64-bit views)
/// collapse to one.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn match_uninstall_entry<'e>(
    app_name: &str,
    target: Option<&str>,
    entries: &'e [UninstallEntry],
    generic: &[String],
) -> Option<&'e UninstallEntry> {
    let mut candidates: Vec<&UninstallEntry> = Vec::new();
    for e in entries.iter().filter(|e| eligible(e)) {
        let dup = candidates.iter().any(|c| {
            c.display_name.as_deref().map(str::to_lowercase)
                == e.display_name.as_deref().map(str::to_lowercase)
                && c.uninstall_string == e.uninstall_string
        });
        if !dup {
            candidates.push(e);
        }
    }

    let app_name = app_name.trim();
    let same_name = |e: &UninstallEntry| {
        !app_name.is_empty()
            && e.display_name
                .as_deref()
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(app_name))
    };
    let unique = |found: Vec<&'e UninstallEntry>| (found.len() == 1).then(|| found[0]);

    let target = target.map(normalize_win_path).filter(|t| !t.is_empty());
    let Some(target) = target else {
        return unique(candidates.into_iter().filter(|e| same_name(e)).collect());
    };
    let target_dir = win_parent(&target).map(str::to_string);
    let app_tokens = name_tokens(app_name);
    let inside = |l: &str| target.starts_with(&format!("{l}\\"));

    // An install location that leaves the exe out places the product
    // elsewhere: that entry is ruled out, whatever else agrees. (Icons and
    // uninstallers often sit in shared caches — `Windows\Installer`,
    // `Package Cache` — so they only ever count *for* a match.)
    let install_dir = |e: &UninstallEntry| {
        e.install_location
            .as_deref()
            .map(normalize_win_path)
            .filter(|l| !l.is_empty() && !is_generic_install_dir(l, generic))
    };
    let contradicted = |e: &UninstallEntry| install_dir(e).is_some_and(|l| !inside(&l));
    let located: Vec<(&UninstallEntry, Vec<String>)> = candidates
        .into_iter()
        .filter(|e| !contradicted(e))
        .map(|e| (e, entry_locations(e, generic)))
        .collect();

    let strong: Vec<&UninstallEntry> = located
        .iter()
        .filter(|(e, _)| {
            let by_icon = e.display_icon.as_deref().map(normalize_win_path).as_deref()
                == Some(target.as_str());
            let by_dir = target_dir.is_some() && install_dir(e) == target_dir;
            by_icon || by_dir
        })
        .map(|(e, _)| *e)
        .collect();
    if !strong.is_empty() {
        return unique(strong);
    }

    unique(
        located
            .iter()
            .filter(|(e, locs)| {
                let shares_word = e.display_name.as_deref().is_some_and(|n| {
                    let tokens = name_tokens(n);
                    app_tokens.iter().any(|t| tokens.contains(t))
                });
                locs.iter().any(|l| inside(l)) && (same_name(e) || shares_word)
            })
            .map(|(e, _)| *e)
            .collect(),
    )
}

/// Expand `%VAR%` references (REG_EXPAND_SZ values) with `lookup`.
/// Unknown variables are left as written, like `ExpandEnvironmentStrings`.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn expand_env_vars(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match lookup(name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Split an `UninstallString` into (program, parameters) the way
/// `CreateProcess` reads a command line: a quoted program runs to its
/// closing quote; an unquoted one with spaces in its path is resolved by
/// trying ever-longer prefixes that `exists` (`C:\Program Files\App\u.exe
/// /x`), falling back to the first word (`MsiExec.exe /X{…}`).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn split_command_line(cmd: &str, exists: impl Fn(&str) -> bool) -> Option<(String, String)> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    if let Some(quoted) = cmd.strip_prefix('"') {
        let end = quoted.find('"')?;
        let program = quoted[..end].trim();
        if program.is_empty() {
            return None;
        }
        return Some((program.to_string(), quoted[end + 1..].trim().to_string()));
    }
    for (idx, _) in cmd.match_indices(' ') {
        let candidate = &cmd[..idx];
        if exists(candidate) {
            return Some((candidate.to_string(), cmd[idx + 1..].trim().to_string()));
        }
    }
    match cmd.split_once(char::is_whitespace) {
        Some((program, args)) => Some((program.to_string(), args.trim().to_string())),
        None => Some((cmd.to_string(), String::new())),
    }
}

/// Switch names (after `/`, `-` or `--`, lower-cased, before any `=` or
/// `:` value) that make an uninstaller run without asking: NSIS `/S`,
/// Inno `/SILENT` `/VERYSILENT` `/SUPPRESSMSGBOXES`, InstallShield `-s`,
/// WiX Burn and MSI `/quiet` `/passive`, Mozilla `-ms`, Chromium
/// `--force-uninstall`, and the generic `--silent` / `--unattended` /
/// `--no-prompt` family.
const SILENT_SWITCHES: &[&str] = &[
    "s",
    "silent",
    "verysilent",
    "quiet",
    "passive",
    "unattended",
    "ms",
    "suppressmsgboxes",
    "force-uninstall",
    "forceuninstall",
    "no-prompt",
    "noprompt",
    "noninteractive",
    "non-interactive",
    "yes",
    "y",
];

/// Values that make a mode switch unattended (`--mode unattended`,
/// `/mode=silent`, `--ui=none`).
const SILENT_VALUES: &[&str] = &[
    "silent",
    "verysilent",
    "quiet",
    "passive",
    "unattended",
    "none",
];

/// Whether `flag` (a switch name, lower-cased, prefix stripped) is an MSI
/// UI level that runs without the user: `q` (= `qn`), `qn`, `qb`, `qr`, any
/// of them with `+` (a closing dialog), `-` (no modal dialogs) or `!` (no
/// Cancel) — in any combination, e.g. `qb!-` or `qn+`. Only `qf`, the full
/// interactive UI, is accepted, and only without modifiers. `query`,
/// `qualify` and other words starting with `q` aren't UI levels.
fn is_unattended_msi_level(flag: &str) -> bool {
    let Some(rest) = flag.strip_prefix('q') else {
        return false;
    };
    let (level, modifiers) = match rest.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => (Some(c), &rest[1..]),
        _ => (None, rest),
    };
    if !modifiers.chars().all(|c| matches!(c, '+' | '-' | '!')) {
        return false;
    }
    match level {
        None | Some('n') | Some('b') | Some('r') => true,
        Some('f') => !modifiers.is_empty(),
        Some(_) => false,
    }
}

/// Whether uninstall parameters ask for an unattended run. Such an entry
/// is never launched from the launcher: the user must see the app's own
/// uninstaller UI. Errs towards `true` — a false alarm only means the
/// user is sent to Settings → Installed apps instead.
///
/// Switches may start with `/`, `-` or `--` and be glued together
/// (`/x{GUID}/qn`) or nested in a quoted argument (InstallShield's
/// `/v"/qn"`), so every `/`-separated piece is read as a switch.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn has_silent_flag(params: &str) -> bool {
    let tokens: Vec<String> = params
        .split_whitespace()
        .map(|t| t.trim_matches('"').to_lowercase())
        .collect();
    let silent_name = |name: &str| SILENT_SWITCHES.contains(&name) || is_unattended_msi_level(name);
    let silent_switch = |switch: &str| {
        let switch = switch.trim_matches('"');
        let (name, value) = match switch.find(['=', ':']) {
            Some(i) => (&switch[..i], Some(switch[i + 1..].trim_matches('"'))),
            None => (switch, None),
        };
        silent_name(name) || value.is_some_and(|v| SILENT_VALUES.contains(&v))
    };
    tokens.iter().any(|t| {
        // A bare value after a mode switch: `--mode unattended`.
        if SILENT_VALUES.contains(&t.as_str()) {
            return true;
        }
        let (head, glued) = match t.strip_prefix("--") {
            Some(rest) => (Some(rest), ""),
            None => match t.strip_prefix('-') {
                Some(rest) => (Some(rest), ""),
                None => match t.strip_prefix('/') {
                    Some(rest) => (None, rest),
                    None => (None, t.as_str()),
                },
            },
        };
        if let Some(name) = head {
            if silent_switch(name) {
                return true;
            }
        }
        // `/a/b`-style pieces; for a token that didn't start with `/`, only
        // what follows a `/` inside it.
        let mut pieces = glued.split('/');
        if !t.starts_with('/') {
            pieces.next();
        }
        pieces.any(|p| !p.is_empty() && silent_switch(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, uninstall: &str) -> UninstallEntry {
        UninstallEntry {
            display_name: Some(name.into()),
            uninstall_string: Some(uninstall.into()),
            ..Default::default()
        }
    }

    fn generic() -> Vec<String> {
        [
            r"C:\Program Files",
            r"C:\Program Files (x86)",
            r"C:\Windows",
            r"C:\Users\me\AppData\Local",
            r"C:\Users\me\AppData\Local\Programs",
        ]
        .iter()
        .map(|s| normalize_win_path(s))
        .collect()
    }

    #[test]
    fn mac_system_volume_and_sip_bundles_are_protected() {
        assert!(mac_bundle_is_protected(
            Path::new("/System/Applications/Calculator.app"),
            false
        ));
        assert!(mac_bundle_is_protected(
            Path::new("/System/Cryptexes/App/System/Applications/Safari.app"),
            false
        ));
        assert!(mac_bundle_is_protected(
            Path::new("/Applications/Foo.app"),
            true
        ));
        assert!(!mac_bundle_is_protected(
            Path::new("/Applications/Slack.app"),
            false
        ));
        assert!(!mac_bundle_is_protected(
            Path::new("/Users/me/Applications/Systematic.app"),
            false
        ));
    }

    #[test]
    fn normalizes_quotes_icon_index_case_and_separators() {
        assert_eq!(
            normalize_win_path(r#""C:\Program Files\App\App.exe",0"#),
            r"c:\program files\app\app.exe"
        );
        assert_eq!(normalize_win_path(r"C:/Apps/Tool/"), r"c:\apps\tool");
        assert_eq!(normalize_win_path(r"\\?\C:\X\y.exe,-101"), r"c:\x\y.exe");
        // A comma that isn't an icon index stays.
        assert_eq!(
            normalize_win_path(r"C:\Foo, Inc\a.exe"),
            r"c:\foo, inc\a.exe"
        );
    }

    #[test]
    fn matches_by_install_location_of_the_shortcut_target() {
        let entries = vec![
            UninstallEntry {
                install_location: Some(
                    r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\".into(),
                ),
                ..entry(
                    "Microsoft Visual Studio Code (User)",
                    r#""C:\…\unins000.exe""#,
                )
            },
            entry("Git", r#""C:\Program Files\Git\unins000.exe""#),
        ];
        let got = match_uninstall_entry(
            "Visual Studio Code",
            Some(r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe"),
            &entries,
            &generic(),
        );
        assert_eq!(got, Some(&entries[0]));
    }

    #[test]
    fn matches_by_display_icon_or_exact_name() {
        let entries = vec![
            UninstallEntry {
                display_icon: Some(r"C:\Program Files\Notepad++\notepad++.exe,0".into()),
                ..entry(
                    "Notepad++ (64-bit x64)",
                    r"C:\Program Files\Notepad++\uninstall.exe",
                )
            },
            entry(
                "Spotify",
                r"C:\Users\me\AppData\Roaming\Spotify\Spotify.exe /uninstall",
            ),
        ];
        let g = generic();
        assert_eq!(
            match_uninstall_entry(
                "Notepad++",
                Some(r"C:\Program Files\Notepad++\notepad++.exe"),
                &entries,
                &g
            ),
            Some(&entries[0])
        );
        assert_eq!(
            match_uninstall_entry("spotify", None, &entries, &g),
            Some(&entries[1])
        );
    }

    #[test]
    fn generic_install_locations_never_match() {
        let entries = vec![UninstallEntry {
            install_location: Some(r"C:\Program Files".into()),
            ..entry("Some Runtime", "MsiExec.exe /X{1}")
        }];
        assert_eq!(
            match_uninstall_entry(
                "Paint",
                Some(r"C:\Program Files\paint.exe"),
                &entries,
                &generic()
            ),
            None
        );
        // Drive roots and one-level folders are generic too.
        assert!(is_generic_install_dir("c:", &[]));
        assert!(is_generic_install_dir(r"d:\games", &[]));
        assert!(!is_generic_install_dir(r"d:\games\foo", &[]));
        assert!(is_generic_install_dir(r"c:\program files", &generic()));
    }

    #[test]
    fn ambiguous_matches_fall_back_to_settings() {
        let loc = r"C:\Program Files\Microsoft Office\root\Office16";
        let entries = vec![
            UninstallEntry {
                install_location: Some(loc.into()),
                ..entry(
                    "Microsoft 365 Apps",
                    "OfficeClickToRun.exe scenario=install",
                )
            },
            UninstallEntry {
                install_location: Some(loc.into()),
                ..entry(
                    "Microsoft OneNote",
                    "OfficeClickToRun.exe scenario=install productstoremove=OneNote",
                )
            },
        ];
        assert_eq!(
            match_uninstall_entry(
                "Word",
                Some(&format!(r"{loc}\WINWORD.EXE")),
                &entries,
                &generic()
            ),
            None
        );
    }

    #[test]
    fn deeper_location_needs_a_shared_name_word() {
        let entries = vec![UninstallEntry {
            install_location: Some(r"C:\Program Files\JetBrains\IntelliJ IDEA 2024.1".into()),
            ..entry(
                "IntelliJ IDEA 2024.1",
                r#""C:\Program Files\JetBrains\IntelliJ IDEA 2024.1\bin\Uninstall.exe""#,
            )
        }];
        let exe = r"C:\Program Files\JetBrains\IntelliJ IDEA 2024.1\bin\idea64.exe";
        let g = generic();
        assert_eq!(
            match_uninstall_entry("IntelliJ IDEA Ultimate", Some(exe), &entries, &g),
            Some(&entries[0])
        );
        assert_eq!(
            match_uninstall_entry("Code With Me", Some(exe), &entries, &g),
            None
        );
    }

    #[test]
    fn skips_system_components_updates_and_duplicates() {
        let base = entry("Contoso Tool", r#""C:\Contoso\Tool\uninst.exe""#);
        let entries = vec![
            UninstallEntry {
                system_component: true,
                ..base.clone()
            },
            UninstallEntry {
                release_type: Some("Security Update".into()),
                ..base.clone()
            },
            UninstallEntry {
                parent_key_name: Some("Contoso".into()),
                ..base.clone()
            },
            UninstallEntry {
                no_remove: true,
                ..base.clone()
            },
            UninstallEntry {
                uninstall_string: None,
                ..base.clone()
            },
        ];
        assert_eq!(
            match_uninstall_entry("Contoso Tool", None, &entries, &generic()),
            None
        );
        // The same product in both registry views is one match, not two.
        let twice = vec![base.clone(), base.clone()];
        assert_eq!(
            match_uninstall_entry("Contoso Tool", None, &twice, &generic()),
            Some(&twice[0])
        );
    }

    #[test]
    fn a_shared_name_never_overrides_where_the_exe_is() {
        // An installed Contoso Tool, and a portable copy of the same name.
        let installed = UninstallEntry {
            install_location: Some(r"C:\Program Files\Contoso".into()),
            display_icon: Some(r"C:\Program Files\Contoso\tool.exe,0".into()),
            ..entry("Tool", r#""C:\Program Files\Contoso\unins000.exe""#)
        };
        let entries = vec![installed];
        let g = generic();
        assert_eq!(
            match_uninstall_entry("Tool", Some(r"D:\Portable\Tool\tool.exe"), &entries, &g),
            None
        );
        // The installed copy itself still matches.
        assert_eq!(
            match_uninstall_entry(
                "Tool",
                Some(r"C:\Program Files\Contoso\tool.exe"),
                &entries,
                &g
            ),
            Some(&entries[0])
        );
    }

    #[test]
    fn with_a_known_exe_a_name_alone_is_not_enough() {
        let entries = vec![entry("Contoso Tool", "MsiExec.exe /X{1}")];
        let g = generic();
        assert_eq!(
            match_uninstall_entry(
                "Contoso Tool",
                Some(r"C:\Program Files\Contoso\tool.exe"),
                &entries,
                &g
            ),
            None
        );
        // Without one (a shortcut that couldn't be read), it is.
        assert_eq!(
            match_uninstall_entry("Contoso Tool", None, &entries, &g),
            Some(&entries[0])
        );
    }

    #[test]
    fn an_install_location_elsewhere_contradicts_a_matching_icon() {
        let entries = vec![UninstallEntry {
            install_location: Some(r"C:\Program Files\Contoso".into()),
            display_icon: Some(r"D:\Portable\Tool\tool.exe".into()),
            ..entry("Tool", r#""C:\Program Files\Contoso\unins000.exe""#)
        }];
        assert_eq!(
            match_uninstall_entry(
                "Tool",
                Some(r"D:\Portable\Tool\tool.exe"),
                &entries,
                &generic()
            ),
            None
        );
    }

    #[test]
    fn the_uninstallers_folder_is_evidence_with_a_matching_name() {
        // Inno Setup: no InstallLocation, unins000.exe next to the app.
        let entries = vec![entry(
            "Contoso Tool 2.1",
            r#""C:\Program Files\Contoso Tool\unins000.exe""#,
        )];
        let g = generic();
        let exe = r"C:\Program Files\Contoso Tool\tool.exe";
        assert_eq!(
            match_uninstall_entry("Contoso Tool", Some(exe), &entries, &g),
            Some(&entries[0])
        );
        assert_eq!(
            match_uninstall_entry("Paint", Some(exe), &entries, &g),
            None
        );
        // A cache folder (WiX Burn's Package Cache) proves nothing.
        let cached = vec![entry(
            "Contoso Tool",
            r#""C:\ProgramData\Package Cache\{AB-12}\setup.exe" /uninstall"#,
        )];
        assert_eq!(
            match_uninstall_entry("Contoso Tool", Some(exe), &cached, &g),
            None
        );
    }

    #[test]
    fn finds_the_uninstaller_program() {
        assert_eq!(
            uninstaller_program(r#""C:\Program Files\App\u.exe" /x"#).as_deref(),
            Some(r"C:\Program Files\App\u.exe")
        );
        assert_eq!(
            uninstaller_program(r"C:\Program Files\App\uninstall.exe /S").as_deref(),
            Some(r"C:\Program Files\App\uninstall.exe")
        );
        assert_eq!(
            uninstaller_program("MsiExec.exe /X{1}").as_deref(),
            Some("MsiExec.exe")
        );
    }

    #[test]
    fn product_install_dirs_skip_shared_folders_and_roots() {
        let at = |loc: &str| UninstallEntry {
            install_location: Some(loc.into()),
            ..entry("X", "u.exe")
        };
        let entries = vec![
            at(r"C:\Users\me\AppData\Local\Discord\"),
            at(r"c:\users\me\appdata\local\discord"),
            at(r"C:\Program Files\Contoso"),
            at(r"C:\Program Files"),
            at(r"C:\"),
            at(r"D:\Games"),
            at(""),
            UninstallEntry {
                system_component: true,
                ..at(r"C:\Program Files\Runtime\1.0")
            },
        ];
        assert_eq!(
            product_install_dirs(&entries, &generic()),
            vec![
                r"c:\users\me\appdata\local\discord".to_string(),
                r"c:\program files\contoso".to_string(),
                r"c:\program files\runtime\1.0".to_string(),
            ]
        );
    }

    #[test]
    fn splits_quoted_unquoted_and_bare_command_lines() {
        let exists = |p: &str| p == r"C:\Program Files\App\uninstall.exe";
        assert_eq!(
            split_command_line(r#""C:\Program Files\App\u.exe" /x"#, exists),
            Some((r"C:\Program Files\App\u.exe".into(), "/x".into()))
        );
        assert_eq!(
            split_command_line(r"C:\Program Files\App\uninstall.exe --mode remove", exists),
            Some((
                r"C:\Program Files\App\uninstall.exe".into(),
                "--mode remove".into()
            ))
        );
        assert_eq!(
            split_command_line("MsiExec.exe /X{AB-12}", exists),
            Some(("MsiExec.exe".into(), "/X{AB-12}".into()))
        );
        assert_eq!(
            split_command_line(r"C:\Tools\unins000.exe", exists),
            Some((r"C:\Tools\unins000.exe".into(), String::new()))
        );
        assert_eq!(split_command_line("   ", exists), None);
        assert_eq!(split_command_line(r#""unterminated"#, exists), None);
    }

    #[test]
    fn detects_silent_switches_but_not_interactive_ones() {
        for silent in [
            // NSIS, Inno, InstallShield, Mozilla, Chromium, Squirrel.
            "/S",
            "/SILENT",
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "-s",
            "-ms",
            "--force-uninstall",
            "--uninstall -s",
            // Generic.
            "--silent",
            "-silent",
            "--quiet",
            "--unattended",
            "--mode unattended",
            "--mode=unattended",
            "/mode:silent",
            "--ui=none",
            "--no-prompt",
            "-q",
            // MSI / WiX Burn UI levels, every modifier combination, both
            // switch characters.
            "/q",
            "/qn",
            "/QN",
            "/qn+",
            "/qn-",
            "/qb",
            "/qb!",
            "/QB!",
            "/qb-",
            "/qb+",
            "/qb!-",
            "/qb-!",
            "/qb+!",
            "/qb!+",
            "/qr",
            "/qr+",
            "/q+",
            "-qn",
            "-qb!-",
            "/quiet",
            "-quiet",
            "/passive",
            "-passive",
            "/qf+",
            // Placement: after other switches, glued on, quoted, nested.
            "/x {1} /qn+",
            "/X{AB-12}/qn",
            "/I{AB-12} \"/qb!-\"",
            "/s /v\"/qn\"",
        ] {
            assert!(has_silent_flag(silent), "{silent}");
        }
        for interactive in [
            "",
            "/X{AB-12}",
            "/I{AB-12}",
            "/x {AB-12} /qf",
            "/X{AB-12} /norestart",
            "--uninstall",
            "--uninstall --channel=stable --system-level --verbose-logging",
            "/uninstall",
            "-removeonly",
            "-runfromtemp -l0x0409 -removeonly",
            "--mode=remove",
            "/query",
            "/qualify",
            "/qx",
            "scenario=install",
            "scenario=install productstoremove=OneNote",
            "/D=C:\\Program Files\\App",
            "/allusers",
            "/SP-",
        ] {
            assert!(!has_silent_flag(interactive), "{interactive}");
        }
    }

    #[test]
    fn msi_ui_levels_follow_the_grammar() {
        for unattended in [
            "q", "qn", "qb", "qr", "qn+", "qb!-", "qb+!", "qr-", "q!", "qf!",
        ] {
            assert!(is_unattended_msi_level(unattended), "{unattended}");
        }
        for not_level in ["qf", "query", "qualify", "qx", "quiet", "x", "", "qb?"] {
            assert!(!is_unattended_msi_level(not_level), "{not_level}");
        }
    }

    #[test]
    fn expands_known_env_vars_and_keeps_unknown_ones() {
        let lookup = |name: &str| match name.to_uppercase().as_str() {
            "PROGRAMFILES" => Some(r"C:\Program Files".to_string()),
            _ => None,
        };
        assert_eq!(
            expand_env_vars(r"%ProgramFiles%\App\u.exe", lookup),
            r"C:\Program Files\App\u.exe"
        );
        assert_eq!(expand_env_vars(r"%NOPE%\x 100%", lookup), r"%NOPE%\x 100%");
        assert_eq!(expand_env_vars("%%", lookup), "%%");
    }
}
