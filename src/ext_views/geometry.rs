//! Where a view window opens, and the file that remembers where each view
//! was (docs/design/EXTENSION_VIEWS.md §7.4, §7.5).
//!
//! [`place`] is pure and unit-tested with fake monitor lists. Positions and
//! sizes are physical pixels, stored with the monitor they were on, so a
//! window comes back where it was even when monitors have different
//! scales.
//!
//! The file is `<data_local_dir>/kage/extension-views.json`, one entry per
//! `(extension, view)`, per machine. It isn't part of `Config`, of any
//! backup, or of folder sync: window geometry is device-local.

use crate::extensions::views::ViewDecl;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The geometry file's name under `<data_local_dir>/kage/`.
pub const FILE_NAME: &str = "extension-views.json";
/// Schema version written to the file.
pub const FILE_VERSION: u32 = 1;
/// Most entries kept; the oldest `saved_at` goes first.
pub const MAX_ENTRIES: usize = 64;
/// A new window's size is at most this share of the work area.
pub const WORK_AREA_FRACTION: f64 = 0.9;
/// Gap between the launcher and a view placed below it, in logical pixels.
pub const LAUNCHER_GAP: f64 = 16.0;
/// Largest scale factor a saved monitor may claim.
const MAX_SCALE: f64 = 16.0;

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(&self) -> i64 {
        self.x as i64 + self.width as i64
    }

    fn bottom(&self) -> i64 {
        self.y as i64 + self.height as i64
    }

    fn area(&self) -> i64 {
        self.width as i64 * self.height as i64
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x as f64
            && x < self.right() as f64
            && y >= self.y as f64
            && y < self.bottom() as f64
    }

    fn intersection_area(&self, other: &Rect) -> i64 {
        let w = self.right().min(other.right()) - (self.x as i64).max(other.x as i64);
        let h = self.bottom().min(other.bottom()) - (self.y as i64).max(other.y as i64);
        if w <= 0 || h <= 0 {
            0
        } else {
            w * h
        }
    }
}

/// The monitor a saved window was on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedMonitor {
    #[serde(default)]
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

impl SavedMonitor {
    fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

/// One view's saved window geometry (physical pixels).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
    pub monitor: SavedMonitor,
    /// RFC 3339, UTC. Orders entries for the [`MAX_ENTRIES`] cap.
    #[serde(default)]
    pub saved_at: String,
}

impl SavedGeometry {
    fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }

    /// Whether the entry is usable: a real size on a monitor with a sane
    /// scale. A hand-edited entry that isn't is dropped on read.
    fn is_sane(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.monitor.scale > 0.0
            && self.monitor.scale <= MAX_SCALE
    }
}

/// `extension-views.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GeometryFile {
    pub version: u32,
    /// Keyed by `<extension>/<view>`.
    pub views: BTreeMap<String, SavedGeometry>,
}

impl Default for GeometryFile {
    fn default() -> Self {
        Self {
            version: FILE_VERSION,
            views: BTreeMap::new(),
        }
    }
}

/// The key of one view in the file.
pub fn entry_key(extension_id: &str, view_id: &str) -> String {
    format!("{extension_id}/{view_id}")
}

impl GeometryFile {
    /// Read the file's JSON. Anything unreadable gives an empty file (it is
    /// rewritten on the next save); an entry that doesn't parse or isn't
    /// sane is dropped on its own.
    pub fn parse(text: &str) -> Self {
        let mut file = Self::default();
        let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(text)
        else {
            return file;
        };
        let Some(serde_json::Value::Object(views)) = root.get("views") else {
            return file;
        };
        for (key, value) in views {
            if let Ok(entry) = serde_json::from_value::<SavedGeometry>(value.clone()) {
                if entry.is_sane() {
                    file.views.insert(key.clone(), entry);
                }
            }
        }
        file
    }

    /// Insert or replace one entry, then drop the oldest beyond
    /// [`MAX_ENTRIES`].
    pub fn put(&mut self, key: String, entry: SavedGeometry) {
        self.views.insert(key, entry);
        while self.views.len() > MAX_ENTRIES {
            let Some(oldest) = self
                .views
                .iter()
                .min_by(|a, b| a.1.saved_at.cmp(&b.1.saved_at).then(a.0.cmp(b.0)))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.views.remove(&oldest);
        }
    }

