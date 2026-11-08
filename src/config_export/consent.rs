//! What an extension may do is consented to on this machine, never taken
//! from a backup.
//!
//! An import, of any kind and bundle version (encrypted or not, v1 to v3),
//! never adds or widens a capability grant: the merged config's
//! `extension_grants` is this machine's, whole. A grant only the backup
//! has is dropped, and if the extension is installed here and its grant
//! here doesn't cover what its manifest requests, it is switched off and
//! named in the summary ("needs your approval again"). The user approves it
//! the way every grant is made: the install-time prompt, which records the
//! grant through `commit_extension_install` (the Store's Approve button
//! re-runs it for an installed extension).
//!
//! `extension_states` may switch an extension on only when its grant here
//! already covers every capability its installed manifest requests; one
//! that isn't installed (no manifest to check) stays as this machine has
//! it. A theme runs no code, so its switch is the backup's.

use crate::config::Config;
use crate::extensions::InstalledItem;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// The capabilities a backup's `config.json` (as JSON, before any merge)
/// says each extension was granted. Read leniently: an entry whose list
/// can't be read claims a grant with no capabilities.
pub fn grant_claims(bundle: &Value) -> BTreeMap<String, Vec<String>> {
    let Some(grants) = bundle.get("extension_grants").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    grants
        .iter()
        .filter(|(id, _)| crate::extensions::validate_extension_id(id).is_ok())
        .map(|(id, g)| {
            let caps = g
                .get("granted")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            (id.clone(), caps)
        })
        .collect()
}

