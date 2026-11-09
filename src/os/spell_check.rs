// Spell checking for Kage's webviews.
//
// Underlines: on macOS, WKWebView draws the red "misspelled" underline only
// when the app's `WebContinuousSpellCheckingEnabled` default is YES. WebKit
// reads that default once, the first time a webview needs it
// (Source/WebKit/UIProcess/mac/TextCheckerMac.mm), and an app that never set
// it reads NO. Automatic correction is different: it follows the system
// setting. So Kage had autocorrect but no underlines. Safari sets the key
// itself, which is why it underlines and we didn't.
//
// WebView2 on Windows already spell-checks editable text. WebKitGTK on
// Linux is unchanged.
//
// Suggestions: Kage's right-click menu is its own popup window, not the
// webview's native menu, so it can't show WebKit's spelling items. It asks
// the OS spell checker directly instead (`check_word`, `learn_word`):
// NSSpellChecker on macOS, the Windows Spell Checking API on Windows.
// Linux has no system spell checker to ask, so it reports every word as
// correct and the menu stays the plain text menu.

/// The longest word `check_word` / `learn_word` accept, in characters.
pub const MAX_WORD_CHARS: usize = 64;

/// The most suggestions `check_word` returns.
pub const MAX_SUGGESTIONS: usize = 5;

/// The longest suggestion `check_word` returns, in characters. Longer
/// guesses (multi-word phrases) are dropped rather than cut.
pub const MAX_SUGGESTION_CHARS: usize = 128;

/// Whether `check_word` / `learn_word` must run on the main (UI) thread.
/// When false they block, so call them from a blocking thread.
pub const NEEDS_MAIN_THREAD: bool = crate::os::platform::spell_check::NEEDS_MAIN_THREAD;

/// Turn on spell-check underlines for every webview Kage creates. Call it
/// before the first webview is built, because WebKit caches the setting.
///
/// The value is registered as a fallback, so an explicit choice still wins:
/// "Check Spelling While Typing" in a text field's context menu writes the
/// user's own value to the app's defaults, and WebKit reads that first.
pub fn enable_webview_spell_checking() {
    crate::os::platform::spell_check::enable_webview_spell_checking_impl()
}

/// `word` trimmed, if it's something the spell checker should be asked
/// about: one word (no whitespace or control characters), at most
/// `MAX_WORD_CHARS` long, with at least one letter.
pub fn normalize_word(word: &str) -> Option<&str> {
    let word = word.trim();
    let ok = !word.is_empty()
        && word.chars().count() <= MAX_WORD_CHARS
        && !word.chars().any(|c| c.is_whitespace() || c.is_control())
        && word.chars().any(char::is_alphabetic);
    ok.then_some(word)
}

/// Check one word with the OS spell checker. `None`: spelled correctly, not
/// a checkable word (see `normalize_word`), or no spell checker on this OS.
/// `Some(guesses)`: misspelled, with up to `MAX_SUGGESTIONS` replacements,
/// best first (possibly none).
///
/// Blocking; run it on the main thread when `NEEDS_MAIN_THREAD`, else on a
/// blocking thread.
pub fn check_word(word: &str) -> Option<Vec<String>> {
    let word = normalize_word(word)?;
    let guesses = crate::os::platform::spell_check::check_word_impl(word)?;
    Some(tidy_suggestions(word, guesses))
}

/// Add `word` to the user's spelling dictionary, so it's no longer flagged
/// here or in other apps that use the OS spell checker. A no-op on an OS
/// without one.
///
/// Blocking; same thread rules as `check_word`.
pub fn learn_word(word: &str) -> Result<(), String> {
    let word = normalize_word(word).ok_or_else(|| "not a single word".to_string())?;
    crate::os::platform::spell_check::learn_word_impl(word)
}

/// Trimmed, non-empty, not the word itself, no duplicates, bounded length,
/// at most `MAX_SUGGESTIONS`.
fn tidy_suggestions(word: &str, guesses: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for guess in guesses {
        let guess = guess.trim();
        if guess.is_empty()
            || guess == word
            || guess.chars().count() > MAX_SUGGESTION_CHARS
            || guess.chars().any(char::is_control)
            || out.iter().any(|g| g == guess)
        {
            continue;
        }
        out.push(guess.to_string());
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_word_accepts_one_word() {
        assert_eq!(normalize_word("  helloo "), Some("helloo"));
        assert_eq!(normalize_word("don't"), Some("don't"));
        assert_eq!(normalize_word("naïve"), Some("naïve"));
        assert_eq!(normalize_word("日本語"), Some("日本語"));
    }

    #[test]
    fn normalize_word_rejects_everything_else() {
        assert_eq!(normalize_word(""), None);
        assert_eq!(normalize_word("   "), None);
        assert_eq!(normalize_word("two words"), None);
        assert_eq!(normalize_word("tab\there"), None);
        assert_eq!(normalize_word("new\nline"), None);
        assert_eq!(normalize_word("nul\u{0}"), None);
        assert_eq!(normalize_word("1234"), None);
        assert_eq!(normalize_word("'"), None);
        assert_eq!(
            normalize_word(&"a".repeat(MAX_WORD_CHARS)).map(str::len),
            Some(64)
        );
        assert_eq!(normalize_word(&"a".repeat(MAX_WORD_CHARS + 1)), None);
        // Counted in characters, not bytes.
        let long_accented = "é".repeat(MAX_WORD_CHARS);
        assert!(normalize_word(&long_accented).is_some());
    }

    #[test]
    fn tidy_suggestions_dedupes_caps_and_drops_junk() {
        let raw = [
            " hello ", "hello", "", "helloo", "hell", "halo", "hallo", "hullo", "jello",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            tidy_suggestions("helloo", raw),
            ["hello", "hell", "halo", "hallo", "hullo"]
        );
        let long = vec!["x".repeat(MAX_SUGGESTION_CHARS + 1), "ok".into()];
        assert_eq!(tidy_suggestions("ko", long), ["ok"]);
    }

    #[test]
    fn invalid_words_are_never_checked_or_learned() {
        assert_eq!(check_word("two words"), None);
        assert_eq!(check_word(""), None);
        assert!(learn_word("two words").is_err());
        assert!(learn_word("").is_err());
    }
}
