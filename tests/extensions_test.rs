use kage::extensions;

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

// ---------------------------------------------------------------------------
// extract_zip — security-critical path-traversal defenses
// ---------------------------------------------------------------------------
//
// These tests build small zip archives in a temp dir and verify that
// extract_zip either succeeds (preserving the intended contents) or
// bails out with a Zip Slip error. Each malicious case is spelled out
// so a future change can't silently loosen the guard.

use std::io::Write;
use std::path::PathBuf;
use uuid::Uuid;
use zip::write::SimpleFileOptions;

struct ScopedTempDir(PathBuf);
impl ScopedTempDir {
    fn path(&self) -> &std::path::Path { &self.0 }
}
impl Drop for ScopedTempDir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn tmpdir() -> ScopedTempDir {
    let dir = std::env::temp_dir().join(format!("kage-ext-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    ScopedTempDir(dir)
}

/// Build a zip archive with the given entries and return its path.
/// `entries` maps relative names to file contents. An empty string
/// means "directory entry" — created only if name ends with '/'.
fn build_zip(dir: &std::path::Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let zip_path = dir.join(format!("test-{}.zip", Uuid::new_v4()));
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for (name, data) in entries {
        if name.ends_with('/') {
            zip.add_directory(*name, opts).unwrap();
        } else {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
    }
    zip.finish().unwrap();
    zip_path
}

#[test]
fn extract_zip_writes_nested_files_successfully() {
    let tmp = tmpdir();
    let zip_path = build_zip(tmp.path(), &[
        ("manifest.json", br#"{"id":"t","name":"T","version":"0.1.0","type":"extension"}"#),
        ("search.js", b"export default class {}"),
        ("assets/icon.svg", b"<svg/>"),
    ]);
    let target = tmp.path().join("extracted");

    extensions::extract_zip(&zip_path, &target).expect("extract");

    assert!(target.join("manifest.json").is_file());
    assert!(target.join("search.js").is_file());
    assert!(target.join("assets").is_dir());
    assert!(target.join("assets/icon.svg").is_file());
    let manifest = std::fs::read_to_string(target.join("manifest.json")).unwrap();
    assert!(manifest.contains("\"id\":\"t\""));
}

#[test]
fn extract_zip_rejects_parent_traversal() {
    let tmp = tmpdir();
    // Classic zip slip: an entry whose path escapes the target dir.
    let zip_path = build_zip(tmp.path(), &[
        ("../evil.js", b"owned"),
    ]);
    let target = tmp.path().join("extracted");

    let err = extensions::extract_zip(&zip_path, &target).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("Zip Slip") || msg.contains("forbidden path"),
        "expected Zip Slip rejection, got: {}",
        msg
    );

    // And of course the file must NOT exist outside the target.
    assert!(!tmp.path().join("evil.js").exists());
}

#[test]
fn extract_zip_rejects_deep_parent_traversal() {
    let tmp = tmpdir();
    let zip_path = build_zip(tmp.path(), &[
        ("ok.js", b"safe"),
        ("../../../../../../../etc/passwd", b"pwned"),
    ]);
    let target = tmp.path().join("extracted");

    let err = extensions::extract_zip(&zip_path, &target).unwrap_err();
    assert!(format!("{}", err).to_lowercase().contains("zip slip"));
}

#[test]
fn extract_zip_creates_target_if_missing() {
    let tmp = tmpdir();
    let zip_path = build_zip(tmp.path(), &[("a.txt", b"hello")]);
    // Deeply-nested target that doesn't exist yet.
    let target = tmp.path().join("does/not/exist/yet");

    extensions::extract_zip(&zip_path, &target).expect("extract creates target");

    assert!(target.join("a.txt").is_file());
    assert_eq!(std::fs::read_to_string(target.join("a.txt")).unwrap(), "hello");
}

#[test]
fn extract_zip_into_existing_target_overlays_files() {
    let tmp = tmpdir();
    let target = tmp.path().join("existing");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("old.txt"), "pre-existing").unwrap();

    let zip_path = build_zip(tmp.path(), &[("new.txt", b"fresh")]);
    extensions::extract_zip(&zip_path, &target).expect("extract overlay");

    // New file is present, pre-existing file is untouched (extract_zip
    // doesn't clear the target — that's the caller's job).
    assert_eq!(std::fs::read_to_string(target.join("new.txt")).unwrap(), "fresh");
    assert_eq!(std::fs::read_to_string(target.join("old.txt")).unwrap(), "pre-existing");
}

#[test]
fn extract_zip_handles_directory_entries() {
    let tmp = tmpdir();
    let zip_path = build_zip(tmp.path(), &[
        ("nested/", b""),
        ("nested/deeply/", b""),
        ("nested/deeply/file.txt", b"data"),
    ]);
    let target = tmp.path().join("out");
    extensions::extract_zip(&zip_path, &target).expect("extract");

    assert!(target.join("nested").is_dir());
    assert!(target.join("nested/deeply").is_dir());
    assert!(target.join("nested/deeply/file.txt").is_file());
}

// ---------------------------------------------------------------------------
// Manifest validation
// ---------------------------------------------------------------------------

#[test]
fn manifest_rejects_missing_required_id() {
    let json = r#"{ "name": "X", "version": "1.0.0", "type": "extension" }"#;
    let result: Result<extensions::ExtensionManifest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "expected deserialization to fail without id");
}

#[test]
fn manifest_rejects_missing_required_version() {
    let json = r#"{ "id": "x", "name": "X", "type": "extension" }"#;
    let result: Result<extensions::ExtensionManifest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "expected deserialization to fail without version");
}

