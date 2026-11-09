// Windows: WebView2 underlines misspellings on its own, so there's nothing
// to register. Suggestions come from the Windows Spell Checking API
// (ISpellCheckerFactory / ISpellChecker), the same checker WebView2 uses.
// See src/os/spell_check.rs.

use crate::os::spell_check::Misspelling;
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Globalization::{
    ISpellChecker, ISpellCheckerFactory, ISpellingError, SpellCheckerFactory,
    CORRECTIVE_ACTION_GET_SUGGESTIONS, CORRECTIVE_ACTION_REPLACE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IEnumString,
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// The spell checker objects are free-threaded, so any blocking thread
/// will do.
pub const NEEDS_MAIN_THREAD: bool = false;

/// Used when none of the user's display languages has a dictionary.
const FALLBACK_LANGUAGE: &str = "en-US";

/// Upper bound on entries read from a suggestion enumerator, in case one
/// never reports the end.
const MAX_ENUMERATED: usize = 32;

pub fn enable_webview_spell_checking_impl() {}

pub fn check_word_impl(word: &str) -> Option<Misspelling> {
    with_com(|| check(word)).unwrap_or_else(|e| {
        log::debug!("[spell-check] check failed: {e}");
        None
    })
}

pub fn learn_word_impl(word: &str) -> Result<(), String> {
    with_com(|| {
        let checker = create_checker()?;
        // SAFETY: `checker` is a live ISpellChecker; the HSTRING outlives
        // the call and is NUL-terminated.
        unsafe { checker.Add(&HSTRING::from(word)) }
    })
    .map_err(|e| e.to_string())
}

/// Run `f` with COM initialised on this thread. A thread that already has
/// an apartment keeps it (CoInitializeEx fails with RPC_E_CHANGED_MODE and
/// COM is still usable); one we initialised is uninitialised again after
/// `f` — whose COM objects are all dropped by then — returns.
fn with_com<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: plain COM initialisation on the calling thread.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let out = f();
    if hr.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    out
}

/// A checker for the first of the user's display languages that has a
/// dictionary (or its base language), else for `FALLBACK_LANGUAGE`.
fn create_checker() -> windows::core::Result<ISpellChecker> {
    // SAFETY: COM is initialised on this thread (`with_com`).
    let factory: ISpellCheckerFactory =
        unsafe { CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER)? };
    for tag in candidate_languages(sys_locale::get_locales()) {
        let tag = HSTRING::from(tag.as_str());
        // SAFETY: `factory` is live; `tag` is NUL-terminated and outlives
        // the calls.
        let supported = unsafe { factory.IsSupported(&tag) }.is_ok_and(|b| b.as_bool());
        if supported {
            return unsafe { factory.CreateSpellChecker(&tag) };
        }
    }
    Err(E_FAIL.into())
}

/// Each preferred language, then its base language, then the fallback;
/// no duplicates.
fn candidate_languages(preferred: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |tag: &str| {
        let tag = tag.trim();
        if !tag.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            out.push(tag.to_string());
        }
    };
    for tag in preferred {
        push(&tag);
        if let Some((base, _)) = tag.split_once('-') {
            push(base);
        }
    }
    push(FALLBACK_LANGUAGE);
    out
}

