// Cross-platform "clickable" system notification.
//
// `tauri-plugin-notification` can display a toast but, on desktop, delivers no
// click/activation callback (its `onAction` is mobile-only). For the
// "response ready" toast we want a click to foreground the app, so on Windows
// we bypass the plugin and drive the WinRT toast directly, which DOES expose an
// activation callback. Other platforms have no working equivalent yet, so the
// abstraction reports whether it handled the request; callers fall back to the
// plain fire-and-forget plugin toast when it didn't.

/// Show a system notification that invokes `on_click` when the user activates
/// (clicks) it.
///
/// - `app_id`: the platform notification identity. On Windows this is the
///   AppUserModelID (AUMID) — must match the installed app's registered id
///   (`config.identifier`) or the toast won't render with the app's identity
///   and the click won't route.
/// - Returns `true` if a clickable notification was shown, `false` if the
///   platform can't (caller should fall back to the plugin toast).
///
/// `on_click` runs on a platform callback thread — it must be `Send` and do its
/// own thread hop if it touches thread-affine state (e.g. Tauri windows).
pub fn show_clickable(
    app_id: &str,
    title: &str,
    body: &str,
    on_click: Box<dyn FnMut() + Send + 'static>,
) -> bool {
    #[cfg(target_os = "windows")]
    {
        crate::os::platform::notification::show_clickable_impl(app_id, title, body, on_click)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // No clickable-toast support on this platform yet.
        let _ = (app_id, title, body, on_click);
        false
    }
}