#[test]
fn manifest_accepts_widgets_contribution() {
    let json = r#"{
        "id": "calendar",
        "name": "Calendar",
        "version": "1.0.0",
        "type": "extension",
        "contributes": {
            "widgets": [
                { "id": "next-meeting", "slot": "floating-bottom", "module": "./widget.js" }
            ]
        }
    }"#;
    let manifest: extensions::ExtensionManifest = serde_json::from_str(json).unwrap();
    let widgets = manifest.contributes.unwrap().widgets.unwrap_or_default();
    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0].id, "next-meeting");
    assert_eq!(widgets[0].slot, "floating-bottom");
    assert_eq!(widgets[0].module, "./widget.js");
}

#[test]
fn manifest_accepts_unknown_top_level_fields() {
    // Forward-compatibility: a manifest from a future version of Kage
    // with extra fields should still deserialize, not blow up.
    let json = r#"{
        "id": "future-ext",
        "name": "Future Ext",
        "version": "1.0.0",
        "type": "extension",
        "_future_field": {"anything": [1, 2, 3]},
        "another": "unknown"
    }"#;
    let manifest: extensions::ExtensionManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.id, "future-ext");
}

// ---------------------------------------------------------------------------
// validate_extension_id — gatekeeper for ids that land in filesystem paths
// ---------------------------------------------------------------------------
//
// install_from_directory and uninstall both call fs::remove_dir_all on a
// path built from the extension id. A hostile manifest with id="../foo"
// would let that path escape the extensions directory before the validator
// existed. These tests pin both halves of the contract: the shape we accept
// and the attack surface we reject.

#[test]
fn validate_id_accepts_typical_extension_ids() {
    for id in ["todos", "kage-math", "color_picker", "a", "ab12", "x-y_z9"] {
        extensions::validate_extension_id(id)
            .unwrap_or_else(|e| panic!("expected {:?} to be accepted, got: {}", id, e));
    }
}

#[test]
fn validate_id_rejects_path_traversal_attempts() {
    // Each of these would break out of the extensions dir if interpolated
    // into target_base.join(...). The validator must reject every one.
    let attacks = [
        "..",
        "../foo",
        "../../etc/passwd",
        "..\\..\\windows\\system32",
        "/etc/something",
        "C:/Windows",
        ".hidden",
        "-leading-dash",
        "_leading-underscore",
    ];
    for id in attacks {
        let result = extensions::validate_extension_id(id);
        assert!(
            result.is_err(),
            "validator must reject hostile id {:?}, but it was accepted",
            id
        );
    }
}

// ---------------------------------------------------------------------------
// Store URL validation — suffix-abuse defense
// ---------------------------------------------------------------------------
//
// The pre-fix check was `url.starts_with("http://localhost") ||
// url.starts_with("http://127.0.0.1")`, which lets "http://localhost.evil.com"
// through because the prefix matches even though the host resolves to an
// attacker-controlled name. The fix parses the URL and compares the host
// component exactly. These tests pin both halves: the shapes we accept and
// the suffix-abuse cases we reject.