fn check(word: &str) -> windows::core::Result<Option<Misspelling>> {
    let checker = create_checker()?;
    let text = HSTRING::from(word);
    // SAFETY: `checker` is live; `text` outlives the call.
    let errors = unsafe { checker.Check(&text)? };
    let mut error: Option<ISpellingError> = None;
    // S_OK with an error, or S_FALSE (and no error) when the text is clean.
    // SAFETY: `error` is a valid out slot for one interface pointer.
    unsafe { errors.Next(&mut error) }.ok()?;
    let Some(error) = error else {
        return Ok(None);
    };
    // SAFETY: `error` is a live ISpellingError.
    let action = unsafe { error.CorrectiveAction()? };
    if action != CORRECTIVE_ACTION_REPLACE && action != CORRECTIVE_ACTION_GET_SUGGESTIONS {
        // CORRECTIVE_ACTION_DELETE (a repeated word) can't apply to one
        // word; NONE means nothing to fix.
        return Ok(None);
    }
    // The flagged part of the text, in UTF-16 units: usually the whole
    // word, but a checker may flag only part of it ("l'" + "ordinateurr").
    // SAFETY: `error` is a live ISpellingError.
    let (start, len) = unsafe { (error.StartIndex()? as usize, error.Length()? as usize) };
    let range = flagged_range(start, len, text.len());
    let guesses = if action == CORRECTIVE_ACTION_REPLACE {
        // An autocorrect entry: one definite replacement for the range.
        // SAFETY: as above; the returned string is ours to free.
        unsafe { take_co_string(error.Replacement()?) }
            .into_iter()
            .collect()
    } else {
        let flagged = HSTRING::from_wide(&text[range.clone()]);
        // SAFETY: `checker` is live; `flagged` outlives the call.
        let suggestions = unsafe { checker.Suggest(&flagged)? };
        read_strings(&suggestions)
    };
    Ok(Some(Misspelling { range, guesses }))
}

/// `start..start + len` within a text `total` UTF-16 units long; the whole
/// text if that's empty or out of bounds.
fn flagged_range(start: usize, len: usize, total: usize) -> std::ops::Range<usize> {
    match start.checked_add(len) {
        Some(end) if len > 0 && end <= total => start..end,
        _ => 0..total,
    }
}

/// Every string in `list` (up to `MAX_ENUMERATED`), freeing each.
fn read_strings(list: &IEnumString) -> Vec<String> {
    let mut out = Vec::new();
    for _ in 0..MAX_ENUMERATED {
        let mut slot = [PWSTR::null()];
        let mut fetched = 0u32;
        // SAFETY: one writable slot, and a valid count pointer.
        let hr = unsafe { list.Next(&mut slot, Some(&mut fetched as *mut u32)) };
        if hr.is_err() || fetched == 0 {
            break;
        }
        // SAFETY: the enumerator handed us this CoTaskMem string.
        if let Some(s) = unsafe { take_co_string(slot[0]) } {
            out.push(s);
        }
    }
    out
}

/// Convert a CoTaskMemAlloc'd string and free it. `None` for null or
/// invalid UTF-16.
///
/// # Safety
/// `s` must be null or a NUL-terminated string allocated with
/// CoTaskMemAlloc that nothing else frees.
unsafe fn take_co_string(s: PWSTR) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let text = unsafe { s.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(s.0 as *const _)) };
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_display_languages_then_base_languages_then_en_us() {
        let langs = candidate_languages(["fr-CA", "de-DE", "fr-FR"].map(String::from).into_iter());
        assert_eq!(langs, ["fr-CA", "fr", "de-DE", "de", "fr-FR", "en-US"]);
        assert_eq!(candidate_languages(std::iter::empty()), ["en-US"]);
        let en = candidate_languages(["EN-us".to_string()].into_iter());
        assert_eq!(en, ["EN-us", "EN"]);
    }

    #[test]
    fn flagged_range_stays_inside_the_text() {
        assert_eq!(flagged_range(2, 11, 13), 2..13);
        assert_eq!(flagged_range(0, 6, 6), 0..6);
        assert_eq!(flagged_range(3, 0, 6), 0..6);
        assert_eq!(flagged_range(4, 5, 6), 0..6);
        assert_eq!(flagged_range(usize::MAX, 2, 6), 0..6);
    }

    #[test]
    fn flags_a_misspelling_with_suggestions() {
        let Some(guesses) = crate::os::spell_check::check_word("helloo") else {
            // No dictionary for any display language or en-US on this box.
            return;
        };
        assert!(guesses.len() <= crate::os::spell_check::MAX_SUGGESTIONS);
        assert_eq!(crate::os::spell_check::check_word("hello"), None);
    }
}
