//! Uninstalling an app from the launcher's result menu — always after the
//! user confirmed in Kage, and always through the OS's own mechanism:
//!
//! - **macOS:** move the `.app` bundle to the Trash. Bundles on the system
//!   volume (`/System`, which is where `/Applications/Safari.app` really
//!   lives) or carrying the SIP `restricted` flag are refused. If macOS
//!   denies the move (root-owned or App Store bundles), the bundle is
//!   revealed in Finder so the user can remove it there.
//! - **Windows:** match the app to its HKLM / HKCU Uninstall entry with
//!   evidence (`match_uninstall_entry`), then run an uninstaller only in
//!   the two cases `plan_uninstall` allows — Windows Installer's own
//!   `msiexec.exe /x {product code}`, or a plain uninstaller executable in
//!   the product's own folder with no argument or one interactive one.
//!   Everything else, Store/MSIX apps, anything without a confident
//!   match, and any folder Kage can't establish isn't shared (`WinFs`) go
//!   to Settings → Installed apps. Elevation only through the OS
//!   prompt; `QuietUninstallString` is never read.
//! - **Linux:** not offered — package managers own app removal and there's
//!   no one correct way to drive them.

use crate::error::AppError;
use crate::os::file_ops::Preflight;
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

#[derive(Debug, Clone)]
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
    /// The preflight refused (macOS: the bundle isn't the one checked).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Refused(AppError),
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Failed(String),
}

/// Uninstall the launcher app `name` found at `path` (a macOS bundle, a
/// Windows Start Menu shortcut / exe / `shell:AppsFolder\…` id). On macOS
/// `preflight` runs right before the bundle is moved, on the same thread
/// (see `os::file_ops`). Blocking: run it off the async runtime.
pub fn uninstall(
    path: &Path,
    name: &str,
    preflight: Preflight,
) -> Result<UninstallOutcome, UninstallError> {
    crate::os::platform::app_uninstall::uninstall_impl(path, name, preflight)
}

/// Whether `bundle` must never be removed from the launcher (macOS: on the
/// system volume or SIP-restricted). Always `false` elsewhere.
pub fn is_protected_app(bundle: &Path) -> bool {
    crate::os::platform::app_uninstall::is_protected_app_impl(bundle)
}

/// The folders the OS records installed products in — Windows: each
/// Uninstall entry's `InstallLocation` that's established as not shared
/// (`product_install_dirs`). Empty elsewhere. Blocking: reads the registry.
pub fn installed_product_dirs() -> Vec<PathBuf> {
    crate::os::platform::app_uninstall::installed_product_dirs_impl()
}

/// The app's bundle identifier (macOS `CFBundleIdentifier`), if any.
pub fn bundle_identifier(bundle: &Path) -> Option<String> {
    crate::os::platform::app_uninstall::bundle_identifier_impl(bundle)
}

// ---------------------------------------------------------------------
// Kage's own installation
// ---------------------------------------------------------------------

/// The MCP sidecar's file name stem (`build.rs` stages it next to the app).
pub const SIDECAR_STEM: &str = "kage-computer-control-mcp";

/// What "Kage itself" is on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnInstall {
    /// A folder that is verifiably Kage's installation, so all of it is
    /// Kage: the `.app` bundle on macOS, or on Windows a folder laid out
    /// the way Kage's installer leaves it (`uninstall.exe` and the sidecar
    /// next to the executable). `None` for a loose executable — a
    /// `Downloads\kage.exe` doesn't make Downloads part of Kage.
    pub root: Option<PathBuf>,
    /// Without a `root`: Kage's own files — the executable and, if it's
    /// there, the sidecar next to it.
    pub files: Vec<PathBuf>,
}

impl OwnInstall {
    /// Everything to protect as a whole tree.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.root.iter().chain(&self.files).cloned().collect()
    }

    /// The folder holding Kage's executable, if that folder is all Kage.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
}

/// Decide what Kage's installation is from the (canonical) path of its
/// executable. `exists` reports whether a file is there; `mac` / `windows`
/// pick the platform's layout.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn own_install_layout(
    exe: &Path,
    mac: bool,
    windows: bool,
    exists: impl Fn(&Path) -> bool,
) -> OwnInstall {
    if mac {
        if let Some(bundle) = exe
            .ancestors()
            .skip(1)
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
        {
            return OwnInstall {
                root: Some(bundle.to_path_buf()),
                files: Vec::new(),
            };
        }
    }
    let Some(dir) = exe.parent() else {
        return OwnInstall {
            root: None,
            files: vec![exe.to_path_buf()],
        };
    };
    let sidecar = dir.join(if windows {
        format!("{SIDECAR_STEM}.exe")
    } else {
        SIDECAR_STEM.to_string()
    });
    // Kage's NSIS installer writes `uninstall.exe` into the install folder
    // and ships the sidecar beside the app: both there means this folder is
    // an installation, not a folder a copy of kage.exe was dropped into.
    if windows && exists(&dir.join("uninstall.exe")) && exists(&sidecar) {
        return OwnInstall {
            root: Some(dir.to_path_buf()),
            files: Vec::new(),
        };
    }
    let mut files = vec![exe.to_path_buf()];
    if exists(&sidecar) {
        files.push(sidecar);
    }
    OwnInstall { root: None, files }
}

/// Kage's installation, from the running executable.
pub fn own_install() -> OwnInstall {
    let Ok(exe) = std::env::current_exe() else {
        return OwnInstall::default();
    };
    let exe = std::fs::canonicalize(&exe)
        .map(crate::launcher_actions::strip_verbatim)
        .unwrap_or(exe);
    own_install_layout(
        &exe,
        cfg!(target_os = "macos"),
        cfg!(target_os = "windows"),
        |p| p.is_file(),
    )
}

/// macOS: a canonical bundle path under `/System` (the sealed system
/// volume, cryptex-hosted Safari included), or one carrying the SIP
/// `restricted` flag, belongs to the OS.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn mac_bundle_is_protected(canonical: &Path, sip_restricted: bool) -> bool {
    sip_restricted || canonical.starts_with("/System")
}

// ---------------------------------------------------------------------
// Windows Uninstall-key matching and the uninstall allowlist. Pure so
// they're tested on every OS; only the Windows implementation calls them.
// ---------------------------------------------------------------------

/// One subkey of a `…\CurrentVersion\Uninstall` key, as read from the
/// registry (empty strings already mapped to `None`, `%VAR%`s expanded).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub struct UninstallEntry {
    /// The subkey's name — a Windows Installer product's `{ProductCode}`.
    pub key_name: String,
    pub display_name: Option<String>,
    pub uninstall_string: Option<String>,
    pub install_location: Option<String>,
    pub display_icon: Option<String>,
    /// `WindowsInstaller = 1`: an MSI product.
    pub windows_installer: bool,
    pub system_component: bool,
    pub no_remove: bool,
    pub parent_key_name: Option<String>,
    pub release_type: Option<String>,
}

/// A Windows path for comparing: unquoted, lower-cased, `\`-separated,
/// repeated separators collapsed, no trailing separator, and the verbatim
/// forms read back as ordinary ones (`\\?\C:\x` → `c:\x`,
/// `\\?\UNC\srv\share\x` → `\\srv\share\x`).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn normalize_win_path(raw: &str) -> String {
    let s = raw
        .trim()
        .trim_matches('"')
        .trim()
        .replace('/', "\\")
        .to_lowercase();
    let s = if let Some(rest) = s
        .strip_prefix(r"\\?\unc\")
        .or_else(|| s.strip_prefix(r"\??\unc\"))
    {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\??\")) {
        rest.to_string()
    } else {
        s
    };
    let (lead, body) = match s.strip_prefix(r"\\") {
        Some(body) => (r"\\", body),
        None => ("", s.as_str()),
    };
    let mut out = String::with_capacity(s.len());
    out.push_str(lead);
    let mut prev_sep = false;
    for c in body.chars() {
        if c == '\\' {
            if !prev_sep {
                out.push(c);
            }
            prev_sep = true;
        } else {
            out.push(c);
            prev_sep = false;
        }
    }
    while out.len() > lead.len() && out.ends_with('\\') {
        out.pop();
    }
    out
}

/// Whether `p` (normalised) is an absolute local or UNC path Kage can
/// reason about: `x:` / `x:\…` or `\\server\share…`, no `.` / `..`
/// component, no wildcard, quote, pipe or control character, and no `:`
/// but the drive's.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn is_absolute_win_path(p: &str) -> bool {
    if p.chars()
        .any(|c| c.is_control() || matches!(c, '*' | '?' | '"' | '<' | '>' | '|' | '%'))
    {
        return false;
    }
    let b = p.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    let rest = if drive {
        if b.len() > 2 && b[2] != b'\\' {
            return false;
        }
        &p[2..]
    } else if let Some(unc) = p.strip_prefix(r"\\") {
        let mut parts = unc.split('\\');
        let (server, share) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if server.is_empty() || share.is_empty() {
            return false;
        }
        unc
    } else {
        return false;
    };
    // `.`, `..`, and any other component of only dots and spaces (which
    // Windows would trim to nothing).
    !rest.contains(':')
        && !rest
            .split('\\')
            .any(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '.' | ' ')))
}

/// `p` (normalised) with each component's trailing dots and spaces
/// dropped: the one form every spelling Windows opens as the same folder
/// shares. Win32 trims them when it opens a path — all of them from the
/// last component, a `.` from any other — so `Downloads.`, `Downloads `
/// and `Downloads. .` all open Downloads. This trims every component, a
/// little more than Windows does, so it's only for deciding a folder is
/// shared, where reading two folders as one errs safe. Components of only
/// dots and spaces are left alone (`is_absolute_win_path` refuses them).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn alias_key(p: &str) -> String {
    let (lead, body) = match p.strip_prefix(r"\\") {
        Some(body) => (r"\\", body),
        None => ("", p),
    };
    let parts: Vec<&str> = body
        .split('\\')
        .map(|c| match c.trim_end_matches(['.', ' ']) {
            "" => c,
            trimmed => trimmed,
        })
        .collect();
    format!("{lead}{}", parts.join("\\"))
}

/// A drive root (`c:`) or a share root (`\\srv\share`), normalised.
fn is_volume_root(p: &str) -> bool {
    let b = p.as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return true;
    }
    p.strip_prefix(r"\\")
        .is_some_and(|unc| unc.split('\\').filter(|c| !c.is_empty()).count() <= 2)
}

