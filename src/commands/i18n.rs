//! Tauri commands for the i18n bridge.
//!
//! The frontend asks for the catalog once during window startup via
//! `get_i18n_catalog`; we hand back the raw JSON (active locale + English
//! fallback) plus the metadata it needs to set `<html dir>` and surface the
//! "machine translated" banner.
//!
//! `set_language` is called from Settings → Appearance. It mutates the active
//! locale in `i18n.rs`, persists `config.ui.language`, and emits
//! `config_updated` so every window re-fetches the catalog. The frontend
//! `i18n.js` listens for that event and re-applies static translations and
//! direction without a window reload.

use crate::config::Config;
use crate::error::AppError;
use crate::i18n;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

/// Payload returned by `get_i18n_catalog`. Mirrors the shape consumed by
/// `ui/js/shared/i18n.js`.
#[derive(Debug, Serialize)]
pub struct I18nPayload {
    /// Active language code (the canonical key, e.g. "en", "zh-CN").
    pub language: String,
    /// OS-detected locale at app startup (e.g. "en-US", "ja-JP"), regardless
    /// of any user override. Used by the settings picker to render a "System
    /// default ({system})" hint that points at the *system* language, not
    /// whatever the user picked. Empty string when sys-locale couldn't
    /// detect anything.
    pub system_language: String,
    /// `true` if the active language is right-to-left.
    pub rtl: bool,
    /// `true` if the active catalog is mostly machine-translated.
    pub machine_translated: bool,
    /// Active locale's catalog as a `key → { message, description }` map.
    pub catalog: HashMap<String, crate::i18n::EntrySnapshot>,
    /// English fallback catalog — same shape. Always shipped so a missing
    /// translation in a non-English locale degrades gracefully without a
    /// second IPC roundtrip. For active=en this duplicates `catalog` but
    /// the cost is negligible and the frontend code path stays uniform.
    pub fallback: HashMap<String, crate::i18n::EntrySnapshot>,
}

/// Catalog payload metadata exposed for the language picker. The full per-language
/// catalog isn't shipped with the picker — only the codes + display names —
/// because we don't want to embed 30 catalogs in the dropdown.
#[derive(Debug, Serialize)]
pub struct AvailableLanguage {
    pub code: String,
    pub name: String,
    pub rtl: bool,
    pub machine_translated: bool,
}

#[tauri::command]
pub fn get_i18n_catalog() -> Result<I18nPayload, AppError> {
    let language = i18n::active_language();
    let rtl = i18n::active_is_rtl();
    let machine_translated = i18n::active_is_machine_translated();
    // sys_locale returns whatever the OS told us at startup; this is
    // independent of the user's `config.ui.language` override and lets the
    // settings UI honestly answer "what is the system language?".
    let system_language = sys_locale::get_locale().unwrap_or_default();

    let catalog = i18n::serialise_catalog(&language).unwrap_or_default();
    let fallback = if language == "en" {
        catalog.clone()
    } else {
        i18n::serialise_catalog("en").unwrap_or_default()
    };

    Ok(I18nPayload {
        language,
        system_language,
        rtl,
        machine_translated,
        catalog,
        fallback,
    })
}

#[tauri::command]
pub fn get_available_languages() -> Vec<AvailableLanguage> {
    i18n::available_languages()
        .into_iter()
        .map(|(code, name, rtl, machine_translated)| AvailableLanguage {
            code,
            name,
            rtl,
            machine_translated,
        })
        .collect()
}

#[tauri::command]
pub fn set_language(
    app: AppHandle,
    config: State<'_, std::sync::Arc<Mutex<Config>>>,
    language: Option<String>,
) -> Result<(), AppError> {
    // `None` (or an empty string) means "follow system locale". We persist
    // it as `None` so the next launch re-runs system detection rather than
    // pinning to whatever happened to be active when the user clicked.
    let normalised: Option<String> = language.filter(|s| !s.trim().is_empty());

    {
        let mut cfg = config.lock().map_err(|_| {
            AppError::keyed(
                crate::error::ErrorKind::LockError,
                "errors.lock.acquire_failed",
                &[],
            )
        })?;
        cfg.ui.language = normalised.clone();
        cfg.save().map_err(|e| {
            AppError::keyed(
                crate::error::ErrorKind::Internal,
                "errors.config.save_failed",
                &[("reason", &e.to_string())],
            )
        })?;
    }

    // Apply immediately so a follow-up `get_i18n_catalog` from any window
    // returns the new active locale. If the request was "follow system",
    // re-resolve via sys-locale; otherwise honour the explicit override.
    let preferred = normalised.clone().or_else(sys_locale::get_locale);
    i18n::set_language(preferred.as_deref().unwrap_or("en"));

    // Broadcast so every window rerenders.
    let _ = app.emit(crate::events::CONFIG_UPDATED, ());
    Ok(())
}
