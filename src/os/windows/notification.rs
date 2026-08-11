// Windows clickable toast via `tauri-winrt-notification`.
//
// The notification plugin can only display a toast on desktop; it wires up no
// activation callback. This crate's `Toast::on_activated` fires (in-process,
// on a WinRT callback thread) when the user clicks the toast, which is exactly
// what we need to foreground the app. It works only for an app with a
// registered AUMID — the NSIS-installed build has one (`config.identifier`);
// dev builds run under the PowerShell AUMID and clicks won't route, so callers
// only take this path for installed builds.

use tauri_winrt_notification::{Duration, Toast};

/// Show a toast whose click invokes `on_click`. Returns `true` on success.
pub fn show_clickable_impl(
    app_id: &str,
    title: &str,
    body: &str,
    on_click: Box<dyn FnMut() + Send + 'static>,
) -> bool {
    // `on_activated` wants `FnMut(Option<String>) -> Result<()>`; adapt our
    // arg-less callback. We use no action buttons, so the activation argument
    // is irrelevant — any activation means "the user clicked the toast".
    let mut cb = on_click;
    let toast = Toast::new(app_id)
        .title(title)
        .text1(body)
        .duration(Duration::Short)
        .on_activated(move |_arg| {
            cb();
            Ok(())
        });

    match toast.show() {
        Ok(()) => true,
        Err(e) => {
            // English-only log (see I18N contract).
            log::warn!("WinRT clickable toast failed ({e}); caller will fall back");
            false
        }
    }
}
