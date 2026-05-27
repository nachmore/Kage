use kage::config::{Config, ShortcutActionKind, ShortcutConfig};

#[test]
fn test_shortcut_config_serialization() {
    let shortcut = ShortcutConfig {
        name: "Test Shortcut".to_string(),
        shortcut: "test".to_string(),
        action_type: ShortcutActionKind::RunProgram,
        icon: None,
        path: Some("/usr/bin/test".to_string()),
        url: None,
        working_directory: Some("/home/user".to_string()),
        arguments: Some("{*}".to_string()),
        prompt: None,
        script: None,
        script_action: None,
    };

    let json = serde_json::to_string(&shortcut).unwrap();
    let deserialized: ShortcutConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(shortcut.name, deserialized.name);
    assert_eq!(shortcut.shortcut, deserialized.shortcut);
    assert_eq!(shortcut.path, deserialized.path);
    assert_eq!(shortcut.working_directory, deserialized.working_directory);
    assert_eq!(shortcut.arguments, deserialized.arguments);
}

#[test]
fn test_shortcut_config_optional_fields() {
    let shortcut = ShortcutConfig {
        name: "Simple Shortcut".to_string(),
        shortcut: "simple".to_string(),
        action_type: ShortcutActionKind::RunProgram,
        icon: None,
        path: Some("/usr/bin/simple".to_string()),
        url: None,
        working_directory: None,
        arguments: None,
        prompt: None,
        script: None,
        script_action: None,
    };

    let json = serde_json::to_string(&shortcut).unwrap();
    let deserialized: ShortcutConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(shortcut.name, deserialized.name);
    assert!(deserialized.working_directory.is_none());
    assert!(deserialized.arguments.is_none());
}

#[test]
fn test_config_with_shortcuts() {
    let mut config = Config::default();

    config.shortcuts.push(ShortcutConfig {
        name: "VSCode".to_string(),
        shortcut: "code".to_string(),
        action_type: ShortcutActionKind::RunProgram,
        icon: None,
        path: Some("C:\\Program Files\\VSCode\\code.exe".to_string()),
        url: None,
        working_directory: None,
        arguments: Some("{*}".to_string()),
        prompt: None,
        script: None,
        script_action: None,
    });

    let json = serde_json::to_string_pretty(&config).unwrap();
    let deserialized: Config = serde_json::from_str(&json).unwrap();

    assert_eq!(config.shortcuts.len(), deserialized.shortcuts.len());
    assert_eq!(config.shortcuts[0].name, deserialized.shortcuts[0].name);
}

#[test]
fn test_config_empty_shortcuts() {
    let config = Config::default();
    assert_eq!(config.shortcuts.len(), 0);
}

#[test]
fn test_shortcut_json_format() {
    let json = r#"{
        "name": "Git Status",
        "shortcut": "gs",
        "path": "/usr/bin/git",
        "working_directory": "/home/user/project",
        "arguments": "status"
    }"#;

    let shortcut: ShortcutConfig = serde_json::from_str(json).unwrap();
    assert_eq!(shortcut.name, "Git Status");
    assert_eq!(shortcut.shortcut, "gs");
    assert_eq!(shortcut.path, Some("/usr/bin/git".to_string()));
    assert_eq!(
        shortcut.working_directory,
        Some("/home/user/project".to_string())
    );
    assert_eq!(shortcut.arguments, Some("status".to_string()));
}

#[test]
fn test_shortcuts_array_json() {
    let json = r#"[
        {
            "name": "Shortcut 1",
            "shortcut": "s1",
            "path": "/usr/bin/s1"
        },
        {
            "name": "Shortcut 2",
            "shortcut": "s2",
            "path": "/usr/bin/s2",
            "arguments": "{*}"
        }
    ]"#;

    let shortcuts: Vec<ShortcutConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(shortcuts.len(), 2);
    assert_eq!(shortcuts[0].name, "Shortcut 1");
    assert_eq!(shortcuts[1].arguments, Some("{*}".to_string()));
}
