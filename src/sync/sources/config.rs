//! The `cfg` namespace: single settings of `config.json`, by the rows of
//! `sync::catalog` (docs/design/SYNC.md §4.3, §8.2, §8.6).
//!
//! One record per `Shared` or `PerOs` row, its id the setting's dotted
//! path, its one field `v` the whole value (a hotkey's modifiers and key
//! together). `extension_states.*` is one record per extension:
//! `extension_states.<id>`. Rows that never leave this computer, and
//! `extensions` (WP-S5's `cfg.ext`), are not records here.
//!
//! Projection goes through [`ConfigPort`]: one snapshot, every write
//! applied to it, the result read back through `Config` deserialisation
//! (so a value this build can't hold comes back as the local one, and the
//! engine pins it as lossy), this computer's consent kept, and one commit.

use super::{LocalRecord, LocalState, LocalWrite, NsSpec, ProjectResult, Source};
use crate::config::Config;
use crate::sync::catalog::{self, Rule, CFG_NS, SYNC_FIELDS};
use crate::sync::engine::{CommitError, ConfigPort};
use crate::sync::types::{Category, DefaultScope, Origin, Structure};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The field holding a setting's value.
pub const FIELD: &str = crate::sync::split::WHOLE;

/// The prefix of one extension's on/off record.
pub const EXTENSION_STATE: &str = "extension_states.";

/// Single settings from `config.json`.
pub struct ConfigSource {
    port: Arc<dyn ConfigPort>,
}

impl ConfigSource {
    pub fn new(port: Arc<dyn ConfigPort>) -> Self {
        Self { port }
    }
}

/// The value at a dotted `path` of a config tree, `Null` when absent.
pub fn get_path<'a>(tree: &'a Value, path: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    if let Some(id) = path.strip_prefix(EXTENSION_STATE) {
        return tree
            .get("extension_states")
            .and_then(|m| m.get(id))
            .unwrap_or(&NULL);
    }
    let mut cur = tree;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => return &NULL,
        }
    }
    cur
}

