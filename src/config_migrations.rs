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
///
/// Pre-launch baseline: we're at v1. Concrete migrations between
/// versions don't exist yet because there are no users with prior
/// schemas to migrate. The framework + tests stay so adding a real
/// migration later is mechanical.
pub const CURRENT_VERSION: u32 = 1;

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

    // Ensure the on-disk shape is self-describing even when no
    // migrations ran (e.g. a v1 install whose config was written before
    // the framework existed and so has no `version` field). Only stamp
    // when the value is an object so non-object roots still pass
    // through untouched for the corrupt-backup path to handle.
    if let Value::Object(ref mut map) = value {
        map.entry("version".to_string())
            .or_insert(json!(CURRENT_VERSION));
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
///
/// Currently empty: there are no shipped users with prior schemas to
/// migrate from, so concrete migrations don't exist yet. The chain
/// runner above will only call this when `CURRENT_VERSION` is bumped
/// past 1, at which point the new arm goes here.
fn migrate_one_step(from: u32, _value: Value) -> Result<Value> {
    bail!("no migration registered from version {}", from)
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
    fn future_version_errors_out() {
        let v = json!({ "version": CURRENT_VERSION + 1 });
        let err = migrate(v).unwrap_err();
        assert!(format!("{}", err).contains("newer"));
    }

    #[test]
    fn version_below_minimum_errors_out() {
        // Only meaningful when MIN_SUPPORTED_VERSION > 1.
        if MIN_SUPPORTED_VERSION <= 1 {
            return;
        }
        let v = json!({ "version": MIN_SUPPORTED_VERSION - 1 });
        let err = migrate(v).unwrap_err();
        assert!(format!("{}", err).contains("older"));
    }

    #[test]
    fn non_object_root_passes_through_unchanged() {
        // If someone hands us a non-object, we can't add a version
        // field but also shouldn't panic. With no migrations to run
        // (CURRENT_VERSION == stored), the chain runner short-circuits
        // and returns the input as-is. The outer load path will refuse
        // to deserialize this as Config and the corrupt-backup path
        // takes over.
        let v = json!([1, 2, 3]);
        let out = migrate(v.clone()).unwrap();
        assert_eq!(out, v);
    }

    /// When the next migration is added: bump CURRENT_VERSION, write
    /// the migration function, register it in `migrate_one_step`, and
    /// add a test here that verifies before/after JSON shape. This
    /// stub reminds future-us that it's wired up:
    #[test]
    fn migrate_one_step_rejects_unknown_versions() {
        // Sanity check: even with no migrations registered, the
        // dispatcher must reject unknown versions cleanly rather than
        // panicking. This is the failure mode if CURRENT_VERSION gets
        // bumped without adding a corresponding match arm.
        let err = migrate_one_step(1, json!({})).unwrap_err();
        assert!(format!("{}", err).contains("no migration registered"));
    }
}
