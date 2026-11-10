//! Tests of the extension source that need no engine. The engine-level
//! behaviour (bases, the feed, origins, uninstall) is in
//! `tests/sync_extensions_test.rs`.

use super::*;
use crate::sync::engine::LoadReset;
use serde_json::json;

fn item(manifest: Value) -> InstalledItem {
    let mut m = json!({ "id": "notes", "name": "Notes", "version": "1.0.0", "type": "extension" });
    for (k, v) in manifest.as_object().unwrap() {
        m[k] = v.clone();
    }
    InstalledItem {
        manifest: serde_json::from_value(m).unwrap(),
        path: String::new(),
        enabled: true,
    }
}

fn notes() -> InstalledItem {
    item(json!({
        "permissions": ["storage", "sync"],
        "sensitiveData": ["session_*"],
        "config": {
            "units": { "type": "string", "enum": ["metric", "imperial"] },
            "city": { "type": "string", "default": "" },
            "names": { "type": "string", "sync": "shared" },
            "hotkey": { "type": "string", "sync": "os" },
            "organize": { "type": "boolean", "sync": "machine" },
            "api_key": { "type": "string", "sensitive": true, "sync": "shared" }
        },
        "sync": {
            "keys": ["prefs", "view-*", "session_x", "token"],
            "lists": { "todos": { "array": "items", "id": "id", "label": "title" } },
            "collections": {
                "items": { "label": "title", "sets": ["tags"], "maps": ["meta"], "local": ["rev"] },
                "bodies": { "whole": true, "large": true, "maxBytes": 4194304 }
            }
        }
    }))
}

struct Port {
    config: Mutex<Config>,
    installed: Vec<InstalledItem>,
}

impl ConfigPort for Port {
    fn snapshot(&self) -> (u64, Config) {
        (1, self.config.lock().unwrap().clone())
    }
    fn commit(&self, _: u64, next: Config, _: Origin) -> Result<u64, CommitError> {
        *self.config.lock().unwrap() = next;
        Ok(2)
    }
    fn load_was_reset(&self) -> LoadReset {
        LoadReset::None
    }
    fn saves_suspended(&self) -> bool {
        false
    }
    fn installed(&self) -> Vec<InstalledItem> {
        self.installed.clone()
    }
}

