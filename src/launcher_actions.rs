//! Right-click actions for launcher results (apps, files, folders).
//!
//! The webview names a result — `kind: "app"` with the app's display name,
//! or `kind: "path"` with a filesystem path — and never supplies a path to
//! act on for an app: apps are looked up in Kage's own app registry, and
//! paths are validated (absolute, plain, existing, fully canonical) before
//! anything touches them. `actions_for` decides which actions a resolved
//! target offers on this OS; `launcher_result_action` re-resolves and
//! re-checks that list on every call, so a crafted request can't run an
//! action the menu wouldn't have shown — and right before anything is
//! moved to the Trash, the item's identity is checked once more.
//!
//! An app bundle reached by path is still an app: it offers Uninstall, with
//! every uninstall safeguard and the confirmation, never a bare Trash.

use crate::error::{AppError, ErrorKind};
use crate::os::app_uninstall::{UninstallError, UninstallOutcome};
use crate::os::file_ops::{FileId, TrashError};
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
        /// Not Kage, not an OS component, not in a protected place.
        uninstallable: bool,
    },
    Path {
        is_dir: bool,
        /// May be moved away (Trash, or Cut then paste): not a protected
        /// location, not part of an installed app.
        removable: bool,
        /// This machine has a Trash.
        trash: bool,
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
        Facts::Path {
            removable, trash, ..
        } => {
            out.push(Reveal);
            // A cut file is moved when it's pasted: only what may be moved.
            if windows && removable {
                out.push(CutFile);
            }
            if platform != Platform::Linux {
                out.push(CopyFile);
            }
            out.push(CopyPath);
            if removable && trash {
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

/// Whether `s` is a plain Windows path: no device or verbatim prefix
/// (`\\?\`, `\\.\`, `\??\`), no wildcard, reserved or control characters,
/// and no `:` but a drive letter's — so no alternate data streams
/// (`Documents::$INDEX_ALLOCATION` opens the folder itself).
pub fn windows_path_is_plain(s: &str) -> bool {
    let unified = s.replace('/', "\\");
    if unified.starts_with(r"\\.\") || unified.starts_with(r"\??\") {
        return false;
    }
    if s.chars()
        .any(|c| c.is_control() || matches!(c, '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    s.char_indices()
        .all(|(i, c)| c != ':' || (i == 1 && s.starts_with(|d: char| d.is_ascii_alphabetic())))
}

/// `raw` as an absolute path (`~` expanded against `home`), or `None` for
/// empty, relative or NUL-carrying input — and, on Windows, anything that
/// isn't a plain path (`windows_path_is_plain`).
pub fn resolve_user_path(raw: &str, home: Option<&Path>, platform: Platform) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    if platform == Platform::Windows && !windows_path_is_plain(raw) {
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

/// The path `path` really names, to check and act on. Fully canonical —
/// links in its folders, `.`/`..`, letter case and (Windows) 8.3 aliases
/// such as `DOCUME~1` or a trailing `.` or space all resolved — except
/// that a link itself (symlink, or a Windows junction or other reparse
/// point) stays the link: only its folder is canonicalised and its name
/// read back from the directory, because trashing a link must never trash
/// what it points at. Fails if nothing exists at `path`.
pub fn canonical_target(path: &Path) -> std::io::Result<PathBuf> {
    let meta = std::fs::symlink_metadata(path)?;
    if crate::os::file_ops::is_link(&meta) {
        if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
            // Looked up without any trailing separator the path was typed with.
            let on_disk = crate::os::file_ops::on_disk_name(&parent.join(name));
            let parent = strip_verbatim(std::fs::canonicalize(parent)?);
            return Ok(parent.join(on_disk.unwrap_or_else(|| name.to_os_string())));
        }
    }
    Ok(strip_verbatim(std::fs::canonicalize(path)?))
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

/// Whether `path` must not be moved or removed: a filesystem root, one of
/// the `guarded` folders (home, Desktop, …) or a folder containing one, or
/// anything inside or containing one of the `subtrees`.
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

/// Whether `path` lies inside a macOS `.app` bundle (part of an app).
pub fn inside_app_bundle(path: &Path) -> bool {
    path.ancestors().skip(1).any(has_app_extension)
}

fn has_app_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
}

/// What Kage knows about itself, from the running app.
#[derive(Debug, Clone, Default)]
pub struct OwnLocations {
    /// The bundle identifier (`com.kage.launcher`).
    pub bundle_id: String,
    /// Tauri's resource directory — part of the installation.
    pub resource_dir: Option<PathBuf>,
}

/// A bundle id safe to use as a folder name.
fn valid_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Kage's own storage, protected as whole trees: `<base>/kage` (config,
/// data, logs — `app_log` writes under `<data-local>/kage/logs` — and
/// cache) and `<base>/<bundle id>` (Tauri's and the webview's per-app
/// folders) for each per-user `base`, plus — given the macOS home — the
/// WebKit, log, HTTP-storage, cookie, preference and saved-state entries
/// macOS keys by bundle id.
pub fn own_storage_dirs(
    bases: &[PathBuf],
    bundle_id: &str,
    mac_home: Option<&Path>,
) -> Vec<PathBuf> {
    let id = valid_bundle_id(bundle_id).then_some(bundle_id);
    let mut out = Vec::new();
    for base in bases {
        out.push(base.join("kage"));
        if let Some(id) = id {
            out.push(base.join(id));
        }
    }
    if let (Some(home), Some(id)) = (mac_home, id) {
        let lib = home.join("Library");
        for sub in ["WebKit", "Logs", "HTTPStorages"] {
            out.push(lib.join(sub).join(id));
        }
        out.push(lib.join("Cookies").join(format!("{id}.binarycookies")));
        out.push(lib.join("Preferences").join(format!("{id}.plist")));
        out.push(
            lib.join("Saved Application State")
                .join(format!("{id}.savedState")),
        );
    }
    out
}

/// Where moving things away (Trash, Cut) and uninstalling are refused.
#[derive(Debug, Clone, Default)]
pub struct Protection {
    /// Never moved, nor any folder containing one: home, Desktop,
    /// Documents, the per-user base folders, the OS's top-level folders.
    pub guarded: Vec<PathBuf>,
    /// Nothing inside these is moved or uninstalled, nor any folder
    /// containing one: OS trees, Kage's own storage and installation.
    pub subtrees: Vec<PathBuf>,
    /// Where apps are installed (`/Applications`, Program Files): nothing
    /// inside is moved to the Trash or cut — apps go through Uninstall,
    /// with its safeguards.
    pub app_trees: Vec<PathBuf>,
}

impl Protection {
    /// Whether moving `path` away (Trash, Cut) must be refused.
    pub fn blocks_removal(&self, path: &Path, case_insensitive: bool) -> bool {
        is_protected_path(path, &self.guarded, &self.subtrees, case_insensitive)
            || is_protected_path(path, &[], &self.app_trees, case_insensitive)
    }

    /// Whether uninstalling the app at `path` must be refused.
    pub fn blocks_uninstall(&self, path: &Path, case_insensitive: bool) -> bool {
        is_protected_path(path, &self.guarded, &self.subtrees, case_insensitive)
    }

    /// Whether `path` is a guarded folder or contains one.
    fn guards(&self, path: &Path, case_insensitive: bool) -> bool {
        is_protected_path(path, &self.guarded, &[], case_insensitive)
    }

    /// The lists for this machine. Canonical forms are added too (macOS
    /// `/tmp` is `/private/tmp`).
    pub fn for_this_machine(own: &OwnLocations) -> Self {
        let home = crate::paths::home_dir();
        let bases: Vec<PathBuf> = [
            crate::paths::config_dir(),
            crate::paths::data_dir(),
            crate::paths::data_local_dir(),
            crate::paths::cache_dir(),
        ]
        .into_iter()
        .flatten()
        .collect();

        let mut guarded: Vec<PathBuf> = bases.clone();
        guarded.extend(home.clone());
        guarded.push(std::env::temp_dir());
        guarded.extend(crate::commands::input::well_known_dirs());

        let mac_home = (Platform::CURRENT == Platform::MacOs)
            .then_some(home.as_deref())
            .flatten();
        let mut subtrees = own_storage_dirs(&bases, &own.bundle_id, mac_home);
        subtrees.extend(
            [
                crate::os::app_uninstall::own_install_root(),
                std::env::current_exe().ok(),
                own.resource_dir.clone(),
            ]
            .into_iter()
            .flatten(),
        );

        let mut app_trees: Vec<PathBuf> = Vec::new();
        match Platform::CURRENT {
            Platform::MacOs => {
                guarded.extend(
                    ["/Users", "/Volumes", "/Library", "/opt", "/private"].map(PathBuf::from),
                );
                subtrees.extend(
                    ["/System", "/usr", "/bin", "/sbin", "/etc", "/var", "/cores"]
                        .map(PathBuf::from),
                );
                app_trees.push(PathBuf::from("/Applications"));
                app_trees.extend(home.as_ref().map(|h| h.join("Applications")));
            }
            Platform::Windows => {
                let env = |var: &str| std::env::var_os(var).map(PathBuf::from);
                for var in [
                    "ProgramData",
                    "USERPROFILE",
                    "PUBLIC",
                    "LOCALAPPDATA",
                    "APPDATA",
                ] {
                    guarded.extend(env(var));
                }
                for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
                    app_trees.extend(env(var));
                }
                // Per-user installs (VS Code, …), and every product's own
                // install folder the Uninstall keys record (Squirrel apps
                // live straight under %LOCALAPPDATA%).
                app_trees.extend(env("LOCALAPPDATA").map(|d| d.join("Programs")));
                app_trees.extend(crate::os::app_uninstall::installed_product_dirs());
                subtrees.extend(env("SystemRoot"));
            }
            Platform::Linux => {
                guarded.extend(
                    ["/home", "/opt", "/media", "/mnt", "/srv", "/root"].map(PathBuf::from),
                );
                subtrees.extend(
                    [
                        "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/boot",
                        "/proc", "/sys", "/dev", "/run", "/var", "/snap",
                    ]
                    .map(PathBuf::from),
                );
                // Running from an AppImage: the image file is the install.
                subtrees.extend(std::env::var_os("APPIMAGE").map(PathBuf::from));
            }
        }
        let with_canonical = |list: Vec<PathBuf>| -> Vec<PathBuf> {
            let canonical: Vec<PathBuf> = list
                .iter()
                .filter_map(|p| std::fs::canonicalize(p).ok())
                .map(strip_verbatim)
                .collect();
            let mut out = list;
            out.extend(canonical);
            out
        };
        Protection {
            guarded: with_canonical(guarded),
            subtrees: with_canonical(subtrees),
            app_trees: with_canonical(app_trees),
        }
    }
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
    /// `canonical_target(path)` when it was resolved (`None` for apps that
    /// aren't files: Store apps, URI pages).
    pub canonical: Option<PathBuf>,
    /// The item's identity then.
    pub identity: Option<FileId>,
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

    /// Right before something is moved: `path` must still name the item
    /// that was checked — the same canonical path and the same file. This
    /// narrows the window for swapping in a link, or another folder, in
    /// between.
    pub fn verify_unchanged(&self) -> Result<(), AppError> {
        let changed = |why: &str| {
            log::warn!(
                "[launcher-action] {} changed since it was checked ({why}); refusing",
                self.path.display()
            );
            AppError::keyed(
                ErrorKind::Internal,
                "errors.launcher.target_changed",
                &[("name", &self.name)],
            )
        };
        let Some(expected) = &self.canonical else {
            return Err(changed("no canonical path"));
        };
        let now = match canonical_target(&self.path) {
            Ok(now) => now,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AppError::keyed(
                    ErrorKind::Internal,
                    "errors.launcher.not_found",
                    &[("name", &self.name)],
                ))
            }
            Err(_) => return Err(changed("unreadable")),
        };
        let ci = Platform::CURRENT.case_insensitive_paths();
        if path_key(&now, ci) != path_key(expected, ci) {
            return Err(changed("resolves elsewhere"));
        }
        if crate::os::file_ops::file_id(&now) != self.identity {
            return Err(changed("different file"));
        }
        Ok(())
    }
}

fn is_kage_name(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(KAGE_PRODUCT_NAME)
}

/// Resolve an app result. `registry` is the registry's (display name,
/// path) for the requested name — `None` when Kage doesn't know the app.
/// Blocking.
pub fn resolve_app(
    requested: &str,
    registry: Option<(String, PathBuf)>,
    own: &OwnLocations,
) -> Result<Resolved, AppError> {
    let Some((name, path)) = registry else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.unknown_app",
            &[("name", requested)],
        ));
    };
    let form = classify_app_path(&path.to_string_lossy(), Platform::CURRENT);
    let (canonical, identity) = if form.on_disk() {
        let canonical = canonical_target(&path).map_err(|_| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.launcher.not_found",
                &[("name", &name)],
            )
        })?;
        let identity = crate::os::file_ops::file_id(&canonical);
        (Some(canonical), identity)
    } else {
        (None, None)
    };
    // Never Kage itself, never an OS component, never a URI page.
    let uninstallable = match form {
        AppForm::Uri => false,
        AppForm::Bundle => {
            bundle_uninstallable(&name, &path, canonical.as_deref().unwrap_or(&path), own)
        }
        _ => !is_kage_name(&name) && !is_own_install(&path),
    };
    Ok(Resolved {
        kind: TargetKind::App,
        name,
        path,
        facts: Facts::App {
            form,
            uninstallable,
        },
        canonical,
        identity,
    })
}

