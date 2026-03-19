use kiro_assistant::extensions;

#[test]
fn test_kind_to_subdir_extension() {
    assert_eq!(extensions::kind_to_subdir("extension").unwrap(), "extensions");
}

#[test]
fn test_kind_to_subdir_theme() {
    assert_eq!(extensions::kind_to_subdir("theme").unwrap(), "themes");
}

#[test]
fn test_kind_to_subdir_commands() {
    assert_eq!(extensions::kind_to_subdir("commands").unwrap(), "command-packs");
}

#[test]
fn test_kind_to_subdir_invalid() {
    assert!(extensions::kind_to_subdir("invalid").is_err());
}

#[test]
fn test_extension_manifest_deserialization() {
    let json = r#"{
        "id": "test-ext",
        "name": "Test Extension",
        "version": "1.0.0",
        "type": "extension",
        "description": "A test extension"
    }"#;
    let manifest: extensions::ExtensionManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.id, "test-ext");
    assert_eq!(manifest.name, "Test Extension");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.kind, "extension");
}

#[test]
fn test_extension_manifest_optional_fields() {
    let json = r#"{
        "id": "minimal",
        "name": "Minimal",
        "version": "0.1.0",
        "type": "extension"
    }"#;
    let manifest: extensions::ExtensionManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.id, "minimal");
    assert!(manifest.description.is_empty());
    assert!(manifest.contributes.is_none());
}

#[test]
fn test_extension_manifest_with_contributes() {
    let json = r#"{
        "id": "themed",
        "name": "My Theme",
        "version": "1.0.0",
        "type": "theme",
        "contributes": {
            "themes": {"dark": "dark.json", "light": "light.json"}
        }
    }"#;
    let manifest: extensions::ExtensionManifest = serde_json::from_str(json).unwrap();
    assert!(manifest.contributes.is_some());
    let contrib = manifest.contributes.unwrap();
    assert!(contrib.themes.is_some());
    let themes = contrib.themes.unwrap();
    assert!(themes.dark.is_some() || themes.light.is_some());
}
