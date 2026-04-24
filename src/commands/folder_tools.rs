use crate::error::AppError;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri_plugin_dialog::DialogExt;

/// Metadata for a single file entry returned by scan_folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative path from the scanned root (forward slashes)
    pub path: String,
    /// Size in bytes
    pub size: u64,
    /// Last modified as compact timestamp (YYYY-MM-DDTHH:MM)
    pub modified: String,
    /// Whether this is a directory
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_dir: bool,
    /// Fast content hash (hex) for duplicate detection — only for files ≤ 50 MB
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// Result of scanning a folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub root: String,
    pub total_files: usize,
    pub total_dirs: usize,
    pub total_size: u64,
    pub entries: Vec<FileEntry>,
    /// Groups of duplicate files (same hash). Key = hash, value = list of relative paths.
    pub duplicates: HashMap<String, Vec<String>>,
    /// Whether the scan was truncated due to file count limit
    pub truncated: bool,
}

/// A single operation in a folder organization plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderOperation {
    /// "move", "rename", or "delete"
    pub action: String,
    /// Source path (relative to root)
    pub from: String,
    /// Destination path (relative to root) — not used for "delete"
    #[serde(default)]
    pub to: Option<String>,
    /// Human-readable reason for this operation (e.g. "empty directory", "temporary file")
    #[serde(default)]
    pub reason: Option<String>,
}

/// Result of executing a folder plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanExecutionResult {
    pub success: bool,
    pub operations_completed: usize,
    pub operations_failed: usize,
    pub errors: Vec<String>,
    /// Rollback manifest — list of (from, to) pairs that were actually moved
    pub rollback: Vec<(String, String)>,
}

const MAX_FILES: usize = 10_000;
const MAX_HASH_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
const MAX_DEPTH: usize = 10;

/// Return a map of well-known folder names to their absolute paths on this system.
#[tauri::command]
pub fn get_common_folders() -> HashMap<String, String> {
    let mut folders = HashMap::new();

    #[allow(clippy::type_complexity)]
    let candidates: &[(&str, fn() -> Option<PathBuf>)] = &[
        ("downloads", dirs::download_dir),
        ("documents", dirs::document_dir),
        ("pictures", dirs::picture_dir),
        ("videos", dirs::video_dir),
        ("music", dirs::audio_dir),
        ("desktop", dirs::desktop_dir),
        ("home", dirs::home_dir),
        ("templates", dirs::template_dir),
        ("public", dirs::public_dir),
        ("cache", dirs::cache_dir),
        ("config", dirs::config_dir),
        ("data", dirs::data_dir),
    ];

    for (name, resolver) in candidates {
        if let Some(path) = resolver() {
            if path.is_dir() {
                folders.insert(name.to_string(), path.to_string_lossy().to_string());
            }
        }
    }

    // Screenshots subfolder of pictures
    if let Some(pics) = dirs::picture_dir() {
        let screenshots = pics.join("Screenshots");
        if screenshots.is_dir() {
            folders.insert("screenshots".to_string(), screenshots.to_string_lossy().to_string());
        }
    }

    // System fonts directory
    if let Some(font_dir) = crate::os::fonts_dir() {
        if font_dir.is_dir() {
            folders.insert("fonts".to_string(), font_dir.to_string_lossy().to_string());
        }
    }

    // Temp directory
    let temp = std::env::temp_dir();
    if temp.is_dir() {
        folders.insert("temp".to_string(), temp.to_string_lossy().to_string());
    }

    folders
}

/// Open a native folder picker dialog. Returns the selected path or null.
#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, AppError> {
    info!("Opening native folder picker dialog");

    // Use blocking_pick_folder on a blocking thread to avoid blocking the async runtime
    let result = tauri::async_runtime::spawn_blocking(move || {
        let file_resp = app.dialog().file().set_title("Select Folder to Organize").blocking_pick_folder();
        file_resp.map(|p| p.to_string())
    })
    .await
    .map_err(|e| format!("Dialog task failed: {}", e))?;

    if let Some(ref path) = result {
        info!("Folder selected: {}", path);
    } else {
        info!("Folder picker cancelled");
    }
    Ok(result)
}

