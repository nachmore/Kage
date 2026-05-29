//! AppError shape contract.
//!
//! AppError serialises as `{ kind, key, message }` to the frontend. `key` is the
//! i18n key the frontend can also use to match programmatically; `message` is the
//! materialised text in the active locale.
//!
//! The boundary contract:
//!   - kind: snake_case `ErrorKind` variant, stable wire format
//!   - key: i18n key (e.g. `errors.connection.lost`) — also stable
//!   - message: localised UI string, derived from key + active locale at serialise time
//!
//! Free-form errors (legacy `AppError::internal(s)` etc.) flow through the
//! `errors.passthrough` key so the wire shape stays uniform.

use kage::error::{AppError, ErrorKind};

fn init_en() {
    // Tests run in any order; init() is idempotent.
    kage::i18n::init(Some("en"));
}

#[test]
fn keyed_construction_uses_real_key() {
    init_en();
    let err = AppError::keyed(
        ErrorKind::ConnectionLost,
        "errors.connection.lost",
        &[("reason", "socket closed")],
    );
    assert!(matches!(err.kind, ErrorKind::ConnectionLost));
    assert_eq!(err.key, "errors.connection.lost");
    assert_eq!(err.params.get("reason").unwrap(), "socket closed");
    assert_eq!(err.localised_message(), "Connection lost: socket closed");
}

#[test]
fn raw_construction_uses_passthrough() {
    init_en();
    let err = AppError::raw(ErrorKind::Internal, "agent said something weird");
    assert_eq!(err.key, "errors.passthrough");
    assert_eq!(err.localised_message(), "agent said something weird");
}

#[test]
fn legacy_constructors_still_work_via_passthrough() {
    init_en();
    let err = AppError::connection_lost("server gone");
    assert!(matches!(err.kind, ErrorKind::ConnectionLost));
    // Legacy free-form constructors route through the passthrough template
    // so the message text reaches the user verbatim while the kind stays
    // structured.
    assert_eq!(err.key, "errors.passthrough");
    assert_eq!(err.localised_message(), "server gone");
}

#[test]
fn display_uses_english_regardless_of_active_locale() {
    init_en();
    let err = AppError::keyed(
        ErrorKind::ConnectionLost,
        "errors.connection.not_connected",
        &[],
    );
    assert_eq!(format!("{}", err), "Not connected");
}

#[test]
fn from_string_makes_internal_passthrough_error() {
    init_en();
    let err: AppError = "oops".to_string().into();
    assert!(matches!(err.kind, ErrorKind::Internal));
    assert_eq!(err.key, "errors.passthrough");
    assert_eq!(err.localised_message(), "oops");
}

#[test]
fn from_str_makes_internal_passthrough_error() {
    init_en();
    let err: AppError = "oops".into();
    assert!(matches!(err.kind, ErrorKind::Internal));
    assert_eq!(err.localised_message(), "oops");
}

#[test]
fn from_anyhow_makes_internal_passthrough_error() {
    init_en();
    let anyhow_err = anyhow::anyhow!("anyhow error");
    let err: AppError = anyhow_err.into();
    assert!(matches!(err.kind, ErrorKind::Internal));
    assert_eq!(err.localised_message(), "anyhow error");
}

#[test]
fn serialisation_contract_matches_frontend_expectations() {
    init_en();
    // Wire shape: { kind, key, message } — `errMessage(e)` reads `message`,
    // `errKind(e)` reads `kind`. Tests on the frontend rely on `key` for
    // exact-match assertions without depending on translated text.
    let err = AppError::keyed(
        ErrorKind::ConnectionLost,
        "errors.connection.lost",
        &[("reason", "disconnected")],
    );
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["kind"], "connection_lost");
    assert_eq!(json["key"], "errors.connection.lost");
    assert_eq!(json["message"], "Connection lost: disconnected");
}

#[test]
fn all_error_kinds_serialise_to_snake_case() {
    init_en();
    let cases: Vec<(AppError, &str)> = vec![
        (AppError::connection_lost(""), "connection_lost"),
        (AppError::timeout(""), "timeout"),
        (AppError::session_corrupted(""), "session_corrupted"),
        (AppError::image_unsupported(""), "image_unsupported"),
        (AppError::lock(""), "lock_error"),
        (
            AppError::new(ErrorKind::SerializeError, ""),
            "serialize_error",
        ),
        (AppError::new(ErrorKind::RateLimited, ""), "rate_limited"),
        (AppError::internal(""), "internal"),
    ];
    for (err, expected_kind) in cases {
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json["kind"], expected_kind,
            "ErrorKind serialization mismatch"
        );
    }
}

#[test]
fn question_mark_from_string_result_works() {
    init_en();
    fn inner() -> Result<(), AppError> {
        let r: Result<(), String> = Err("fail".to_string());
        r?;
        Ok(())
    }
    let err = inner().unwrap_err();
    assert_eq!(err.localised_message(), "fail");
}

#[test]
fn switching_locale_changes_serialised_message_but_not_key() {
    // Verify the boundary translation: same AppError serialises with
    // different `message` text under different locales, but `key` is stable.
    // This is the property the frontend relies on for stable error matching.
    init_en();
    let err = AppError::keyed(
        ErrorKind::ConnectionLost,
        "errors.connection.not_connected",
        &[],
    );
    let en_json = serde_json::to_value(&err).unwrap();
    assert_eq!(en_json["message"], "Not connected");
    assert_eq!(en_json["key"], "errors.connection.not_connected");
    let en_message = en_json["message"].as_str().unwrap().to_string();

    // Switch to Japanese. JA ships a real catalog, so `message` MUST change
    // to a localised string while `key` stays stable. The exact JA wording
    // can drift across translate.py runs, so we assert the contract — not
    // a specific translation.
    kage::i18n::set_language("ja");
    let ja_json = serde_json::to_value(&err).unwrap();
    assert_eq!(ja_json["key"], "errors.connection.not_connected");
    let ja_message = ja_json["message"].as_str().unwrap();
    assert!(
        !ja_message.is_empty(),
        "JA message should be a non-empty string, got {ja_message:?}"
    );
    assert_ne!(
        ja_message, en_message,
        "JA message should differ from EN — if they're the same it means \
         the JA catalog wasn't loaded correctly"
    );
    // restore
    kage::i18n::set_language("en");
}
