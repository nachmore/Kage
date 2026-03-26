use kage::app_launcher::AppLauncher;

#[test]
fn test_new_launcher_empty_registry() {
    let launcher = AppLauncher::new().unwrap();
    let results = launcher.find_app("anything");
    assert!(results.is_empty());
}

#[test]
fn test_find_app_exact_match() {
    let mut launcher = AppLauncher::new().unwrap();
    let mut registry = std::collections::HashMap::new();
    registry.insert("notepad".to_string(), kage::app_launcher::Application {
        name: "Notepad".to_string(),
        path: std::path::PathBuf::from("C:\\Windows\\notepad.exe"),
        aliases: vec!["notepad".to_string()],
        icon_base64: None,
        emoji_icon: None,
    });
    launcher.apply_registry(registry);

    let results = launcher.find_app("notepad");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "Notepad");
}

#[test]
fn test_find_app_starts_with() {
    let mut launcher = AppLauncher::new().unwrap();
    let mut registry = std::collections::HashMap::new();
    registry.insert("notepad".to_string(), kage::app_launcher::Application {
        name: "Notepad".to_string(),
        path: std::path::PathBuf::from("notepad.exe"),
        aliases: vec!["notepad".to_string()],
        icon_base64: None,
        emoji_icon: None,
    });
    launcher.apply_registry(registry);

    let results = launcher.find_app("note");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "Notepad");
}

#[test]
fn test_find_app_contains() {
    let mut launcher = AppLauncher::new().unwrap();
    let mut registry = std::collections::HashMap::new();
    registry.insert("notepad".to_string(), kage::app_launcher::Application {
        name: "Notepad".to_string(),
        path: std::path::PathBuf::from("notepad.exe"),
        aliases: vec!["notepad".to_string()],
        icon_base64: None,
        emoji_icon: None,
    });
    launcher.apply_registry(registry);

    let results = launcher.find_app("pad");
    assert_eq!(results.len(), 1);
}

#[test]
fn test_find_app_case_insensitive() {
    let mut launcher = AppLauncher::new().unwrap();
    let mut registry = std::collections::HashMap::new();
    registry.insert("notepad".to_string(), kage::app_launcher::Application {
        name: "Notepad".to_string(),
        path: std::path::PathBuf::from("notepad.exe"),
        aliases: vec!["notepad".to_string()],
        icon_base64: None,
        emoji_icon: None,
    });
    launcher.apply_registry(registry);

    assert_eq!(launcher.find_app("NOTEPAD").len(), 1);
    assert_eq!(launcher.find_app("NotePad").len(), 1);
}

#[test]
fn test_find_app_no_match() {
    let mut launcher = AppLauncher::new().unwrap();
    let mut registry = std::collections::HashMap::new();
    registry.insert("notepad".to_string(), kage::app_launcher::Application {
        name: "Notepad".to_string(),
        path: std::path::PathBuf::from("notepad.exe"),
        aliases: vec!["notepad".to_string()],
        icon_base64: None,
        emoji_icon: None,
    });
    launcher.apply_registry(registry);

    let results = launcher.find_app("zzzzz");
    assert!(results.is_empty());
}

#[test]
fn test_find_app_max_results() {
    let mut launcher = AppLauncher::new().unwrap();
    let mut registry = std::collections::HashMap::new();
    for i in 0..10 {
        let name = format!("app{}", i);
        registry.insert(name.clone(), kage::app_launcher::Application {
            name: name.clone(),
            path: std::path::PathBuf::from(format!("{}.exe", name)),
            aliases: vec![name],
            icon_base64: None,
            emoji_icon: None,
        });
    }
    launcher.apply_registry(registry);

    // "app" matches all 10, but find_app caps at 5
    let results = launcher.find_app("app");
    assert!(results.len() <= 5);
}

#[test]
fn test_find_app_alias_no_spaces() {
    let mut launcher = AppLauncher::new().unwrap();
    let mut registry = std::collections::HashMap::new();
    registry.insert("microsoft word".to_string(), kage::app_launcher::Application {
        name: "Microsoft Word".to_string(),
        path: std::path::PathBuf::from("winword.exe"),
        aliases: vec!["microsoft word".to_string(), "microsoftword".to_string()],
        icon_base64: None,
        emoji_icon: None,
    });
    launcher.apply_registry(registry);

    // Should match via the no-spaces alias
    assert_eq!(launcher.find_app("microsoftword").len(), 1);
}
