use super::{kind_to_subdir, user_item_dir, ExtensionManifest, InstalledItem};
use log::warn;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Scan a directory for manifest.json files, returning discovered items.
fn scan_directory(dir: &PathBuf, enabled_states: &HashMap<String, bool>) -> Vec<InstalledItem> {
    let mut items = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return items,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Dot-dirs are install staging/backup/parked-upgrade siblings (see
        // install.rs); loading one would duplicate or downgrade an item.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        match fs::read_to_string(&manifest_path) {
            Ok(content) => match serde_json::from_str::<ExtensionManifest>(&content) {
                Ok(manifest) => {
                    let enabled = enabled_states.get(&manifest.id).copied().unwrap_or(true);
                    items.push(InstalledItem {
                        path: path.to_string_lossy().to_string(),
                        enabled,
                        manifest,
                    });
                }
                Err(e) => {
                    warn!("Invalid manifest at {:?}: {}", manifest_path, e);
                }
            },
            Err(e) => {
                warn!("Failed to read {:?}: {}", manifest_path, e);
            }
        }
    }
    items
}

/// Discover all installed items of a given kind. `kind` is "extension",
/// "theme", or "commands". Reads from the per-user install dir under
/// `<config_dir>/kage/<subdir>/`. Was previously bundled+user-merged with
/// the binary shipping a small set of in-tree extensions; that mode is
/// gone now — every extension goes through the normal store install path.
pub fn discover_items(kind: &str, enabled_states: &HashMap<String, bool>) -> Vec<InstalledItem> {
    let mut by_id: HashMap<String, InstalledItem> = HashMap::new();

    if let Ok(subdir) = kind_to_subdir(kind) {
        if let Ok(user_dir) = user_item_dir(subdir) {
            for item in scan_directory(&user_dir, enabled_states) {
                by_id.insert(item.manifest.id.clone(), item);
            }
        }
    }

    let mut items: Vec<InstalledItem> = by_id.into_values().collect();
    items.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_skips_hidden_install_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        for dir in ["todo", ".todo.pending", ".todo.bak"] {
            let path = base.join(dir);
            fs::create_dir_all(&path).unwrap();
            let manifest = serde_json::json!({
                "id": "todo", "name": dir, "version": "1.0.0", "type": "extension",
            });
            fs::write(path.join("manifest.json"), manifest.to_string()).unwrap();
        }

        let items = scan_directory(&base, &HashMap::new());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].manifest.name, "todo");
    }
}