/// The file a `DisplayIcon` value names, its icon index removed:
/// `"C:\App\app.exe",0`, `C:\App\app.exe,-101` → `C:\App\app.exe`. Only
/// `DisplayIcon` carries an index; install folders are read as written.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn display_icon_path(raw: &str) -> String {
    let s = raw.trim();
    if let Some(quoted) = s.strip_prefix('"') {
        if let Some(end) = quoted.find('"') {
            return quoted[..end].trim().to_string();
        }
    }
    let s = s.trim_matches('"');
    match s.rsplit_once(',') {
        Some((file, index)) if is_icon_index(index) => file.trim().trim_matches('"').to_string(),
        _ => s.to_string(),
    }
}

fn is_icon_index(s: &str) -> bool {
    let s = s.trim();
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Where a Windows path really leads, as the machine reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub enum Resolution {
    /// It leads here.
    Resolved(String),
    /// Nothing is there (`NotFound`).
    Missing,
    /// Resolving it failed some other way — access denied, a link that
    /// can't be followed, a device that isn't ready — or it leads somewhere
    /// Kage can't reason about: where it leads isn't known.
    Unknown,
}

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
impl Resolution {
    /// What `canonicalize` reported: `Ok` is `Resolved` (`Unknown` if the
    /// path isn't valid Unicode), `NotFound` is `Missing`, and every other
    /// error is `Unknown`.
    pub fn of(result: std::io::Result<PathBuf>) -> Self {
        match result {
            Ok(p) => p
                .into_os_string()
                .into_string()
                .map_or(Self::Unknown, Self::Resolved),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Err(_) => Self::Unknown,
        }
    }
}

/// What's known about whether a folder is shared (`WinFs::sharing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub enum Sharing {
    /// A drive or share root, a shared folder, or a folder holding one —
    /// as written or where it leads.
    Shared,
    /// Established as not shared: it resolves, where every shared folder
    /// really is is known, and neither the folder as written nor where it
    /// leads is shared.
    NotShared,
    /// Neither could be established: it doesn't resolve, or a shared
    /// folder's real location is unknown. Treated as shared wherever
    /// ownership authorizes or protects anything.
    Unknown,
}

/// Where a Windows path really leads (see `WinFs::new`).
type Resolver = Box<dyn Fn(&str) -> Resolution>;

/// What the Windows path decisions need from the machine: its shared
/// folders, and where a path really leads.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub struct WinFs {
    /// `alias_key` of each shared folder, as configured and as resolved.
    shared: Vec<String>,
    /// A shared folder resolved `Unknown`: it could really be anywhere, so
    /// no folder is established as not shared.
    shared_unknown: bool,
    resolve: Resolver,
}

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
impl WinFs {
    /// `shared`: this machine's shared folders (Program Files, Windows,
    /// ProgramData, the user profile roots, AppData, Desktop / Documents /
    /// Downloads, temp, …: `shared_dirs` on Windows). `resolve`: where a
    /// path really leads, every link, junction, 8.3 name and trailing dot
    /// or space resolved — Windows' `canonicalize`, read by
    /// `Resolution::of`.
    ///
    /// A shared folder that's `Missing` is only compared as configured:
    /// nothing is there, so nothing resolves into it. One that's `Unknown`
    /// makes every folder's sharing `Unknown`.
    pub fn new(
        shared: impl IntoIterator<Item = String>,
        resolve: impl Fn(&str) -> Resolution + 'static,
    ) -> Self {
        let mut fs = Self {
            shared: Vec::new(),
            shared_unknown: false,
            resolve: Box::new(resolve),
        };
        let mut keys = Vec::new();
        for dir in shared {
            let dir = normalize_win_path(&dir);
            match fs.resolve(&dir) {
                Resolution::Resolved(real) => keys.push(alias_key(&real)),
                Resolution::Missing => {}
                Resolution::Unknown => {
                    log::warn!(
                        "[uninstall] can't resolve shared folder {dir}; no folder counts as a product's own"
                    );
                    fs.shared_unknown = true;
                }
            }
            keys.push(alias_key(&dir));
        }
        keys.sort();
        keys.dedup();
        fs.shared = keys;
        fs
    }

    /// Where `p` really leads, normalised. A path that resolves somewhere
    /// Kage can't reason about (`is_absolute_win_path`) is `Unknown`.
    pub fn resolve(&self, p: &str) -> Resolution {
        match (self.resolve)(p) {
            Resolution::Resolved(r) => {
                let r = normalize_win_path(&r);
                if is_absolute_win_path(&r) {
                    Resolution::Resolved(r)
                } else {
                    Resolution::Unknown
                }
            }
            other => other,
        }
    }

    /// `p` as written (normalised) and, if it resolves, where it leads.
    fn forms(&self, p: &str) -> Vec<String> {
        let p = normalize_win_path(p);
        let resolved = match self.resolve(&p) {
            Resolution::Resolved(r) if r != p => Some(r),
            _ => None,
        };
        std::iter::once(p).chain(resolved).collect()
    }

    /// Whether normalised `dir`, read through `alias_key`, is a drive or
    /// share root, one of the shared folders, or a folder holding one.
    fn names_shared(&self, dir: &str) -> bool {
        let k = alias_key(dir);
        is_volume_root(&k) || self.shared.iter().any(|s| within(s, &k))
    }

    /// What's known about whether `dir` is a shared folder — never one
    /// product's own folder, so a product "inside" it says nothing about
    /// which product that is, and it's never protected as one product's
    /// tree. Decided for `dir` as written and where it resolves, each read
    /// through `alias_key`, so no spelling of a shared folder —
    /// `Downloads.`, `DOWNLO~1`, a junction to it — counts as a product's
    /// own. A folder that isn't shared as far as Kage can tell is still
    /// `Unknown`, not `NotShared`, if it doesn't resolve or a shared
    /// folder's real location is unknown.
    pub fn sharing(&self, dir: &str) -> Sharing {
        let written = normalize_win_path(dir);
        let real = self.resolve(&written);
        if self.names_shared(&written)
            || matches!(&real, Resolution::Resolved(r) if self.names_shared(r))
        {
            return Sharing::Shared;
        }
        match real {
            Resolution::Resolved(_) if !self.shared_unknown => Sharing::NotShared,
            _ => Sharing::Unknown,
        }
    }

    /// Whether `dir` is known to be shared (`Sharing::Shared`). Only for
    /// discarding evidence: a folder this can't decide isn't shared by
    /// this test. Anything that authorizes or protects uses `is_unshared`.
    pub fn is_shared(&self, dir: &str) -> bool {
        self.sharing(dir) == Sharing::Shared
    }

    /// Whether `dir` is established as not shared (`Sharing::NotShared`):
    /// the only folders that can verify an uninstaller or be protected as
    /// one product's tree.
    pub fn is_unshared(&self, dir: &str) -> bool {
        self.sharing(dir) == Sharing::NotShared
    }

    /// Whether `p`, as written or where it resolves, is one of `roots`
    /// (normalised) or inside one.
    pub fn is_within_any(&self, p: &str, roots: &[String]) -> bool {
        self.forms(p)
            .iter()
            .any(|p| roots.iter().any(|r| within(p, r)))
    }
}

/// A normalised absolute path from a registry value, or `None` if it isn't
/// one Kage can reason about.
fn location(raw: &str) -> Option<String> {
    let p = normalize_win_path(raw);
    is_absolute_win_path(&p).then_some(p)
}

/// The entry's `InstallLocation`, if it's a usable absolute path (shared
/// or not).
fn install_location(e: &UninstallEntry) -> Option<String> {
    e.install_location.as_deref().and_then(location)
}

/// Each entry's `InstallLocation` that names one product's own folder
/// (normalised, deduplicated) — every entry, removable or not: they're
/// all installed software. Only folders established as not shared
/// (`WinFs::is_unshared`) count, so a sloppy `InstallLocation` of `C:\`
/// or `Downloads.` can't claim the whole drive or Downloads, and neither
/// can a folder Kage can't resolve, or any folder on a machine where a
/// shared folder's real location is unknown.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn product_install_dirs(entries: &[UninstallEntry], fs: &WinFs) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for loc in entries.iter().filter_map(install_location) {
        if !out.contains(&loc) && fs.is_unshared(&loc) {
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

/// Distinctive words (3+ letters/digits, not "the"/"and"/"for") of a
/// product name.
fn name_tokens(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !matches!(*w, "the" | "and" | "for"))
        .map(str::to_string)
        .collect()
}

/// The program an `UninstallString` runs, for *locating* the product
/// only: a quoted program, else everything up to the first `.exe`, else
/// the first word.
fn uninstaller_program(cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    if let Some(quoted) = cmd.strip_prefix('"') {
        return quoted.split_once('"').map(|(p, _)| p.to_string());
    }
    if let Some(idx) = cmd.to_ascii_lowercase().find(".exe") {
        return Some(cmd[..idx + 4].to_string());
    }
    cmd.split_whitespace().next().map(str::to_string)
}

/// The folder part of a normalised Windows path.
fn win_parent(path: &str) -> Option<&str> {
    path.rsplit_once('\\').map(|(dir, _)| dir)
}

/// Whether normalised `path` is `dir` or inside it.
fn within(path: &str, dir: &str) -> bool {
    path == dir || path.starts_with(&format!("{dir}\\"))
}

/// Where an entry says its product lives — `InstallLocation`, the folder
/// of `DisplayIcon`, the folder of the uninstaller — normalised, with
/// folders known to be shared dropped.
fn entry_locations(e: &UninstallEntry, fs: &WinFs) -> Vec<String> {
    let icon = e
        .display_icon
        .as_deref()
        .map(display_icon_path)
        .and_then(|p| location(&p));
    let uninstaller = e
        .uninstall_string
        .as_deref()
        .and_then(uninstaller_program)
        .and_then(|p| location(&p));
    let mut out: Vec<String> = Vec::new();
    let candidates = [
        install_location(e),
        icon.as_deref().and_then(win_parent).map(str::to_string),
        uninstaller
            .as_deref()
            .and_then(win_parent)
            .map(str::to_string),
    ];
    for loc in candidates.into_iter().flatten() {
        if !out.contains(&loc) && !fs.is_shared(&loc) {
            out.push(loc);
        }
    }
    out
}

/// An Uninstall entry matched to a launcher app.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub struct UninstallMatch<'e> {
    pub entry: &'e UninstallEntry,
    /// The product's own folder, as matching verified it: an entry
    /// location (not known to be shared) that contains the app's
    /// executable — `InstallLocation` when it qualifies, else the
    /// outermost of the icon's and the uninstaller's folders that does.
    /// `None` when the executable isn't known: nothing was verified.
    /// `plan_uninstall` still requires it be established as not shared
    /// before it authorizes anything.
    pub install_dir: Option<String>,
}

