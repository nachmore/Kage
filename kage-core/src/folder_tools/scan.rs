use super::normalize_whitespace;
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

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
/// Bytes read from each end of a large file for its fingerprint.
const FINGERPRINT_CHUNK: u64 = 64 * 1024;
/// Files up to this size are hashed in full; larger ones get a head+tail
/// fingerprint. Either way, hash matches are confirmed by content
/// comparison before being reported as duplicates.
const FULL_HASH_LIMIT: u64 = 2 * FINGERPRINT_CHUNK;

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

    walk_dir(root, root, 0, max_depth, compute_hashes, &mut state);

    let duplicates = if compute_hashes {
        find_duplicates(root, &mut state.entries)
    } else {
        HashMap::new()
    };

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

/// Group entries by hash into duplicate sets of 2+ files. The agent plans
/// deletes from these groups, so a match must mean identical content:
/// each hash group (large-file hashes skip the middle) is split by a
/// byte comparison, and each split-off class gets a distinct `-N` hash
/// suffix so equal `entry.hash` values keep meaning "same content".
fn find_duplicates(root: &Path, entries: &mut [FileEntry]) -> HashMap<String, Vec<String>> {
    // Hash → indices into `entries`
    let mut by_hash: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(ref hash) = entry.hash {
            by_hash.entry(hash.clone()).or_default().push(i);
        }
    }

    let mut duplicates: HashMap<String, Vec<String>> = HashMap::new();
    for (hash, indices) in by_hash {
        if indices.len() < 2 {
            continue;
        }
        // Confirm every group by content, not just large-file ones: a
        // 64-bit FNV match is not proof of identity (collisions can be
        // crafted), and the agent may delete files based on these groups.
        // Small files are cheap to re-read.
        let paths: Vec<PathBuf> = indices
            .iter()
            .map(|&i| root.join(&entries[i].path))
            .collect();
        let classes: Vec<Vec<usize>> = partition_by_content(&paths)
            .into_iter()
            .map(|class| class.into_iter().map(|p| indices[p]).collect())
            .collect();
        for (n, class) in classes.into_iter().enumerate() {
            let key = if n == 0 {
                hash.clone()
            } else {
                let key = format!("{}-{}", hash, n);
                for &i in &class {
                    entries[i].hash = Some(key.clone());
                }
                key
            };
            if class.len() > 1 {
                let paths: Vec<String> = class.iter().map(|&i| entries[i].path.clone()).collect();
                duplicates.insert(key, paths);
            }
        }
    }
    duplicates
}

/// Split `paths` into classes of byte-identical files, returned as indices
/// into `paths`. A file that can't be read ends up in a class of its own.
fn partition_by_content(paths: &[PathBuf]) -> Vec<Vec<usize>> {
    let mut classes: Vec<Vec<usize>> = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        match classes
            .iter_mut()
            .find(|class| files_equal(&paths[class[0]], path))
        {
            Some(class) => class.push(i),
            None => classes.push(vec![i]),
        }
    }
    classes
}

fn files_equal(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let mut buf_a = Vec::with_capacity(FINGERPRINT_CHUNK as usize);
    let mut buf_b = Vec::with_capacity(FINGERPRINT_CHUNK as usize);
    loop {
        buf_a.clear();
        buf_b.clear();
        // take+read_to_end keeps reading through short reads, so equal
        // files always yield equal chunks.
        let (Ok(n), Ok(_)) = (
            (&mut fa).take(FINGERPRINT_CHUNK).read_to_end(&mut buf_a),
            (&mut fb).take(FINGERPRINT_CHUNK).read_to_end(&mut buf_b),
        ) else {
            return false;
        };
        if buf_a != buf_b {
            return false;
        }
        if n == 0 {
            return true;
        }
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

/// Compute a fast FNV-1a hash of a file: size + full content for files up
/// to FULL_HASH_LIMIT, size + first and last FINGERPRINT_CHUNK otherwise.
/// Hash matches are confirmed by content in `find_duplicates`.
fn compute_file_hash(path: &Path) -> Option<String> {
    use std::io::Seek;

    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();

    let mut hasher_data = Vec::with_capacity((8 + file_len.min(FULL_HASH_LIMIT)) as usize);

    // Include file size in the hash
    hasher_data.extend_from_slice(&file_len.to_le_bytes());

    // take+read_to_end keeps reading through short reads, unlike one read().
    if file_len <= FULL_HASH_LIMIT {
        (&mut file)
            .take(FULL_HASH_LIMIT)
            .read_to_end(&mut hasher_data)
            .ok()?;
    } else {
        (&mut file)
            .take(FINGERPRINT_CHUNK)
            .read_to_end(&mut hasher_data)
            .ok()?;
        file.seek(std::io::SeekFrom::Start(file_len - FINGERPRINT_CHUNK))
            .ok()?;
        file.take(FINGERPRINT_CHUNK)
            .read_to_end(&mut hasher_data)
            .ok()?;
    }

    // Simple FNV-1a 64-bit hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in &hasher_data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }

    Some(format!("{:016x}", hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh scratch dir per test so parallel tests don't collide.
    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kage_scan_test_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hash_of(result: &ScanResult, path: &str) -> Option<String> {
        result
            .entries
            .iter()
            .find(|e| e.path == path)
            .and_then(|e| e.hash.clone())
    }

    fn sorted_group(result: &ScanResult) -> Vec<String> {
        let mut group = result.duplicates.values().next().unwrap().clone();
        group.sort();
        group
    }

    #[test]
    fn mid_size_files_differing_after_first_chunk_are_not_duplicates() {
        let dir = scratch_dir("mid");
        let a = vec![7u8; 100 * 1024];
        let mut b = a.clone();
        b[90 * 1024] = 8;
        std::fs::write(dir.join("a.bin"), &a).unwrap();
        std::fs::write(dir.join("b.bin"), &b).unwrap();

        let result = scan_directory(&dir, 1, true);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.duplicates.is_empty(), "{:?}", result.duplicates);
        assert_ne!(hash_of(&result, "a.bin"), hash_of(&result, "b.bin"));
    }

    #[test]
    fn large_files_differing_only_in_middle_are_not_duplicates() {
        let dir = scratch_dir("large");
        let b = vec![1u8; 300 * 1024];
        let mut a = b.clone();
        a[150 * 1024] = 2;
        std::fs::write(dir.join("a.bin"), &a).unwrap();
        std::fs::write(dir.join("b.bin"), &b).unwrap();
        std::fs::write(dir.join("c.bin"), &b).unwrap();

        let result = scan_directory(&dir, 1, true);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result.duplicates.len(), 1, "{:?}", result.duplicates);
        assert_eq!(sorted_group(&result), vec!["b.bin", "c.bin"]);
        assert_ne!(hash_of(&result, "a.bin"), hash_of(&result, "b.bin"));
        assert_eq!(hash_of(&result, "b.bin"), hash_of(&result, "c.bin"));
    }

    #[test]
    fn identical_small_files_are_duplicates() {
        let dir = scratch_dir("small");
        std::fs::write(dir.join("a.txt"), b"same content").unwrap();
        std::fs::write(dir.join("b.txt"), b"same content").unwrap();
        std::fs::write(dir.join("c.txt"), b"other content").unwrap();

        let result = scan_directory(&dir, 1, true);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result.duplicates.len(), 1, "{:?}", result.duplicates);
        assert_eq!(sorted_group(&result), vec!["a.txt", "b.txt"]);
    }
}
