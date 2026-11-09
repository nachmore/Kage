//! The Rust half of the shared extension-view fixture
//! (`tests/fixtures/extension-views-policy.json`). The JS half,
//! `ui-tests/shared/extension-view-policy.test.js`, runs the same cases
//! against `ui/js/shared/extension-view-policy.js`, so the two
//! normalisations of `contributes.views` and the `open_view` params rule
//! can't drift apart (docs/design/EXTENSION_VIEWS.md §4, §6.2).

use kage::extensions::views::{normalize_views, validate_params};
use serde_json::Value;

fn fixture() -> Value {
    // Nothing here touches user directories, but every integration test
    // isolates them first. The directory lives as long as the process.
    static ISOLATED: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = ISOLATED.get_or_init(|| tempfile::tempdir().expect("tempdir"));
    kage::paths::isolate(dir.path().to_path_buf());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extension-views-policy.json");
    let text = std::fs::read_to_string(&path).expect("read the fixture");
    serde_json::from_str(&text).expect("parse the fixture")
}

#[test]
fn normalize_views_matches_the_shared_fixture() {
    let fx = fixture();
    let name = fx["name"].as_str().unwrap();
    let icon = fx["icon"].as_str().unwrap();
    let cases = fx["normalize"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let label = case["case"].as_str().unwrap();
        let got = normalize_views(Some(&case["views"]), name, icon, "fixture");
        let got = serde_json::to_value(&got).unwrap();
        assert_eq!(got, case["expected"], "case: {label}");
    }
}

#[test]
fn validate_params_matches_the_shared_fixture() {
    let fx = fixture();
    let cases = fx["params"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let label = case["case"].as_str().unwrap();
        let got = validate_params(Some(&case["params"]));
        match case["error"].as_str() {
            None => assert!(got.is_ok(), "case: {label}: {got:?}"),
            Some(key) => assert_eq!(got.expect_err(label).key, key, "case: {label}"),
        }
    }
}