/// Pick the Uninstall entry for the app called `app_name` whose executable
/// is `target` (a shortcut's target, or the exe itself). `shortcut_args`:
/// the shortcut passes arguments to that executable — it may be a
/// launcher starting something else (a GOG or Steam game shortcut runs
/// the store client). Returns `None` unless exactly one eligible entry
/// matches with evidence — the caller then sends the user to Settings
/// rather than guess.
///
/// With the executable known, its location decides, and the names must
/// agree too:
///
/// - An entry whose `InstallLocation` is a usable path that doesn't
///   contain the exe places the product elsewhere: it's excluded,
///   whatever else agrees (a portable `E:\Portable\Tool\tool.exe` never
///   selects the product installed in `D:\Tool`).
/// - *Strong:* `DisplayIcon` is the exe, or `InstallLocation` is the exe's
///   own folder.
/// - *Weaker, used only without a strong match:* the exe sits inside one
///   of the entry's locations (`entry_locations`).
/// - Either way the names are equal or share a distinctive word — and
///   with `shortcut_args`, they must be equal.
///
/// A name alone never decides. A known executable whose path Kage can't
/// reason about (`is_absolute_win_path`) matches nothing. Only without
/// one — a shortcut that couldn't be resolved, `target` `None` — does an
/// exact `DisplayName` match count, and then no install folder is
/// verified.
///
/// Folders known to be shared, however they're spelled
/// (`WinFs::is_shared`), never count as a location, and duplicates (the
/// same product listed in the 32- and 64-bit views) collapse to one.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn match_uninstall_entry<'e>(
    app_name: &str,
    target: Option<&str>,
    shortcut_args: bool,
    entries: &'e [UninstallEntry],
    fs: &WinFs,
) -> Option<UninstallMatch<'e>> {
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

    let Some(target) = target else {
        return unique(candidates.into_iter().filter(|e| same_name(e)).collect()).map(|entry| {
            UninstallMatch {
                entry,
                install_dir: None,
            }
        });
    };
    // Known but not a path Kage can compare (a `%` in a folder name, a
    // volume GUID path): no evidence either way, so no match.
    let target = location(target)?;
    let target_dir = win_parent(&target).map(str::to_string);
    let app_tokens = name_tokens(app_name);
    let names_agree = |e: &UninstallEntry| {
        same_name(e)
            || (!shortcut_args
                && e.display_name.as_deref().is_some_and(|n| {
                    let tokens = name_tokens(n);
                    app_tokens.iter().any(|t| tokens.contains(t))
                }))
    };

    // Icons and uninstallers often sit in shared caches (`Windows\Installer`,
    // `Package Cache`), so they only ever count *for* a match.
    let contradicted =
        |e: &UninstallEntry| install_location(e).is_some_and(|l| !within(&target, &l));
    let located: Vec<(&UninstallEntry, Vec<String>)> = candidates
        .into_iter()
        .filter(|e| !contradicted(e) && names_agree(e))
        .map(|e| (e, entry_locations(e, fs)))
        .collect();

    let product_dir = |e: &UninstallEntry| install_location(e).filter(|l| !fs.is_shared(l));
    let strong: Vec<&UninstallEntry> = located
        .iter()
        .filter(|(e, _)| {
            let by_icon = e
                .display_icon
                .as_deref()
                .map(display_icon_path)
                .and_then(|p| location(&p))
                .as_deref()
                == Some(target.as_str());
            let by_dir = target_dir.is_some() && product_dir(e) == target_dir;
            by_icon || by_dir
        })
        .map(|(e, _)| *e)
        .collect();
    let entry = if strong.is_empty() {
        unique(
            located
                .iter()
                .filter(|(_, locs)| locs.iter().any(|l| within(&target, l)))
                .map(|(e, _)| *e)
                .collect(),
        )
    } else {
        unique(strong)
    }?;

    let install_dir = product_dir(entry).or_else(|| {
        entry_locations(entry, fs)
            .into_iter()
            .filter(|l| within(&target, l) && &target != l)
            .min_by_key(String::len)
    });
    Some(UninstallMatch { entry, install_dir })
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

/// The only arguments an uninstaller may be run with: the common forms
/// that open the app's own interactive uninstall (Spotify, Zoom, Squirrel
/// apps, …). Compared case-insensitively, as one whole argument.
pub const INTERACTIVE_UNINSTALL_ARGS: &[&str] = &["/uninstall", "--uninstall", "-uninstall", "/u"];

/// What Kage does to uninstall a matched Windows product.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub enum UninstallPlan {
    /// `msiexec.exe /x {product code}` — Windows Installer's own removal,
    /// interactive (no UI switch); the stored `UninstallString` isn't used.
    Msi { product_code: String },
    /// The product's own uninstaller, as the entry names it: no argument,
    /// or `arg`, one of `INTERACTIVE_UNINSTALL_ARGS`.
    Run {
        program: String,
        arg: Option<String>,
    },
    /// Settings → Installed apps.
    Settings,
}

/// A Windows Installer product code: `{8-4-4-4-12}` hex digits.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn product_code(key_name: &str) -> Option<String> {
    let inner = key_name.strip_prefix('{')?.strip_suffix('}')?;
    let groups: Vec<&str> = inner.split('-').collect();
    let shape = [8, 4, 4, 4, 12];
    (groups.len() == shape.len()
        && groups
            .iter()
            .zip(shape)
            .all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit())))
    .then(|| key_name.to_ascii_uppercase())
}

/// Split a stored `UninstallString` into the program and what follows:
/// `"C:\…\u.exe" rest`, or unquoted up to the first `.exe` that ends a
/// word (`C:\Program Files\App\u.exe rest`). `None` for anything else —
/// an unterminated quote, text glued to the closing quote, no `.exe`.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn split_uninstall_string(cmd: &str) -> Option<(String, String)> {
    let cmd = cmd.trim();
    if let Some(quoted) = cmd.strip_prefix('"') {
        let end = quoted.find('"')?;
        let rest = &quoted[end + 1..];
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        return Some((quoted[..end].to_string(), rest.trim().to_string()));
    }
    // ASCII lower-casing keeps byte offsets.
    let lower = cmd.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(".exe") {
        let end = from + i + 4;
        if end == cmd.len() || cmd[end..].starts_with(char::is_whitespace) {
            return Some((cmd[..end].to_string(), cmd[end..].trim().to_string()));
        }
        from = end;
    }
    None
}

/// Decide how to uninstall the matched product — an allowlist; anything
/// it doesn't recognise goes to Settings:
///
/// 1. A Windows Installer product (`WindowsInstaller = 1` and a
///    `{ProductCode}` key name): `msiexec.exe /x {product code}`,
///    whatever the stored `UninstallString` says.
/// 2. A plain executable (quoted or not) with no argument, or exactly one
///    of `INTERACTIVE_UNINSTALL_ARGS`, that `exists` and sits inside the
///    product's verified install folder (`UninstallMatch::install_dir`) —
///    as written and where both really lead (`WinFs::resolve`), so a
///    junction, 8.3 name or trailing dot can't move either one: the
///    resolved uninstaller must be inside the resolved folder, and both
///    the install folder and the folder the uninstaller resolves into must
///    be established as not shared (`WinFs::is_unshared`) — a folder that
///    doesn't resolve, or any folder while a shared folder's real location
///    is unknown, authorizes nothing. Both must resolve to a local drive:
///    a network path can lead back to this machine
///    (`\\localhost\c$\Users\me\Downloads` is Downloads) where nothing
///    shows it.
///
/// Every other argument, a value that only looks like a switch, a script,
/// `rundll32`, a path with `.` / `..`, an uninstaller in a shared cache
/// (`Package Cache`, `InstallShield Installation Information`), one that
/// doesn't resolve, one on a network share, one whose folders' sharing
/// can't be established, and anything unparseable: Settings.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn plan_uninstall(
    m: &UninstallMatch,
    fs: &WinFs,
    exists: impl Fn(&str) -> bool,
) -> UninstallPlan {
    let e = m.entry;
    if e.windows_installer {
        if let Some(product_code) = product_code(&e.key_name) {
            return UninstallPlan::Msi { product_code };
        }
    }
    let Some((program, rest)) = e
        .uninstall_string
        .as_deref()
        .and_then(split_uninstall_string)
    else {
        return UninstallPlan::Settings;
    };
    let arg = if rest.is_empty() {
        None
    } else if INTERACTIVE_UNINSTALL_ARGS
        .iter()
        .any(|a| a.eq_ignore_ascii_case(&rest))
    {
        Some(rest)
    } else {
        return UninstallPlan::Settings;
    };
    let normalized = normalize_win_path(&program);
    let Some(dir) = m.install_dir.as_deref() else {
        return UninstallPlan::Settings;
    };
    if !is_absolute_win_path(&normalized)
        || !normalized.ends_with(".exe")
        || !strictly_within(&normalized, dir)
        || win_parent(&normalized).is_some_and(|folder| fs.is_shared(folder))
        || !exists(&program)
    {
        return UninstallPlan::Settings;
    }
    // Where they really lead, compared exactly: the canonical forms carry
    // no aliases left to read through.
    let (Resolution::Resolved(real_dir), Resolution::Resolved(real_program)) =
        (fs.resolve(dir), fs.resolve(&program))
    else {
        return UninstallPlan::Settings;
    };
    if real_dir.starts_with(r"\\")
        || !strictly_within(&real_program, &real_dir)
        || !fs.is_unshared(dir)
        || !win_parent(&real_program).is_some_and(|folder| fs.is_unshared(folder))
    {
        return UninstallPlan::Settings;
    }
    UninstallPlan::Run { program, arg }
}

