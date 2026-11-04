//! Navigation guard for every Kage webview.
//!
//! Kage windows are privileged: they get `window.__TAURI__` and can call
//! every app command. None of them should ever *become* a web page. Before
//! this guard, a plain `<a href="https://…">` (no `target`) clicked in a
//! window without the JS link handler navigated that window to the remote
//! site — the Spotify extension's "developer.spotify.com/dashboard" setup
//! link replaced the whole Settings window with Spotify's dashboard, with
//! no way back.
//!
//! The JS layer (`ui/js/shared/link-handler.js`, installed in every
//! window) is the primary fix: it intercepts link clicks and opens http(s)
//! / mailto through `open_url`. This module is the backstop below it:
//!
//! - **In-place navigations** (all windows, including the ones Tauri
//!   preloads from `tauri.conf.json`): a Tauri plugin's `on_navigation`
//!   hook runs for every webview the app manager creates. Anything that
//!   isn't the app's own origin or an inert in-process document is
//!   cancelled. See [`decide_navigation`] for when a cancelled http(s)
//!   navigation is also handed to the system browser.
//! - **New-window requests** (`target=_blank`, `window.open`, middle-click):
//!   with no handler, wry denies these on every platform, so the preloaded
//!   windows (`main`, `floating`, `inline-assist`) are already safe and
//!   keep their exact startup path. Windows built on demand
//!   (settings, store, welcome, context menu, chat) additionally get
//!   [`NewWindowGuardExt::guard_new_windows`], which still denies the
//!   in-app window but forwards http(s)/mailto to the browser.
//!
//! What keeps working (and why it's allowed):
//! - Window loads and in-app routing (`settings.html?section=…`,
//!   `location.reload()`): the app origin — `tauri://localhost`
//!   (macOS/Linux), `http(s)://tauri.localhost` (Windows), or the dev
//!   server origin when built without `custom-protocol` (`cargo tauri dev`).
//! - Extension sandbox iframes and the code "Try" runner: they boot from
//!   `srcdoc` / no src (`about:srcdoc`, `about:blank`).
//! - `blob:` / `data:` documents: created by script already running in the
//!   page; `<a download href="blob:…">` exports route through the
//!   navigation policy on some engines (WebKitGTK, pre-11.3 WKWebView).
//! - OAuth loopback flows and `kage://` deep links never navigate a
//!   webview — the browser hits the Rust listener / the OS hands the URL
//!   to the single-instance plugin.

use tauri::webview::NewWindowResponse;
use tauri::{Manager, Runtime, Url, Webview, WebviewWindowBuilder};

/// Where a navigation is headed, as far as the guard is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTarget {
    /// The app's own frontend.
    App,
    /// In-process documents that can't reach the network on their own:
    /// `about:` (iframe bootstraps), `blob:`, `data:`.
    Inert,
    /// Belongs in the user's browser / mail client: `http`, `https`, `mailto`.
    External,
    /// Never followed and never handed off: `javascript:`, `file:`,
    /// `kage:` (routed in JS), Tauri's internal protocols (`asset:`, `ipc:`,
    /// `*.localhost` on Windows), and every other scheme. Custom app
    /// schemes launch other programs — that's the `launch` capability's
    /// territory, not a link click's.
    Disallowed,
}

/// What the guard does with a navigation / new-window request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Let the webview load it.
    Allow,
    /// Cancel it; open nothing.
    Block,
    /// Cancel it in the webview and open the URL in the system handler.
    BlockAndOpen,
}

