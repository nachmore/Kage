//! Guard: Kage code resolves per-user directories through `kage::paths`,
//! never `dirs::*_dir()` directly, so tests can point them at a scratch
//! dir. A direct call is how a local `cargo test` once overwrote a
//! developer's real config.json (see kage_core::paths).
//!
//! Function *references* (`dirs::config_dir` without a call, used by the
//! "open my Documents folder" tables) are fine: they resolve folders for
//! the user to look at, not files Kage writes.

use std::path::Path;

const DIRECT_CALLS: &[&str] = &[
    "dirs::config_dir()",
    "dirs::config_local_dir()",
    "dirs::data_dir()",
    "dirs::data_local_dir()",
    "dirs::cache_dir()",
    "dirs::home_dir()",
];

fn scan(dir: &Path, offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, offenders);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") || path.ends_with("src/paths.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if DIRECT_CALLS.iter().any(|c| line.contains(c)) {
                offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
}

#[test]
fn app_code_resolves_user_dirs_through_kage_paths() {
    let mut offenders = Vec::new();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut offenders,
    );
    assert!(
        offenders.is_empty(),
        "use crate::paths::<dir>() instead of dirs::<dir>():\n{}",
        offenders.join("\n")
    );
}