    /// Remove one view's entry, or every entry of the extension when
    /// `view_id` is `None`. Returns how many went.
    pub fn remove(&mut self, extension_id: &str, view_id: Option<&str>) -> u32 {
        let before = self.views.len();
        match view_id {
            Some(view) => {
                self.views.remove(&entry_key(extension_id, view));
            }
            None => {
                let prefix = format!("{extension_id}/");
                self.views.retain(|k, _| !k.starts_with(&prefix));
            }
        }
        (before - self.views.len()) as u32
    }
}

/// A monitor as placement sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub name: String,
    pub rect: Rect,
    pub work_area: Rect,
    pub scale: f64,
    pub primary: bool,
}

impl MonitorInfo {
    /// The monitor as the file records it.
    pub fn saved(&self) -> SavedMonitor {
        SavedMonitor {
            name: self.name.clone(),
            x: self.rect.x,
            y: self.rect.y,
            width: self.rect.width,
            height: self.rect.height,
            scale: self.scale,
        }
    }
}

/// Which monitor contains a point (the cursor), if any.
pub fn monitor_at(monitors: &[MonitorInfo], x: f64, y: f64) -> Option<usize> {
    monitors.iter().position(|m| m.rect.contains(x, y))
}

/// What opened the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenerKind {
    /// The floating launcher: place the view below it when it fits.
    Floating,
    Other,
}

/// The opener window, physical.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenerInfo {
    pub kind: OpenerKind,
    pub rect: Rect,
    /// Index into the monitor list.
    pub monitor: Option<usize>,
}

/// Where and how big a view window opens (physical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub width: u32,
    pub height: u32,
    /// `None`: let the platform place it (Wayland), or centre it when no
    /// monitor is known.
    pub position: Option<(i32, i32)>,
    /// Maximise after showing.
    pub maximized: bool,
}

fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 && scale <= MAX_SCALE {
        scale
    } else {
        1.0
    }
}

fn to_physical(logical: f64, scale: f64) -> u32 {
    (logical * scale).round().clamp(1.0, u32::MAX as f64) as u32
}

/// Shrink `rect` to fit `area` and move it inside.
fn fit_into(rect: Rect, area: Rect) -> Rect {
    let width = rect.width.min(area.width).max(1);
    let height = rect.height.min(area.height).max(1);
    let max_x = area.right() - width as i64;
    let max_y = area.bottom() - height as i64;
    let x = (rect.x as i64).clamp(area.x as i64, max_x.max(area.x as i64));
    let y = (rect.y as i64).clamp(area.y as i64, max_y.max(area.y as i64));
    Rect::new(x as i32, y as i32, width, height)
}

/// One dimension of a new window: at most 90% of the work area, at least
/// the minimum, and never beyond the work area.
fn clamp_dim(size: u32, min: u32, work: u32) -> u32 {
    let cap = ((work as f64) * WORK_AREA_FRACTION).floor() as u32;
    size.min(cap.max(1)).max(min).min(work.max(1))
}

