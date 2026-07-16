//! Integration tests for folder-plan execution, focused on the rollback
//! manifest: the recorded trash path must be the file's ACTUAL location
//! so undo can find it.

use kage::commands::folder_tools::{execute_plan, FolderOperation};
use std::fs;
use std::path::Path;

fn delete_op(from: &str) -> FolderOperation {
    FolderOperation {
        action: "delete".to_string(),
        from: from.to_string(),
        to: None,
        reason: None,
    }
}

/// The rollback entry for a deleted file must point at where the file
/// actually landed in the trash — resolvable against the plan root.
fn assert_rollback_resolves(root: &Path, rollback: &[(String, String)]) {
    for (trash_rel, _orig) in rollback {
        let p = root.join(trash_rel);
        assert!(
            p.exists(),
            "rollback records {:?} but nothing exists there",
            trash_rel
        );
    }
}

#[test]
fn delete_rollback_records_actual_trash_path_for_nested_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/a.txt"), "hello").unwrap();

    let result = execute_plan(root, &[delete_op("docs/a.txt")]);

    assert!(result.success, "errors: {:?}", result.errors);
    assert_eq!(result.rollback.len(), 1);
    // Trash flattens to basename — the manifest must reflect that.
    assert_eq!(result.rollback[0].0, "_kage_trash/a.txt");
    assert_eq!(result.rollback[0].1, "docs/a.txt");
    assert_rollback_resolves(root, &result.rollback);
}

#[test]
fn delete_rollback_records_suffixed_name_on_trash_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("a.txt"), "first").unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/a.txt"), "second").unwrap();

    // First delete puts a.txt in the trash; second collides and gets a
    // timestamp suffix.
    let r1 = execute_plan(root, &[delete_op("a.txt")]);
    assert!(r1.success, "errors: {:?}", r1.errors);
    let r2 = execute_plan(root, &[delete_op("docs/a.txt")]);
    assert!(r2.success, "errors: {:?}", r2.errors);

    assert_eq!(r2.rollback.len(), 1);
    let recorded = &r2.rollback[0].0;
    assert_ne!(
        recorded, "_kage_trash/a.txt",
        "collision-renamed file must not record the colliding name"
    );
    assert_rollback_resolves(root, &r2.rollback);
    // Both files still exist in the trash with distinct names.
    assert!(root.join("_kage_trash/a.txt").exists());
}

#[test]
fn move_rollback_records_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("a.txt"), "x").unwrap();
    fs::create_dir_all(root.join("dst")).unwrap();

    let result = execute_plan(
        root,
        &[FolderOperation {
            action: "move".to_string(),
            from: "a.txt".to_string(),
            to: Some("dst/a.txt".to_string()),
            reason: None,
        }],
    );

    assert!(result.success, "errors: {:?}", result.errors);
    assert_eq!(result.rollback.len(), 1);
    assert_rollback_resolves(root, &result.rollback);
}