/// Scan a folder recursively and return metadata for all entries.
#[tauri::command]
pub async fn scan_folder(
    path: String,
    max_depth: Option<usize>,
    compute_hashes: Option<bool>,
) -> Result<ScanResult, AppError> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("'{}' is not a directory", path).into());
    }

    let depth_limit = max_depth.unwrap_or(MAX_DEPTH);
    let do_hashes = compute_hashes.unwrap_or(true);

    info!("Scanning folder: {} (depth={}, hashes={})", path, depth_limit, do_hashes);

    // Run the scan on a blocking thread since it's I/O heavy
    let result = tauri::async_runtime::spawn_blocking(move || {
        scan_directory(&root, depth_limit, do_hashes)
    })
    .await
    .map_err(|e| format!("Scan task failed: {}", e))?;

    info!(
        "Scan complete: {} files, {} dirs, {} bytes, {} duplicate groups",
        result.total_files, result.total_dirs, result.total_size,
        result.duplicates.len()
    );
    Ok(result)
}

/// Execute a folder organization plan (list of move/rename/delete operations).
#[tauri::command]
pub async fn execute_folder_plan(
    root: String,
    operations: Vec<FolderOperation>,
) -> Result<PlanExecutionResult, AppError> {
    let root_path = PathBuf::from(&root);
    if !root_path.is_dir() {
        return Err(format!("'{}' is not a directory", root).into());
    }

    info!("Executing folder plan: {} operations in {}", operations.len(), root);

    let result = tauri::async_runtime::spawn_blocking(move || {
        execute_plan(&root_path, &operations)
    })
    .await
    .map_err(|e| format!("Plan execution task failed: {}", e))?;

    info!(
        "Plan execution: {} completed, {} failed",
        result.operations_completed, result.operations_failed
    );
    Ok(result)
}

// ── Internal helpers ──────────────────────────────────────────────────

/// Scan a directory recursively and return a manifest of all files.
/// Public so the computer-control MCP binary can use it directly.
pub fn scan_directory(root: &Path, max_depth: usize, compute_hashes: bool) -> ScanResult {
    let mut state = WalkState {
        entries: Vec::new(),
        total_files: 0,
        total_dirs: 0,
        total_size: 0,
        truncated: false,
    };

    // Hash → list of relative paths (for duplicate detection)
    let mut hash_map: HashMap<String, Vec<String>> = HashMap::new();

    walk_dir(root, root, 0, max_depth, compute_hashes, &mut state);

    // Build duplicate groups from hashes
    if compute_hashes {
        for entry in &state.entries {
            if let Some(ref hash) = entry.hash {
                hash_map.entry(hash.clone()).or_default().push(entry.path.clone());
            }
        }
    }

    // Only keep groups with 2+ files
    let duplicates: HashMap<String, Vec<String>> = hash_map
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .collect();

    ScanResult {
        root: root.to_string_lossy().to_string(),
        total_files: state.total_files,
        total_dirs: state.total_dirs,
        total_size: state.total_size,
        entries: state.entries,
        duplicates,
        truncated: state.truncated,
    }
}

/// Mutable state accumulated during directory walk.
struct WalkState {
    entries: Vec<FileEntry>,
    total_files: usize,
    total_dirs: usize,
    total_size: u64,
    truncated: bool,
}