#[test]
fn store_url_accepts_https_anywhere() {
    extensions::validate_store_url("https://store.example.com").unwrap();
    extensions::validate_store_url("https://example.com/path").unwrap();
    extensions::validate_store_url("https://localhost").unwrap();
}

#[test]
fn store_url_accepts_http_only_for_loopback_hosts() {
    for url in [
        "http://localhost",
        "http://localhost:1420",
        "http://localhost:1420/store",
        "http://127.0.0.1",
        "http://127.0.0.1:8080/path",
        "http://[::1]/foo",
    ] {
        extensions::validate_store_url(url)
            .unwrap_or_else(|e| panic!("expected {:?} to be accepted, got: {}", url, e));
    }
}

#[test]
fn store_url_rejects_suffix_abuse_against_localhost_prefix() {
    // The whole point of P1.9: starts_with("http://localhost") matched these,
    // but they all resolve to attacker-controlled hosts.
    let attacks = [
        "http://localhost.attacker.com",
        "http://localhost.attacker.com/store",
        "http://localhost.evil/store",
        "http://127.0.0.1.attacker.com",
        "http://127.0.0.1.evil.example/path",
        // Userinfo trick: the real host is after the @
        "http://localhost@evil.com/store",
        "http://127.0.0.1@evil.com",
    ];
    for url in attacks {
        let result = extensions::validate_store_url(url);
        assert!(
            result.is_err(),
            "validator must reject suffix-abuse url {:?}, but it was accepted",
            url
        );
    }
}

#[test]
fn store_url_rejects_non_http_schemes() {
    for url in [
        "file:///etc/passwd",
        "ftp://store.example.com",
        "javascript:alert(1)",
        "data:text/html,abc",
    ] {
        assert!(
            extensions::validate_store_url(url).is_err(),
            "non-http(s) scheme {:?} must be rejected",
            url
        );
    }
}

#[test]
fn store_url_rejects_unparseable_input() {
    assert!(extensions::validate_store_url("").is_err());
    assert!(extensions::validate_store_url("not a url").is_err());
    assert!(extensions::validate_store_url("://nohost").is_err());
}

// ---------------------------------------------------------------------------
// Per-extension data layout — namespacing + legacy migration
// ---------------------------------------------------------------------------

#[test]
fn data_path_isolates_extensions_into_separate_subdirs() {
    // Two different extensions storing under the same key must land at
    // distinct paths. Pre-fix they collided in a flat directory and either
    // could read/overwrite the other.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let p_a = extensions::resolve_extension_data_path(root, "todos", "kage-todos")
        .expect("extension a path resolves");
    let p_b = extensions::resolve_extension_data_path(root, "color-picker", "kage-todos")
        .expect("extension b path resolves");

    assert_ne!(p_a, p_b, "same key under different extension ids must not collide");
    assert!(p_a.starts_with(root.join("todos")), "path must live under its extension dir");
    assert!(p_b.starts_with(root.join("color-picker")));
    assert_eq!(p_a.file_name().and_then(|n| n.to_str()), Some("kage-todos.json"));
}

#[test]
fn data_path_rejects_hostile_extension_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Path traversal in the extension_id slot must be rejected before any
    // filesystem op runs.
    assert!(extensions::resolve_extension_data_path(root, "../escape", "k").is_err());
    assert!(extensions::resolve_extension_data_path(root, "", "k").is_err());
    assert!(extensions::resolve_extension_data_path(root, "UPPER", "k").is_err());
}

#[test]
fn data_path_rejects_hostile_data_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    assert!(extensions::resolve_extension_data_path(root, "todos", "..").is_err());
    assert!(extensions::resolve_extension_data_path(root, "todos", "a/b").is_err());
    assert!(extensions::resolve_extension_data_path(root, "todos", "").is_err());
}

#[test]
fn validate_id_rejects_empty_oversized_uppercase_and_unicode() {
    assert!(extensions::validate_extension_id("").is_err(), "empty id");
    assert!(
        extensions::validate_extension_id(&"a".repeat(65)).is_err(),
        "overlong id"
    );
    assert!(
        extensions::validate_extension_id("HasUpper").is_err(),
        "mixed case"
    );
    // Unicode lookalike that would canonicalize to traversal in some FS:
    assert!(
        extensions::validate_extension_id("ext\u{2024}name").is_err(),
        "Unicode one-dot leader"
    );
    // NUL byte:
    assert!(
        extensions::validate_extension_id("ext\0name").is_err(),
        "embedded NUL"
    );
}
