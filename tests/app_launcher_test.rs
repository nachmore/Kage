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

// ----------------------------------------------------------------------------
// Additional coverage for edge cases, ordering, the empty() fallback, and
// the fuzzy-match branch. Each test builds its own registry to stay
// independent of the others.
// ----------------------------------------------------------------------------

fn make_app(name: &str, aliases: &[&str]) -> kage::app_launcher::Application {
    kage::app_launcher::Application {
        name: name.to_string(),
        path: std::path::PathBuf::from(format!("{}.exe", name.to_lowercase())),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        icon_base64: None,
        emoji_icon: None,
    }
}

fn seed(apps: Vec<(&str, kage::app_launcher::Application)>) -> AppLauncher {
    let mut launcher = AppLauncher::empty();
    let mut registry = std::collections::HashMap::new();
    for (key, app) in apps {
        registry.insert(key.to_string(), app);
    }
    launcher.apply_registry(registry);
    launcher
}

#[test]
fn empty_launcher_finds_nothing() {
    let launcher = AppLauncher::empty();
    assert!(launcher.find_app("anything").is_empty());
    // Critically, empty() must not panic even when called in a tight loop.
    for _ in 0..50 {
        let _ = AppLauncher::empty();
    }
}

#[test]
fn find_app_orders_exact_above_starts_with_above_contains() {
    // "note" is:
    //   - an exact match for "note" app
    //   - a starts-with match for "notepad"
    //   - a contains match for "evernote"
    let launcher = seed(vec![
        ("notepad", make_app("Notepad", &["notepad"])),
        ("note", make_app("Note", &["note"])),
        ("evernote", make_app("Evernote", &["evernote"])),
    ]);

    let results = launcher.find_app("note");
    // Should have all three, with exact ranked first.
    assert!(results.len() >= 3, "expected 3 hits, got {:?}", results.iter().map(|a| &a.name).collect::<Vec<_>>());
    assert_eq!(results[0].name, "Note", "exact match should rank first");
    // Starts-with ("Notepad") outranks contains ("Evernote").
    let notepad_pos = results.iter().position(|a| a.name == "Notepad").unwrap();
    let evernote_pos = results.iter().position(|a| a.name == "Evernote").unwrap();
    assert!(notepad_pos < evernote_pos, "starts-with should outrank contains");
}

#[test]
fn find_app_empty_query_still_returns_contains_matches() {
    // Empty string is contained in every alias. The contract isn't
    // "return nothing for empty input" — it's "treat empty as contains".
    // This locks in current behavior so if we ever want to change it
    // we do so intentionally.
    let launcher = seed(vec![("notepad", make_app("Notepad", &["notepad"]))]);
    let results = launcher.find_app("");
    // Empty query matches via starts-with (every string starts with "").
    assert!(!results.is_empty());
}

#[test]
fn find_app_whitespace_insensitive_via_no_spaces_alias() {
    // Registry stores both "microsoft word" and "microsoftword" in the
    // aliases vec. Either form should hit.
    let launcher = seed(vec![(
        "microsoft word",
        make_app("Microsoft Word", &["microsoft word", "microsoftword"]),
    )]);

    assert_eq!(launcher.find_app("microsoftword").len(), 1);
    assert_eq!(launcher.find_app("microsoft word").len(), 1);
    assert_eq!(launcher.find_app("MICROSOFTWORD").len(), 1);
}

#[test]
fn find_app_unicode_matches_by_lowercase() {
    // Kage is launcher-first; users will type app names in their own
    // casing. Verify unicode case-folding works end-to-end.
    let launcher = seed(vec![(
        "café",
        make_app("Café", &["café"]),
    )]);
    assert_eq!(launcher.find_app("CAFÉ").len(), 1);
    assert_eq!(launcher.find_app("Café").len(), 1);
}

#[test]
fn find_app_fuzzy_branch_catches_one_char_typo() {
    // "ntepad" (missing 'o') — not exact, not starts-with, not contains.
    // Should still fall into the fuzzy branch for a close enough match.
    let launcher = seed(vec![(
        "notepad",
        make_app("Notepad", &["notepad"]),
    )]);
    let results = launcher.find_app("ntepad");
    // Current similarity threshold is > 60%. 6/7 chars match in order
    // => ~85% similarity, well above the bar.
    assert_eq!(results.len(), 1, "fuzzy match missed 'ntepad'");
    assert_eq!(results[0].name, "Notepad");
}

#[test]
fn find_app_similarity_rejects_unrelated_strings() {
    let launcher = seed(vec![(
        "notepad",
        make_app("Notepad", &["notepad"]),
    )]);
    assert!(launcher.find_app("zzqqxx").is_empty());
    assert!(launcher.find_app("completely-different").is_empty());
}

#[test]
fn find_app_caps_results_even_with_many_starts_with_hits() {
    let mut launcher = AppLauncher::empty();
    let mut registry = std::collections::HashMap::new();
    // 20 apps all starting with "wx".
    for i in 0..20 {
        let alias = format!("wx-app-{:02}", i);
        registry.insert(alias.clone(), make_app(&alias, &[&alias]));
    }
    launcher.apply_registry(registry);

    let results = launcher.find_app("wx");
    assert!(results.len() <= 5, "find_app must cap at 5 results, got {}", results.len());
}

#[test]
fn apply_registry_replaces_the_whole_set() {
    let mut launcher = AppLauncher::empty();
    let mut initial = std::collections::HashMap::new();
    initial.insert("alpha".to_string(), make_app("Alpha", &["alpha"]));
    launcher.apply_registry(initial);
    assert_eq!(launcher.find_app("alpha").len(), 1);

    // Swapping in a new registry should drop the old entries entirely.
    let mut replacement = std::collections::HashMap::new();
    replacement.insert("beta".to_string(), make_app("Beta", &["beta"]));
    launcher.apply_registry(replacement);

    assert!(launcher.find_app("alpha").is_empty(), "old registry entry should be gone");
    assert_eq!(launcher.find_app("beta").len(), 1);
}

#[test]
fn apply_registry_can_replace_with_empty() {
    let mut launcher = AppLauncher::empty();
    let mut initial = std::collections::HashMap::new();
    initial.insert("alpha".to_string(), make_app("Alpha", &["alpha"]));
    launcher.apply_registry(initial);
    assert_eq!(launcher.find_app("alpha").len(), 1);

    launcher.apply_registry(std::collections::HashMap::new());
    assert!(launcher.find_app("alpha").is_empty());
}