fn walk_dir(
    root: &Path,
    current: &Path,
    depth: usize,
    max_depth: usize,
    compute_hashes: bool,
    state: &mut WalkState,
) {
    if depth > max_depth || state.truncated {
        return;
    }

    let read_dir = match std::fs::read_dir(current) {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Cannot read directory {}: {}", current.display(), e);
            return;
        }
    };

    for entry_result in read_dir {
        if state.truncated {
            return;
        }

        let dir_entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = dir_entry.path();
        let file_name = dir_entry.file_name().to_string_lossy().to_string();

        // Normalize Unicode whitespace (e.g. non-breaking space U+00A0) to regular space.
        // macOS screenshots and some apps use non-breaking spaces in filenames, which
        // causes mismatches after JSON round-tripping through the agent.
        let file_name = normalize_whitespace(&file_name);

        // Skip hidden files/dirs (starting with .)
        if file_name.starts_with('.') {
            continue;
        }

        // Skip our own trash directory to avoid scanning/nesting it
        if file_name == "_kage_trash" {
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let relative = path.strip_prefix(root).unwrap_or(&path);
        let relative_str = relative.to_string_lossy().replace('\\', "/");

        let is_dir = metadata.is_dir();
        let size = if is_dir { 0 } else { metadata.len() };
        let modified = metadata.modified()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%dT%H:%M").to_string()
            })
            .unwrap_or_default();

        // Compute hash for non-directory files within size limit
        let hash = if !is_dir && compute_hashes && size > 0 && size <= MAX_HASH_SIZE {
            compute_file_hash(&path)
        } else {
            None
        };

        if is_dir {
            state.total_dirs += 1;
        } else {
            state.total_files += 1;
            state.total_size += size;
        }

        state.entries.push(FileEntry {
            path: relative_str,
            size,
            modified,
            is_dir,
            hash,
        });

        if state.entries.len() >= MAX_FILES {
            state.truncated = true;
            return;
        }

        // Recurse into subdirectories
        if is_dir {
            walk_dir(root, &path, depth + 1, max_depth, compute_hashes, state);
        }
    }
}

/// Compute a fast hash of a file's contents using a simple FNV-like approach.
/// We read the first 64KB + last 64KB + file size for a fast fingerprint.
fn compute_file_hash(path: &Path) -> Option<String> {
    use std::io::Read;
    use std::io::Seek;

    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();

    let mut hasher_data = Vec::new();

    // Include file size in the hash
    hasher_data.extend_from_slice(&file_len.to_le_bytes());

    // Read first 64KB
    let mut buf = vec![0u8; 65536.min(file_len as usize)];
    let n = file.read(&mut buf).ok()?;
    hasher_data.extend_from_slice(&buf[..n]);

    // If file is larger than 128KB, also read the last 64KB
    if file_len > 131072 {
        let seek_pos = file_len - 65536;
        file.seek(std::io::SeekFrom::Start(seek_pos)).ok()?;
        let mut tail = vec![0u8; 65536];
        let n = file.read(&mut tail).ok()?;
        hasher_data.extend_from_slice(&tail[..n]);
    }

    // Simple FNV-1a 64-bit hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in &hasher_data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }

    Some(format!("{:016x}", hash))
}

