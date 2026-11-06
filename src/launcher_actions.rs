//! Right-click actions for launcher results (apps, files, folders).
//!
//! The webview names a result — `kind: "app"` with the app's display name,
//! or `kind: "path"` with a filesystem path — and never supplies a path to
//! act on for an app: apps are looked up in Kage's own app registry, and
//! paths are validated (absolute, existing, canonical parent) before
//! anything touches them. `actions_for` decides which actions a resolved
//! target offers on this OS; `launcher_result_action` re-resolves and
//! re-checks that list on every call, so a crafted request can't run an
//! action the menu wouldn't have shown.

use crate::error::{AppError, ErrorKind};
use crate::os::app_uninstall::{UninstallError, UninstallOutcome};
use crate::os::file_ops::TrashError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The product name; an app of this name is never offered for uninstall.
pub const KAGE_PRODUCT_NAME: &str = "Kage";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultAction {
    Open,
    RunAsAdmin,
    Reveal,
    CopyPath,
    CopyFile,
    CutFile,
    Trash,
    Uninstall,
    Properties,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    };

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::MacOs => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
        }
    }

    /// Whether paths compare case-insensitively (NTFS, APFS by default).
    /// Only used to *widen* protection, so erring towards `true` is safe.
    fn case_insensitive_paths(self) -> bool {
        !matches!(self, Platform::Linux)
    }
}

/// What an app registry entry's `path` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppForm {
    /// macOS `.app` bundle.
    Bundle,
    /// Windows Start Menu `.lnk`.
    Shortcut,
    /// A plain executable (Windows registry-found `.exe`, Linux binary).
    Executable,
    /// Linux `.desktop` entry.
    DesktopEntry,
    /// Windows Store/MSIX app (`shell:AppsFolder\<AUMID>`).
    Packaged,
    /// A URI pseudo-app (System Settings / `ms-settings:` pages).
    Uri,
}

impl AppForm {
    fn on_disk(self) -> bool {
        matches!(
            self,
            AppForm::Bundle | AppForm::Shortcut | AppForm::Executable | AppForm::DesktopEntry
        )
    }
}

/// `scheme:` with a 2+ character scheme (so `C:\…` isn't one).
fn looks_like_uri(s: &str) -> bool {
    s.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() >= 2
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

pub fn classify_app_path(path: &str, platform: Platform) -> AppForm {
    let lower = path.to_lowercase();
    match platform {
        Platform::Windows => {
            if lower.starts_with(r"shell:appsfolder\") {
                AppForm::Packaged
            } else if looks_like_uri(&lower) {
                AppForm::Uri
            } else if lower.ends_with(".lnk") {
                AppForm::Shortcut
            } else {
                AppForm::Executable
            }
        }
        Platform::MacOs | Platform::Linux => {
            if !path.starts_with('/') {
                AppForm::Uri
            } else if platform == Platform::MacOs && lower.ends_with(".app") {
                AppForm::Bundle
            } else if platform == Platform::Linux && lower.ends_with(".desktop") {
                AppForm::DesktopEntry
            } else {
                AppForm::Executable
            }
        }
    }
}

/// The facts about a resolved result that decide its menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facts {
    App {
        form: AppForm,
        /// Not Kage, not an OS component.
        uninstallable: bool,
    },
    Path {
        is_dir: bool,
        /// Not a protected location, and this machine has a Trash.
        trashable: bool,
    },
}

/// Which app forms each OS can uninstall from the launcher.
fn uninstall_supported(form: AppForm, platform: Platform) -> bool {
    match platform {
        Platform::MacOs => form == AppForm::Bundle,
        Platform::Windows => matches!(
            form,
            AppForm::Shortcut | AppForm::Executable | AppForm::Packaged
        ),
        Platform::Linux => false,
    }
}

