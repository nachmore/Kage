//! Spelling suggestions for Kage's own right-click menu (the launcher
//! input's), which can't show the webview's native spelling items. The OS
//! spell checker does the work: see `crate::os::spell_check`.

use crate::error::{AppError, ErrorKind};
use crate::os::spell_check;
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellSuggestions {
    /// The OS spell checker flags the word. Always false where there's no
    /// spell checker (Linux) or for anything that isn't a single word.
    pub misspelled: bool,
    /// Replacements, best first, at most `spell_check::MAX_SUGGESTIONS`;
    /// may be empty even when `misspelled`.
    pub suggestions: Vec<String>,
}

impl SpellSuggestions {
    fn from_check(result: Option<Vec<String>>) -> Self {
        match result {
            Some(suggestions) => Self {
                misspelled: true,
                suggestions,
            },
            None => Self::default(),
        }
    }
}

/// Run blocking spell-checker work where the platform needs it: the main
/// thread (awaited without blocking this worker), or a blocking thread.
async fn run_spell_task<R, T, F>(app: &tauri::AppHandle<R>, f: F) -> Result<T, AppError>
where
    R: tauri::Runtime,
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if spell_check::NEEDS_MAIN_THREAD {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| AppError::internal(format!("run_on_main_thread: {e}")))?;
        rx.await
            .map_err(|e| AppError::internal(format!("spell check task dropped: {e}")))
    } else {
        tauri::async_runtime::spawn_blocking(f)
            .await
            .map_err(|e| AppError::internal(format!("spell check task failed: {e}")))
    }
}

/// Whether `word` is misspelled, and what the OS spell checker suggests
/// instead. Anything that isn't one short word comes back as not
/// misspelled without asking the OS.
#[tauri::command]
pub async fn spell_suggestions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    word: String,
) -> Result<SpellSuggestions, AppError> {
    let Some(word) = spell_check::normalize_word(&word).map(str::to_owned) else {
        return Ok(SpellSuggestions::default());
    };
    let result = run_spell_task(&app, move || spell_check::check_word(&word)).await?;
    Ok(SpellSuggestions::from_check(result))
}

/// Add `word` to the user's spelling dictionary ("Learn Spelling").
#[tauri::command]
pub async fn learn_spelling<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    word: String,
) -> Result<(), AppError> {
    let Some(word) = spell_check::normalize_word(&word).map(str::to_owned) else {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.spell_check.invalid_word",
            &[],
        ));
    };
    log::info!(
        "[spell-check] learning a word ({} chars)",
        word.chars().count()
    );
    run_spell_task(&app, move || spell_check::learn_word(&word))
        .await?
        .map_err(|e| AppError::internal(format!("learn spelling: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_misspelling_carries_its_suggestions() {
        let hit = SpellSuggestions::from_check(Some(vec!["hello".into()]));
        assert!(hit.misspelled);
        assert_eq!(hit.suggestions, ["hello"]);
        let none = SpellSuggestions::from_check(Some(vec![]));
        assert!(none.misspelled && none.suggestions.is_empty());
        assert_eq!(
            SpellSuggestions::from_check(None),
            SpellSuggestions::default()
        );
    }

    #[test]
    fn serializes_camel_case_for_the_frontend() {
        let json = serde_json::to_value(SpellSuggestions::from_check(Some(vec!["a".into()])))
            .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({ "misspelled": true, "suggestions": ["a"] })
        );
    }
}