/// Reject relative paths that would escape the root (contain `..`, absolute, or have prefixes).
/// Returns the normalized relative form, or an error reason.
fn validate_rel_path(rel: &str) -> Result<String, String> {
    if rel.is_empty() {
        return Err("empty path".to_string());
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                return Err("'..' components are not allowed".to_string());
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                return Err("path prefixes/root are not allowed".to_string());
            }
            _ => {}
        }
    }
    Ok(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Confirm that `candidate` resolves to a path inside `root`. Works for paths that
/// may not yet exist by canonicalizing the deepest existing ancestor. Symlinks
/// that point outside are rejected.
fn ensure_within_root(root: &Path, candidate: &Path) -> Result<(), String> {
    let root_canon = match root.canonicalize() {
        Ok(p) => p,
        Err(_) => root.to_path_buf(),
    };
    // Walk up to find the nearest existing ancestor, then canonicalize that and
    // append the remaining components without following further symlinks.
    let mut existing = candidate.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let anchor = existing.canonicalize().unwrap_or(existing);
    let mut resolved = anchor;
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    if resolved.starts_with(&root_canon) {
        Ok(())
    } else {
        Err(format!(
            "path '{}' escapes root '{}'",
            resolved.display(),
            root_canon.display()
        ))
    }
}

/// Execute a folder organization plan. Public for MCP binary access.
pub fn execute_plan(root: &Path, operations: &[FolderOperation]) -> PlanExecutionResult {
    let mut completed = 0;
    let mut failed = 0;
    let mut errors = Vec::new();
    let mut rollback = Vec::new();

    for op in operations {
        // Validate the source relative path — reject .., absolute, prefixes.
        let from_normalized = match validate_rel_path(&op.from) {
            Ok(n) => n,
            Err(e) => {
                errors.push(format!("'{}': invalid source path ({})", op.from, e));
                failed += 1;
                continue;
            }
        };
        let from_abs = root.join(&from_normalized);

        // If exact path doesn't exist, resolve via normalized whitespace lookup.
        // This handles filenames with non-breaking spaces (U+00A0) that got normalized
        // to regular spaces during scan → JSON → agent → JSON round-trip.
        let from_abs = if from_abs.exists() {
            from_abs
        } else {
            resolve_normalized_path(root, &from_normalized).unwrap_or(from_abs)
        };

        // Defence-in-depth: ensure resolved source sits inside the root (symlink-safe).
        if let Err(e) = ensure_within_root(root, &from_abs) {
            errors.push(format!("'{}': refused ({})", op.from, e));
            failed += 1;
            continue;
        }

        match op.action.as_str() {
            "move" | "rename" => {
                let to_rel = match &op.to {
                    Some(t) => t,
                    None => {
                        errors.push(format!("'{}': move/rename requires 'to' field", op.from));
                        failed += 1;
                        continue;
                    }
                };
                let to_normalized = match validate_rel_path(to_rel) {
                    Ok(n) => n,
                    Err(e) => {
                        errors.push(format!("'{}': invalid destination ({})", to_rel, e));
                        failed += 1;
                        continue;
                    }
                };
                let to_abs = root.join(&to_normalized);
                if let Err(e) = ensure_within_root(root, &to_abs) {
                    errors.push(format!("'{}': destination refused ({})", to_rel, e));
                    failed += 1;
                    continue;
                }

                // Create parent directories
                if let Some(parent) = to_abs.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        errors.push(format!("Cannot create dir {}: {}", parent.display(), e));
                        failed += 1;
                        continue;
                    }
                }

                // Don't overwrite existing files
                if to_abs.exists() {
                    errors.push(format!("'{}': destination already exists", to_rel));
                    failed += 1;
                    continue;
                }

                // Pre-check: verify source exists
                if !from_abs.exists() {
                    // Log the actual directory contents for debugging
                    let parent = from_abs.parent().unwrap_or(root);
                    let dir_entries: Vec<String> = std::fs::read_dir(parent)
                        .map(|rd| rd.filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().to_string())
                            .collect())
                        .unwrap_or_default();
                    warn!("Source file not found: {} (parent has {} entries: {:?})",
                        from_abs.display(), dir_entries.len(),
                        dir_entries.iter().take(5).collect::<Vec<_>>());
                    errors.push(format!("Move {} → {}: source file not found", op.from, to_rel));
                    failed += 1;
                    continue;
                }

                match std::fs::rename(&from_abs, &to_abs) {
                    Ok(_) => {
                        rollback.push((to_rel.clone(), op.from.clone()));
                        completed += 1;
                    }
                    Err(e) => {
                        // Log hex bytes of the filename for debugging encoding issues
                        let from_hex: String = from_abs.to_string_lossy()
                            .chars().map(|c| {
                                if c.is_ascii_graphic() || c == ' ' { format!("{}", c) }
                                else { format!("[U+{:04X}]", c as u32) }
                            }).collect();
                        let exists = from_abs.exists();
                        warn!("Failed to move '{}' → '{}': {} (exists={}, from_abs={}, hex={})",
                            op.from, to_rel, e, exists, from_abs.display(), from_hex);
                        errors.push(format!("Move {} → {}: {}", op.from, to_rel, e));
                        failed += 1;
                    }
                }
            }
            "delete" => {
                if !from_abs.exists() {
                    errors.push(format!("'{}': file not found", op.from));
                    failed += 1;
                    continue;
                }

                // Don't re-trash files that are already in the trash
                if op.from.starts_with("_kage_trash/") || op.from.starts_with("_kage_trash\\") {
                    errors.push(format!("'{}': already in trash, skipping", op.from));
                    failed += 1;
                    continue;
                }

                // Safety: move to a _kage_trash subfolder instead of actual delete
                let trash_dir = root.join("_kage_trash");
                if let Err(e) = std::fs::create_dir_all(&trash_dir) {
                    errors.push(format!("Cannot create trash dir: {}", e));
                    failed += 1;
                    continue;
                }

                // Use just the filename to avoid nesting paths inside trash
                let file_name = Path::new(&op.from).file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| op.from.clone());
                let trash_dest = trash_dir.join(&file_name);

                // If a file with the same name already exists in trash, add a timestamp
                // plus a counter suffix to avoid collisions within the same second.
                let trash_dest = if trash_dest.exists() {
                    let stem = Path::new(&file_name).file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| file_name.clone());
                    let ext = Path::new(&file_name).extension()
                        .map(|e| format!(".{}", e.to_string_lossy()))
                        .unwrap_or_default();
                    let ts = chrono::Local::now().format("%Y%m%d%H%M%S");
                    let mut candidate = trash_dir.join(format!("{}_{}{}", stem, ts, ext));
                    let mut counter: u32 = 1;
                    while candidate.exists() {
                        candidate = trash_dir.join(format!("{}_{}_{}{}", stem, ts, counter, ext));
                        counter += 1;
                        if counter > 10_000 {
                            // Extremely unlikely; bail out of the loop with the last candidate.
                            break;
                        }
                    }
                    candidate
                } else {
                    trash_dest
                };

                match std::fs::rename(&from_abs, &trash_dest) {
                    Ok(_) => {
                        let trash_rel = format!("_kage_trash/{}", op.from);
                        rollback.push((trash_rel, op.from.clone()));
                        completed += 1;
                    }
                    Err(e) => {
                        errors.push(format!("Delete (trash) {}: {}", op.from, e));
                        failed += 1;
                    }
                }
            }
            other => {
                errors.push(format!("Unknown action '{}' for '{}'", other, op.from));
                failed += 1;
            }
        }
    }

    PlanExecutionResult {
        success: failed == 0,
        operations_completed: completed,
        operations_failed: failed,
        errors,
        rollback,
    }
}

/// Replace Unicode whitespace characters with regular ASCII space (U+0020).
/// This handles non-breaking spaces (U+00A0), thin spaces, etc. that appear
/// in filenames from macOS screenshots and other apps.
fn normalize_whitespace(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c != ' ' && c.is_whitespace() {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Resolve a normalized relative path back to the actual OS path.
/// Walks each path component and finds the real directory entry whose
/// normalized name matches.
fn resolve_normalized_path(root: &Path, normalized_rel: &str) -> Option<PathBuf> {
    let components: Vec<&str> = normalized_rel.split(std::path::MAIN_SEPARATOR).collect();
    let mut current = root.to_path_buf();

    for component in &components {
        let target_normalized = *component;
        let mut found = false;

        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                let real_name = entry.file_name().to_string_lossy().to_string();
                if normalize_whitespace(&real_name) == target_normalized {
                    current = entry.path();
                    found = true;
                    break;
                }
            }
        }

        if !found {
            return None;
        }
    }

    Some(current)
}