/// Whether the macOS bundle at `path` (`canonical` resolved) may be
/// uninstalled: not Kage (by name, location or bundle id), not part of the
/// OS, not nested in another app, not in a protected location.
fn bundle_uninstallable(name: &str, path: &Path, canonical: &Path, own: &OwnLocations) -> bool {
    !is_kage_name(name)
        && !is_own_install(path)
        && !is_os_or_own_bundle(path, &own.bundle_id)
        && !inside_app_bundle(canonical)
        && !Protection::for_this_machine(own)
            .blocks_uninstall(canonical, Platform::CURRENT.case_insensitive_paths())
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
pub fn resolve_path(raw: &str, own: &OwnLocations) -> Result<Resolved, AppError> {
    let home = crate::paths::home_dir();
    let Some(path) = resolve_user_path(raw, home.as_deref(), Platform::CURRENT) else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.invalid_target",
            &[],
        ));
    };
    let not_found = |p: &Path| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.launcher.not_found",
            &[("name", &p.to_string_lossy())],
        )
    };
    let path = canonical_target(&path).map_err(|_| not_found(&path))?;
    let meta = std::fs::symlink_metadata(&path).map_err(|_| not_found(&path))?;
    let link = crate::os::file_ops::is_link(&meta);
    let identity = crate::os::file_ops::file_id(&path);
    let leaf = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string_lossy().into_owned())
    };
    let mac = Platform::CURRENT == Platform::MacOs;

    // An app bundle is an app however it was reached: Uninstall, with the
    // running-app, system and Kage checks and the confirmation — not Trash.
    if mac && !link && meta.is_dir() && has_app_extension(&path) {
        let name = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| leaf(&path));
        let uninstallable = bundle_uninstallable(&name, &path, &path, own);
        return Ok(Resolved {
            kind: TargetKind::App,
            name,
            path: path.clone(),
            facts: Facts::App {
                form: AppForm::Bundle,
                uninstallable,
            },
            canonical: Some(path),
            identity,
        });
    }

    let is_dir = std::fs::metadata(&path).is_ok_and(|m| m.is_dir());
    let ci = Platform::CURRENT.case_insensitive_paths();
    let protection = Protection::for_this_machine(own);
    // Anything inside a bundle is part of an app.
    let part_of_app = mac && inside_app_bundle(&path);
    let mut removable = !part_of_app && !protection.blocks_removal(&path, ci);
    // A link standing in for a protected folder (Windows' legacy
    // `My Documents` junction) is left alone like the folder.
    if removable && link {
        if let Ok(target) = std::fs::canonicalize(&path).map(strip_verbatim) {
            removable = !protection.guards(&target, ci);
        }
    }
    let name = leaf(&path);
    Ok(Resolved {
        kind: if is_dir {
            TargetKind::Folder
        } else {
            TargetKind::File
        },
        name,
        path: path.clone(),
        facts: Facts::Path {
            is_dir,
            removable,
            trash: crate::os::file_ops::trash_available(),
        },
        canonical: Some(path),
        identity,
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
        ResultAction::Trash => {
            target.verify_unchanged()?;
            match crate::os::file_ops::move_to_trash(&target.path) {
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
            }
        }
        ResultAction::Uninstall => {
            // A bundle is moved to the Trash: same last-moment check.
            if matches!(
                target.facts,
                Facts::App {
                    form: AppForm::Bundle,
                    ..
                }
            ) {
                target.verify_unchanged()?;
            }
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

    fn own() -> OwnLocations {
        OwnLocations {
            bundle_id: "com.kage.launcher".into(),
            resource_dir: None,
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
            removable: true,
            trash: true,
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
        // Linux without a Trash tool: nothing to move it with.
        let no_trash = Facts::Path {
            is_dir: false,
            removable: true,
            trash: false,
        };
        assert!(!actions_for(&no_trash, Platform::Linux).contains(&Trash));
    }

    #[test]
    fn protected_items_offer_neither_trash_nor_cut() {
        let protected = Facts::Path {
            is_dir: true,
            removable: false,
            trash: true,
        };
        for p in [Platform::MacOs, Platform::Windows, Platform::Linux] {
            let actions = actions_for(&protected, p);
            assert!(!actions.contains(&Trash), "{p:?}");
            assert!(!actions.contains(&CutFile), "{p:?}");
            assert!(actions.contains(&CopyPath), "{p:?}");
        }
        // Copying a protected folder leaves it where it is.
        assert!(actions_for(&protected, Platform::Windows).contains(&CopyFile));
    }

    #[test]
    fn user_paths_must_be_absolute() {
        let home = Path::new("/Users/me");
        let mac = Platform::MacOs;
        assert_eq!(resolve_user_path("", Some(home), mac), None);
        assert_eq!(resolve_user_path("   ", Some(home), mac), None);
        assert_eq!(resolve_user_path("relative/x", Some(home), mac), None);
        assert_eq!(resolve_user_path("/tmp/a\0b", Some(home), mac), None);
        assert_eq!(
            resolve_user_path("~/Documents/a.txt", Some(home), mac),
            Some(PathBuf::from("/Users/me/Documents/a.txt"))
        );
        assert_eq!(
            resolve_user_path("~", Some(home), mac),
            Some(home.to_path_buf())
        );
        assert_eq!(resolve_user_path("~/x", None, mac), None);
        assert_eq!(
            resolve_user_path(" /etc/hosts ", None, mac),
            Some(PathBuf::from("/etc/hosts"))
        );
    }

    #[test]
    fn windows_paths_must_be_plain() {
        for plain in [
            r"C:\Users\me\Documents",
            r"C:\Users\me\Documents.",
            r"c:/Users/me/a.txt",
            r"\\server\share\a.txt",
            r"D:\Tools\x — ü.exe",
        ] {
            assert!(windows_path_is_plain(plain), "{plain}");
        }
        for odd in [
            r"\\?\C:\Users\me\Documents",
            r"//?/C:/Users/me",
            r"\\.\C:\Users\me",
            r"\??\C:\Users\me",
            r"C:\Users\me\Documents::$INDEX_ALLOCATION",
            r"C:\Users\me\a.txt:hidden",
            r"C:\Users\me\*",
            r"C:\Users\me\a?.txt",
            "C:\\Users\\me\\a\u{1}b",
            r"C:\Users\me\<x>",
            r"1:\x",
        ] {
            assert!(!windows_path_is_plain(odd), "{odd}");
        }
        assert_eq!(
            resolve_user_path(r"\\?\C:\Users", None, Platform::Windows),
            None
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
    fn canonical_target_resolves_folders_but_keeps_a_symlink_itself() {
        let dir = tempfile::tempdir().unwrap();
        let canon_dir = strip_verbatim(std::fs::canonicalize(dir.path()).unwrap());
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(canonical_target(&dir.path().join("missing.txt")).is_err());
        assert_eq!(canonical_target(&real).unwrap(), canon_dir.join("real.txt"));
        // `.` and `..` spellings name the folder they resolve to.
        assert_eq!(
            canonical_target(&dir.path().join("sub").join("..")).unwrap(),
            canon_dir
        );
        assert_eq!(
            canonical_target(&dir.path().join("sub").join(".")).unwrap(),
            canon_dir.join("sub")
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_eq!(canonical_target(&link).unwrap(), canon_dir.join("link.txt"));
            // A link to a folder, written with a trailing `/`, names the
            // folder — and that is what would be acted on.
            let dlink = dir.path().join("dlink");
            std::os::unix::fs::symlink(dir.path().join("sub"), &dlink).unwrap();
            assert_eq!(canonical_target(&dlink).unwrap(), canon_dir.join("dlink"));
            assert_eq!(
                canonical_target(Path::new(&format!("{}/", dlink.display()))).unwrap(),
                canon_dir.join("sub")
            );
        }
    }

    /// Case-insensitive APFS: another spelling of a folder resolves to the
    /// folder's real name, so it matches the protected entry.
    #[cfg(target_os = "macos")]
    #[test]
    fn canonical_target_uses_the_on_disk_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Documents")).unwrap();
        let canon_dir = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            canonical_target(&dir.path().join("DOCUMENTS")).unwrap(),
            canon_dir.join("Documents")
        );
    }

    /// Windows resolves `Documents.`, `Documents ` and 8.3 aliases to
    /// `Documents` on open; the canonical form must too.
    #[cfg(windows)]
    #[test]
    fn canonical_target_resolves_windows_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("Documents Folder");
        std::fs::create_dir(&real).unwrap();
        let want = canonical_target(&real).unwrap();
        assert!(want.ends_with("Documents Folder"), "{}", want.display());
        let parent = dir.path().to_string_lossy().into_owned();
        for alias in [
            format!(r"{parent}\Documents Folder."),
            format!(r"{parent}\Documents Folder "),
            format!(r"{parent}\DOCUMENTS FOLDER"),
        ] {
            assert_eq!(
                canonical_target(Path::new(&alias)).unwrap(),
                want,
                "{alias}"
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
    fn kage_storage_is_protected_as_whole_trees() {
        let support = PathBuf::from("/Users/me/Library/Application Support");
        let caches = PathBuf::from("/Users/me/Library/Caches");
        let dirs = own_storage_dirs(
            &[support.clone(), caches.clone()],
            "com.kage.launcher",
            Some(Path::new("/Users/me")),
        );
        let protection = Protection {
            guarded: vec![support.clone(), caches.clone()],
            subtrees: dirs,
            app_trees: vec![],
        };
        let blocked = |p: &str| protection.blocks_removal(Path::new(p), true);
        for p in [
            "/Users/me/Library/Application Support/kage",
            "/Users/me/Library/Application Support/kage/config.json",
            "/Users/me/Library/Application Support/Kage/permission-audit.jsonl",
            "/Users/me/Library/Application Support/com.kage.launcher",
            "/Users/me/Library/Caches/kage/x",
            "/Users/me/Library/Caches/com.kage.launcher/WebKit",
            "/Users/me/Library/WebKit/com.kage.launcher",
            "/Users/me/Library/Preferences/com.kage.launcher.plist",
            "/Users/me/Library/Application Support",
        ] {
            assert!(blocked(p), "{p}");
        }
        for p in [
            "/Users/me/Library/Application Support/kagero",
            "/Users/me/Library/Application Support/Slack/cache",
            "/Users/me/Library/Preferences/com.example.app.plist",
        ] {
            assert!(!blocked(p), "{p}");
        }

        // Windows: data and logs under %LOCALAPPDATA%\kage, config and
        // the webview's folder under %APPDATA% / %LOCALAPPDATA%.
        let local = PathBuf::from(r"C:\Users\me\AppData\Local");
        let roaming = PathBuf::from(r"C:\Users\me\AppData\Roaming");
        let win = Protection {
            guarded: vec![local.clone(), roaming.clone()],
            subtrees: own_storage_dirs(&[roaming, local], "com.kage.launcher", None),
            app_trees: vec![],
        };
        assert!(win.blocks_removal(Path::new(r"C:\Users\me\AppData\Local\kage\logs"), true));
        assert!(win.blocks_removal(
            Path::new(r"c:\users\me\appdata\local\COM.KAGE.LAUNCHER\EBWebView"),
            true
        ));
        assert!(win.blocks_removal(Path::new(r"C:\Users\me\AppData\Roaming\kage"), true));
        assert!(!win.blocks_removal(Path::new(r"C:\Users\me\AppData\Roaming\Code"), true));
    }

    #[test]
    fn a_bad_bundle_id_never_widens_the_storage_list() {
        let base = PathBuf::from("/Users/me/Library/Application Support");
        for id in ["", ".", "..", "../x", "a/b", r"a\b"] {
            assert_eq!(
                own_storage_dirs(
                    std::slice::from_ref(&base),
                    id,
                    Some(Path::new("/Users/me"))
                ),
                vec![base.join("kage")],
                "{id:?}"
            );
        }
    }

    #[test]
    fn app_trees_refuse_trash_and_cut_but_not_uninstall() {
        let protection = Protection {
            guarded: vec![PathBuf::from("/Users/me")],
            subtrees: vec![PathBuf::from("/System")],
            app_trees: vec![PathBuf::from("/Applications")],
        };
        let slack = Path::new("/Applications/Slack.app");
        assert!(protection.blocks_removal(slack, true));
        assert!(!protection.blocks_uninstall(slack, true));
        assert!(protection.blocks_removal(Path::new("/Applications/Vendor Suite"), true));
        assert!(protection.blocks_uninstall(Path::new("/System/Applications/Notes.app"), true));
        assert!(!protection.blocks_removal(Path::new("/Users/me/Downloads/old.zip"), true));

        let win = Protection {
            guarded: vec![],
            subtrees: vec![],
            app_trees: vec![PathBuf::from(r"C:\Program Files")],
        };
        assert!(win.blocks_removal(Path::new(r"C:\Program Files\Contoso\tool.exe"), true));
        assert!(!win.blocks_removal(Path::new(r"D:\Portable\Tool\tool.exe"), true));
    }

    #[test]
    fn finds_paths_inside_app_bundles() {
        assert!(inside_app_bundle(Path::new(
            "/Applications/Slack.app/Contents/MacOS/Slack"
        )));
        assert!(inside_app_bundle(Path::new(
            "/Applications/Xcode.app/Contents/Applications/Instruments.app"
        )));
        assert!(!inside_app_bundle(Path::new("/Applications/Slack.app")));
        assert!(!inside_app_bundle(Path::new("/Users/me/notes.app.txt")));
    }

    #[test]
    fn unknown_apps_are_refused() {
        let e = resolve_app("Nope", None, &own()).unwrap_err();
        assert_eq!(e.key, "errors.launcher.unknown_app");
    }

    #[test]
    fn invalid_or_missing_paths_are_refused() {
        assert_eq!(
            resolve_path("", &own()).unwrap_err().key,
            "errors.launcher.invalid_target"
        );
        assert_eq!(
            resolve_path("relative.txt", &own()).unwrap_err().key,
            "errors.launcher.invalid_target"
        );
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone.txt");
        assert_eq!(
            resolve_path(&missing.to_string_lossy(), &own())
                .unwrap_err()
                .key,
            "errors.launcher.not_found"
        );
    }

    #[test]
    fn resolves_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "x").unwrap();
        let r = resolve_path(&file.to_string_lossy(), &own()).unwrap();
        assert_eq!(r.kind, TargetKind::File);
        assert_eq!(r.name, "note.txt");
        assert!(r.identity.is_some());
        let info = r.info();
        assert_eq!(info.platform, Platform::CURRENT.as_str());
        assert!(info.actions.contains(&Open));
        assert!(info.actions.contains(&CopyPath));
    }

    #[test]
    fn kage_own_folders_are_never_removable() {
        let config = crate::paths::config_dir().unwrap().join("kage");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.json"), "{}").unwrap();
        for p in [config.clone(), config.join("config.json")] {
            let r = resolve_path(&p.to_string_lossy(), &own()).unwrap();
            assert!(
                matches!(
                    r.facts,
                    Facts::Path {
                        removable: false,
                        ..
                    }
                ),
                "{}",
                p.display()
            );
            assert!(!r.actions().contains(&Trash));
            assert!(!r.actions().contains(&CutFile));
        }
    }

    /// A bundle reached by path is an app: Uninstall (with its checks),
    /// never Trash.
    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundles_by_path_resolve_as_apps() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Tool.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        let r = resolve_path(&bundle.to_string_lossy(), &own()).unwrap();
        assert_eq!(r.kind, TargetKind::App);
        assert_eq!(r.name, "Tool");
        assert!(matches!(
            r.facts,
            Facts::App {
                form: AppForm::Bundle,
                ..
            }
        ));
        assert!(!r.actions().contains(&Trash));
        // Inside a bundle is part of the app: not removable either.
        let inner = resolve_path(&bundle.join("Contents").to_string_lossy(), &own()).unwrap();
        assert!(matches!(
            inner.facts,
            Facts::Path {
                removable: false,
                ..
            }
        ));
    }

    #[test]
    fn verify_unchanged_catches_a_swapped_item() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let r = resolve_path(&file.to_string_lossy(), &own()).unwrap();
        r.verify_unchanged().unwrap();
        // Replaced by another file under the same name.
        let other = dir.path().join("b.txt");
        std::fs::write(&other, "y").unwrap();
        std::fs::rename(&other, &file).unwrap();
        assert_eq!(
            r.verify_unchanged().unwrap_err().key,
            "errors.launcher.target_changed"
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(
            r.verify_unchanged().unwrap_err().key,
            "errors.launcher.not_found"
        );
    }

    #[cfg(unix)]
    #[test]
    fn verify_unchanged_catches_a_folder_swapped_for_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(sub.join("x"), "x").unwrap();
        std::fs::write(elsewhere.join("x"), "y").unwrap();
        let r = resolve_path(&sub.join("x").to_string_lossy(), &own()).unwrap();
        std::fs::rename(&sub, dir.path().join("old")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &sub).unwrap();
        assert_eq!(
            r.verify_unchanged().unwrap_err().key,
            "errors.launcher.target_changed"
        );
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