/// Whether normalised `path` is inside `dir`, not `dir` itself.
fn strictly_within(path: &str, dir: &str) -> bool {
    path != dir && within(path, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, uninstall: &str) -> UninstallEntry {
        UninstallEntry {
            key_name: name.into(),
            display_name: Some(name.into()),
            uninstall_string: Some(uninstall.into()),
            ..Default::default()
        }
    }

    fn at(loc: &str, e: UninstallEntry) -> UninstallEntry {
        UninstallEntry {
            install_location: Some(loc.into()),
            ..e
        }
    }

    /// This machine's shared folders, as `shared_dirs` lists them on
    /// Windows.
    const SHARED: &[&str] = &[
        r"C:\Program Files",
        r"C:\Program Files (x86)",
        r"C:\Program Files\Common Files",
        r"C:\ProgramData",
        r"C:\ProgramData\Package Cache",
        r"C:\Windows",
        r"C:\Windows\System32",
        r"C:\Windows\Installer",
        r"C:\Users",
        r"C:\Users\me",
        r"C:\Users\me\Downloads",
        r"C:\Users\me\OneDrive\Documents",
        r"C:\Users\me\AppData\Local",
        r"C:\Users\me\AppData\Local\Programs",
        r"C:\Users\me\AppData\Roaming",
    ];

    /// The machine with `SHARED`, a stand-in for Windows' `canonicalize`
    /// as the resolver: trailing dots and spaces dropped (as Win32 does
    /// when it opens a path), then each `links` prefix — an 8.3 name, a
    /// junction — replaced by where it leads. A path resolves only if
    /// something on `disk` is there: that file, or a folder holding one;
    /// otherwise it's `Missing`.
    fn fake_fs(links: &[(&str, &str)], disk: &[&str]) -> WinFs {
        failing_fs(links, disk, &[])
    }

    /// `fake_fs`, where resolving anything written inside one of `failing`
    /// fails (access denied, a link that can't be followed): `Unknown`.
    fn failing_fs(links: &[(&str, &str)], disk: &[&str], failing: &[&str]) -> WinFs {
        let key = |p: &str| alias_key(&normalize_win_path(p));
        let links: Vec<(String, String)> = links.iter().map(|(l, to)| (key(l), key(to))).collect();
        let disk: Vec<String> = disk.iter().map(|d| key(d)).collect();
        let failing: Vec<String> = failing.iter().map(|f| key(f)).collect();
        WinFs::new(SHARED.iter().map(|s| s.to_string()), move |p| {
            let mut real = key(p);
            if failing.iter().any(|f| within(&real, f)) {
                return Resolution::Unknown;
            }
            for (link, to) in &links {
                if within(&real, link) {
                    real = format!("{to}{}", &real[link.len()..]);
                }
            }
            if disk.iter().any(|d| within(d, &real)) {
                Resolution::Resolved(real)
            } else {
                Resolution::Missing
            }
        })
    }

    /// `SHARED`, with no links and nothing on disk: whether a folder is
    /// shared is read only as written, and none is established as not
    /// shared.
    fn shared() -> WinFs {
        fake_fs(&[], &[])
    }

    fn matched<'e>(
        name: &str,
        exe: Option<&str>,
        entries: &'e [UninstallEntry],
    ) -> Option<&'e UninstallEntry> {
        match_uninstall_entry(name, exe, false, entries, &shared()).map(|m| m.entry)
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

    // -- Kage's own installation -----------------------------------------

    /// Host paths, so `Path::parent` splits them on every OS.
    fn home() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Users\me")
        } else {
            PathBuf::from("/Users/me")
        }
    }

    #[test]
    fn an_installed_kage_folder_is_kage() {
        let dir = home().join("AppData").join("Local").join("Kage");
        let exe = dir.join("kage.exe");
        let present = [
            dir.join("uninstall.exe"),
            dir.join(format!("{SIDECAR_STEM}.exe")),
        ];
        let own = own_install_layout(&exe, false, true, |p| present.iter().any(|q| q == p));
        assert_eq!(own.root(), Some(dir.as_path()));
        assert_eq!(own.paths(), vec![dir]);
    }

    #[test]
    fn a_loose_kage_exe_protects_only_its_own_files() {
        let downloads = home().join("Downloads");
        let exe = downloads.join("kage.exe");
        let sidecar = downloads.join(format!("{SIDECAR_STEM}.exe"));
        // The sidecar next to it, but no installer layout.
        let own = own_install_layout(&exe, false, true, |p| p == sidecar);
        assert_eq!(own.root(), None);
        assert_eq!(own.paths(), vec![exe.clone(), sidecar.clone()]);
        // Someone else's uninstall.exe in Downloads isn't Kage's layout
        // without the sidecar.
        let other = downloads.join("uninstall.exe");
        let own = own_install_layout(&exe, false, true, |p| p == other);
        assert_eq!(own.paths(), vec![exe]);
    }

    #[test]
    fn a_mac_bundle_is_kage_and_a_dev_build_is_just_its_files() {
        let bundle = PathBuf::from("/Applications/Kage.app");
        let exe = bundle.join("Contents").join("MacOS").join("kage");
        let own = own_install_layout(&exe, true, false, |_| true);
        assert_eq!(own.paths(), vec![bundle]);
        // `cargo build`: target/debug holds every other build product.
        let debug = home().join("src").join("kage").join("target").join("debug");
        let exe = debug.join("kage");
        let sidecar = debug.join(SIDECAR_STEM);
        let own = own_install_layout(&exe, true, false, |p| p == sidecar);
        assert_eq!(own.root(), None);
        assert_eq!(own.paths(), vec![exe, sidecar]);
    }

    // -- Registry paths ------------------------------------------------

    #[test]
    fn normalizes_quotes_case_separators_and_verbatim_prefixes() {
        assert_eq!(
            normalize_win_path(r#""C:\Program Files\App\""#),
            r"c:\program files\app"
        );
        assert_eq!(normalize_win_path(r"C:/Apps//Tool/"), r"c:\apps\tool");
        assert_eq!(normalize_win_path(r"\\?\C:\X\y.exe"), r"c:\x\y.exe");
        assert_eq!(normalize_win_path(r"C:\"), "c:");
        // UNC: the verbatim form keeps its root.
        assert_eq!(
            normalize_win_path(r"\\?\UNC\server\share\Tool"),
            r"\\server\share\tool"
        );
        assert_eq!(
            normalize_win_path(r"\\Server\Share\\Tool\"),
            r"\\server\share\tool"
        );
        // A comma is part of the path: only DisplayIcon has an index.
        assert_eq!(normalize_win_path(r"D:\Tool,1"), r"d:\tool,1");
    }

    #[test]
    fn reads_the_icon_index_off_display_icon_only() {
        assert_eq!(
            display_icon_path(r#""C:\Program Files\App\App.exe",0"#),
            r"C:\Program Files\App\App.exe"
        );
        assert_eq!(display_icon_path(r"C:\X\y.exe,-101"), r"C:\X\y.exe");
        assert_eq!(display_icon_path(r"C:\X\y.exe"), r"C:\X\y.exe");
        assert_eq!(
            display_icon_path(r"C:\Foo, Inc\a.exe"),
            r"C:\Foo, Inc\a.exe"
        );
        assert_eq!(display_icon_path(r"C:\Foo,1\a.exe,0"), r"C:\Foo,1\a.exe");
    }

    #[test]
    fn only_plain_absolute_paths_are_locations() {
        for ok in [
            "c:",
            r"c:\tool",
            r"d:\tool\bin\tool.exe",
            r"\\server\share",
            r"\\server\share\tool",
        ] {
            assert!(is_absolute_win_path(ok), "{ok}");
        }
        for bad in [
            "",
            "tool",
            r"tool\bin",
            r"c:tool",
            r"\tool",
            r"\\server",
            r"\\.\c:\x",
            r"c:\program files\app\..\..\windows",
            r"c:\x\.\y",
            r"c:\x\a.txt:stream",
            r"c:\x\*",
            r"%programfiles%\app",
        ] {
            assert!(!is_absolute_win_path(bad), "{bad}");
        }
    }

    #[test]
    fn shared_folders_are_named_not_guessed_from_depth() {
        let g = shared();
        for shared_dir in [
            "c:",
            "d:",
            r"\\server\share",
            r"c:\program files",
            r"c:\windows",
            r"c:\users\me",
            r"c:\users\me\appdata\local",
            r"c:\users\me\downloads",
        ] {
            assert!(g.is_shared(shared_dir), "{shared_dir}");
        }
        for own in [
            r"d:\tool",
            r"c:\tools",
            r"\\server\share\tool",
            r"c:\program files\contoso",
        ] {
            assert!(!g.is_shared(own), "{own}");
        }
    }

    #[test]
    fn product_install_dirs_keep_one_level_folders_and_drop_shared_ones() {
        let x = || entry("X", "u.exe");
        let entries = vec![
            at(r"C:\Users\me\AppData\Local\Discord\", x()),
            at(r"c:\users\me\appdata\local\discord", x()),
            at(r"C:\Program Files\Contoso", x()),
            at(r"D:\Tool", x()),
            // Kage's own installer quotes it.
            at(r#""C:\Users\me\AppData\Local\Kage""#, x()),
            at(r"\\?\UNC\server\share\Tool", x()),
            at(r"C:\Program Files", x()),
            at(r"C:\", x()),
            at(r"\\server\share", x()),
            at(r"Tool", x()),
            at(r"%ProgramFiles%\Unexpanded", x()),
            at("", x()),
            UninstallEntry {
                system_component: true,
                ..at(r"C:\Program Files\Runtime\1.0", x())
            },
        ];
        let products = [
            r"c:\users\me\appdata\local\discord",
            r"c:\program files\contoso",
            r"d:\tool",
            r"c:\users\me\appdata\local\kage",
            r"\\server\share\tool",
            r"c:\program files\runtime\1.0",
        ];
        let disk: Vec<String> = products.iter().map(|p| format!(r"{p}\app.exe")).collect();
        let disk: Vec<&str> = disk.iter().map(String::as_str).collect();
        assert_eq!(
            product_install_dirs(&entries, &fake_fs(&[], &disk)),
            products
        );
        // Only folders that resolve are established as a product's own.
        assert_eq!(
            product_install_dirs(&entries, &shared()),
            Vec::<String>::new()
        );
    }

    // -- Matching --------------------------------------------------------

    #[test]
    fn matches_by_install_location_of_the_shortcut_target() {
        let entries = vec![
            at(
                r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\",
                entry(
                    "Microsoft Visual Studio Code (User)",
                    r#""C:\Users\me\AppData\Local\Programs\Microsoft VS Code\unins000.exe""#,
                ),
            ),
            entry("Git", r#""C:\Program Files\Git\unins000.exe""#),
        ];
        let m = match_uninstall_entry(
            "Visual Studio Code",
            Some(r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe"),
            false,
            &entries,
            &shared(),
        )
        .unwrap();
        assert_eq!(m.entry, &entries[0]);
        assert_eq!(
            m.install_dir.as_deref(),
            Some(r"c:\users\me\appdata\local\programs\microsoft vs code")
        );
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
        let m = match_uninstall_entry(
            "Notepad++",
            Some(r"C:\Program Files\Notepad++\notepad++.exe"),
            false,
            &entries,
            &shared(),
        )
        .unwrap();
        assert_eq!(m.entry, &entries[0]);
        assert_eq!(
            m.install_dir.as_deref(),
            Some(r"c:\program files\notepad++")
        );
        // No exe: the exact name decides, and no folder is verified.
        let m = match_uninstall_entry("spotify", None, false, &entries, &shared()).unwrap();
        assert_eq!(m.entry, &entries[1]);
        assert_eq!(m.install_dir, None);
    }

    #[test]
    fn a_one_level_install_folder_is_a_product_folder() {
        let entries = vec![at(r"D:\Tool", entry("Tool", r#""D:\Tool\unins000.exe""#))];
        let m = match_uninstall_entry(
            "Tool",
            Some(r"D:\Tool\tool.exe"),
            false,
            &entries,
            &shared(),
        )
        .unwrap();
        assert_eq!(m.install_dir.as_deref(), Some(r"d:\tool"));
    }

    #[test]
    fn a_portable_copy_never_selects_the_installed_product() {
        let entries = vec![at(
            r"D:\Tool",
            UninstallEntry {
                display_icon: Some(r"E:\Portable\Tool\tool.exe".into()),
                ..entry("Tool", r#""D:\Tool\unins000.exe""#)
            },
        )];
        assert_eq!(
            matched("Tool", Some(r"E:\Portable\Tool\tool.exe"), &entries),
            None
        );
        // UNC install folders work the same way.
        let unc = vec![at(
            r"\\?\UNC\server\share\Tool",
            entry("Tool", r#""\\server\share\Tool\unins000.exe""#),
        )];
        assert_eq!(
            matched("Tool", Some(r"E:\Portable\Tool\tool.exe"), &unc),
            None
        );
        assert_eq!(
            matched("Tool", Some(r"\\server\share\Tool\tool.exe"), &unc),
            Some(&unc[0])
        );
    }

    #[test]
    fn a_known_exe_kage_cant_read_matches_nothing() {
        // `%` is legal in a folder name; with the exe known but unusable,
        // the same-named MSI product elsewhere must not be picked by name.
        let msi = vec![at(
            r"D:\Tool",
            UninstallEntry {
                key_name: "{23170F69-40C1-2702-2301-000001000000}".into(),
                windows_installer: true,
                ..entry(
                    "Tool",
                    "MsiExec.exe /X{23170F69-40C1-2702-2301-000001000000}",
                )
            },
        )];
        for exe in [
            r"E:\Portable\100% Tool\tool.exe",
            r"\\?\Volume{0b1c2d3e-0000-0000-0000-100000000000}\Tool\tool.exe",
            r"Tool\tool.exe",
        ] {
            assert_eq!(matched("Tool", Some(exe), &msi), None, "{exe}");
        }
        // Only an unknown exe falls back to the exact name.
        assert_eq!(matched("Tool", None, &msi), Some(&msi[0]));
    }

    #[test]
    fn a_launcher_shortcut_never_uninstalls_the_launcher() {
        // "The Witcher 3.lnk" → GalaxyClient.exe /command=runGame …: the
        // client's own entry is a strong match by folder and icon.
        let galaxy = r"C:\Program Files (x86)\GOG Galaxy";
        let entries = vec![
            at(
                &format!(r"{galaxy}\"),
                UninstallEntry {
                    display_icon: Some(format!(r"{galaxy}\GalaxyClient.exe")),
                    ..entry("GOG GALAXY", &format!(r#""{galaxy}\unins000.exe""#))
                },
            ),
            at(
                &format!(r"{galaxy}\Games\The Witcher 3"),
                entry(
                    "The Witcher 3: Wild Hunt",
                    &format!(r#""{galaxy}\Games\The Witcher 3\unins000.exe""#),
                ),
            ),
        ];
        let client = format!(r"{galaxy}\GalaxyClient.exe");
        let pick = |name: &str, args: bool| {
            match_uninstall_entry(name, Some(&client), args, &entries, &shared()).map(|m| m.entry)
        };
        assert_eq!(pick("The Witcher 3", true), None);
        // No shared word: no match even without arguments.
        assert_eq!(pick("The Witcher 3", false), None);
        // The client's own shortcut still matches it.
        assert_eq!(pick("GOG GALAXY", true), Some(&entries[0]));
        assert_eq!(pick("GOG Galaxy 2.0", false), Some(&entries[0]));
        // With arguments the names must be equal, not just share a word
        // ("GOG Galaxy Game" → the client, started with a game id).
        assert_eq!(pick("GOG Galaxy Game", true), None);
    }

    #[test]
    fn a_squirrel_shortcut_with_arguments_still_matches_its_app() {
        // Discord.lnk → Update.exe --processStart Discord.exe
        let entries = vec![at(
            r"C:\Users\me\AppData\Local\Discord",
            entry(
                "Discord",
                r#""C:\Users\me\AppData\Local\Discord\Update.exe" --uninstall"#,
            ),
        )];
        let m = match_uninstall_entry(
            "Discord",
            Some(r"C:\Users\me\AppData\Local\Discord\Update.exe"),
            true,
            &entries,
            &shared(),
        )
        .unwrap();
        assert_eq!(m.entry, &entries[0]);
        let fs = fake_fs(&[], &[r"C:\Users\me\AppData\Local\Discord\Update.exe"]);
        assert_eq!(
            plan_uninstall(&m, &fs, |_| true),
            run(
                r"C:\Users\me\AppData\Local\Discord\Update.exe",
                Some("--uninstall")
            )
        );
    }

    #[test]
    fn shared_install_locations_never_match() {
        let entries = vec![at(
            r"C:\Program Files",
            entry("Some Runtime", "MsiExec.exe /X{1}"),
        )];
        assert_eq!(
            matched("Paint", Some(r"C:\Program Files\paint.exe"), &entries),
            None
        );
    }

    #[test]
    fn ambiguous_matches_fall_back_to_settings() {
        let loc = r"C:\Program Files\Microsoft Office\root\Office16";
        let entries = vec![
            at(
                loc,
                entry(
                    "Microsoft 365 Apps",
                    "OfficeClickToRun.exe scenario=install",
                ),
            ),
            at(
                loc,
                entry(
                    "Microsoft OneNote",
                    "OfficeClickToRun.exe scenario=install productstoremove=OneNote",
                ),
            ),
        ];
        assert_eq!(
            matched("Word", Some(&format!(r"{loc}\WINWORD.EXE")), &entries),
            None
        );
    }

    #[test]
    fn deeper_location_needs_a_shared_name_word() {
        let entries = vec![at(
            r"C:\Program Files\JetBrains\IntelliJ IDEA 2024.1",
            entry(
                "IntelliJ IDEA 2024.1",
                r#""C:\Program Files\JetBrains\IntelliJ IDEA 2024.1\bin\Uninstall.exe""#,
            ),
        )];
        let exe = r"C:\Program Files\JetBrains\IntelliJ IDEA 2024.1\bin\idea64.exe";
        assert_eq!(
            matched("IntelliJ IDEA Ultimate", Some(exe), &entries),
            Some(&entries[0])
        );
        assert_eq!(matched("Code With Me", Some(exe), &entries), None);
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
        assert_eq!(matched("Contoso Tool", None, &entries), None);
        // The same product in both registry views is one match, not two.
        let twice = vec![base.clone(), base.clone()];
        assert_eq!(matched("Contoso Tool", None, &twice), Some(&twice[0]));
    }

    #[test]
    fn a_shared_name_never_overrides_where_the_exe_is() {
        let installed = at(
            r"C:\Program Files\Contoso",
            UninstallEntry {
                display_icon: Some(r"C:\Program Files\Contoso\tool.exe,0".into()),
                ..entry("Tool", r#""C:\Program Files\Contoso\unins000.exe""#)
            },
        );
        let entries = vec![installed];
        assert_eq!(
            matched("Tool", Some(r"D:\Portable\Tool\tool.exe"), &entries),
            None
        );
        assert_eq!(
            matched("Tool", Some(r"C:\Program Files\Contoso\tool.exe"), &entries),
            Some(&entries[0])
        );
    }

    #[test]
    fn with_a_known_exe_a_name_alone_is_not_enough() {
        let entries = vec![entry("Contoso Tool", "MsiExec.exe /X{1}")];
        assert_eq!(
            matched(
                "Contoso Tool",
                Some(r"C:\Program Files\Contoso\tool.exe"),
                &entries
            ),
            None
        );
        assert_eq!(matched("Contoso Tool", None, &entries), Some(&entries[0]));
    }

    #[test]
    fn the_uninstallers_folder_is_evidence_with_a_matching_name() {
        // NSIS without InstallLocation: uninstall.exe beside the app.
        let entries = vec![entry(
            "Contoso Tool 2.1",
            r#""C:\Program Files\Contoso Tool\uninstall.exe""#,
        )];
        let exe = r"C:\Program Files\Contoso Tool\bin\tool.exe";
        let m =
            match_uninstall_entry("Contoso Tool", Some(exe), false, &entries, &shared()).unwrap();
        assert_eq!(
            m.install_dir.as_deref(),
            Some(r"c:\program files\contoso tool")
        );
        assert_eq!(matched("Paint", Some(exe), &entries), None);
        // A cache folder (WiX Burn's Package Cache) proves nothing.
        let cached = vec![entry(
            "Contoso Tool",
            r#""C:\ProgramData\Package Cache\{AB-12}\setup.exe" /uninstall"#,
        )];
        assert_eq!(matched("Contoso Tool", Some(exe), &cached), None);
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

    // -- The allowlist ---------------------------------------------------

    #[test]
    fn product_codes_are_braced_guids() {
        assert_eq!(
            product_code("{23170f69-40c1-2702-2301-000001000000}").as_deref(),
            Some("{23170F69-40C1-2702-2301-000001000000}")
        );
        for not_one in [
            "",
            "Kage",
            "{23170F69-40C1-2702-2301-000001000000}_is1",
            "23170F69-40C1-2702-2301-000001000000",
            "{23170F69-40C1-2702-2301-00000100000}",
            "{23170F69-40C1-2702-2301-00000100000G}",
            "{AB-12}",
        ] {
            assert_eq!(product_code(not_one), None, "{not_one}");
        }
    }

    #[test]
    fn splits_stored_uninstall_strings() {
        let split = |s: &str| split_uninstall_string(s);
        assert_eq!(
            split(r#""C:\Program Files\App\u.exe" /uninstall"#),
            Some((r"C:\Program Files\App\u.exe".into(), "/uninstall".into()))
        );
        assert_eq!(
            split(r"C:\Program Files\App\uninstall.exe --uninstall"),
            Some((
                r"C:\Program Files\App\uninstall.exe".into(),
                "--uninstall".into()
            ))
        );
        assert_eq!(
            split(r"C:\Tools\foo.exe.d\unins000.exe"),
            Some((r"C:\Tools\foo.exe.d\unins000.exe".into(), String::new()))
        );
        assert_eq!(split(r#""C:\App\u.exe"/uninstall"#), None);
        assert_eq!(split(r#""unterminated"#), None);
        assert_eq!(split(r"C:\App\uninstall.bat"), None);
        assert_eq!(split("   "), None);
    }

    /// The decision for one stored entry, the exe found at `exe`; every
    /// path in `on_disk` exists.
    fn plan_for(e: UninstallEntry, exe: Option<&str>, on_disk: &[&str]) -> UninstallPlan {
        plan_on(&fake_fs(&[], on_disk), e, exe, on_disk)
    }

    /// `plan_for` on the machine `fs`.
    fn plan_on(
        fs: &WinFs,
        e: UninstallEntry,
        exe: Option<&str>,
        on_disk: &[&str],
    ) -> UninstallPlan {
        let name = e.display_name.clone().unwrap_or_default();
        let entries = [e];
        let Some(m) = match_uninstall_entry(&name, exe, false, &entries, fs) else {
            return UninstallPlan::Settings;
        };
        plan_uninstall(&m, fs, |p| {
            on_disk.iter().any(|d| d.eq_ignore_ascii_case(p))
        })
    }

    fn run(program: &str, arg: Option<&str>) -> UninstallPlan {
        UninstallPlan::Run {
            program: program.into(),
            arg: arg.map(str::to_string),
        }
    }

    #[test]
    fn windows_installer_products_run_msiexec_whatever_the_stored_string() {
        let code = "{23170F69-40C1-2702-2301-000001000000}";
        let msi = |uninstall: &str| UninstallEntry {
            key_name: code.into(),
            windows_installer: true,
            ..entry("7-Zip", uninstall)
        };
        let want = UninstallPlan::Msi {
            product_code: code.into(),
        };
        for stored in [
            &format!("MsiExec.exe /I{code}"),
            &format!("MsiExec.exe /X{code}"),
            &format!("MsiExec.exe /X{code} /qn"),
            &format!("MsiExec.exe /X{code} REBOOT=ReallySuppress /passive"),
        ] {
            assert_eq!(plan_for(msi(stored), None, &[]), want, "{stored}");
        }
        // The flag without a product-code key name, or a GUID key without
        // the flag (InstallShield and others use GUID keys too): no msiexec.
        let no_code = UninstallEntry {
            key_name: "7-Zip".into(),
            ..msi(&format!("MsiExec.exe /X{code}"))
        };
        assert_eq!(plan_for(no_code, None, &[]), UninstallPlan::Settings);
        let no_flag = UninstallEntry {
            windows_installer: false,
            ..msi(&format!("MsiExec.exe /X{code}"))
        };
        assert_eq!(plan_for(no_flag, None, &[]), UninstallPlan::Settings);
    }

    #[test]
    fn plain_uninstallers_in_the_product_folder_are_allowed() {
        let cases: Vec<(UninstallEntry, &str, UninstallPlan)> = vec![
            // Inno Setup (VS Code, Git): no argument.
            (
                at(
                    r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\",
                    entry(
                        "Microsoft Visual Studio Code (User)",
                        r#""C:\Users\me\AppData\Local\Programs\Microsoft VS Code\unins000.exe""#,
                    ),
                ),
                r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe",
                run(
                    r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\unins000.exe",
                    None,
                ),
            ),
            (
                at(
                    r"C:\Program Files\Git\",
                    entry("Git", r#""C:\Program Files\Git\unins000.exe""#),
                ),
                r"C:\Program Files\Git\git-bash.exe",
                run(r"C:\Program Files\Git\unins000.exe", None),
            ),
            // NSIS without InstallLocation (Notepad++), unquoted.
            (
                UninstallEntry {
                    display_icon: Some(r"C:\Program Files\Notepad++\notepad++.exe".into()),
                    ..entry("Notepad++", r"C:\Program Files\Notepad++\uninstall.exe")
                },
                r"C:\Program Files\Notepad++\notepad++.exe",
                run(r"C:\Program Files\Notepad++\uninstall.exe", None),
            ),
            // Firefox: a helper in a subfolder of the install folder.
            (
                at(
                    r"C:\Program Files\Mozilla Firefox",
                    entry(
                        "Mozilla Firefox",
                        r#""C:\Program Files\Mozilla Firefox\uninstall\helper.exe""#,
                    ),
                ),
                r"C:\Program Files\Mozilla Firefox\firefox.exe",
                run(
                    r"C:\Program Files\Mozilla Firefox\uninstall\helper.exe",
                    None,
                ),
            ),
            // Kage-style NSIS: quoted InstallLocation.
            (
                at(
                    r#""C:\Users\me\AppData\Local\Contoso""#,
                    entry(
                        "Contoso",
                        r#""C:\Users\me\AppData\Local\Contoso\uninstall.exe""#,
                    ),
                ),
                r"C:\Users\me\AppData\Local\Contoso\contoso.exe",
                run(r"C:\Users\me\AppData\Local\Contoso\uninstall.exe", None),
            ),
            // One allowlisted argument: Spotify (unquoted), Squirrel, Zoom.
            (
                at(
                    r"C:\Users\me\AppData\Roaming\Spotify",
                    entry(
                        "Spotify",
                        r"C:\Users\me\AppData\Roaming\Spotify\Spotify.exe /uninstall",
                    ),
                ),
                r"C:\Users\me\AppData\Roaming\Spotify\Spotify.exe",
                run(
                    r"C:\Users\me\AppData\Roaming\Spotify\Spotify.exe",
                    Some("/uninstall"),
                ),
            ),
            (
                at(
                    r"C:\Users\me\AppData\Local\Discord",
                    entry(
                        "Discord",
                        r#""C:\Users\me\AppData\Local\Discord\Update.exe" --uninstall"#,
                    ),
                ),
                r"C:\Users\me\AppData\Local\Discord\app-1.0.9\Discord.exe",
                run(
                    r"C:\Users\me\AppData\Local\Discord\Update.exe",
                    Some("--uninstall"),
                ),
            ),
            (
                at(
                    r"C:\Users\me\AppData\Roaming\Zoom",
                    entry(
                        "Zoom Workplace",
                        r#""C:\Users\me\AppData\Roaming\Zoom\uninstall\Installer.exe" /Uninstall"#,
                    ),
                ),
                r"C:\Users\me\AppData\Roaming\Zoom\bin\Zoom.exe",
                run(
                    r"C:\Users\me\AppData\Roaming\Zoom\uninstall\Installer.exe",
                    Some("/Uninstall"),
                ),
            ),
            (
                at(
                    r"D:\Tool",
                    entry("Tool", r#""D:\Tool\uninst.exe" -uninstall"#),
                ),
                r"D:\Tool\tool.exe",
                run(r"D:\Tool\uninst.exe", Some("-uninstall")),
            ),
            (
                at(r"D:\Tool", entry("Tool", r"D:\Tool\uninst.exe /U")),
                r"D:\Tool\tool.exe",
                run(r"D:\Tool\uninst.exe", Some("/U")),
            ),
        ];
        for (e, exe, want) in cases {
            let UninstallPlan::Run { program, .. } = &want else {
                unreachable!()
            };
            let program = program.clone();
            let stored = e.uninstall_string.clone().unwrap_or_default();
            assert_eq!(plan_for(e, Some(exe), &[&program]), want, "{stored}");
        }
    }

    #[test]
    fn everything_else_goes_to_settings() {
        let dir = r"C:\Program Files\App";
        let exe = r"C:\Program Files\App\app.exe";
        let u = r"C:\Program Files\App\uninstall.exe";
        let on_disk = [u, r"C:\Program Files\App\uninstall.bat"];
        for stored in [
            // Silent and unattended switches, every vendor's spelling.
            format!(r#""{u}" /S"#),
            format!(r#""{u}" /SILENT"#),
            format!(r#""{u}" /VERYSILENT /SUPPRESSMSGBOXES"#),
            format!(r#""{u}" -s"#),
            format!(r#""{u}" --silent"#),
            format!(r#""{u}" --uninstall -s"#),
            format!(r#""{u}" /uninstall /S"#),
            format!(r#""{u}" /uninstall /quiet"#),
            format!(r#""{u}" -ms"#),
            format!(r#""{u}" --force-uninstall"#),
            format!(r#""{u}" --mode unattended"#),
            // MSI UI levels forwarded through a wrapper, nested and glued.
            format!(r#""{u}" /s /v"/qn""#),
            format!(r#""{u}" -v"/qn""#),
            format!(r#""{u}" /v"-qn""#),
            format!(r#""{u}" /x {{AB-12}} /qn+"#),
            format!(r#""{u}"/uninstall"#),
            // Ordinary arguments are refused too: the allowlist doesn't
            // try to read them.
            format!(r#""{u}" /log "C:/S/uninstall.log""#),
            format!(r#""{u}" quiet"#),
            format!(r#""{u}" "/uninstall""#),
            format!(r#""{u}" _?={dir}"#),
            format!(r#""{u}" /uninstall /norestart"#),
            format!(r#""{u}" --uninstall --channel=stable --system-level"#),
            format!(r#""{u}" -runfromtemp -l0x0409 -removeonly"#),
            format!(r#""{u}" /D={dir}"#),
            format!(r#"{u} /x"#),
            // Not a plain executable in the product folder.
            format!(r#""{dir}\uninstall.bat""#),
            format!(r#""{dir}\..\..\Windows\System32\cmd.exe""#),
            r#""C:\Windows\System32\cmd.exe" /c rd /s /q C:\Program Files\App"#.to_string(),
            r#"rundll32.exe "C:\Program Files\App\uninst.dll",Uninstall"#.to_string(),
            r#""C:\ProgramData\Package Cache\{AB-12}\setup.exe" /uninstall"#.to_string(),
            r#""C:\Program Files (x86)\InstallShield Installation Information\{AB-12}\setup.exe""#
                .to_string(),
            r"%ProgramFiles%\App\uninstall.exe".to_string(),
            "MsiExec.exe /X{23170F69-40C1-2702-2301-000001000000}".to_string(),
            format!(r#""{u}"#),
            String::new(),
        ] {
            let e = at(dir, entry("App", &stored));
            assert_eq!(
                plan_for(e, Some(exe), &on_disk),
                UninstallPlan::Settings,
                "{stored}"
            );
        }
    }

    #[test]
    fn an_allowed_uninstaller_must_exist_and_the_folder_must_be_verified() {
        let e = || at(r"D:\Tool", entry("Tool", r#""D:\Tool\uninst.exe""#));
        assert_eq!(
            plan_for(e(), Some(r"D:\Tool\tool.exe"), &[]),
            UninstallPlan::Settings
        );
        // Without the app's exe nothing verified the folder.
        assert_eq!(
            plan_for(e(), None, &[r"D:\Tool\uninst.exe"]),
            UninstallPlan::Settings
        );
        assert_eq!(
            plan_for(e(), Some(r"D:\Tool\tool.exe"), &[r"D:\Tool\uninst.exe"]),
            run(r"D:\Tool\uninst.exe", None)
        );
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

    // -- Aliases: trailing dots and spaces, 8.3 names, junctions ---------

    #[test]
    fn alias_key_drops_the_trailing_dots_and_spaces_windows_ignores() {
        for (p, key) in [
            (r"c:\users\me\downloads.", r"c:\users\me\downloads"),
            (r"c:\users\me\downloads ", r"c:\users\me\downloads"),
            (r"c:\users\me\downloads. .", r"c:\users\me\downloads"),
            (r"c:\users\me.\downloads..", r"c:\users\me\downloads"),
            (
                r"c:\users\me\downloads.\tool.exe",
                r"c:\users\me\downloads\tool.exe",
            ),
            (
                r"c:\users\me\downloads \tool.exe",
                r"c:\users\me\downloads\tool.exe",
            ),
            (r"\\server\share.\tool", r"\\server\share\tool"),
            (r"c:\program files\app", r"c:\program files\app"),
            ("c:", "c:"),
            // Only the file system knows an 8.3 name.
            (r"c:\users\me\downlo~1", r"c:\users\me\downlo~1"),
            // Nothing left to trim to: kept, and refused as a location.
            (r"c:\x\...", r"c:\x\..."),
        ] {
            assert_eq!(alias_key(p), key, "{p}");
        }
        for bad in [
            r"c:\x\...",
            r"c:\x\...\y",
            r"c:\ \x",
            r"c:\x\. .",
            r"\\server\share\ ",
        ] {
            assert!(!is_absolute_win_path(bad), "{bad}");
        }
    }

    #[test]
    fn every_spelling_of_a_shared_folder_is_shared() {
        let fs = fake_fs(
            &[
                (r"C:\Users\me\DOWNLO~1", r"C:\Users\me\Downloads"),
                (r"D:\Tool", r"C:\Users\me\Downloads"),
                (r"D:\Root", r"E:\"),
                (r"D:\Docs", r"C:\Users\me\OneDrive"),
            ],
            &[
                r"C:\Users\me\Downloads\a.txt",
                r"C:\Users\me\OneDrive\Documents\b.txt",
                r"E:\c.txt",
            ],
        );
        for dir in [
            r"C:\Users\me\Downloads",
            r"C:\Users\me\Downloads.",
            r"C:\Users\me\Downloads \",
            r"C:\Users\me\Downloads. .",
            r"C:\USERS\ME.\DOWNLOADS",
            r"C:\Users\me\DOWNLO~1",
            // Junctions to Downloads, to a drive root.
            r"D:\Tool",
            r"D:\Root",
            // Holding a shared folder (Documents redirected into OneDrive),
            // directly or through a junction.
            r"C:\Users\me\OneDrive",
            r"D:\Docs",
        ] {
            assert!(fs.is_shared(dir), "{dir}");
        }
        for own in [
            r"C:\Users\me\Downloads\Tool",
            r"C:\Users\me\DOWNLO~1\Tool",
            r"D:\Tool\Sub",
            r"D:\Tools",
            r"C:\Users\me\OneDrive\Tool",
        ] {
            assert!(!fs.is_shared(own), "{own}");
        }
    }

    #[test]
    fn a_shared_folders_real_location_is_shared_too() {
        // Downloads moved to another drive, a junction left behind.
        let fs = fake_fs(&[(r"C:\Users\me\Downloads", r"D:\DL")], &[r"D:\DL\a.txt"]);
        assert!(fs.is_shared(r"D:\DL"));
        assert!(fs.is_shared(r"D:\DL."));
        assert!(!fs.is_shared(r"D:\DL\Tool"));
    }

    #[test]
    fn an_aliased_shared_folder_never_authorizes_an_uninstaller() {
        // Downloads spelled so the strings miss it, an app there, and
        // another app's uninstaller beside it.
        let fs = fake_fs(
            &[
                (r"C:\Users\me\DOWNLO~1", r"C:\Users\me\Downloads"),
                (r"D:\Tool", r"C:\Users\me\Downloads"),
            ],
            &[
                r"C:\Users\me\Downloads\Tool.exe",
                r"C:\Users\me\Downloads\OtherApp\uninstall.exe",
            ],
        );
        for (location, dir) in [
            (r"C:\Users\me\Downloads.", r"C:\Users\me\Downloads."),
            (r"C:\Users\me\Downloads \", r"C:\Users\me\Downloads "),
            (r"C:\Users\me\Downloads. .", r"C:\Users\me\Downloads. ."),
            (r"C:\Users\me\DOWNLO~1", r"C:\Users\me\DOWNLO~1"),
            (r"D:\Tool", r"D:\Tool"),
        ] {
            let uninstaller = format!(r"{dir}\OtherApp\uninstall.exe");
            let e = at(location, entry("Tool", &format!(r#""{uninstaller}""#)));
            let entries = [e.clone()];
            // Not a protected product tree either.
            assert_eq!(
                product_install_dirs(&entries, &fs),
                Vec::<String>::new(),
                "{location}"
            );
            let exe = format!(r"{dir}\Tool.exe");
            assert_eq!(
                plan_on(&fs, e, Some(&exe), &[&uninstaller]),
                UninstallPlan::Settings,
                "{location}"
            );
        }
    }

    #[test]
    fn the_resolved_folder_and_uninstaller_are_checked_again() {
        // A match as the strings saw it; what's on disk decides.
        let e = at(r"D:\Tool", entry("Tool", r#""D:\Tool\bin\uninst.exe""#));
        let m = UninstallMatch {
            entry: &e,
            install_dir: Some(r"d:\tool".into()),
        };
        let plan = |links: &[(&str, &str)], disk: &[&str]| {
            plan_uninstall(&m, &fake_fs(links, disk), |_| true)
        };
        let ok = run(r"D:\Tool\bin\uninst.exe", None);
        assert_eq!(plan(&[], &[r"D:\Tool\bin\uninst.exe"]), ok);
        // A junction to an ordinary folder is fine.
        assert_eq!(
            plan(
                &[(r"D:\Tool", r"E:\Real Tool")],
                &[r"E:\Real Tool\bin\uninst.exe"]
            ),
            ok
        );
        for (links, disk) in [
            // The install folder is a junction to Downloads, to a root.
            (
                vec![(r"D:\Tool", r"C:\Users\me\Downloads")],
                vec![r"C:\Users\me\Downloads\bin\uninst.exe"],
            ),
            (vec![(r"D:\Tool", r"E:\")], vec![r"E:\bin\uninst.exe"]),
            // The uninstaller's folder resolves into a shared folder.
            (
                vec![(r"D:\Tool\bin", r"C:\Windows\System32")],
                vec![r"D:\Tool\tool.exe", r"C:\Windows\System32\uninst.exe"],
            ),
            // The uninstaller itself leads out of the folder.
            (
                vec![(r"D:\Tool\bin\uninst.exe", r"E:\Other\uninst.exe")],
                vec![r"D:\Tool\tool.exe", r"E:\Other\uninst.exe"],
            ),
            // Nothing resolves.
            (vec![], vec![]),
        ] {
            assert_eq!(plan(&links, &disk), UninstallPlan::Settings, "{links:?}");
        }
    }

    #[test]
    fn uninstallers_on_a_network_share_go_to_settings() {
        // `\\localhost\c$` is this machine's C: — nothing resolves that.
        for dir in [r"\\localhost\c$\Users\me\Downloads", r"\\server\share\Tool"] {
            let uninstaller = format!(r"{dir}\uninst.exe");
            let exe = format!(r"{dir}\tool.exe");
            let e = at(dir, entry("Tool", &format!(r#""{uninstaller}""#)));
            let fs = fake_fs(&[], &[&uninstaller, &exe]);
            assert_eq!(
                plan_on(&fs, e, Some(&exe), &[&uninstaller]),
                UninstallPlan::Settings,
                "{dir}"
            );
        }
        // A mapped drive resolves to the share too.
        let fs = fake_fs(
            &[(r"Z:\", r"\\server\share")],
            &[r"\\server\share\Tool\uninst.exe"],
        );
        let e = at(r"Z:\Tool", entry("Tool", r#""Z:\Tool\uninst.exe""#));
        assert_eq!(
            plan_on(&fs, e, Some(r"Z:\Tool\tool.exe"), &[r"Z:\Tool\uninst.exe"]),
            UninstallPlan::Settings
        );
    }

    #[test]
    fn an_ordinary_product_folder_spelled_with_8dot3_names_still_uninstalls() {
        let fs = fake_fs(
            &[(r"C:\PROGRA~1", r"C:\Program Files")],
            &[
                r"C:\Program Files\Contoso\contoso.exe",
                r"C:\Program Files\Contoso\uninst.exe",
            ],
        );
        let e = at(
            r"C:\PROGRA~1\Contoso",
            entry("Contoso", r"C:\PROGRA~1\Contoso\uninst.exe"),
        );
        assert_eq!(
            plan_on(
                &fs,
                e,
                Some(r"C:\PROGRA~1\Contoso\contoso.exe"),
                &[r"C:\PROGRA~1\Contoso\uninst.exe"]
            ),
            run(r"C:\PROGRA~1\Contoso\uninst.exe", None)
        );
        // `C:\PROGRA~1` itself is Program Files.
        assert!(fs.is_shared(r"C:\PROGRA~1"));
    }

    #[test]
    fn kages_own_folder_is_recognised_however_its_spelled() {
        let own = vec![normalize_win_path(r"C:\Users\me\AppData\Local\Kage")];
        let fs = fake_fs(
            &[
                (
                    r"C:\Users\me\AppData\Local\KAGE~1",
                    r"C:\Users\me\AppData\Local\Kage",
                ),
                (r"D:\K", r"C:\Users\me\AppData\Local\Kage"),
            ],
            &[r"C:\Users\me\AppData\Local\Kage\kage.exe"],
        );
        for p in [
            r"C:\Users\me\AppData\Local\Kage",
            r"C:\Users\me\AppData\Local\Kage\kage.exe",
            r"C:\Users\me\AppData\Local\Kage.\kage.exe",
            r"C:\Users\me\AppData\Local\KAGE~1\kage.exe",
            r"D:\K\kage.exe",
        ] {
            assert!(fs.is_within_any(p, &own), "{p}");
        }
        for p in [r"C:\Users\me\AppData\Local\Kagex\kage.exe", r"D:\Kage.exe"] {
            assert!(!fs.is_within_any(p, &own), "{p}");
        }
    }

    // -- Failing closed: what can't be resolved authorizes nothing -------

    #[test]
    fn resolution_reads_canonicalize_results() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            Resolution::of(Ok(PathBuf::from(r"C:\Tool"))),
            Resolution::Resolved(r"C:\Tool".into())
        );
        assert_eq!(
            Resolution::of(Err(Error::from(ErrorKind::NotFound))),
            Resolution::Missing
        );
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidInput,
            ErrorKind::Interrupted,
            ErrorKind::Other,
        ] {
            assert_eq!(
                Resolution::of(Err(Error::from(kind))),
                Resolution::Unknown,
                "{kind:?}"
            );
        }
        // A path that isn't valid Unicode can't be compared.
        #[cfg(unix)]
        let garbled = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![b'C', 0xff])
        };
        #[cfg(windows)]
        let garbled = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[u16::from(b'C'), 0xd800])
        };
        assert_eq!(
            Resolution::of(Ok(PathBuf::from(garbled))),
            Resolution::Unknown
        );
    }

    #[test]
    fn leading_somewhere_kage_cant_reason_about_is_unknown() {
        let volume = r"\\?\Volume{0b1c2d3e-0000-0000-0000-100000000000}\Tool";
        let fs = WinFs::new(Vec::new(), move |_| Resolution::Resolved(volume.into()));
        assert_eq!(fs.resolve(r"D:\Tool"), Resolution::Unknown);
        assert_eq!(fs.sharing(r"D:\Tool"), Sharing::Unknown);
        // A shared folder leading there leaves every folder undecided.
        let fs = WinFs::new(vec![r"C:\Users\me\Downloads".to_string()], move |p| {
            if normalize_win_path(p).contains("downloads") {
                Resolution::Resolved(volume.into())
            } else {
                Resolution::Resolved(p.into())
            }
        });
        assert_eq!(fs.sharing(r"D:\Tool"), Sharing::Unknown);
        assert_eq!(fs.sharing(r"C:\Users\me\Downloads"), Sharing::Shared);
    }

    #[test]
    fn a_shared_folder_that_cant_be_resolved_leaves_every_folder_undecided() {
        // Downloads is a junction to D:\DL; resolving the configured path
        // fails, while D:\DL itself resolves fine.
        let links = [(r"C:\Users\me\Downloads", r"D:\DL")];
        let disk = [
            r"D:\DL\Tool.exe",
            r"D:\DL\OtherApp\uninst.exe",
            r"D:\Tool\tool.exe",
            r"D:\Tool\uninst.exe",
        ];
        let fs = failing_fs(&links, &disk, &[r"C:\Users\me\Downloads"]);
        assert_eq!(fs.resolve(r"D:\DL"), Resolution::Resolved(r"d:\dl".into()));
        // Nothing shows D:\DL is Downloads, so nothing is established.
        assert!(!fs.is_shared(r"D:\DL"));
        for dir in [r"D:\DL", r"D:\DL\OtherApp", r"D:\Tool"] {
            assert_eq!(fs.sharing(dir), Sharing::Unknown, "{dir}");
        }
        // An entry claiming D:\DL, with another app's uninstaller there.
        let sloppy = at(r"D:\DL", entry("Tool", r#""D:\DL\OtherApp\uninst.exe""#));
        let tool = at(r"D:\Tool", entry("Tool", r#""D:\Tool\uninst.exe""#));
        let entries = [sloppy.clone(), tool.clone()];
        assert_eq!(product_install_dirs(&entries, &fs), Vec::<String>::new());
        let m = UninstallMatch {
            entry: &sloppy,
            install_dir: Some(r"d:\dl".into()),
        };
        assert_eq!(plan_uninstall(&m, &fs, |_| true), UninstallPlan::Settings);
        assert_eq!(
            plan_on(
                &fs,
                sloppy.clone(),
                Some(r"D:\DL\Tool.exe"),
                &[r"D:\DL\OtherApp\uninst.exe"]
            ),
            UninstallPlan::Settings
        );
        // An ordinary product goes to Settings too until it can be told
        // apart from Downloads.
        assert_eq!(
            plan_on(
                &fs,
                tool.clone(),
                Some(r"D:\Tool\tool.exe"),
                &[r"D:\Tool\uninst.exe"]
            ),
            UninstallPlan::Settings
        );

        // With Downloads resolving, D:\DL is Downloads and D:\Tool a
        // product of its own.
        let fs = fake_fs(&links, &disk);
        assert!(fs.is_shared(r"D:\DL"));
        assert_eq!(product_install_dirs(&entries, &fs), vec![r"d:\tool"]);
        assert_eq!(plan_uninstall(&m, &fs, |_| true), UninstallPlan::Settings);
        assert_eq!(
            plan_on(
                &fs,
                tool,
                Some(r"D:\Tool\tool.exe"),
                &[r"D:\Tool\uninst.exe"]
            ),
            run(r"D:\Tool\uninst.exe", None)
        );
    }

    #[test]
    fn a_shared_folder_with_nothing_there_is_compared_as_configured() {
        // Downloads configured, but missing: nothing resolves into it, and
        // the rest of the machine is decided as usual.
        let fs = fake_fs(&[], &[r"D:\Tool\uninst.exe"]);
        assert_eq!(fs.resolve(r"C:\Users\me\Downloads"), Resolution::Missing);
        assert!(fs.is_shared(r"C:\Users\me\Downloads."));
        assert!(fs.is_unshared(r"D:\Tool"));
    }

    #[test]
    fn a_candidate_alias_that_cant_be_resolved_is_no_products_own() {
        // `DOWNLO~1` is Downloads, but resolving it fails, or nothing is
        // there to resolve: as written, it isn't known to be shared.
        let dir = r"C:\Users\me\DOWNLO~1";
        let disk = [
            r"C:\Users\me\Downloads\Tool.exe",
            r"C:\Users\me\Downloads\OtherApp\uninst.exe",
        ];
        let links = [(dir, r"C:\Users\me\Downloads")];
        for fs in [failing_fs(&links, &disk, &[dir]), fake_fs(&[], &disk)] {
            assert!(!fs.is_shared(dir));
            assert_eq!(fs.sharing(dir), Sharing::Unknown);
            // The rest of the machine is decided.
            assert!(fs.is_unshared(r"C:\Users\me\Downloads\OtherApp"));
            let uninstaller = format!(r"{dir}\OtherApp\uninst.exe");
            let e = at(dir, entry("Tool", &format!(r#""{uninstaller}""#)));
            assert_eq!(
                product_install_dirs(std::slice::from_ref(&e), &fs),
                Vec::<String>::new()
            );
            let m = UninstallMatch {
                entry: &e,
                install_dir: Some(normalize_win_path(dir)),
            };
            assert_eq!(plan_uninstall(&m, &fs, |_| true), UninstallPlan::Settings);
            assert_eq!(
                plan_on(&fs, e, Some(&format!(r"{dir}\Tool.exe")), &[&uninstaller]),
                UninstallPlan::Settings
            );
        }
    }

    #[test]
    fn an_uninstaller_resolving_into_an_undecided_folder_goes_to_settings() {
        // Both resolve, the uninstaller inside the install folder, but the
        // folder the uninstaller resolves into can't be resolved itself.
        let e = at(r"D:\Tool", entry("Tool", r#""D:\Tool\bin\uninst.exe""#));
        let m = UninstallMatch {
            entry: &e,
            install_dir: Some(r"d:\tool".into()),
        };
        let links = [(r"D:\Tool", r"E:\Real Tool")];
        let disk = [r"E:\Real Tool\bin\uninst.exe"];
        let fs = failing_fs(&links, &disk, &[r"E:\Real Tool\bin"]);
        assert!(fs.is_unshared(r"D:\Tool"));
        assert_eq!(
            fs.resolve(r"D:\Tool\bin\uninst.exe"),
            Resolution::Resolved(r"e:\real tool\bin\uninst.exe".into())
        );
        assert_eq!(fs.sharing(r"e:\real tool\bin"), Sharing::Unknown);
        assert_eq!(plan_uninstall(&m, &fs, |_| true), UninstallPlan::Settings);
        // Once it resolves, the same uninstaller runs.
        let fs = fake_fs(&links, &disk);
        assert_eq!(
            plan_uninstall(&m, &fs, |_| true),
            run(r"D:\Tool\bin\uninst.exe", None)
        );
    }

    #[test]
    fn ordinary_products_still_uninstall_and_are_their_own_trees() {
        let fs = fake_fs(
            &[(r"C:\PROGRA~1", r"C:\Program Files")],
            &[
                r"D:\Tool\tool.exe",
                r"D:\Tool\uninst.exe",
                r"C:\Program Files\Contoso\contoso.exe",
                r"C:\Program Files\Contoso\uninst.exe",
                r"C:\Users\me\Downloads\old.zip",
            ],
        );
        for (name, loc, exe, uninstaller) in [
            (
                "Tool",
                r"D:\Tool",
                r"D:\Tool\tool.exe",
                r"D:\Tool\uninst.exe",
            ),
            (
                "Contoso",
                r"C:\Program Files\Contoso",
                r"C:\Program Files\Contoso\contoso.exe",
                r"C:\Program Files\Contoso\uninst.exe",
            ),
            (
                "Contoso",
                r"C:\PROGRA~1\Contoso",
                r"C:\PROGRA~1\Contoso\contoso.exe",
                r"C:\PROGRA~1\Contoso\uninst.exe",
            ),
        ] {
            assert!(fs.is_unshared(loc), "{loc}");
            let e = at(loc, entry(name, &format!(r#""{uninstaller}""#)));
            assert_eq!(
                product_install_dirs(std::slice::from_ref(&e), &fs),
                vec![normalize_win_path(loc)],
                "{loc}"
            );
            assert_eq!(
                plan_on(&fs, e, Some(exe), &[uninstaller]),
                run(uninstaller, None),
                "{loc}"
            );
        }
    }

    #[test]
    fn ordinary_downloads_files_stay_trashable() {
        use crate::launcher_actions::Protection;
        let links = [(r"C:\Users\me\Downloads", r"D:\DL")];
        let disk = [
            r"D:\DL\old.zip",
            r"D:\DL\OtherApp\uninst.exe",
            r"D:\Tool\tool.exe",
        ];
        // Entries claiming Downloads, however it's spelled, beside a real
        // product.
        let entries = [
            at(r"D:\DL", entry("Sloppy", r#""D:\DL\OtherApp\uninst.exe""#)),
            at(r"C:\Users\me\Downloads.", entry("Sloppy", "u.exe")),
            at(r"D:\Tool", entry("Tool", r#""D:\Tool\uninst.exe""#)),
        ];
        for (fs, tool_is_a_tree) in [
            (fake_fs(&links, &disk), true),
            (
                failing_fs(&links, &disk, &[r"C:\Users\me\Downloads"]),
                false,
            ),
        ] {
            let protection = Protection {
                app_trees: product_install_dirs(&entries, &fs)
                    .into_iter()
                    .map(PathBuf::from)
                    .collect(),
                ..Default::default()
            };
            for file in [r"D:\DL\old.zip", r"C:\Users\me\Downloads\old.zip"] {
                assert!(!protection.blocks_removal(Path::new(file), true), "{file}");
            }
            assert_eq!(
                protection.blocks_removal(Path::new(r"D:\Tool\tool.exe"), true),
                tool_is_a_tree
            );
        }
    }
}
