// Linux: no system spell checker to ask, and nothing to register for
// underlines. See src/os/spell_check.rs.

pub const NEEDS_MAIN_THREAD: bool = false;

pub fn enable_webview_spell_checking_impl() {}

/// Every word counts as correct, so the menu stays the plain text menu.
pub fn check_word_impl(_word: &str) -> Option<Vec<String>> {
    None
}

pub fn learn_word_impl(_word: &str) -> Result<(), String> {
    Ok(())
}