/// The actions a result offers on `platform`, in canonical order (the
/// frontend groups and labels them).
pub fn actions_for(facts: &Facts, platform: Platform) -> Vec<ResultAction> {
    use ResultAction::*;
    let windows = platform == Platform::Windows;
    let mut out = vec![Open];
    match *facts {
        Facts::App {
            form,
            uninstallable,
        } => {
            let shell_item = windows && matches!(form, AppForm::Shortcut | AppForm::Executable);
            if shell_item {
                out.push(RunAsAdmin);
            }
            if form.on_disk() {
                out.extend([Reveal, CopyPath]);
            }
            if uninstallable && uninstall_supported(form, platform) {
                out.push(Uninstall);
            }
            if shell_item {
                out.push(Properties);
            }
        }
        Facts::Path { trashable, .. } => {
            out.push(Reveal);
            if windows {
                out.push(CutFile);
            }
            if platform != Platform::Linux {
                out.push(CopyFile);
            }
            out.push(CopyPath);
            if trashable {
                out.push(Trash);
            }
            if windows {
                out.push(Properties);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// Path validation
// ---------------------------------------------------------------------

/// `raw` as an absolute path (`~` expanded against `home`), or `None` for
/// empty, relative or NUL-carrying input.
pub fn resolve_user_path(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    let expanded = if raw == "~" {
        home?.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        home?.join(rest)
    } else {
        PathBuf::from(raw)
    };
    expanded.is_absolute().then_some(expanded)
}

/// Drop Windows' `\\?\` verbatim prefix `canonicalize` adds — Explorer and
/// the clipboard want the ordinary form.
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// Canonicalise `path`'s parent but keep its last component as written, so
/// a symlink stays the symlink: trashing a link must not trash its target.
/// Fails if nothing exists at `path`.
pub fn canonicalize_keep_leaf(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::symlink_metadata(path)?;
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok(strip_verbatim(std::fs::canonicalize(parent)?).join(name)),
        _ => Ok(strip_verbatim(std::fs::canonicalize(path)?)),
    }
}

fn path_key(p: &Path, case_insensitive: bool) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/');
    if case_insensitive {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

fn is_within(inner: &str, outer: &str) -> bool {
    !outer.is_empty() && (inner == outer || inner.starts_with(&format!("{outer}/")))
}

/// Whether moving `path` to the Trash must be refused: a filesystem root,
/// one of the `guarded` folders (home, Desktop, Kage's own data, …) or an
/// ancestor of one, or anything inside or containing an OS `subtree`.
pub fn is_protected_path(
    path: &Path,
    guarded: &[PathBuf],
    subtrees: &[PathBuf],
    case_insensitive: bool,
) -> bool {
    if path.parent().is_none() {
        return true;
    }
    let k = path_key(path, case_insensitive);
    if k.is_empty() {
        return true;
    }
    guarded
        .iter()
        .any(|g| is_within(&path_key(g, case_insensitive), &k))
        || subtrees.iter().any(|s| {
            let s = path_key(s, case_insensitive);
            is_within(&k, &s) || is_within(&s, &k)
        })
}

/// The folders whose removal from the launcher is refused, and the OS
/// trees nothing inside of may be trashed. Canonical forms are added too
/// (macOS `/tmp` is `/private/tmp`).
fn trash_protection_lists() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut guarded: Vec<PathBuf> = [
        crate::paths::home_dir(),
        crate::paths::config_dir(),
        crate::paths::data_dir(),
        crate::paths::data_local_dir(),
        crate::paths::cache_dir(),
        Some(std::env::temp_dir()),
        crate::os::app_uninstall::own_install_root(),
        std::env::current_exe().ok(),
    ]
    .into_iter()
    .flatten()
    .collect();
    guarded.extend(crate::commands::input::well_known_dirs());
    let mut subtrees: Vec<PathBuf> = Vec::new();
    match Platform::CURRENT {
        Platform::MacOs => {
            guarded.extend(
                [
                    "/Applications",
                    "/Users",
                    "/Volumes",
                    "/Library",
                    "/opt",
                    "/private",
                ]
                .map(PathBuf::from),
            );
            subtrees.extend(
                ["/System", "/usr", "/bin", "/sbin", "/etc", "/var", "/cores"].map(PathBuf::from),
            );
        }
        Platform::Windows => {
            for var in [
                "ProgramFiles",
                "ProgramFiles(x86)",
                "ProgramW6432",
                "ProgramData",
                "USERPROFILE",
                "PUBLIC",
                "LOCALAPPDATA",
                "APPDATA",
            ] {
                if let Ok(v) = std::env::var(var) {
                    guarded.push(PathBuf::from(v));
                }
            }
            if let Ok(root) = std::env::var("SystemRoot") {
                subtrees.push(PathBuf::from(root));
            }
        }
        Platform::Linux => {
            guarded.extend(["/home", "/opt", "/media", "/mnt", "/srv", "/root"].map(PathBuf::from));
            subtrees.extend(
                [
                    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/boot", "/proc",
                    "/sys", "/dev", "/run", "/var", "/snap",
                ]
                .map(PathBuf::from),
            );
        }
    }
    let with_canonical = |list: Vec<PathBuf>| -> Vec<PathBuf> {
        let mut out = list.clone();
        out.extend(
            list.iter()
                .filter_map(|p| std::fs::canonicalize(p).ok())
                .map(strip_verbatim),
        );
        out
    };
    (with_canonical(guarded), with_canonical(subtrees))
}

// ---------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    App,
    File,
    Folder,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub kind: TargetKind,
    pub name: String,
    pub path: PathBuf,
    pub facts: Facts,
}

/// What the frontend gets to build a menu from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultActionsInfo {
    pub platform: &'static str,
    pub kind: TargetKind,
    pub name: String,
    pub path: String,
    pub actions: Vec<ResultAction>,
}

impl Resolved {
    pub fn actions(&self) -> Vec<ResultAction> {
        actions_for(&self.facts, Platform::CURRENT)
    }

    pub fn info(&self) -> ResultActionsInfo {
        ResultActionsInfo {
            platform: Platform::CURRENT.as_str(),
            kind: self.kind,
            name: self.name.clone(),
            path: self.path.to_string_lossy().into_owned(),
            actions: self.actions(),
        }
    }
}

/// Resolve an app result. `registry` is the registry's (display name,
/// path) for the requested name — `None` when Kage doesn't know the app.
/// `own_bundle_id` is Kage's bundle identifier. Blocking.
pub fn resolve_app(
    requested: &str,
    registry: Option<(String, PathBuf)>,
    own_bundle_id: &str,
) -> Result<Resolved, AppError> {
    let Some((name, path)) = registry else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.unknown_app",
            &[("name", requested)],
        ));
    };
    let form = classify_app_path(&path.to_string_lossy(), Platform::CURRENT);
    if form.on_disk() && std::fs::symlink_metadata(&path).is_err() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.not_found",
            &[("name", &name)],
        ));
    }
    // Never Kage itself, never an OS component, never a URI page.
    let refused = form == AppForm::Uri
        || name.trim().eq_ignore_ascii_case(KAGE_PRODUCT_NAME)
        || is_own_install(&path)
        || (form == AppForm::Bundle && is_os_or_own_bundle(&path, own_bundle_id));
    let uninstallable = !refused;
    Ok(Resolved {
        kind: TargetKind::App,
        name,
        path,
        facts: Facts::App {
            form,
            uninstallable,
        },
    })
}