/// Classify `url`. `dev_origin` is the dev server URL when the binary is a
/// dev build (see [`dev_origin`]); `None` in release builds, so a release
/// binary never treats `http://localhost:1420` as part of the app.
pub fn classify(url: &Url, dev_origin: Option<&Url>) -> NavTarget {
    let host = url.host_str().unwrap_or("");
    match url.scheme() {
        // macOS / Linux custom-protocol origin.
        "tauri" if host == "localhost" => NavTarget::App,
        "http" | "https" => {
            if host == "tauri.localhost" {
                // Windows custom-protocol origin (http, or https with
                // `useHttpsScheme`).
                NavTarget::App
            } else if dev_origin.is_some_and(|d| d.origin() == url.origin()) {
                NavTarget::App
            } else if host.ends_with(".localhost") {
                // WebView2 exposes every other Tauri custom protocol as
                // `http://<scheme>.localhost` (`asset.localhost`,
                // `ipc.localhost`, …). Not a web page and not the app.
                NavTarget::Disallowed
            } else {
                NavTarget::External
            }
        }
        "mailto" => NavTarget::External,
        "about" | "blob" | "data" => NavTarget::Inert,
        _ => NavTarget::Disallowed,
    }
}

/// Whether the engine's navigation callback only ever reports the *main
/// frame*. Only then can a cancelled external navigation safely be handed
/// to the browser.
///
/// - WebView2 (Windows): wry hooks `NavigationStarting`, which is main-frame
///   only (iframes raise `FrameNavigationStarting`, which wry doesn't
///   forward). A main-frame navigation can only come from first-party code
///   or a user clicking a link in a first-party document: sandboxed iframes
///   lack `allow-top-navigation`.
/// - WKWebView (macOS) / WebKitGTK (Linux): wry forwards
///   `decidePolicyForNavigationAction` / `decide-policy` for *every* frame
///   without saying which. An extension's sandbox iframe can run
///   `location.href = "https://…"` at any time, no gesture needed;
///   opening the browser for that would let an extension without the
///   `urls` capability open arbitrary sites. There we cancel and log only —
///   the JS link handler is what opens links the user actually clicked.
pub const NAVIGATION_CALLBACK_IS_MAIN_FRAME_ONLY: bool = cfg!(target_os = "windows");

/// Decide an in-place navigation (any frame — see
/// [`NAVIGATION_CALLBACK_IS_MAIN_FRAME_ONLY`]).
pub fn decide_navigation(target: NavTarget, main_frame_only: bool) -> Decision {
    match target {
        NavTarget::App | NavTarget::Inert => Decision::Allow,
        NavTarget::External if main_frame_only => Decision::BlockAndOpen,
        NavTarget::External | NavTarget::Disallowed => Decision::Block,
    }
}

/// Decide a new-window request. Never `Allow`: Kage has no use for
/// engine-created popup windows (its own windows are built in Rust).
/// Opening externally is safe on every platform because only a
/// first-party top-level document can raise one — our iframes are all
/// sandboxed without `allow-popups`, so the engine blocks theirs before it
/// asks us.
pub fn decide_new_window(target: NavTarget) -> Decision {
    match target {
        NavTarget::External => Decision::BlockAndOpen,
        NavTarget::App | NavTarget::Inert | NavTarget::Disallowed => Decision::Block,
    }
}

/// `scheme://host` (or just `scheme:`) for logs. Paths and query strings
/// are dropped — OAuth codes and tokens live there, and `app.jsonl` gets
/// attached to bug reports.
pub fn redact_for_log(url: &Url) -> String {
    match url.host_str() {
        Some(host) => format!("{}://{}", url.scheme(), host),
        None => format!("{}:", url.scheme()),
    }
}

/// The dev server origin when this is a dev build (no `custom-protocol`
/// feature — `cargo tauri dev`, plain `cargo build`), else `None`.
pub fn dev_origin<R: Runtime, M: Manager<R>>(manager: &M) -> Option<Url> {
    if !tauri::is_dev() {
        return None;
    }
    manager.config().build.dev_url.clone()
}

/// Hand `url` to the OS default handler off the calling thread (navigation
/// callbacks run on the UI thread; `ShellExecuteExW` with `NOASYNC` blocks).
fn open_externally(url: &Url) {
    let url = url.to_string();
    std::thread::spawn(move || {
        if let Err(e) = crate::os::open_url(&url) {
            log::warn!("[nav-guard] failed to open external URL: {e}");
        }
    });
}

