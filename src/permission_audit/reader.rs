use super::{default_log_path, AuditEntry};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Read the last `limit` entries, most-recent-first. Tolerates
/// corrupt/partial JSON lines by logging and skipping. A missing log
/// file is NOT an error — the return is just an empty Vec.
///
/// Reads the file backwards in chunks so the caller pays for ~`limit`
/// entries regardless of how large the log has grown. JSONL is
/// well-suited to this: every line is self-contained, so a chunk-aligned
/// suffix can be parsed in isolation as long as we discard the partial
/// first line (which gets re-read as part of the next chunk further back).
pub fn read_recent(path: &Path, limit: usize) -> Vec<AuditEntry> {
    if limit == 0 || !path.exists() {
        return Vec::new();
    }
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("audit log: failed to open {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let total_len = match file.seek(SeekFrom::End(0)) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("audit log: seek failed for {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    if total_len == 0 {
        return Vec::new();
    }

    // 32 KB chunks — typical entry is well under 512 B (tool name +
    // timestamp + a small args preview), so one chunk usually covers
    // 60+ entries. We grow it in subsequent reads if a single chunk
    // didn't cover `limit` lines.
    const CHUNK: u64 = 32 * 1024;

    // Buffer holds the suffix of the file we've read so far. We always
    // hold the bytes for at least one *complete* line at the start,
    // which lets us hand the rest off to the line iterator below
    // confident that no entry is split across our parse boundary.
    let mut tail: Vec<u8> = Vec::new();
    let mut cursor = total_len;
    let mut hit_bof = false;

    let mut entries: Vec<AuditEntry> = Vec::new();

    while !hit_bof && entries.len() < limit {
        let read_size = CHUNK.min(cursor);
        cursor -= read_size;
        if cursor == 0 {
            hit_bof = true;
        }

        if let Err(e) = file.seek(SeekFrom::Start(cursor)) {
            log::warn!("audit log: seek failed for {}: {}", path.display(), e);
            return Vec::new();
        }
        let mut chunk = vec![0u8; read_size as usize];
        if let Err(e) = file.read_exact(&mut chunk) {
            log::warn!("audit log: read failed for {}: {}", path.display(), e);
            return Vec::new();
        }

        // Prepend the chunk we just read to whatever tail we already
        // had. Together they form a contiguous suffix of the file.
        chunk.extend_from_slice(&tail);
        tail = chunk;

        // If we haven't reached BOF yet, the first line in `tail` is
        // probably partial (the previous read split through the middle
        // of a line). Drop it — we'll pick it up on the next chunk.
        // When we *have* reached BOF, the first line is whole.
        let parse_start = if hit_bof {
            0
        } else {
            match memchr_newline(&tail) {
                Some(i) => i + 1, // skip past the newline
                None => {
                    // No newline yet — the entire chunk is a single
                    // partial line. Loop and read more.
                    continue;
                }
            }
        };

        // Parse all complete lines in tail[parse_start..], collect
        // entries newest-last (file order), then reverse only the
        // batch we just gathered to push them newest-first into
        // `entries`.
        let mut batch: Vec<AuditEntry> = Vec::new();
        for line in tail[parse_start..].split(|&b| b == b'\n') {
            // Trim a trailing \r so Windows-style line endings parse cleanly.
            let line = match line.split_last() {
                Some((&b'\r', rest)) => rest,
                _ => line,
            };
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<AuditEntry>(line) {
                Ok(entry) => batch.push(entry),
                Err(e) => {
                    log::warn!(
                        "audit log: skipping malformed entry in {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }
        // batch is in file order — newest-last. Push reversed so
        // `entries` is newest-first overall.
        for e in batch.into_iter().rev() {
            entries.push(e);
            if entries.len() >= limit {
                break;
            }
        }

        // We've consumed everything up to the last newline boundary in
        // `tail`. Reset for the next iteration: keep only the partial
        // prefix (bytes before parse_start) so the next read can stitch
        // onto it.
        tail.truncate(parse_start);
    }

    entries
}

/// Find the index of the first `\n` in `bytes`, if any.
/// Tiny helper — kept inline rather than pulling in the `memchr` crate.
fn memchr_newline(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|&b| b == b'\n')
}

/// Convenience: read from the default path.
pub fn read_recent_default(limit: usize) -> Vec<AuditEntry> {
    let Some(path) = default_log_path() else {
        return Vec::new();
    };
    read_recent(&path, limit)
}
