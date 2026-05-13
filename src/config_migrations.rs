//! Config migrations.
//!
//! Each migration takes a `serde_json::Value` representing the entire
//! config JSON at version N and returns a `Value` at version N+1. The
//! caller handles reading the `version` field, applying the chain, and
//! finally deserializing into the `Config` struct.
//!
//! Guidelines for writing a migration:
//!   1. Operate on the JSON representation, not the `Config` struct.
//!      The struct moves; migrations should not.
//!   2. Be conservative: only touch fields you explicitly understand.
//!      Unknown fields are preserved automatically because we're
//!      manipulating a `Value`.
//!   3. Never panic. Return `Err` for anything unexpected and let the
//!      caller decide whether to back up and reset.
//!   4. Update the `version` field last. The chain runner will do it
//!      if you don't, but it's good practice to be explicit.
//!   5. Add a unit test covering a realistic before/after sample.

use anyhow::{bail, Result};
use serde_json::{json, Value};

/// The highest config schema version this build understands. Bump when
/// you add a migration; add the migration function to `migrate_one_step`
/// below.
pub const CURRENT_VERSION: u32 = 4;

/// The lowest version we can still migrate from. If a config on disk is
/// older than this we treat it as corrupt and reset (after backing up).
/// Today we accept v1 as the floor because that's what shipped before
/// the migration framework existed.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// Migrate the given JSON config from its stored `version` up to
/// `CURRENT_VERSION`. Returns the mutated `Value` with an updated
/// `version` field.
///
/// Behaviour:
///   - If the stored version equals `CURRENT_VERSION`, returns the
///     input unchanged.
///   - If the stored version is newer than `CURRENT_VERSION`, returns
///     an error. The caller should log and preserve the file on disk
///     rather than downgrade it.
///   - If the stored version is older than `MIN_SUPPORTED_VERSION`,
///     returns an error. The caller should back up and reset.
///   - Missing `version` field is treated as `1` (pre-migration
///     baseline), because that's what all existing installs are.
pub fn migrate(mut value: Value) -> Result<Value> {
    let stored = read_version(&value);

    if stored > CURRENT_VERSION {
        bail!(
            "config version {} is newer than this build understands (max {}); \
             refusing to migrate to avoid data loss",
            stored,
            CURRENT_VERSION
        );
    }
    if stored < MIN_SUPPORTED_VERSION {
        bail!(
            "config version {} is older than the minimum supported version {}",
            stored,
            MIN_SUPPORTED_VERSION
        );
    }

    let mut current = stored;
    while current < CURRENT_VERSION {
        value = migrate_one_step(current, value)?;
        current += 1;
        // Always normalize the version after a successful step so a
        // partial-chain failure still leaves a consistent `version`
        // field for the next attempt.
        if let Value::Object(ref mut map) = value {
            map.insert("version".to_string(), json!(current));
        }
    }

    Ok(value)
}

/// Read the `version` field from the config JSON. Missing or non-numeric
/// treated as `1` (the baseline).
fn read_version(value: &Value) -> u32 {
    value
        .get("version")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(1)
}

/// Dispatch a single migration step. Add a new arm here when you add
/// a new migration function.
fn migrate_one_step(from: u32, value: Value) -> Result<Value> {
    match from {
        1 => migrate_1_to_2(value),
        2 => migrate_2_to_3(value),
        3 => migrate_3_to_4(value),
        other => bail!("no migration registered from version {}", other),
    }
}

/// v1 → v2: the no-op bootstrap migration. We introduce the migration
/// framework at schema version 2; everything that shipped before this
/// is considered v1. Since v2 is a superset of v1 (all added fields
/// have serde defaults), no field-level work is needed here.
///
/// Future migrations that rename or reshape fields will be more
/// interesting. This function exists so there's always at least one
/// migration in the chain, which exercises the code path.
fn migrate_1_to_2(value: Value) -> Result<Value> {
    Ok(value)
}

/// v2 → v3: telemetry rollout. Existing users (those already past the
/// welcome flow) never saw the privacy disclosure page, so we can't
/// treat their default `enabled=true` as consent — that would be a
/// silent opt-in. For already-completed first-run configs, force
/// telemetry off; the user can turn it on from Settings → Privacy
/// after reading the disclosure there. Brand-new installs still hit
/// the welcome step and get the documented opt-out flow.
fn migrate_2_to_3(mut value: Value) -> Result<Value> {
    if let Value::Object(ref mut map) = value {
        let already_completed = map
            .get("first_run_completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if already_completed {
            let telemetry = map
                .entry("telemetry".to_string())
                .or_insert_with(|| json!({}));
            if let Value::Object(tmap) = telemetry {
                tmap.insert("enabled".to_string(), json!(false));
            }
        }
    }
    Ok(value)
}