fn on_navigation<R: Runtime>(webview: &Webview<R>, url: &Url) -> bool {
    let dev = dev_origin(webview);
    let target = classify(url, dev.as_ref());
    match decide_navigation(target, NAVIGATION_CALLBACK_IS_MAIN_FRAME_ONLY) {
        Decision::Allow => true,
        Decision::Block => {
            log::warn!(
                "[nav-guard] blocked navigation in '{}' to {} ({:?})",
                webview.label(),
                redact_for_log(url),
                target
            );
            false
        }
        Decision::BlockAndOpen => {
            log::info!(
                "[nav-guard] navigation in '{}' to {} redirected to the system browser",
                webview.label(),
                redact_for_log(url)
            );
            open_externally(url);
            false
        }
    }
}

/// The plugin that installs [`on_navigation`] on every webview — including
/// the ones Tauri builds from `tauri.conf.json` before `setup()` runs,
/// which no `WebviewWindowBuilder` hook can reach.
pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("kage-navigation-guard")
        .on_navigation(on_navigation)
        .build()
}

/// Builder extension for on-demand windows: deny every engine-created
/// popup, forwarding http(s)/mailto to the system browser instead.
pub trait NewWindowGuardExt<R: Runtime> {
    fn guard_new_windows<M2: Manager<R>>(self, manager: &M2) -> Self;
}