fn is_own_install(path: &Path) -> bool {
    let Some(root) = crate::os::app_uninstall::own_install_root() else {
        return false;
    };
    let ci = Platform::CURRENT.case_insensitive_paths();
    let target = std::fs::canonicalize(path)
        .map(strip_verbatim)
        .unwrap_or_else(|_| path.to_path_buf());
    is_within(&path_key(&target, ci), &path_key(&root, ci))
}

fn is_os_or_own_bundle(bundle: &Path, own_bundle_id: &str) -> bool {
    crate::os::app_uninstall::is_protected_app(bundle)
        || crate::os::app_uninstall::bundle_identifier(bundle)
            .is_some_and(|id| !own_bundle_id.is_empty() && id.eq_ignore_ascii_case(own_bundle_id))
}

/// Resolve a file/folder result from a raw path. Blocking.
pub fn resolve_path(raw: &str) -> Result<Resolved, AppError> {
    let home = crate::paths::home_dir();
    let Some(path) = resolve_user_path(raw, home.as_deref()) else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.invalid_target",
            &[],
        ));
    };
    let path = canonicalize_keep_leaf(&path).map_err(|_| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.not_found",
            &[("name", &path.to_string_lossy())],
        )
    })?;
    let is_dir = std::fs::metadata(&path).is_ok_and(|m| m.is_dir());
    let (guarded, subtrees) = trash_protection_lists();
    let trashable = !is_protected_path(
        &path,
        &guarded,
        &subtrees,
        Platform::CURRENT.case_insensitive_paths(),
    ) && crate::os::file_ops::trash_available();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    Ok(Resolved {
        kind: if is_dir {
            TargetKind::Folder
        } else {
            TargetKind::File
        },
        name,
        path,
        facts: Facts::Path { is_dir, trashable },
    })
}

