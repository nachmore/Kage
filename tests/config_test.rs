use kage::config::{Config, HotkeyConfig};
use kage::config_migrations::CURRENT_VERSION;

#[test]
fn test_config_default_values() {
    let config = Config::default();
    assert_eq!(config.version, CURRENT_VERSION);
    assert_eq!(config.hotkey.modifiers, vec!["Alt".to_string()]);
    assert_eq!(config.hotkey.key, "Space");
    assert!(config.shortcuts.is_empty());
    assert!(!config.first_run_completed);
}

#[test]
fn test_config_serialization_roundtrip() {
    let config = Config::default();
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.version, config.version);
    assert_eq!(deserialized.hotkey.key, config.hotkey.key);
    assert_eq!(deserialized.hotkey.modifiers, config.hotkey.modifiers);
    assert_eq!(deserialized.shortcuts.len(), config.shortcuts.len());
}

#[test]
fn test_config_backward_compatibility_extra_fields() {
    // Unknown fields should be silently ignored when all required fields are present
    let config = Config::default();
    let mut json_val: serde_json::Value = serde_json::to_value(&config).unwrap();
    json_val["some_future_field"] = serde_json::json!(true);
    json_val["another"] = serde_json::json!([1, 2, 3]);
    let json_str = serde_json::to_string(&json_val).unwrap();
    let loaded: Config = serde_json::from_str(&json_str).unwrap();
    assert_eq!(loaded.version, CURRENT_VERSION);
}

#[test]
fn test_config_backward_compatibility_missing_optional_fields() {
    // Start with a full default config, remove optional fields, should still parse
    let config = Config::default();
    let mut json_val: serde_json::Value = serde_json::to_value(&config).unwrap();
    // Remove fields that have #[serde(default)]
    json_val.as_object_mut().unwrap().remove("shortcuts");
    json_val.as_object_mut().unwrap().remove("debug_mode");
    json_val.as_object_mut().unwrap().remove("tool_permissions");
    json_val.as_object_mut().unwrap().remove("pocket_tts");
    json_val.as_object_mut().unwrap().remove("extensions");
    let json_str = serde_json::to_string(&json_val).unwrap();
    let loaded: Config = serde_json::from_str(&json_str).unwrap();
    assert!(loaded.shortcuts.is_empty());
    assert!(!loaded.debug_mode);
}

#[test]
fn test_hotkey_string_generation() {
    let mut config = Config::default();
    assert_eq!(config.get_hotkey_string(), "Alt+Space");

    config.hotkey = HotkeyConfig {
        modifiers: vec!["Ctrl".to_string(), "Shift".to_string()],
        key: "K".to_string(),
    };
    assert_eq!(config.get_hotkey_string(), "Ctrl+Shift+K");
}

#[test]
fn test_hotkey_string_single_modifier() {
    let mut config = Config::default();
    config.hotkey = HotkeyConfig {
        modifiers: vec!["Super".to_string()],
        key: "Space".to_string(),
    };
    assert_eq!(config.get_hotkey_string(), "Super+Space");
}

#[test]
fn test_config_ui_defaults() {
    let config = Config::default();
    assert_eq!(config.ui.theme, "system");
    assert_eq!(config.ui.font_size, 14);
    assert_eq!(config.ui.window_start_position, "center");
}

#[test]
fn test_config_with_shortcuts_roundtrip() {
    let mut config = Config::default();
    config.shortcuts.push(kage::config::ShortcutConfig {
        name: "VSCode".to_string(),
        shortcut: "code".to_string(),
        action_type: "run_program".to_string(),
        icon: None,
        path: Some("C:\\Program Files\\VSCode\\code.exe".to_string()),
        url: None,
        working_directory: None,
        arguments: Some("{*}".to_string()),
        prompt: None,
        script: None,
        script_action: None,
    });

    let json = serde_json::to_string(&config).unwrap();
    let config2: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(config2.shortcuts.len(), 1);
    assert_eq!(config2.shortcuts[0].name, "VSCode");
    assert_eq!(config2.shortcuts[0].arguments, Some("{*}".to_string()));
}

#[test]
fn test_config_migration_missing_version_loads_as_current() {
    // Simulate a config file from before the `version` field existed.
    // (Unlikely in practice — the field has shipped since v1 — but
    // worth covering because the migration framework treats missing
    // versions as 1.)
    let config = Config::default();
    let mut json_val: serde_json::Value = serde_json::to_value(&config).unwrap();
    json_val.as_object_mut().unwrap().remove("version");

    let migrated = kage::config_migrations::migrate(json_val).unwrap();
    let loaded: Config = serde_json::from_value(migrated).unwrap();
    assert_eq!(loaded.version, CURRENT_VERSION);
}

#[test]
fn test_config_migration_v1_config_upgrades_to_current() {
    // A realistic v1 payload: minimal shape, version=1, unknown field.
    // Migration should bump the version and leave other fields intact
    // (the current-to-current chain is a no-op, but exercising the
    // path catches broken dispatch wiring).
    let payload = serde_json::json!({
        "version": 1,
        "hotkey": { "modifiers": ["Alt"], "key": "Space" },
        "acp": {
            "mode": { "type": "remote", "host": "127.0.0.1", "port": 8765, "timeout_ms": 30000 },
            "agent": {}
        },
        "ui": {
            "theme": "system",
            "floating_window_opacity": 1.0
        },
        "system": { "auto_start": false },
        "_legacy_unknown_field": "preserved"
    });
    let migrated = kage::config_migrations::migrate(payload).unwrap();
    assert_eq!(
        migrated.get("version").and_then(|n| n.as_u64()),
        Some(CURRENT_VERSION as u64)
    );
    // Unknown fields are preserved through the migration pipeline.
    assert_eq!(migrated.get("_legacy_unknown_field"), Some(&serde_json::json!("preserved")));
    // And it still deserializes into a Config.
    let loaded: Config = serde_json::from_value(migrated).unwrap();
    assert_eq!(loaded.version, CURRENT_VERSION);
}

#[test]
fn test_config_migration_future_version_rejected() {
    let payload = serde_json::json!({
        "version": CURRENT_VERSION + 1,
        "hotkey": { "modifiers": ["Alt"], "key": "Space" }
    });
    let err = kage::config_migrations::migrate(payload).unwrap_err();
    assert!(format!("{}", err).contains("newer"));
}