/// Where a view window opens (§7.5):
///
/// 1. **Saved and still valid**: a monitor with the saved name and
///    physical rect exists, and the saved rect overlaps its work area by at
///    least half its own area. The saved rect, shrunk to fit the work area.
/// 2. **Otherwise** on the opener's monitor, else the cursor's, else the
///    primary. Size: the saved logical size at the target's scale, else
///    `defaultSize`, at most 90% of the work area and at least `minSize`.
///    Position: centred 16 px below the floating launcher when it fits
///    there, else centred in the work area.
/// 3. With `can_position == false` (Wayland) only the size counts.
///
/// `maximized` follows the saved entry.
pub fn place(
    saved: Option<&SavedGeometry>,
    monitors: &[MonitorInfo],
    opener: Option<&OpenerInfo>,
    cursor_monitor: Option<usize>,
    decl: &ViewDecl,
    can_position: bool,
) -> Placement {
    let maximized = saved.is_some_and(|s| s.maximized);

    if let Some(s) = saved {
        let rect = s.rect();
        let still_there = monitors
            .iter()
            .find(|m| m.name == s.monitor.name && m.rect == s.monitor.rect());
        if let Some(m) = still_there {
            if rect.area() > 0 && 2 * rect.intersection_area(&m.work_area) >= rect.area() {
                let fitted = fit_into(rect, m.work_area);
                return Placement {
                    width: fitted.width,
                    height: fitted.height,
                    position: can_position.then_some((fitted.x, fitted.y)),
                    maximized,
                };
            }
        }
    }

    let in_range = |i: &usize| *i < monitors.len();
    let target = opener
        .and_then(|o| o.monitor)
        .filter(in_range)
        .or(cursor_monitor.filter(in_range))
        .or_else(|| monitors.iter().position(|m| m.primary))
        .or(if monitors.is_empty() { None } else { Some(0) });

    // The logical size to aim for.
    let (logical_w, logical_h) = match saved {
        Some(s) => {
            let scale = sane_scale(s.monitor.scale);
            (s.width as f64 / scale, s.height as f64 / scale)
        }
        None => (
            decl.default_size.width as f64,
            decl.default_size.height as f64,
        ),
    };

    let Some(index) = target else {
        // No monitor information: logical pixels at scale 1, placed by the
        // window system (the caller centres it).
        return Placement {
            width: to_physical(logical_w, 1.0).max(decl.min_size.width),
            height: to_physical(logical_h, 1.0).max(decl.min_size.height),
            position: None,
            maximized,
        };
    };
    let monitor = &monitors[index];
    let scale = sane_scale(monitor.scale);
    let work = monitor.work_area;
    let width = clamp_dim(
        to_physical(logical_w, scale),
        to_physical(decl.min_size.width as f64, scale),
        work.width,
    );
    let height = clamp_dim(
        to_physical(logical_h, scale),
        to_physical(decl.min_size.height as f64, scale),
        work.height,
    );

    if !can_position {
        return Placement {
            width,
            height,
            position: None,
            maximized,
        };
    }

    let centre_x = |anchor: i64| -> i32 {
        let x = anchor - width as i64 / 2;
        let max_x = (work.right() - width as i64).max(work.x as i64);
        x.clamp(work.x as i64, max_x) as i32
    };

    if let Some(o) = opener.filter(|o| o.kind == OpenerKind::Floating && o.monitor == Some(index)) {
        let top = o.rect.bottom() + (LAUNCHER_GAP * scale).round() as i64;
        if top + height as i64 <= work.bottom() {
            let anchor = o.rect.x as i64 + o.rect.width as i64 / 2;
            return Placement {
                width,
                height,
                position: Some((centre_x(anchor), top as i32)),
                maximized,
            };
        }
    }

    let anchor = work.x as i64 + work.width as i64 / 2;
    let y = work.y as i64 + (work.height as i64 - height as i64) / 2;
    Placement {
        width,
        height,
        position: Some((centre_x(anchor), y.max(work.y as i64) as i32)),
        maximized,
    }
}

/// What to save for a window: its rect and monitor now, or, while it is
/// maximised, the size it had before (so un-maximising restores a usable
/// window) with the flag set. A maximised window with nothing saved yet
/// keeps its maximised rect.
pub fn captured(
    previous: Option<&SavedGeometry>,
    rect: Rect,
    monitor: &MonitorInfo,
    maximized: bool,
    saved_at: String,
) -> SavedGeometry {
    match previous {
        Some(prev) if maximized => SavedGeometry {
            maximized: true,
            saved_at,
            ..prev.clone()
        },
        _ => SavedGeometry {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            maximized,
            monitor: monitor.saved(),
            saved_at,
        },
    }
}

/// Serialises read-modify-write of the file within this process.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// `<data_local_dir>/kage/extension-views.json`.
pub fn file_path() -> Option<PathBuf> {
    Some(crate::paths::data_local_dir()?.join("kage").join(FILE_NAME))
}

fn read_at(path: &Path) -> GeometryFile {
    match std::fs::read_to_string(path) {
        Ok(text) => GeometryFile::parse(&text),
        Err(_) => GeometryFile::default(),
    }
}

fn write_at(path: &Path, file: &GeometryFile) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(file).map_err(io::Error::other)?;
    crate::atomic_write::write(path, json, crate::atomic_write::Perms::Private)
}

fn no_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "no local data directory for window positions",
    )
}

/// The saved geometry of one view, if any.
pub fn load_entry(extension_id: &str, view_id: &str) -> Option<SavedGeometry> {
    let path = file_path()?;
    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_at(&path)
        .views
        .remove(&entry_key(extension_id, view_id))
}