// ---------------------------------------------------------------------
// Performing
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    /// Opened / revealed / launched — the launcher can get out of the way.
    Done,
    /// The user declined the OS prompt (UAC); nothing happened.
    Cancelled,
    /// Something is on the clipboard.
    Copied,
    MovedToTrash,
    UninstallerLaunched,
    SettingsOpened,
    /// macOS refused the move; the app is selected in Finder instead.
    RevealedForManualRemoval,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub outcome: ActionOutcome,
    pub name: String,
}

/// Run `action` on `target`. Blocking — call from a blocking thread.
/// `Properties` needs the main thread on Windows; the command dispatches
/// it there and never calls this for it.
pub fn perform(target: &Resolved, action: ResultAction) -> Result<ActionOutcome, AppError> {
    let path_str = target.path.to_string_lossy().into_owned();
    let failed = |reason: String| {
        log::warn!("[launcher-action] {action:?} failed: {reason}");
        AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.action_failed",
            &[("reason", &reason)],
        )
    };
    match action {
        ResultAction::Open => {
            match target.kind {
                TargetKind::App => crate::os::launch_application(&target.path),
                _ => crate::os::open_path(&path_str),
            }
            .map_err(|e| failed(e.to_string()))?;
            Ok(ActionOutcome::Done)
        }
        ResultAction::RunAsAdmin => match crate::os::file_ops::run_as_admin(&target.path) {
            Ok(true) => Ok(ActionOutcome::Done),
            Ok(false) => Ok(ActionOutcome::Cancelled),
            Err(e) => Err(failed(e.to_string())),
        },
        ResultAction::Reveal => {
            crate::os::reveal_in_file_manager(&path_str).map_err(|e| failed(e.to_string()))?;
            Ok(ActionOutcome::Done)
        }
        ResultAction::CopyPath => {
            if crate::os::write_clipboard(&path_str) {
                Ok(ActionOutcome::Copied)
            } else {
                Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.launcher.clipboard_failed",
                    &[],
                ))
            }
        }
        ResultAction::CopyFile | ResultAction::CutFile => {
            crate::os::file_ops::copy_files_to_clipboard(
                std::slice::from_ref(&target.path),
                action == ResultAction::CutFile,
            )
            .map_err(|e| {
                log::warn!("[launcher-action] file clipboard write failed: {e}");
                AppError::keyed(ErrorKind::Internal, "errors.launcher.clipboard_failed", &[])
            })?;
            Ok(ActionOutcome::Copied)
        }
        ResultAction::Trash => match crate::os::file_ops::move_to_trash(&target.path) {
            Ok(()) => {
                log::info!(
                    "[launcher-action] moved {} to the Trash",
                    target.path.display()
                );
                Ok(ActionOutcome::MovedToTrash)
            }
            Err(TrashError::PermissionDenied(_)) => Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.launcher.trash_denied",
                &[("name", &target.name)],
            )),
            Err(TrashError::Unavailable) => Err(AppError::keyed(
                ErrorKind::Internal,
                "errors.launcher.trash_unavailable",
                &[],
            )),
            Err(TrashError::Other(reason)) => Err(failed(reason)),
        },
        ResultAction::Uninstall => {
            match crate::os::app_uninstall::uninstall(&target.path, &target.name) {
                Ok(UninstallOutcome::MovedToTrash) => Ok(ActionOutcome::MovedToTrash),
                Ok(UninstallOutcome::UninstallerLaunched) => Ok(ActionOutcome::UninstallerLaunched),
                Ok(UninstallOutcome::SettingsOpened) => Ok(ActionOutcome::SettingsOpened),
                Ok(UninstallOutcome::RevealedForManualRemoval) => {
                    Ok(ActionOutcome::RevealedForManualRemoval)
                }
                Err(UninstallError::Protected) => Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.launcher.protected",
                    &[("name", &target.name)],
                )),
                Err(UninstallError::IsKage) => Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.launcher.uninstall_self",
                    &[],
                )),
                Err(UninstallError::AppRunning) => Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.launcher.app_running",
                    &[("name", &target.name)],
                )),
                Err(UninstallError::Unsupported) => Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.launcher.action_unavailable",
                    &[],
                )),
                Err(UninstallError::Failed(reason)) => Err(failed(reason)),
            }
        }
        ResultAction::Properties => {
            crate::os::file_ops::show_properties(&target.path)
                .map_err(|e| failed(e.to_string()))?;
            Ok(ActionOutcome::Done)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ResultAction::*;

    fn app(form: AppForm, uninstallable: bool) -> Facts {
        Facts::App {
            form,
            uninstallable,
        }
    }

    #[test]
    fn classifies_registry_paths_per_os() {
        use AppForm::*;
        let cases = [
            ("/Applications/Slack.app", Platform::MacOs, Bundle),
            (
                "x-apple.systempreferences:com.apple.Sound",
                Platform::MacOs,
                Uri,
            ),
            (
                r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Git\Git Bash.lnk",
                Platform::Windows,
                Shortcut,
            ),
            (
                r"C:\Program Files\Tool\tool.exe",
                Platform::Windows,
                Executable,
            ),
            (
                r"shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
                Platform::Windows,
                Packaged,
            ),
            ("ms-settings:display", Platform::Windows, Uri),
            (
                "/usr/share/applications/firefox.desktop",
                Platform::Linux,
                DesktopEntry,
            ),
            ("/opt/tool/bin/tool", Platform::Linux, Executable),
        ];
        for (path, platform, want) in cases {
            assert_eq!(classify_app_path(path, platform), want, "{path}");
        }
    }

    #[test]
    fn macos_app_menu() {
        let p = Platform::MacOs;
        assert_eq!(
            actions_for(&app(AppForm::Bundle, true), p),
            vec![Open, Reveal, CopyPath, Uninstall]
        );
        // System/SIP apps and Kage itself: no Uninstall.
        assert_eq!(
            actions_for(&app(AppForm::Bundle, false), p),
            vec![Open, Reveal, CopyPath]
        );
        // System Settings pages are URIs: nothing to reveal or remove.
        assert_eq!(actions_for(&app(AppForm::Uri, true), p), vec![Open]);
    }

    #[test]
    fn windows_app_menu() {
        let p = Platform::Windows;
        assert_eq!(
            actions_for(&app(AppForm::Shortcut, true), p),
            vec![Open, RunAsAdmin, Reveal, CopyPath, Uninstall, Properties]
        );
        assert_eq!(
            actions_for(&app(AppForm::Executable, false), p),
            vec![Open, RunAsAdmin, Reveal, CopyPath, Properties]
        );
        // Store apps: no file to reveal, admin or properties; uninstall
        // goes through Settings.
        assert_eq!(
            actions_for(&app(AppForm::Packaged, true), p),
            vec![Open, Uninstall]
        );
        assert_eq!(actions_for(&app(AppForm::Uri, true), p), vec![Open]);
    }

    #[test]
    fn linux_app_menu_has_no_uninstall() {
        let p = Platform::Linux;
        assert_eq!(
            actions_for(&app(AppForm::DesktopEntry, true), p),
            vec![Open, Reveal, CopyPath]
        );
    }

    #[test]
    fn file_menus_per_os() {
        let file = Facts::Path {
            is_dir: false,
            trashable: true,
        };
        assert_eq!(
            actions_for(&file, Platform::MacOs),
            vec![Open, Reveal, CopyFile, CopyPath, Trash]
        );
        assert_eq!(
            actions_for(&file, Platform::Windows),
            vec![Open, Reveal, CutFile, CopyFile, CopyPath, Trash, Properties]
        );
        assert_eq!(
            actions_for(&file, Platform::Linux),
            vec![Open, Reveal, CopyPath, Trash]
        );
        let protected = Facts::Path {
            is_dir: true,
            trashable: false,
        };
        assert!(!actions_for(&protected, Platform::MacOs).contains(&Trash));
    }

    #[test]
    fn user_paths_must_be_absolute() {
        let home = Path::new("/Users/me");
        assert_eq!(resolve_user_path("", Some(home)), None);
        assert_eq!(resolve_user_path("   ", Some(home)), None);
        assert_eq!(resolve_user_path("relative/x", Some(home)), None);
        assert_eq!(resolve_user_path("/tmp/a\0b", Some(home)), None);
        assert_eq!(
            resolve_user_path("~/Documents/a.txt", Some(home)),
            Some(PathBuf::from("/Users/me/Documents/a.txt"))
        );
        assert_eq!(resolve_user_path("~", Some(home)), Some(home.to_path_buf()));
        assert_eq!(resolve_user_path("~/x", None), None);
        assert_eq!(
            resolve_user_path(" /etc/hosts ", None),
            Some(PathBuf::from("/etc/hosts"))
        );
    }

    #[test]
    fn strips_windows_verbatim_prefixes() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Users\me\a.txt")),
            PathBuf::from(r"C:\Users\me\a.txt")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\a.txt")),
            PathBuf::from(r"\\server\share\a.txt")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/Users/me/a.txt")),
            PathBuf::from("/Users/me/a.txt")
        );
    }

    #[test]
    fn canonicalizing_keeps_a_symlink_itself() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "x").unwrap();
        assert!(canonicalize_keep_leaf(&dir.path().join("missing.txt")).is_err());
        let canon_dir = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            canonicalize_keep_leaf(&real).unwrap(),
            strip_verbatim(canon_dir.join("real.txt"))
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_eq!(
                canonicalize_keep_leaf(&link).unwrap(),
                canon_dir.join("link.txt")
            );
        }
    }

    #[test]
    fn protects_roots_guarded_dirs_their_ancestors_and_os_trees() {
        let guarded = vec![
            PathBuf::from("/Users/me"),
            PathBuf::from("/Users/me/Documents"),
        ];
        let subtrees = vec![PathBuf::from("/System")];
        let prot = |p: &str| is_protected_path(Path::new(p), &guarded, &subtrees, true);
        assert!(prot("/"));
        assert!(prot("/Users"));
        assert!(prot("/Users/me"));
        assert!(prot("/Users/me/Documents/"));
        assert!(prot("/users/ME/documents"));
        assert!(prot("/System/Library/CoreServices/Finder.app"));
        assert!(!prot("/Users/me/Documents/report.pdf"));
        assert!(!prot("/Users/me/Downloads"));
        assert!(!prot("/Users/meow"));
        assert!(!prot("/Systematic/file"));
        // Case-sensitive filesystems don't fold case.
        assert!(!is_protected_path(
            Path::new("/users/ME/documents"),
            &guarded,
            &subtrees,
            false
        ));
    }

    #[test]
    fn protects_windows_paths_case_insensitively() {
        let guarded = vec![PathBuf::from(r"C:\Users\me")];
        let subtrees = vec![PathBuf::from(r"C:\Windows")];
        let prot = |p: &str| is_protected_path(Path::new(p), &guarded, &subtrees, true);
        assert!(prot(r"c:\users"));
        assert!(prot(r"C:\WINDOWS\System32\drivers"));
        assert!(!prot(r"C:\Users\me\Desktop\notes.txt"));
    }

    #[test]
    fn unknown_apps_are_refused() {
        let e = resolve_app("Nope", None, "com.kage.launcher").unwrap_err();
        assert_eq!(e.key, "errors.launcher.unknown_app");
    }

    #[test]
    fn invalid_or_missing_paths_are_refused() {
        assert_eq!(
            resolve_path("").unwrap_err().key,
            "errors.launcher.invalid_target"
        );
        assert_eq!(
            resolve_path("relative.txt").unwrap_err().key,
            "errors.launcher.invalid_target"
        );
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone.txt");
        assert_eq!(
            resolve_path(&missing.to_string_lossy()).unwrap_err().key,
            "errors.launcher.not_found"
        );
    }

    #[test]
    fn resolves_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "x").unwrap();
        let r = resolve_path(&file.to_string_lossy()).unwrap();
        assert_eq!(r.kind, TargetKind::File);
        assert_eq!(r.name, "note.txt");
        let info = r.info();
        assert_eq!(info.platform, Platform::CURRENT.as_str());
        assert!(info.actions.contains(&Open));
        assert!(info.actions.contains(&CopyPath));
    }

    #[test]
    fn actions_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&[RunAsAdmin, CopyPath, CutFile]).unwrap(),
            r#"["run_as_admin","copy_path","cut_file"]"#
        );
        let a: ResultAction = serde_json::from_str(r#""uninstall""#).unwrap();
        assert_eq!(a, Uninstall);
        assert!(serde_json::from_str::<ResultAction>(r#""rm_rf""#).is_err());
    }
}