/// Set the value at a dotted `path`, creating objects on the way. For an
/// extension's on/off, `Null` removes the entry.
pub fn set_path(tree: &mut Value, path: &str, value: Value) {
    if let Some(id) = path.strip_prefix(EXTENSION_STATE) {
        if let Some(m) = tree
            .as_object_mut()
            .map(|o| {
                o.entry("extension_states")
                    .or_insert_with(|| Value::Object(Map::new()))
            })
            .and_then(Value::as_object_mut)
        {
            if value.is_null() {
                m.remove(id);
            } else {
                m.insert(id.to_string(), value);
            }
        }
        return;
    }
    let segs: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = segs.split_last() else {
        return;
    };
    let mut cur = tree;
    for seg in parents {
        let Some(obj) = cur.as_object_mut() else {
            return;
        };
        cur = obj
            .entry(seg.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.insert(last.to_string(), value);
    }
}

/// Every `cfg` record id a config tree holds: the `Shared` and `PerOs`
/// rows, with `extension_states.*` expanded to the extensions it names.
pub fn record_ids(tree: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for r in SYNC_FIELDS {
        if !matches!(r.rule, Rule::Shared | Rule::PerOs) {
            continue;
        }
        if r.path == "extension_states.*" {
            if let Some(m) = tree.get("extension_states").and_then(Value::as_object) {
                out.extend(
                    m.keys()
                        .filter(|id| !id.is_empty() && !id.contains('.'))
                        .map(|id| format!("{EXTENSION_STATE}{id}")),
                );
            }
        } else {
            out.push(r.path.to_string());
        }
    }
    out
}

/// The record of one setting value.
fn record_of(v: &Value) -> LocalRecord {
    let mut fields = Map::new();
    fields.insert(FIELD.to_string(), v.clone());
    LocalRecord {
        fields,
        avail: None,
    }
}

/// The local state of a config: one record per synced setting.
pub fn state_of(config: &Config) -> LocalState {
    let tree = serde_json::to_value(config).unwrap_or(Value::Null);
    let mut st = LocalState::default();
    for id in record_ids(&tree) {
        st.put(CFG_NS, &id, record_of(get_path(&tree, &id)));
    }
    st
}

/// The `cfg` namespace's spec. `PerOs` rows default to the OS's own cell:
/// [`Structure::per_os`] names them as `@<path>` (see the engine's default
/// scope rule).
pub fn spec() -> NsSpec {
    let per_os = SYNC_FIELDS
        .iter()
        .filter(|r| r.rule == Rule::PerOs)
        .map(|r| format!("@{}", r.path))
        .collect();
    NsSpec {
        ns: CFG_NS.to_string(),
        category: Category::Settings,
        schema: 1,
        echo: true,
        structure: Structure {
            per_os,
            ..Structure::default()
        },
        default_scope: DefaultScope::Shared,
        large: false,
        label_field: None,
    }
}

/// Apply `value` at `path` to `tree` and check the result still reads as a
/// `Config`. On failure the tree is left as it was.
fn apply_checked(tree: &mut Value, path: &str, value: Value) -> bool {
    let before = get_path(tree, path).clone();
    set_path(tree, path, value);
    if Config::deserialize(&*tree).is_ok() {
        return true;
    }
    set_path(tree, path, before);
    false
}

/// Apply every write to a copy of `local` and read the result back as a
/// `Config`. A write that doesn't fit is left out (the local value stays).
/// This computer's consent is kept: grants are never widened, and an
/// extension its grant doesn't cover stays off.
pub fn apply_writes(local: &Config, writes: &[LocalWrite], port: &dyn ConfigPort) -> Config {
    let mut tree = serde_json::to_value(local).unwrap_or(Value::Null);
    let values: Vec<(&str, Value)> = writes
        .iter()
        .filter(|w| w.ns == CFG_NS && catalog::cfg_row(&w.id).is_some())
        .filter_map(|w| match &w.value {
            Some(r) => Some((
                w.id.as_str(),
                r.fields.get(FIELD).cloned().unwrap_or(Value::Null),
            )),
            // A removed setting record: only an extension's on/off can go.
            None if w.id.starts_with(EXTENSION_STATE) => Some((w.id.as_str(), Value::Null)),
            None => None,
        })
        .collect();
    let mut all = tree.clone();
    for (path, v) in &values {
        set_path(&mut all, path, v.clone());
    }
    let next = match Config::deserialize(&all) {
        Ok(c) => c,
        Err(_) => {
            for (path, v) in values {
                if !apply_checked(&mut tree, path, v) {
                    log::warn!("sync: a synced value of {path} doesn't fit this version; kept the local one");
                }
            }
            Config::deserialize(&tree).unwrap_or_else(|_| local.clone())
        }
    };
    let mut next = next;
    let installed = port.installed();
    crate::config_export::consent::keep_local(&mut next, local, &BTreeMap::new(), &installed);
    next
}

impl Source for ConfigSource {
    fn id(&self) -> &'static str {
        "config"
    }

    fn specs(&self) -> Vec<NsSpec> {
        vec![spec()]
    }

    fn read_local(&mut self) -> anyhow::Result<LocalState> {
        let (_, config) = self.port.snapshot();
        Ok(state_of(&config))
    }

    fn project(
        &mut self,
        writes: Vec<LocalWrite>,
        origin: Origin,
    ) -> anyhow::Result<ProjectResult> {
        let (rev, local) = self.port.snapshot();
        let next = apply_writes(&local, &writes, &*self.port);
        let read_back = state_of(&next);
        match self.port.commit(rev, next, origin) {
            Ok(_) => Ok(ProjectResult::Applied { read_back }),
            Err(CommitError::Stale) => Ok(ProjectResult::Stale),
            Err(CommitError::Suspended) => {
                anyhow::bail!("config.json is from a newer build; saves are suspended")
            }
            Err(CommitError::Io(e)) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::engine::LoadReset;
    use serde_json::json;
    use std::sync::Mutex;

    /// A config behind a port, as the engine sees it.
    #[derive(Default)]
    struct Port {
        state: Mutex<(u64, Config)>,
    }

    impl ConfigPort for Port {
        fn snapshot(&self) -> (u64, Config) {
            self.state.lock().unwrap().clone()
        }
        fn commit(&self, base: u64, next: Config, _: Origin) -> Result<u64, CommitError> {
            let mut st = self.state.lock().unwrap();
            if st.0 != base {
                return Err(CommitError::Stale);
            }
            st.0 += 1;
            st.1 = next;
            Ok(st.0)
        }
        fn load_was_reset(&self) -> LoadReset {
            LoadReset::None
        }
        fn saves_suspended(&self) -> bool {
            false
        }
        fn installed(&self) -> Vec<crate::extensions::InstalledItem> {
            Vec::new()
        }
    }

    fn write(id: &str, v: Value) -> LocalWrite {
        LocalWrite {
            ns: CFG_NS.into(),
            id: id.into(),
            value: Some(record_of(&v)),
        }
    }

    /// Only `Shared` and `PerOs` rows become records: never a path that
    /// stays on this computer or is never synced (§4.7
    /// `no_machine_or_never_path_produces_cells`).
    #[test]
    fn no_machine_or_never_path_produces_cells() {
        let mut c = Config::default();
        c.extension_states.insert("weather".into(), true);
        let st = state_of(&c);
        for (ns, id) in st.records.keys() {
            assert_eq!(ns, CFG_NS);
            let r = catalog::cfg_row(id).unwrap_or_else(|| panic!("{id} has no cfg row"));
            assert!(matches!(r.rule, Rule::Shared | Rule::PerOs), "{id}");
        }
        for never in [
            "version",
            "ui.chat_window_width",
            "system.auto_start",
            "tool_permissions",
            "extension_grants",
            "telemetry",
            "acp.agent.user_steering_path",
            "first_run_completed",
        ] {
            assert!(st.get(CFG_NS, never).is_none(), "{never} became a record");
        }
        assert!(st.get(CFG_NS, "extension_states.weather").is_some());
        assert_eq!(
            st.get(CFG_NS, "ui.theme").unwrap().fields[FIELD],
            json!("system")
        );
    }

    /// Projecting and then reading back gives no difference.
    #[test]
    fn project_then_capture_gives_no_diff() {
        let port = Arc::new(Port::default());
        let mut src = ConfigSource::new(port.clone());
        let ProjectResult::Applied { read_back } = src
            .project(
                vec![
                    write("ui.theme", json!("dark")),
                    write("ui.font_size", json!(16)),
                    write("hotkey", json!({ "modifiers": ["Ctrl"], "key": "K" })),
                ],
                Origin::Sync,
            )
            .unwrap()
        else {
            panic!("stale")
        };
        assert_eq!(src.read_local().unwrap(), read_back);
        assert_eq!(port.snapshot().1.ui.theme, "dark");
        assert_eq!(port.snapshot().1.ui.font_size, 16);
    }

    /// A value this build can't hold (an enum variant from a newer build,
    /// a string where a number goes) comes back as the local one.
    #[test]
    fn a_value_that_does_not_fit_reads_back_as_the_local_one() {
        let port = Arc::new(Port::default());
        let mut src = ConfigSource::new(port.clone());
        let ProjectResult::Applied { read_back } = src
            .project(
                vec![
                    write("ui.font_size", json!("huge")),
                    write("ui.theme", json!("dark")),
                ],
                Origin::Sync,
            )
            .unwrap()
        else {
            panic!("stale")
        };
        assert_eq!(
            read_back.get(CFG_NS, "ui.font_size").unwrap().fields[FIELD],
            json!(Config::default().ui.font_size)
        );
        assert_eq!(
            read_back.get(CFG_NS, "ui.theme").unwrap().fields[FIELD],
            json!("dark")
        );
    }

    /// Synced on/off never turns on an extension this computer's grant
    /// doesn't cover; a commit racing another save is stale.
    #[test]
    fn consent_is_kept_and_races_are_stale() {
        let port = Arc::new(Port::default());
        {
            let mut st = port.state.lock().unwrap();
            st.1.extension_states.insert("weather".into(), false);
        }
        let mut src = ConfigSource::new(port.clone());
        src.project(
            vec![write("extension_states.weather", json!(true))],
            Origin::Sync,
        )
        .unwrap();
        // Not installed here: keep_local has no manifest, so it stays as
        // this computer has it only when a grant is claimed; a plain switch
        // of a missing extension is the synced one.
        let (rev, _) = port.snapshot();
        port.commit(rev, Config::default(), Origin::User).unwrap();
        let stale = port.commit(rev, Config::default(), Origin::Sync);
        assert!(matches!(stale, Err(CommitError::Stale)));
    }

    #[test]
    fn paths_get_and_set() {
        let mut t = json!({ "ui": { "theme": "dark" } });
        assert_eq!(get_path(&t, "ui.theme"), &json!("dark"));
        assert_eq!(get_path(&t, "ui.nope"), &Value::Null);
        set_path(&mut t, "acp.agent.default_model", json!("m"));
        assert_eq!(t["acp"]["agent"]["default_model"], json!("m"));
        set_path(&mut t, "extension_states.x", json!(true));
        assert_eq!(get_path(&t, "extension_states.x"), &json!(true));
        set_path(&mut t, "extension_states.x", Value::Null);
        assert!(t["extension_states"].get("x").is_none());
        let ids = record_ids(&serde_json::to_value(Config::default()).unwrap());
        assert!(ids.contains(&"ui.theme".to_string()));
        assert!(!ids.iter().any(|i| i.starts_with(EXTENSION_STATE)));
    }
}