/// Read the view's entry, let `update` compute the new one (`None` keeps
/// the file as it is), and write the file.
pub fn update_entry(
    extension_id: &str,
    view_id: &str,
    update: impl FnOnce(Option<&SavedGeometry>) -> Option<SavedGeometry>,
) -> io::Result<()> {
    let path = file_path().ok_or_else(no_path)?;
    update_entry_at(&path, extension_id, view_id, update)
}

fn update_entry_at(
    path: &Path,
    extension_id: &str,
    view_id: &str,
    update: impl FnOnce(Option<&SavedGeometry>) -> Option<SavedGeometry>,
) -> io::Result<()> {
    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = read_at(path);
    let key = entry_key(extension_id, view_id);
    let Some(entry) = update(file.views.get(&key)) else {
        return Ok(());
    };
    file.put(key, entry);
    write_at(path, &file)
}

/// Forget one view's geometry, or all of an extension's (`view_id` is
/// `None`). Returns how many entries went; the file is rewritten only when
/// one did.
pub fn remove_entries(extension_id: &str, view_id: Option<&str>) -> io::Result<u32> {
    let Some(path) = file_path() else {
        return Ok(0);
    };
    remove_entries_at(&path, extension_id, view_id)
}

fn remove_entries_at(path: &Path, extension_id: &str, view_id: Option<&str>) -> io::Result<u32> {
    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !path.exists() {
        return Ok(0);
    }
    let mut file = read_at(path);
    let removed = file.remove(extension_id, view_id);
    if removed > 0 {
        write_at(path, &file)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::views::{ViewDecl, ViewSize};

    fn decl() -> ViewDecl {
        ViewDecl {
            id: "explorer".into(),
            title: "Explorer".into(),
            icon: "🗺️".into(),
            module: "./explorer.js".into(),
            css: vec![],
            default_size: ViewSize {
                width: 1200,
                height: 800,
            },
            min_size: ViewSize {
                width: 640,
                height: 420,
            },
        }
    }

    /// 2560×1440 at scale 1 on the left, 5120×2880 at scale 2 on the
    /// right; each with a 40 px menu bar at the top.
    fn monitors() -> Vec<MonitorInfo> {
        vec![
            MonitorInfo {
                name: "DISPLAY1".into(),
                rect: Rect::new(0, 0, 2560, 1440),
                work_area: Rect::new(0, 40, 2560, 1400),
                scale: 1.0,
                primary: true,
            },
            MonitorInfo {
                name: "DISPLAY2".into(),
                rect: Rect::new(2560, 0, 5120, 2880),
                work_area: Rect::new(2560, 80, 5120, 2800),
                scale: 2.0,
                primary: false,
            },
        ]
    }

    fn saved_on(m: &MonitorInfo, rect: Rect) -> SavedGeometry {
        SavedGeometry {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            maximized: false,
            monitor: m.saved(),
            saved_at: "2026-11-09T10:00:00Z".into(),
        }
    }

    #[test]
    fn saved_and_valid_restores_the_rect() {
        let mons = monitors();
        let saved = saved_on(&mons[1], Rect::new(3000, 300, 2400, 1600));
        let p = place(Some(&saved), &mons, None, None, &decl(), true);
        assert_eq!(
            p,
            Placement {
                width: 2400,
                height: 1600,
                position: Some((3000, 300)),
                maximized: false
            }
        );
        let mut maxed = saved.clone();
        maxed.maximized = true;
        assert!(place(Some(&maxed), &mons, None, None, &decl(), true).maximized);
    }

    #[test]
    fn saved_rect_is_shrunk_and_moved_to_fit() {
        let mons = monitors();
        // Hanging off the right edge by less than half: kept, moved in.
        let saved = saved_on(&mons[0], Rect::new(1800, 100, 1200, 800));
        let p = place(Some(&saved), &mons, None, None, &decl(), true);
        assert_eq!(p.position, Some((2560 - 1200, 100)));
        // Taller than the work area: shrunk to it.
        let saved = saved_on(&mons[0], Rect::new(0, 0, 1000, 2000));
        let p = place(Some(&saved), &mons, None, None, &decl(), true);
        assert_eq!((p.width, p.height), (1000, 1400));
        assert_eq!(p.position, Some((0, 40)));
    }

    #[test]
    fn mostly_off_screen_is_not_restored() {
        let mons = monitors();
        // Only a quarter of it is on the work area.
        let saved = saved_on(&mons[0], Rect::new(2560 - 300, 100, 1200, 800));
        let p = place(Some(&saved), &mons, None, None, &decl(), true);
        // Rule 2: primary monitor, saved logical size, centred.
        assert_eq!((p.width, p.height), (1200, 800));
        assert_eq!(p.position, Some(((2560 - 1200) / 2, 40 + (1400 - 800) / 2)));
    }

    #[test]
    fn monitor_gone_centres_on_the_active_monitor_at_the_saved_logical_size() {
        let mons = monitors();
        let gone = MonitorInfo {
            name: "DISPLAY3".into(),
            rect: Rect::new(-1920, 0, 1920, 1080),
            work_area: Rect::new(-1920, 0, 1920, 1080),
            scale: 2.0,
            primary: false,
        };
        // 1400×900 physical at scale 2 is 700×450 logical.
        let saved = saved_on(&gone, Rect::new(-1800, 50, 1400, 900));
        let p = place(Some(&saved), &mons, None, Some(1), &decl(), true);
        // On the cursor's monitor (scale 2): 1400×900 physical again.
        assert_eq!((p.width, p.height), (1400, 900));
        assert_eq!(
            p.position,
            Some((2560 + (5120 - 1400) / 2, 80 + (2800 - 900) / 2))
        );
        // On a scale-1 monitor the same logical size is 700×450, but never
        // below minSize (640×420).
        let p = place(Some(&saved), &mons, None, Some(0), &decl(), true);
        assert_eq!((p.width, p.height), (700, 450));
    }

    #[test]
    fn monitor_resized_counts_as_gone() {
        let mons = monitors();
        let mut was = mons[0].clone();
        was.rect = Rect::new(0, 0, 1920, 1080);
        let saved = saved_on(&was, Rect::new(100, 100, 1000, 700));
        let p = place(Some(&saved), &mons, None, None, &decl(), true);
        // Not restored at (100, 100): centred on the primary instead.
        assert_eq!(p.position, Some(((2560 - 1000) / 2, 40 + (1400 - 700) / 2)));
    }

    #[test]
    fn different_scale_converts_the_default_size() {
        let mons = monitors();
        let opener = OpenerInfo {
            kind: OpenerKind::Other,
            rect: Rect::new(3000, 200, 1600, 1400),
            monitor: Some(1),
        };
        let p = place(None, &mons, Some(&opener), Some(0), &decl(), true);
        assert_eq!((p.width, p.height), (2400, 1600));
        assert_eq!(
            p.position,
            Some((2560 + (5120 - 2400) / 2, 80 + (2800 - 1600) / 2))
        );
    }

    #[test]
    fn size_is_capped_at_90_percent_of_the_work_area() {
        let small = vec![MonitorInfo {
            name: "LAPTOP".into(),
            rect: Rect::new(0, 0, 1280, 800),
            work_area: Rect::new(0, 25, 1280, 775),
            scale: 1.0,
            primary: true,
        }];
        let p = place(None, &small, None, None, &decl(), true);
        assert_eq!((p.width, p.height), (1152, 697));
    }

    #[test]
    fn min_size_is_a_floor_but_never_beyond_the_work_area() {
        let tiny = vec![MonitorInfo {
            name: "TINY".into(),
            rect: Rect::new(0, 0, 700, 500),
            work_area: Rect::new(0, 0, 700, 430),
            scale: 1.0,
            primary: true,
        }];
        let p = place(None, &tiny, None, None, &decl(), true);
        // 90% would be 630×387; minSize 640×420 wins.
        assert_eq!((p.width, p.height), (640, 420));
        let tinier = vec![MonitorInfo {
            work_area: Rect::new(0, 0, 600, 400),
            ..tiny[0].clone()
        }];
        let p = place(None, &tinier, None, None, &decl(), true);
        assert_eq!((p.width, p.height), (600, 400));
        assert_eq!(p.position, Some((0, 0)));
    }

    #[test]
    fn below_the_launcher_when_it_fits() {
        let mons = monitors();
        let launcher = OpenerInfo {
            kind: OpenerKind::Floating,
            rect: Rect::new(1000, 100, 570, 76),
            monitor: Some(0),
        };
        let p = place(None, &mons, Some(&launcher), None, &decl(), true);
        assert_eq!((p.width, p.height), (1200, 800));
        assert_eq!(p.position, Some((1000 + 285 - 600, 100 + 76 + 16)));
        // At the left edge: clamped into the work area.
        let left = OpenerInfo {
            rect: Rect::new(0, 100, 570, 76),
            ..launcher.clone()
        };
        let p = place(None, &mons, Some(&left), None, &decl(), true);
        assert_eq!(p.position, Some((0, 192)));
        // On a scale-2 monitor the gap is 32 physical pixels.
        let hi = OpenerInfo {
            kind: OpenerKind::Floating,
            rect: Rect::new(4000, 200, 1140, 152),
            monitor: Some(1),
        };
        let p = place(None, &mons, Some(&hi), None, &decl(), true);
        assert_eq!(p.position.map(|(_, y)| y), Some(200 + 152 + 32));
    }

    #[test]
    fn centred_when_it_does_not_fit_below_the_launcher() {
        let mons = monitors();
        let launcher = OpenerInfo {
            kind: OpenerKind::Floating,
            rect: Rect::new(1000, 900, 570, 76),
            monitor: Some(0),
        };
        let p = place(None, &mons, Some(&launcher), None, &decl(), true);
        assert_eq!(p.position, Some(((2560 - 1200) / 2, 40 + (1400 - 800) / 2)));
        // Another opener kind is never placed below.
        let settings = OpenerInfo {
            kind: OpenerKind::Other,
            rect: Rect::new(1000, 100, 570, 76),
            monitor: Some(0),
        };
        let p = place(None, &mons, Some(&settings), None, &decl(), true);
        assert_eq!(p.position, Some(((2560 - 1200) / 2, 40 + (1400 - 800) / 2)));
    }

    #[test]
    fn wayland_gets_a_size_only() {
        let mons = monitors();
        let saved = saved_on(&mons[1], Rect::new(3000, 300, 2400, 1600));
        let p = place(Some(&saved), &mons, None, None, &decl(), false);
        assert_eq!((p.width, p.height, p.position), (2400, 1600, None));
        let p = place(None, &mons, None, None, &decl(), false);
        assert_eq!((p.width, p.height, p.position), (1200, 800, None));
    }

    #[test]
    fn no_monitors_uses_logical_sizes() {
        let p = place(None, &[], None, None, &decl(), true);
        assert_eq!(
            p,
            Placement {
                width: 1200,
                height: 800,
                position: None,
                maximized: false
            }
        );
    }

    #[test]
    fn cursor_monitor_lookup() {
        let mons = monitors();
        assert_eq!(monitor_at(&mons, 10.0, 10.0), Some(0));
        assert_eq!(monitor_at(&mons, 2560.0, 10.0), Some(1));
        assert_eq!(monitor_at(&mons, -5.0, 10.0), None);
    }

    #[test]
    fn capture_keeps_the_restored_size_while_maximised() {
        let mons = monitors();
        let prev = saved_on(&mons[0], Rect::new(100, 100, 1200, 800));
        let full = Rect::new(0, 40, 2560, 1400);
        let got = captured(Some(&prev), full, &mons[0], true, "t2".into());
        assert_eq!(got.rect(), prev.rect());
        assert!(got.maximized);
        assert_eq!(got.saved_at, "t2");
        let got = captured(None, full, &mons[0], true, "t3".into());
        assert_eq!(got.rect(), full);
        assert!(got.maximized);
        let moved = Rect::new(3000, 300, 2400, 1600);
        let got = captured(Some(&prev), moved, &mons[1], false, "t4".into());
        assert_eq!(got.rect(), moved);
        assert_eq!(got.monitor, mons[1].saved());
        assert!(!got.maximized);
    }

    fn temp_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kage").join(FILE_NAME);
        (dir, path)
    }

    #[test]
    fn file_round_trip() {
        let (_dir, path) = temp_file();
        let mons = monitors();
        let entry = saved_on(&mons[1], Rect::new(312, 140, 2400, 1600));
        update_entry_at(&path, "mindmap", "explorer", |prev| {
            assert!(prev.is_none());
            Some(entry.clone())
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["views"]["mindmap/explorer"]["width"], 2400);
        assert_eq!(
            json["views"]["mindmap/explorer"]["monitor"]["name"],
            "DISPLAY2"
        );
        assert_eq!(json["views"]["mindmap/explorer"]["monitor"]["scale"], 2.0);
        assert_eq!(read_at(&path).views.get("mindmap/explorer"), Some(&entry));
        // `None` from the update leaves the file alone.
        update_entry_at(&path, "mindmap", "explorer", |prev| {
            assert_eq!(prev, Some(&entry));
            None
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "owner-only");
        }
    }

    #[test]
    fn corrupt_file_is_ignored_and_rewritten() {
        let (_dir, path) = temp_file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(read_at(&path).views.is_empty());
        let mons = monitors();
        update_entry_at(&path, "a", "b", |_| {
            Some(saved_on(&mons[0], Rect::new(0, 40, 800, 600)))
        })
        .unwrap();
        assert_eq!(read_at(&path).views.len(), 1);
    }

    #[test]
    fn bad_entries_are_dropped_one_by_one() {
        let text = r#"{ "version": 1, "views": {
            "a/ok": { "x": 1, "y": 2, "width": 800, "height": 600,
                      "monitor": { "name": "M", "x": 0, "y": 0, "width": 1920, "height": 1080, "scale": 1.0 } },
            "a/zero": { "x": 1, "y": 2, "width": 0, "height": 600,
                      "monitor": { "name": "M", "x": 0, "y": 0, "width": 1920, "height": 1080, "scale": 1.0 } },
            "a/scale": { "x": 1, "y": 2, "width": 800, "height": 600,
                      "monitor": { "name": "M", "x": 0, "y": 0, "width": 1920, "height": 1080, "scale": 0 } },
            "a/junk": "nope"
        } }"#;
        let file = GeometryFile::parse(text);
        assert_eq!(file.views.keys().collect::<Vec<_>>(), vec!["a/ok"]);
        assert!(GeometryFile::parse("[]").views.is_empty());
        assert!(GeometryFile::parse(r#"{"views": []}"#).views.is_empty());
    }

    #[test]
    fn at_most_64_entries_oldest_first() {
        let mons = monitors();
        let mut file = GeometryFile::default();
        for i in 0..MAX_ENTRIES + 3 {
            let mut entry = saved_on(&mons[0], Rect::new(0, 40, 800, 600));
            entry.saved_at = format!("2026-11-09T{:02}:{:02}:00Z", i / 60, i % 60);
            file.put(format!("ext/v{i:03}"), entry);
        }
        assert_eq!(file.views.len(), MAX_ENTRIES);
        for gone in ["ext/v000", "ext/v001", "ext/v002"] {
            assert!(!file.views.contains_key(gone), "{gone}");
        }
        assert!(file.views.contains_key("ext/v003"));
        assert!(file
            .views
            .contains_key(&format!("ext/v{:03}", MAX_ENTRIES + 2)));
    }

    #[test]
    fn removal_by_view_and_by_extension() {
        let (_dir, path) = temp_file();
        assert_eq!(remove_entries_at(&path, "mindmap", None).unwrap(), 0);
        assert!(!path.exists(), "nothing to remove writes nothing");
        let mons = monitors();
        for key in [("mindmap", "explorer"), ("mindmap", "board"), ("mind", "x")] {
            update_entry_at(&path, key.0, key.1, |_| {
                Some(saved_on(&mons[0], Rect::new(0, 40, 800, 600)))
            })
            .unwrap();
        }
        assert_eq!(
            remove_entries_at(&path, "mindmap", Some("board")).unwrap(),
            1
        );
        assert_eq!(
            remove_entries_at(&path, "mindmap", Some("board")).unwrap(),
            0
        );
        // By extension: a prefix of another id ("mind") isn't touched.
        assert_eq!(remove_entries_at(&path, "mindmap", None).unwrap(), 1);
        assert_eq!(
            read_at(&path).views.keys().collect::<Vec<_>>(),
            vec!["mind/x"]
        );
    }

    #[test]
    fn the_file_lives_in_local_data() {
        let path = file_path().unwrap();
        assert!(path.ends_with(std::path::Path::new("kage").join(FILE_NAME)));
        assert!(path.starts_with(crate::paths::data_local_dir().unwrap()));
    }
}
