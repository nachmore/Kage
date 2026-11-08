// Spell-check underlines in Kage's webviews.
//
// On macOS, WKWebView draws the red "misspelled" underline only when the
// app's `WebContinuousSpellCheckingEnabled` default is YES. WebKit reads
// that default once, the first time a webview needs it
// (Source/WebKit/UIProcess/mac/TextCheckerMac.mm), and an app that never set
// it reads NO. Automatic correction is different: it follows the system
// setting. So Kage had autocorrect but no underlines. Safari sets the key
// itself, which is why it underlines and we didn't.
//
// WebView2 on Windows already spell-checks editable text. WebKitGTK on
// Linux is unchanged.

/// Turn on spell-check underlines for every webview Kage creates. Call it
/// before the first webview is built, because WebKit caches the setting.
///
/// The value is registered as a fallback, so an explicit choice still wins:
/// "Check Spelling While Typing" in a text field's context menu writes the
/// user's own value to the app's defaults, and WebKit reads that first.
pub fn enable_webview_spell_checking() {
    crate::os::platform::spell_check::enable_webview_spell_checking_impl()
}