/// Make `merged` (an import's result) keep this machine's consent: grants
/// exactly `local`'s, and no extension switched on that its grant here
/// doesn't cover. `claims` is what the backup said was granted
/// ([`grant_claims`]). Returns the installed extensions (ids, sorted) left
/// off until the user approves them.
pub fn keep_local(
    merged: &mut Config,
    local: &Config,
    claims: &BTreeMap<String, Vec<String>>,
    installed: &[InstalledItem],
) -> Vec<String> {
    merged.extension_grants = local.extension_grants.clone();

    // Every id the backup could switch on (this machine has it off) or
    // claims a grant for.
    let mut ids: BTreeSet<&str> = claims.keys().map(String::as_str).collect();
    ids.extend(
        local
            .extension_states
            .iter()
            .filter(|(_, on)| !**on)
            .map(|(id, _)| id.as_str()),
    );
    let ids: Vec<String> = ids.into_iter().map(str::to_string).collect();

    let mut needs_approval = Vec::new();
    for id in ids {
        let code: Vec<&InstalledItem> = installed
            .iter()
            .filter(|i| i.manifest.kind == "extension" && i.manifest.id == id)
            .collect();
        let theme_only = code.is_empty()
            && installed
                .iter()
                .any(|i| i.manifest.kind == "theme" && i.manifest.id == id);
        let grant = local.extension_grants.get(&id);
        let granted: HashSet<&str> = grant
            .map(|g| g.granted.iter().map(String::as_str).collect())
            .unwrap_or_default();
        // Every installed folder for the id runs only what this machine
        // granted: the grant covers all each one requests.
        let covered = !code.is_empty()
            && code.iter().all(|item| {
                crate::extensions::normalize_permissions(
                    item.manifest.permissions.as_deref().unwrap_or(&[]),
                    &id,
                )
                .iter()
                .all(|cap| granted.contains(cap.as_str()))
            });
        let on_here = local.extension_states.get(&id) != Some(&false);
        let on_after = merged.extension_states.get(&id) != Some(&false);
        let turns_on = on_after && !on_here;
        // The backup says it was granted something this machine never was.
        let claims_more = claims.get(&id).is_some_and(|caps| {
            grant.is_none() || caps.iter().any(|c| !granted.contains(c.as_str()))
        });

        let blocked_on = turns_on && !covered && !theme_only;
        let unapproved = claims_more && !code.is_empty() && !covered;
        if blocked_on || unapproved {
            merged.extension_states.insert(id.clone(), false);
            if !code.is_empty() {
                needs_approval.push(id);
            }
        }
    }
    needs_approval
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ExtensionGrant;
    use serde_json::json;

    fn item(kind: &str, id: &str, perms: &[&str]) -> InstalledItem {
        InstalledItem {
            manifest: serde_json::from_value(json!({
                "id": id, "name": id, "version": "1.0.0", "type": kind, "permissions": perms
            }))
            .unwrap(),
            path: String::new(),
            enabled: true,
        }
    }

    fn grant(caps: &[&str]) -> ExtensionGrant {
        ExtensionGrant {
            granted: caps.iter().map(|s| s.to_string()).collect(),
            approved_version: "1.0.0".into(),
            approved_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_backup_never_adds_or_widens_a_grant() {
        // The review's probe: no grant here, a crafted bundle granting
        // ["network", "storage"], which the manifest does request.
        let installed = [
            item("extension", "fetcher", &["network", "storage"]),
            item("extension", "todos", &["storage", "network"]),
            item("extension", "plain", &[]),
        ];
        let mut local = Config::default();
        local
            .extension_grants
            .insert("todos".into(), grant(&["storage"]));
        local.extension_states.insert("todos".into(), true);
        let bundle = json!({ "extension_grants": {
            "fetcher": { "granted": ["network", "storage"], "approved_version": "1.0.0" },
            "todos": { "granted": ["storage", "network"], "approved_version": "1.0.0" },
            "plain": { "granted": [] },
            "gone": { "granted": ["storage"] }
        }});
        let mut merged = local.clone();
        merged.extension_grants = serde_json::from_value(json!({
            "fetcher": { "granted": ["network", "storage"], "approved_version": "1.0.0", "approved_at": "x" }
        }))
        .unwrap();
        let needs = keep_local(&mut merged, &local, &grant_claims(&bundle), &installed);
        assert_eq!(
            merged.extension_grants, local.extension_grants,
            "grants stay this machine's"
        );
        assert_eq!(needs, vec!["fetcher".to_string(), "todos".to_string()]);
        assert_eq!(merged.extension_states.get("fetcher"), Some(&false));
        assert_eq!(merged.extension_states.get("todos"), Some(&false));
        // Requests nothing: nothing to approve.
        assert_eq!(merged.extension_states.get("plain"), None);
        // Not installed: the grant is just dropped.
        assert_eq!(merged.extension_states.get("gone"), None);
    }

    #[test]
    fn a_grant_this_machine_has_stays_and_its_extension_keeps_running() {
        let installed = [item("extension", "todos", &["storage"])];
        let mut local = Config::default();
        local
            .extension_grants
            .insert("todos".into(), grant(&["storage"]));
        // The backup's (narrower or equal) grant changes nothing.
        for caps in [json!([]), json!(["storage"])] {
            let bundle = json!({ "extension_grants": { "todos": { "granted": caps } } });
            let mut merged = local.clone();
            let needs = keep_local(&mut merged, &local, &grant_claims(&bundle), &installed);
            assert!(needs.is_empty());
            assert_eq!(merged.extension_grants, local.extension_grants);
            assert_eq!(merged.extension_states.get("todos"), None);
        }
    }

    #[test]
    fn a_backup_switches_an_extension_on_only_when_this_machines_grant_covers_it() {
        let installed = [
            item("extension", "covered", &["storage"]),
            item("extension", "partial", &["storage", "network"]),
            item("extension", "ungranted", &["storage"]),
            item("theme", "skin", &[]),
        ];
        let mut local = Config::default();
        for id in ["covered", "partial", "ungranted", "skin", "elsewhere"] {
            local.extension_states.insert(id.into(), false);
        }
        local
            .extension_grants
            .insert("covered".into(), grant(&["storage"]));
        local
            .extension_grants
            .insert("partial".into(), grant(&["storage"]));
        let mut merged = local.clone();
        for id in ["covered", "partial", "ungranted", "skin", "elsewhere"] {
            merged.extension_states.insert(id.into(), true);
        }
        let needs = keep_local(&mut merged, &local, &BTreeMap::new(), &installed);
        assert_eq!(merged.extension_states.get("covered"), Some(&true));
        assert_eq!(merged.extension_states.get("partial"), Some(&false));
        assert_eq!(merged.extension_states.get("ungranted"), Some(&false));
        assert_eq!(
            merged.extension_states.get("skin"),
            Some(&true),
            "a theme runs no code"
        );
        assert_eq!(
            merged.extension_states.get("elsewhere"),
            Some(&false),
            "not installed: no manifest to check"
        );
        assert_eq!(needs, vec!["partial".to_string(), "ungranted".to_string()]);
        // Removing this machine's "off" is switching it on too.
        let mut merged = local.clone();
        merged.extension_states.remove("ungranted");
        keep_local(&mut merged, &local, &BTreeMap::new(), &installed);
        assert_eq!(merged.extension_states.get("ungranted"), Some(&false));
        // Switching off is always the backup's.
        let mut local = Config::default();
        local.extension_states.insert("ungranted".into(), true);
        let mut merged = local.clone();
        merged.extension_states.insert("ungranted".into(), false);
        assert!(keep_local(&mut merged, &local, &BTreeMap::new(), &installed).is_empty());
        assert_eq!(merged.extension_states.get("ungranted"), Some(&false));
    }

    #[test]
    fn every_folder_for_an_id_must_be_covered() {
        let installed = [
            item("extension", "dup", &["storage"]),
            item("extension", "dup", &["storage", "oauth"]),
        ];
        let mut local = Config::default();
        local.extension_states.insert("dup".into(), false);
        local
            .extension_grants
            .insert("dup".into(), grant(&["storage"]));
        let mut merged = local.clone();
        merged.extension_states.insert("dup".into(), true);
        assert_eq!(
            keep_local(&mut merged, &local, &BTreeMap::new(), &installed),
            vec!["dup".to_string()]
        );
        assert_eq!(merged.extension_states.get("dup"), Some(&false));
    }
}