#[test]
fn store_origins_ignore_a_trailing_slash_and_are_short() {
    let a = store_origin("https://store.example/kage");
    assert_eq!(a, store_origin("https://store.example/kage/"));
    assert_ne!(a, store_origin("https://other.example/kage"));
    let hex = a.strip_prefix("store:").unwrap();
    assert_eq!(hex.len(), 16);
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn namespaces_name_their_extension_and_part() {
    assert_eq!(keys_ns("notes"), "ext.notes.k");
    assert_eq!(
        list_of(&list_ns("notes", "to.dos")),
        Some(("notes", "to.dos"))
    );
    assert_eq!(list_of("ext.notes.k"), None);
    assert_eq!(
        crate::sync::engine::collection_of(&collection_ns("notes", "items")),
        Some(("notes", "items"))
    );
    assert_eq!(crate::sync::engine::collection_of("ext.notes.l.c.x"), None);
    assert_eq!(crate::sync::engine::collection_of("cfg.ext"), None);
}

#[test]
fn record_ids_follow_the_contract() {
    for ok in ["a", "item:1", "A-b_c.d", &"x".repeat(128)] {
        assert!(valid_record_id(ok), "{ok}");
    }
    for bad in ["", "a b", "a/b", "é", &"x".repeat(129)] {
        assert!(!valid_record_id(bad), "{bad}");
    }
}

#[test]
fn pushed_records_are_checked_before_anything_is_stored() {
    let d = notes().manifest.sync_decl().unwrap();
    let items = &d.collections["items"];
    let ok = json!({ "id": "n1", "title": "Hi", "tags": ["a"], "meta": { "k": 1 }, "rev": 3 });
    assert_eq!(check_record(&ok, items), None);
    assert_eq!(
        check_record(&json!({ "id": "a b" }), items),
        Some(RejectCode::BadId)
    );
    assert_eq!(
        check_record(&json!({ "title": "x" }), items),
        Some(RejectCode::BadId)
    );
    assert_eq!(check_record(&json!("text"), items), Some(RejectCode::BadId));
    assert_eq!(
        check_record(&json!({ "id": "a", "password": "x" }), items),
        Some(RejectCode::SensitiveField),
        "a credential-named field is refused, not stored"
    );
    assert_eq!(
        check_record(&json!({ "id": "a", "apiKey": "x" }), items),
        Some(RejectCode::SensitiveField)
    );
    assert_eq!(
        check_record(&json!({ "id": "a", "tags": "not a list" }), items),
        Some(RejectCode::BadCollection)
    );
    assert_eq!(
        check_record(&json!({ "id": "a", "x@y": 1 }), items),
        Some(RejectCode::BadCollection)
    );
    let big = json!({ "id": "a", "text": "x".repeat(70_000) });
    assert_eq!(check_record(&big, items), Some(RejectCode::TooLarge));
    // A local field doesn't count: the host drops it.
    let local_big = json!({ "id": "a", "rev": "x".repeat(70_000) });
    assert_eq!(check_record(&local_big, items), None);
    assert_eq!(
        check_record(&big, &d.collections["bodies"]),
        None,
        "a large collection takes 4 MiB"
    );
}

#[test]
fn list_files_split_and_join() {
    let ld = SyncListDecl {
        array: "items".into(),
        id: "id".into(),
        label: None,
    };
    let v = json!({ "v": 2, "items": [{ "id": "a", "t": 1 }, { "id": "b" }] });
    let l = split_list(&v, &ld).unwrap();
    assert!(!l.unusable);
    assert_eq!(
        l.elements
            .iter()
            .map(|(i, _)| i.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(l.rest.as_ref().unwrap().get("v"), Some(&json!(2)));
    let back = join_list(
        &ld,
        l.rest.as_ref(),
        l.elements.into_iter().map(|(_, e)| e).collect(),
    );
    assert_eq!(back, v);
    // An element without an id, a duplicate or a `~` id makes the list
    // unusable for sync: nothing of it is captured or deleted.
    for bad in [
        json!({ "items": [{ "t": 1 }] }),
        json!({ "items": [{ "id": "a" }, { "id": "a" }] }),
        json!({ "items": [{ "id": "~rest" }] }),
    ] {
        assert!(split_list(&bad, &ld).unwrap().unusable, "{bad}");
    }
    assert!(
        split_list(&json!([1]), &ld).is_none(),
        "not the declared shape"
    );
    let root = SyncListDecl {
        array: String::new(),
        id: "id".into(),
        label: None,
    };
    let l = split_list(&json!([{ "id": "x" }]), &root).unwrap();
    assert!(l.rest.is_none());
    assert_eq!(
        join_list(&root, None, vec![json!({ "id": "x" })]),
        json!([{ "id": "x" }])
    );
    assert_eq!(
        list_ids(&json!({ "items": [{ "id": "a" }] }), &ld),
        ["a".to_string()].into()
    );
}

#[test]
fn settings_follow_the_section_12_1_rule() {
    let n = notes();
    let rules = Rules::new(std::slice::from_ref(&n));
    let s = |f: &str, v: Value| setting_syncs(&rules, Some(&n), "notes", f, &v);
    assert!(s("units", json!("metric")), "a declared enum choice");
    assert!(!s("units", json!("kelvin")), "not one of the choices");
    assert!(!s("city", json!("Oslo")), "a free-form string isn't plain");
    assert!(s("names", json!("Ana")), "marked shared");
    assert!(s("hotkey", json!("Ctrl+K")), "marked os");
    assert!(!s("organize", json!(true)), "marked machine");
    assert!(
        !s("api_key", json!("x")),
        "credentials never sync, whatever sync says"
    );
    assert!(!s("refresh_token", json!(true)), "the safety net");
    assert!(!s("__scratch", json!(true)));
    assert!(!s("enabled", json!(true)));
    assert!(!s("session_id", json!(true)), "a sensitiveData key name");
}

#[test]
fn the_journal_records_only_synced_keys_and_bounds_itself() {
    let j = Journal::default();
    j.note_save("notes", "prefs", "main", Some("{}"));
    j.forget_window("main");
    assert!(
        j.take().is_empty(),
        "nothing syncs before the source says so"
    );
    let d = notes().manifest.sync_decl().unwrap();
    j.set_decls(&[("notes".to_string(), d)].into());
    j.note_load("notes", "prefs", "main", Some(r#"{"a":1}"#));
    j.note_load("notes", "prefs", "main", Some(r#"{"a":1}"#));
    j.note_load("notes", "todos", "main", Some(r#"{"items":[{"id":"t1"}]}"#));
    j.note_load("notes", "other", "main", Some("{}"));
    j.note_load("other", "prefs", "main", Some("{}"));
    j.note_save("notes", "todos", "floating", None);
    assert!(j.has_saves("notes", "todos") && !j.has_saves("notes", "prefs"));
    j.forget_window("gone");
    let ev = |window: &str, key: &str, seen: Seen, save: bool| {
        let (window, ext, key) = (window.to_string(), "notes".to_string(), key.to_string());
        if save {
            Event::Save {
                window,
                ext,
                key,
                seen,
            }
        } else {
            Event::Load {
                window,
                ext,
                key,
                seen,
            }
        }
    };
    assert_eq!(
        j.take(),
        vec![
            ev("main", "prefs", Seen::Hash(hash(&json!({ "a": 1 }))), false),
            ev("main", "todos", Seen::Ids(["t1".to_string()].into()), false),
            ev("floating", "todos", Seen::Gone, true),
            Event::Forget {
                window: "gone".into()
            },
        ],
        "in order, a repeated load once, unsynced keys never"
    );
    for i in 0..(MAX_JOURNAL_EVENTS + 5) {
        j.note_save("notes", "prefs", &format!("w{i}"), Some("1"));
    }
    let all = j.take();
    assert_eq!(all.len(), MAX_JOURNAL_EVENTS, "bounded; the oldest go");
    assert!(matches!(&all[0], Event::Save { window, .. } if window == "w5"));
}

/// A plan with `notes` syncing data and settings, as a prepare would set.
fn active_source(dir: &Path, config: Config) -> ExtensionsSource {
    let n = notes();
    let port = Arc::new(Port {
        config: Mutex::new(config),
        installed: vec![n.clone()],
    });
    let mut s = ExtensionsSource::with_journal(port, dir.to_path_buf(), None, Arc::default());
    let d = n.manifest.sync_decl().unwrap();
    s.plan.settings.insert("notes".into());
    s.plan
        .data
        .insert("notes".into(), data_namespaces("notes", &d));
    s.decls.insert("notes".into(), d);
    s
}

fn grant_config() -> Config {
    let mut c = Config::default();
    c.extension_grants.insert(
        "notes".into(),
        crate::config::ExtensionGrant {
            granted: vec!["storage".into(), "sync".into()],
            approved_version: "1.0.0".into(),
            approved_at: String::new(),
            origin: Some("store:0123456789abcdef".into()),
        },
    );
    c
}

#[test]
fn sensitive_keys_and_settings_never_become_records() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("notes");
    std::fs::create_dir_all(&dir).unwrap();
    for (k, v) in [
        ("prefs", r#"{"zoom":2}"#),
        ("view-main", r#"{"x":1}"#),
        ("token", r#""sk-1""#),
        ("session_x", r#"{"s":1}"#),
        ("unlisted", "{}"),
        ("todos", r#"{"v":1,"items":[{"id":"t1","title":"Milk"}]}"#),
    ] {
        std::fs::write(dir.join(format!("{k}.json")), v).unwrap();
    }
    std::fs::write(
        dir.join("view-big.json"),
        format!("\"{}\"", "x".repeat(MAX_KEY_BYTES as usize)),
    )
    .unwrap();
    let mut c = grant_config();
    c.extensions.insert(
        "notes".into(),
        json!({ "units": "metric", "city": "Oslo", "api_key": "sk-123", "names": "Ana" }),
    );
    let mut s = active_source(tmp.path(), c);
    let st = s.read_local().unwrap();
    let ids = |ns: &str| -> Vec<String> {
        st.records
            .keys()
            .filter(|(n, _)| n == ns)
            .map(|(_, id)| id.clone())
            .collect()
    };
    assert_eq!(ids("ext.notes.k"), ["prefs", "view-big", "view-main"]);
    assert!(
        is_unchanged(&st.get("ext.notes.k", "view-big").unwrap().fields[FIELD]),
        "over 1 MiB: left alone"
    );
    assert_eq!(ids("ext.notes.l.todos"), ["t1", REST_ID]);
    assert_eq!(
        st.get("ext.notes.l.todos", REST_ID).unwrap().fields[FIELD],
        json!({ "v": 1 })
    );
    assert_eq!(ids(SETTINGS_NS), ["notes/names", "notes/units"]);
    assert_eq!(ids(INSTALLS_NS), ["extension:notes"]);
    assert_eq!(
        st.get(INSTALLS_NS, "extension:notes").unwrap().fields[FIELD],
        json!({ "version": "1.0.0", "source": "store", "store": "0123456789abcdef" })
    );
}

#[test]
fn an_install_without_an_origin_isnt_reported_yet() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = grant_config();
    c.extension_grants.get_mut("notes").unwrap().origin = None;
    let mut s = active_source(tmp.path(), c);
    assert!(s
        .read_local()
        .unwrap()
        .get(INSTALLS_NS, "extension:notes")
        .is_none());
}

#[test]
fn projection_writes_keys_and_lists_and_refuses_a_file_that_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("notes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("todos.json"),
        r#"{"v":1,"items":[{"id":"t1"},{"id":"t2"}]}"#,
    )
    .unwrap();
    let mut s = active_source(tmp.path(), grant_config());
    s.read_local().unwrap();
    let w = |ns: &str, id: &str, v: Option<Value>| LocalWrite {
        ns: ns.into(),
        id: id.into(),
        value: v.map(rec_of),
    };
    let r = s
        .project(
            vec![
                w("ext.notes.k", "prefs", Some(json!({ "zoom": 3 }))),
                w(
                    "ext.notes.l.todos",
                    "t1",
                    Some(json!({ "id": "t1", "done": true })),
                ),
                w("ext.notes.l.todos", "t2", None),
                w("ext.notes.l.todos", "t9", Some(json!({ "id": "t9" }))),
                w("ext.notes.l.todos", "t3", Some(json!({ "id": "t3" }))),
            ],
            Origin::Sync,
        )
        .unwrap();
    assert!(matches!(r, ProjectResult::Applied { .. }));
    let read = |k: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{k}.json"))).unwrap())
            .unwrap()
    };
    assert_eq!(read("prefs"), json!({ "zoom": 3 }));
    assert_eq!(
        read("todos"),
        json!({ "v": 1, "items": [{ "id": "t1", "done": true }, { "id": "t3" }, { "id": "t9" }] }),
        "in place, then new ones at the end; the deleted one goes"
    );
    // An extension saved in between: the projection waits.
    std::fs::write(dir.join("prefs.json"), r#"{"zoom":9}"#).unwrap();
    let r = s
        .project(
            vec![w("ext.notes.k", "prefs", Some(json!({ "zoom": 4 })))],
            Origin::Sync,
        )
        .unwrap();
    assert_eq!(r, ProjectResult::Stale);
    assert_eq!(read("prefs"), json!({ "zoom": 9 }));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("todos.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "owner-only, like save_extension_data");
    }
}

#[test]
fn projected_settings_go_into_config_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = grant_config();
    c.extensions
        .insert("notes".into(), json!({ "units": "metric", "city": "Oslo" }));
    let mut s = active_source(tmp.path(), c);
    let r = s
        .project(
            vec![
                LocalWrite {
                    ns: SETTINGS_NS.into(),
                    id: "notes/units".into(),
                    value: Some(rec_of(json!("imperial"))),
                },
                LocalWrite {
                    ns: SETTINGS_NS.into(),
                    id: "notes/names".into(),
                    value: Some(rec_of(json!("Ana"))),
                },
            ],
            Origin::Sync,
        )
        .unwrap();
    let ProjectResult::Applied { read_back } = r else {
        panic!("stale")
    };
    assert_eq!(
        read_back.get(SETTINGS_NS, "notes/units").unwrap().fields[FIELD],
        json!("imperial")
    );
    let (_, c) = s.port.snapshot();
    assert_eq!(
        c.extensions["notes"],
        json!({ "units": "imperial", "city": "Oslo", "names": "Ana" })
    );
}

#[test]
fn specs_declare_the_active_extensions_namespaces() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = active_source(tmp.path(), grant_config());
    s.plan.per_os.insert("notes/hotkey".into());
    let specs = s.specs();
    let names: Vec<&str> = specs.iter().map(|s| s.ns.as_str()).collect();
    assert_eq!(
        names,
        [
            "installs",
            "cfg.ext",
            "ext.notes.k",
            "ext.notes.l.todos",
            "ext.notes.c.bodies",
            "ext.notes.c.items"
        ]
    );
    let items = specs.iter().find(|s| s.ns == "ext.notes.c.items").unwrap();
    assert_eq!(items.structure.sets, ["tags"]);
    assert_eq!(items.structure.local, ["rev"]);
    assert_eq!(items.label_field.as_deref(), Some("title"));
    assert!(
        specs
            .iter()
            .find(|s| s.ns == "ext.notes.c.bodies")
            .unwrap()
            .large
    );
    assert_eq!(specs[1].structure.per_os, ["@notes/hotkey"]);
    assert!(specs.iter().all(|s| s.category == Category::Extensions));
}

#[test]
fn updated_at_reads_numbers_and_rfc3339() {
    assert_eq!(
        updated_at(&json!({ "updatedAt": 1_762_682_400_000i64 })),
        Some(1_762_682_400_000)
    );
    assert_eq!(
        updated_at(&json!({ "updatedAt": "2025-11-09T10:00:00Z" })),
        Some(1_762_682_400_000)
    );
    assert_eq!(updated_at(&json!({ "updatedAt": "soon" })), None);
    assert_eq!(updated_at(&json!({})), None);
}
