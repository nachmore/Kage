// macOS: register `WebContinuousSpellCheckingEnabled = YES` as a fallback
// default so WKWebView underlines misspelled words, and answer spelling
// questions with NSSpellChecker. See src/os/spell_check.rs.

use crate::os::spell_check::Misspelling;
use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSDictionary, NSNotFound, NSNumber, NSString, NSUserDefaults};

/// WebKit's NSUserDefaults key for "Check Spelling While Typing".
const CONTINUOUS_SPELL_CHECKING_KEY: &str = "WebContinuousSpellCheckingEnabled";

/// NSSpellChecker isn't documented as thread-safe for these calls, and
/// WebKit uses the same shared checker from the main thread, so Kage calls
/// it there too.
pub const NEEDS_MAIN_THREAD: bool = true;

pub fn enable_webview_spell_checking_impl() {
    let key = NSString::from_str(CONTINUOUS_SPELL_CHECKING_KEY);
    let yes = NSNumber::new_bool(true);
    let value: &AnyObject = &yes;
    let fallback = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[value]);
    // SAFETY: the dictionary is NSString -> NSNumber, the key and value
    // types `registerDefaults:` expects.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&fallback) };
}

/// `None` when `word` is spelled correctly, else the flagged part of it and
/// NSSpellChecker's guesses for that part, best first. Uses the checker's
/// language setting, which is "Automatic by Language" unless the user
/// picked one in System Settings — the same language WebKit underlines
/// with.
pub fn check_word_impl(word: &str) -> Option<Misspelling> {
    // Drain whatever the checker autoreleases here, on whichever thread.
    autoreleasepool(|_| {
        let checker = NSSpellChecker::sharedSpellChecker();
        let text = NSString::from_str(word);
        let flagged = checker.checkSpellingOfString_startingAt(&text, 0);
        if flagged.length == 0 || flagged.location == NSNotFound as usize {
            return None;
        }
        // Tag 0: no per-document ignore list (WebKit keeps its own per page).
        let guesses = checker
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(flagged, &text, None, 0);
        Some(Misspelling {
            range: flagged.location..flagged.location.saturating_add(flagged.length),
            guesses: guesses
                .map(|list| list.iter().map(|g| g.to_string()).collect())
                .unwrap_or_default(),
        })
    })
}

/// Adds `word` to the user's spelling dictionary (shared by every app,
/// WebKit included).
pub fn learn_word_impl(word: &str) -> Result<(), String> {
    autoreleasepool(|_| {
        NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word));
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_continuous_spell_checking_as_a_fallback_default() {
        enable_webview_spell_checking_impl();
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = NSString::from_str(CONTINUOUS_SPELL_CHECKING_KEY);
        assert!(defaults.boolForKey(&key));
    }

    #[test]
    fn flags_a_misspelling_with_suggestions() {
        let guesses = crate::os::spell_check::check_word("helloo")
            .expect("\"helloo\" should be flagged as misspelled");
        assert!(
            guesses.iter().any(|g| g == "hello"),
            "expected \"hello\" among {guesses:?}"
        );
        assert!(guesses.len() <= crate::os::spell_check::MAX_SUGGESTIONS);
    }

    #[test]
    fn does_not_flag_a_correct_word() {
        assert_eq!(crate::os::spell_check::check_word("hello"), None);
    }
}
