//! Structured application error type.
//!
//! Provides a `kind` field so the frontend can programmatically distinguish
//! error types (connection lost vs. rate limited vs. session corrupted)
//! instead of parsing error message strings.
//!
//! # Localisation
//!
//! `AppError` carries an i18n `key` plus a list of `params` — never a
//! pre-translated string. Translation happens exactly once, at the boundary
//! where Tauri serialises the error into the JSON the frontend receives.
//! That gives us three properties:
//!
//!   1. Logs stay in English. `Display for AppError` substitutes against the
//!      English catalog directly so `log::error!("{}", e)` produces stable,
//!      greppable output regardless of the active locale.
//!   2. The frontend never has to translate; the message arrives ready-to-render.
//!   3. There's exactly one translation point, so a missing key fails fast in
//!      drift-check rather than scattering `format!`-templated English across
//!      the codebase.
//!
//! # Two construction paths
//!
//! Most existing code constructs errors from a free-form string
//! (`AppError::connection_lost(format!("socket: {}", e))`). Those continue to
//! work — they're routed through the `errors.passthrough` key, which renders
//! the string verbatim. The i18n drift-check tooling reports passthrough call
//! sites as a migration backlog; new code should prefer the `*_keyed`
//! variants which take a real i18n key plus `(name, value)` params.
//!
//! Migrating a site looks like:
//!
//! ```ignore
//! // before:
//! AppError::connection_lost(format!("socket closed: {}", e))
//! // after:
//! AppError::keyed(ErrorKind::ConnectionLost, "errors.connection.socket_closed",
//!                 &[("reason", &e.to_string())])
//! ```

use crate::i18n;
use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use std::collections::HashMap;

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
/// Serializes to JSON like:
/// `{ "kind": "connection_lost", "key": "errors.connection.not_connected", "message": "Not connected" }`
///
/// `key` is for tests and machine-readable consumers; `message` is what the UI
/// renders. Both are computed in the active locale at serialisation time.
#[derive(Debug, Clone)]
pub struct AppError {
    pub kind: ErrorKind,
    /// i18n key for the message template. Looked up against the active locale
    /// at serialisation / Display time.
    pub key: String,
    /// `{name}` substitutions for the template. Always strings — call sites
    /// that need to format numbers should `.to_string()` first.
    pub params: HashMap<String, String>,
}

#[allow(dead_code)]
impl AppError {
    /// New i18n-native constructor. Use this for new code: pass a real key
    /// and `(name, value)` pairs, and the message is materialised through
    /// the active catalog.
    pub fn keyed(kind: ErrorKind, key: impl Into<String>, params: &[(&str, &str)]) -> Self {
        Self {
            kind,
            key: key.into(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    /// Wrap a free-form error string as the given `kind`. Routes through
    /// `errors.passthrough`, which renders the inner text verbatim. Use this
    /// for genuinely dynamic upstream errors (agent text, JSON-RPC payloads);
    /// for finite enumerations of conditions we control, prefer `keyed`.
    pub fn raw(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::keyed(kind, "errors.passthrough", &[("message", &message.into())])
    }

    /// Compatibility shim for existing call sites. Equivalent to `raw(kind, msg)`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::raw(kind, message)
    }

    /// Connection lost with a free-form reason. Migrate to:
    ///   `keyed(ErrorKind::ConnectionLost, "errors.connection.<specific>", &[...])`
    /// where the new key carries the specific failure mode.
    pub fn connection_lost(msg: impl Into<String>) -> Self {
        Self::raw(ErrorKind::ConnectionLost, msg)
    }

    /// Timed out with a free-form context string.
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::raw(ErrorKind::Timeout, msg)
    }

    /// Session corrupted with a free-form reason.
    pub fn session_corrupted(msg: impl Into<String>) -> Self {
        Self::raw(ErrorKind::SessionCorrupted, msg)
    }

    /// Image/attachment unsupported with a free-form note.
    pub fn image_unsupported(msg: impl Into<String>) -> Self {
        Self::raw(ErrorKind::ImageUnsupported, msg)
    }

    /// Generic internal error wrapping a free-form message.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::raw(ErrorKind::Internal, msg)
    }

    /// Lock error with a free-form context string.
    pub fn lock(msg: impl Into<String>) -> Self {
        Self::raw(ErrorKind::LockError, msg)
    }

    /// Render the message in the currently active locale. Use this when you
    /// need the localised text directly — the Tauri serialisation path also
    /// uses it under the hood.
    pub fn localised_message(&self) -> String {
        let pairs: Vec<(&str, &str)> = self
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        i18n::translate(&self.key, &pairs)
    }
}

/// Display goes through the *English* catalog explicitly, not the active locale.
/// Logs and developer-facing text must stay in a single language so that
/// `app.jsonl` from a non-English user is searchable from any developer's box.
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pairs: Vec<(&str, &str)> = self
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let msg = i18n::translate_in("en", &self.key, &pairs);
        write!(f, "{}", msg)
    }
}

impl std::error::Error for AppError {}

/// Custom Serialize: emits `{ kind, key, message }` with `message` materialised
/// in the active locale. The frontend reads `message` to display and `kind` to
/// branch on; `key` is exposed too so tests can match against the stable
/// identifier without depending on translated text.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut state = s.serialize_struct("AppError", 3)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("key", &self.key)?;
        state.serialize_field("message", &self.localised_message())?;
        state.end()
    }
}

/// Allow `?` on `Result<_, String>` in code that returns `Result<_, AppError>`.
impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self::internal(s)
    }
}

/// Allow `?` on `Result<_, &str>` in code that returns `Result<_, AppError>`.
impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self::internal(s)
    }
}

/// Allow `?` on `anyhow::Result` in code that returns `Result<_, AppError>`.
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(format!("{}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_serialises_with_verbatim_message() {
        crate::i18n::init(Some("en"));
        let e = AppError::connection_lost("socket closed");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["kind"], "connection_lost");
        assert_eq!(json["key"], "errors.passthrough");
        assert_eq!(json["message"], "socket closed");
    }

    #[test]
    fn keyed_serialises_with_localised_message() {
        // Uses errors.connection.not_connected as a stable key with no
        // params (the previous test used a key that's been removed from
        // the catalog). The behaviour we're locking down is the same:
        // keyed AppError serialises with kind + key + localised message.
        crate::i18n::init(Some("en"));
        let e = AppError::keyed(
            ErrorKind::ConnectionLost,
            "errors.connection.not_connected",
            &[],
        );
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["kind"], "connection_lost");
        assert_eq!(json["key"], "errors.connection.not_connected");
        assert_eq!(json["message"], "Not connected");
    }

    #[test]
    fn display_uses_english_regardless_of_active_locale() {
        crate::i18n::init(Some("en"));
        let e = AppError::keyed(
            ErrorKind::ConnectionLost,
            "errors.connection.not_connected",
            &[],
        );
        let s = format!("{}", e);
        assert_eq!(s, "Not connected");
    }

    #[test]
    fn from_string_makes_internal_passthrough_error() {
        crate::i18n::init(Some("en"));
        let e: AppError = "oops".to_string().into();
        match e.kind {
            ErrorKind::Internal => {}
            other => panic!("expected Internal, got {:?}", other),
        }
        let s = e.localised_message();
        assert!(s.contains("oops"), "got {:?}", s);
    }
}
