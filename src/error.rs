//! Structured application error type.
//!
//! Provides a `kind` field so the frontend can programmatically distinguish
//! error types (connection lost vs. rate limited vs. session corrupted)
//! instead of parsing error message strings.

use serde::Serialize;

/// Error kinds that the frontend can match on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ErrorKind {
    /// ACP server not connected or connection lost
    ConnectionLost,
    /// Request timed out waiting for response
    Timeout,
    /// Session is corrupted or invalid
    SessionCorrupted,
    /// Rate limit exceeded
    RateLimited,
    /// Image/attachment not supported by current model
    ImageUnsupported,
    /// Lock acquisition failed (internal)
    LockError,
    /// Serialization/deserialization failure
    SerializeError,
    /// Generic internal error (catch-all)
    Internal,
}

/// Structured error returned by Tauri commands.
///
/// Serializes to JSON like: `{ "kind": "connection_lost", "message": "..." }`
/// The frontend receives this as the rejection value of the invoke promise.
#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

#[allow(dead_code)]
impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Connection lost or not connected
    pub fn connection_lost(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::ConnectionLost, msg)
    }

    /// Request timed out
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, msg)
    }

    /// Session corrupted
    pub fn session_corrupted(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::SessionCorrupted, msg)
    }

    /// Image/attachment unsupported
    pub fn image_unsupported(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::ImageUnsupported, msg)
    }

    /// Generic internal error — use for anyhow/string errors that don't
    /// need special frontend handling.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, msg)
    }

    /// Lock error
    pub fn lock(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::LockError, msg)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

/// Allow `?` on `Result<_, String>` in code that returns `Result<_, AppError>`
impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self::internal(s)
    }
}

/// Allow `?` on `Result<_, &str>` in code that returns `Result<_, AppError>`
impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self::internal(s)
    }
}

/// Allow `?` on `anyhow::Result` in code that returns `Result<_, AppError>`
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(format!("{}", e))
    }
}

// Tauri uses `Into<InvokeError>` for command error types.
// `InvokeError` implements `From<T: Serialize>`, so our `Serialize` impl is sufficient.
// No additional trait implementation needed.
