use super::normalize_whitespace;
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

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
const MAX_FILES: usize = 10_000;
const MAX_HASH_SIZE: u64 = 50 * 1024 * 1024; // 50 MB

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
                hash_map
                    .entry(hash.clone())
                    .or_default()
                    .push(entry.path.clone());
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
        let modified = metadata
            .modified()
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