impl<'a, R: Runtime, M: Manager<R>> NewWindowGuardExt<R> for WebviewWindowBuilder<'a, R, M> {
    fn guard_new_windows<M2: Manager<R>>(self, manager: &M2) -> Self {
        let dev = dev_origin(manager);
        self.on_new_window(move |url, _features| {
            let target = classify(&url, dev.as_ref());
            match decide_new_window(target) {
                Decision::BlockAndOpen => {
                    log::info!(
                        "[nav-guard] new-window request for {} opened in the system browser",
                        redact_for_log(&url)
                    );
                    open_externally(&url);
                }
                Decision::Block | Decision::Allow => {
                    log::warn!(
                        "[nav-guard] blocked new-window request for {} ({:?})",
                        redact_for_log(&url),
                        target
                    );
                }
            }
            NewWindowResponse::Deny
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap_or_else(|e| panic!("bad test URL {s}: {e}"))
    }

    fn c(s: &str) -> NavTarget {
        classify(&u(s), None)
    }

    #[test]
    fn app_origins_are_app() {
        assert_eq!(c("tauri://localhost/settings.html"), NavTarget::App);
        assert_eq!(
            c("tauri://localhost/settings.html?section=spotify"),
            NavTarget::App
        );
        assert_eq!(c("http://tauri.localhost/floating.html"), NavTarget::App);
        assert_eq!(c("https://tauri.localhost/index.html#x"), NavTarget::App);
    }

    #[test]
    fn lookalike_app_origins_are_not_app() {
        assert_eq!(
            c("tauri://evil.example/settings.html"),
            NavTarget::Disallowed
        );
        assert_eq!(
            c("https://tauri.localhost.evil.example/"),
            NavTarget::External
        );
        assert_eq!(c("https://localhost.tauri.example/"), NavTarget::External);
    }

    #[test]
    fn dev_server_is_app_only_when_given() {
        let dev = u("http://localhost:1420");
        let page = u("http://localhost:1420/settings.html");
        assert_eq!(classify(&page, Some(&dev)), NavTarget::App);
        // Release builds pass None — the dev port is just another site.
        assert_eq!(classify(&page, None), NavTarget::External);
        // Same host, different port/scheme is a different origin.
        assert_eq!(
            classify(&u("http://localhost:3000/"), Some(&dev)),
            NavTarget::External
        );
        assert_eq!(
            classify(&u("https://localhost:1420/"), Some(&dev)),
            NavTarget::External
        );
    }

    #[test]
    fn web_and_mail_links_are_external() {
        assert_eq!(
            c("https://developer.spotify.com/dashboard"),
            NavTarget::External
        );
        assert_eq!(c("http://example.com/"), NavTarget::External);
        assert_eq!(c("mailto:someone@example.com"), NavTarget::External);
        assert_eq!(
            c("http://127.0.0.1:8080/spotify/callback"),
            NavTarget::External
        );
    }

    #[test]
    fn in_process_documents_are_inert() {
        assert_eq!(c("about:blank"), NavTarget::Inert);
        assert_eq!(c("about:srcdoc"), NavTarget::Inert);
        assert_eq!(
            c("blob:tauri://localhost/3b1c4d2e-0000-0000-0000-000000000000"),
            NavTarget::Inert
        );
        assert_eq!(c("data:text/plain,hello"), NavTarget::Inert);
    }

    #[test]
    fn dangerous_and_foreign_schemes_are_disallowed() {
        assert_eq!(c("javascript:alert(1)"), NavTarget::Disallowed);
        assert_eq!(c("JAVASCRIPT:alert(1)"), NavTarget::Disallowed);
        assert_eq!(c("file:///etc/passwd"), NavTarget::Disallowed);
        assert_eq!(
            c("file:///C:/Windows/System32/calc.exe"),
            NavTarget::Disallowed
        );
        assert_eq!(c("kage:settings"), NavTarget::Disallowed);
        assert_eq!(c("kage://install/foo"), NavTarget::Disallowed);
        assert_eq!(c("spotify:track:123"), NavTarget::Disallowed);
        assert_eq!(c("vscode://file/x"), NavTarget::Disallowed);
        assert_eq!(c("tel:+15555550100"), NavTarget::Disallowed);
        assert_eq!(c("ftp://example.com/f"), NavTarget::Disallowed);
        assert_eq!(c("asset://localhost/%2Ftmp%2Fx.png"), NavTarget::Disallowed);
        assert_eq!(c("ipc://localhost/open_url"), NavTarget::Disallowed);
        // Windows spelling of Tauri's internal protocols.
        assert_eq!(
            c("http://asset.localhost/C%3A%2Fx.png"),
            NavTarget::Disallowed
        );
        assert_eq!(c("http://ipc.localhost/open_url"), NavTarget::Disallowed);
    }

    #[test]
    fn navigation_decisions() {
        for main_frame_only in [true, false] {
            assert_eq!(
                decide_navigation(NavTarget::App, main_frame_only),
                Decision::Allow
            );
            assert_eq!(
                decide_navigation(NavTarget::Inert, main_frame_only),
                Decision::Allow
            );
            assert_eq!(
                decide_navigation(NavTarget::Disallowed, main_frame_only),
                Decision::Block
            );
        }
        // Only a main-frame-only callback may hand the URL to the browser…
        assert_eq!(
            decide_navigation(NavTarget::External, true),
            Decision::BlockAndOpen
        );
        // …otherwise a sandbox iframe's `location.href = …` would open it.
        assert_eq!(
            decide_navigation(NavTarget::External, false),
            Decision::Block
        );
    }

    #[test]
    fn main_frame_flag_matches_platform() {
        assert_eq!(
            NAVIGATION_CALLBACK_IS_MAIN_FRAME_ONLY,
            cfg!(target_os = "windows")
        );
    }

    #[test]
    fn new_window_decisions_never_allow() {
        assert_eq!(
            decide_new_window(NavTarget::External),
            Decision::BlockAndOpen
        );
        assert_eq!(decide_new_window(NavTarget::App), Decision::Block);
        assert_eq!(decide_new_window(NavTarget::Inert), Decision::Block);
        assert_eq!(decide_new_window(NavTarget::Disallowed), Decision::Block);
    }

    #[test]
    fn redaction_drops_path_and_query() {
        assert_eq!(
            redact_for_log(&u("http://127.0.0.1:8080/spotify/callback?code=SECRET")),
            "http://127.0.0.1"
        );
        assert_eq!(
            redact_for_log(&u("https://accounts.example/authorize?token=SECRET#frag")),
            "https://accounts.example"
        );
        assert_eq!(redact_for_log(&u("mailto:someone@example.com")), "mailto:");
        assert_eq!(
            redact_for_log(&u("javascript:alert(document.cookie)")),
            "javascript:"
        );
    }
}