/// v3 → v4: add `updates.channel` defaulting to "stable". Existing users
/// who opted into auto-updates did so under the single-track assumption;
/// they keep getting the stable channel unless they explicitly opt into
/// beta/dev in Settings → Updates. Fresh installs get stable by the
/// config default, which matches this migration exactly.
fn migrate_3_to_4(mut value: Value) -> Result<Value> {
    if let Value::Object(ref mut map) = value {
        let updates = map
            .entry("updates".to_string())
            .or_insert_with(|| json!({}));
        if let Value::Object(umap) = updates {
            umap.entry("channel".to_string())
                .or_insert_with(|| json!("stable"));
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_version_is_treated_as_1() {
        let v = json!({ "debug_mode": false });
        let out = migrate(v).unwrap();
        assert_eq!(
            out.get("version").and_then(|n| n.as_u64()),
            Some(CURRENT_VERSION as u64)
        );
    }

    #[test]
    fn current_version_is_unchanged() {
        let v = json!({ "version": CURRENT_VERSION, "debug_mode": true });
        let out = migrate(v).unwrap();
        assert_eq!(
            out.get("version").and_then(|n| n.as_u64()),
            Some(CURRENT_VERSION as u64)
        );
        // Other fields are preserved.
        assert_eq!(out.get("debug_mode"), Some(&json!(true)));
    }

    #[test]
    fn v1_is_migrated_up_to_current() {
        let v = json!({ "version": 1, "debug_mode": true });
        let out = migrate(v).unwrap();
        assert_eq!(
            out.get("version").and_then(|n| n.as_u64()),
            Some(CURRENT_VERSION as u64)
        );
        assert_eq!(out.get("debug_mode"), Some(&json!(true)));
    }

    #[test]
    fn future_version_errors_out() {
        let v = json!({ "version": CURRENT_VERSION + 1 });
        let err = migrate(v).unwrap_err();
        assert!(format!("{}", err).contains("newer"));
    }

    #[test]
    fn version_below_minimum_errors_out() {
        // This test only meaningfully fires when MIN_SUPPORTED_VERSION > 1
        // in the future. Today it's 1, so version 0 is the test input.
        if MIN_SUPPORTED_VERSION == 0 {
            return;
        }
        let v = json!({ "version": MIN_SUPPORTED_VERSION - 1 });
        let err = migrate(v).unwrap_err();
        assert!(format!("{}", err).contains("older"));
    }

    #[test]
    fn non_object_root_preserves_version_update() {
        // If someone hands us a non-object, we can't add a version
        // field but also shouldn't panic. The current impl only
        // inserts into objects, so non-objects are returned as-is
        // (other than the error that the chain would produce if a
        // step needed to mutate). For v1→v2 no-op, this is fine.
        let v = json!([1, 2, 3]);
        // v1 no-op → should succeed, but we'll be left with no version
        // field. That's acceptable: the outer load path will then
        // refuse to deserialize this as Config anyway, and the corrupt
        // backup path kicks in.
        let out = migrate(v.clone()).unwrap();
        assert_eq!(out, v);
    }

    #[test]
    fn v2_to_v3_disables_telemetry_for_existing_users() {
        let v = json!({
            "version": 2,
            "first_run_completed": true,
            "telemetry": { "enabled": true },
        });
        let out = migrate(v).unwrap();
        assert_eq!(
            out.get("telemetry").and_then(|t| t.get("enabled")),
            Some(&json!(false)),
            "completed-first-run users must not auto-opt-in silently"
        );
    }

    #[test]
    fn v3_to_v4_adds_channel_default_stable() {
        let v = json!({
            "version": 3,
            "updates": { "auto_check": true },
        });
        let out = migrate(v).unwrap();
        assert_eq!(
            out.get("updates").and_then(|u| u.get("channel")),
            Some(&json!("stable"))
        );
    }

    #[test]
    fn v3_to_v4_preserves_existing_channel() {
        let v = json!({
            "version": 3,
            "updates": { "channel": "beta" },
        });
        let out = migrate(v).unwrap();
        assert_eq!(
            out.get("updates").and_then(|u| u.get("channel")),
            Some(&json!("beta"))
        );
    }

    #[test]
    fn v2_to_v3_leaves_fresh_installs_alone() {
        // A config that hasn't completed first run yet will hit the
        // welcome screen's privacy step on next launch — that's where
        // consent gets captured. Don't pre-disable on these.
        let v = json!({
            "version": 2,
            "first_run_completed": false,
        });
        let out = migrate(v).unwrap();
        // Either absent (default=true at deserialize time) or not set
        // to false is acceptable. Explicitly false would be a bug.
        let telemetry_enabled = out
            .get("telemetry")
            .and_then(|t| t.get("enabled"))
            .and_then(|v| v.as_bool());
        assert!(
            telemetry_enabled != Some(false),
            "first-run-in-progress configs should not have telemetry forced off"
        );
    }
}
