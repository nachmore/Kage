// macOS: register `WebContinuousSpellCheckingEnabled = YES` as a fallback
// default so WKWebView underlines misspelled words. See src/os/spell_check.rs.

use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};

/// WebKit's NSUserDefaults key for "Check Spelling While Typing".
const CONTINUOUS_SPELL_CHECKING_KEY: &str = "WebContinuousSpellCheckingEnabled";

pub fn enable_webview_spell_checking_impl() {
    let key = NSString::from_str(CONTINUOUS_SPELL_CHECKING_KEY);
    let yes = NSNumber::new_bool(true);
    let value: &AnyObject = &yes;
    let fallback = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[value]);
    // SAFETY: the dictionary is NSString -> NSNumber, the key and value
    // types `registerDefaults:` expects.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&fallback) };
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
}
