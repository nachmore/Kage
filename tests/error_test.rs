use kiro_assistant::error::{AppError, ErrorKind};

#[test]
fn test_app_error_construction() {
    let err = AppError::internal("something broke");
    assert_eq!(err.message, "something broke");
    assert!(matches!(err.kind, ErrorKind::Internal));
}

#[test]
fn test_app_error_connection_lost() {
    let err = AppError::connection_lost("server gone");
    assert_eq!(err.message, "server gone");
    assert!(matches!(err.kind, ErrorKind::ConnectionLost));
}

#[test]
fn test_app_error_display() {
    let err = AppError::timeout("took too long");
    assert_eq!(format!("{}", err), "took too long");
}

#[test]
fn test_app_error_from_string() {
    let err: AppError = "oops".to_string().into();
    assert_eq!(err.message, "oops");
    assert!(matches!(err.kind, ErrorKind::Internal));
}

#[test]
fn test_app_error_from_str() {
    let err: AppError = "oops".into();
    assert_eq!(err.message, "oops");
    assert!(matches!(err.kind, ErrorKind::Internal));
}

#[test]
fn test_app_error_from_anyhow() {
    let anyhow_err = anyhow::anyhow!("anyhow error");
    let err: AppError = anyhow_err.into();
    assert_eq!(err.message, "anyhow error");
    assert!(matches!(err.kind, ErrorKind::Internal));
}

#[test]
fn test_app_error_serialization_contract() {
    // This is the contract with the frontend — AppError must serialize to
    // { "kind": "snake_case_variant", "message": "..." }
    let err = AppError::connection_lost("disconnected");
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["kind"], "connection_lost");
    assert_eq!(json["message"], "disconnected");
}

#[test]
fn test_all_error_kinds_serialize_to_snake_case() {
    let cases: Vec<(AppError, &str)> = vec![
        (AppError::connection_lost(""), "connection_lost"),
        (AppError::timeout(""), "timeout"),
        (AppError::session_corrupted(""), "session_corrupted"),
        (AppError::image_unsupported(""), "image_unsupported"),
        (AppError::lock(""), "lock_error"),
        (AppError::new(ErrorKind::SerializeError, ""), "serialize_error"),
        (AppError::new(ErrorKind::RateLimited, ""), "rate_limited"),
        (AppError::internal(""), "internal"),
    ];
    for (err, expected_kind) in cases {
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], expected_kind, "ErrorKind serialization mismatch");
    }
}

#[test]
fn test_app_error_question_mark_from_string_result() {
    fn inner() -> Result<(), AppError> {
        let r: Result<(), String> = Err("fail".to_string());
        r?;
        Ok(())
    }
    let err = inner().unwrap_err();
    assert_eq!(err.message, "fail");
}
