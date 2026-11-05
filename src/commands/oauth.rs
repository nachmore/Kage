//! Localhost loopback listener for OAuth (and OAuth-shaped) auth flows.
//!
//! How it's meant to be used by an extension:
//!
//! 1. Call [`oauth_loopback_start`] with a path like `/spotify/callback`
//!    and a sane timeout. We bind a fresh ephemeral port on `127.0.0.1`,
//!    register the listener, and return the absolute redirect URI plus an
//!    opaque session id.
//! 2. Use that redirect URI as the `redirect_uri` when constructing the
//!    auth URL, then open the auth URL in the user's browser.
//! 3. Call [`oauth_loopback_await`] with the session id. It resolves with
//!    the query parameters from the first matching request — typically
//!    `{ code, state }` for the success path or `{ error, error_description }`
//!    for the failure path.
//! 4. Call [`oauth_loopback_cancel`] if the user abandons the flow. It works
//!    whether or not an await is in flight: the listener stops (the port is
//!    closed by the time cancel resolves) and a pending await rejects. The
//!    extension host also cancels a sandbox's outstanding listeners when it
//!    tears the sandbox down: Tauri doesn't drop an in-flight await command
//!    when its caller goes away, so this side can't tell on its own.
//!
//! The listener is single-shot: as soon as the callback arrives, the timeout
//! expires, or the flow is cancelled, the listener is dropped, the port is
//! released, and the session id is invalidated. Multiple concurrent flows
//! are fine — each gets its own port and id.
//!
//! Failures are distinct [`AppError`] kinds so callers can branch on `kind`
//! rather than the message:
//!
//! | kind        | key                            | when                                    |
//! |-------------|--------------------------------|-----------------------------------------|
//! | `timeout`   | `errors.passthrough`           | no callback within the timeout          |
//! | `cancelled` | `errors.passthrough`           | `oauth_loopback_cancel` stopped it      |
//! | `internal`  | `errors.oauth.unknown_listener`| unknown / finished / cancelled id       |
//! | `internal`  | `errors.oauth.already_awaited` | a second await on the same id           |
//! | `internal`  | `errors.oauth.listener_failed` | the socket failed while accepting       |
//!
//! `timeout` and `cancelled` deliberately aren't localised: their message is
//! the fixed English text they carried before they had kinds
//! ([`TIMED_OUT_MESSAGE`], [`CANCELLED_MESSAGE`]), in every locale. Shipped
//! extensions (Spotify 0.4.0) tell a timeout apart by matching that text,
//! and a translated message would send their users to a generic error
//! instead of the timeout guidance. New code should branch on `kind`.
//!
//! Why we can't reuse `tauri-plugin-shell`'s `open_url` here: the redirect
//! `code` lives in a query string the user's browser hits, not in our
//! app. The cleanest cross-platform answer is a tiny in-process HTTP
//! listener bound to a loopback address — same pattern Spotify, GitHub,
//! and Google all explicitly bless for native PKCE clients.
//!
//! Security notes:
//!
//! - We bind on `127.0.0.1` only (NOT `0.0.0.0`). Other machines on the
//!   LAN cannot reach this port.
//! - The port is randomly chosen by the OS via port 0, so the listener
//!   isn't guessable across runs.
//! - We accept exactly one matching request (the first GET to the
//!   registered path) and immediately shut down, so a second hit is
//!   refused. Non-matching paths get a 404 and the listener keeps
//!   waiting. This prevents a browser-prefetcher / curl tab from
//!   accidentally double-firing.
//! - A cancelled flow stops listening at once, so a consent the user
//!   gives afterwards can't be told "You're authorized" for a sign-in Kage
//!   has already abandoned.
//! - The listener has a hard timeout (configured by the caller; we cap
//!   at 10 minutes so a forgotten flow doesn't tie up a port forever).
//! - The capability `oauth` gates the commands. Existing storage /
//!   shell capabilities don't unlock loopback — listening on a port is
//!   a distinct primitive.

use crate::error::{AppError, ErrorKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio::time;

/// 10 minutes — twice the typical OAuth `code` lifetime, well under any
/// sensible "user got distracted" upper bound.
const MAX_TIMEOUT_SECS: u64 = 600;

/// Default timeout when the caller passes 0 / nothing.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// How long a finished listener's outcome waits for an
/// [`oauth_loopback_await`] that hasn't been called yet (a callback can land
/// before the extension gets round to awaiting). After this the entry is
/// dropped so a flow that is never awaited or cancelled doesn't leak.
const UNCLAIMED_OUTCOME_TTL: Duration = Duration::from_secs(60);

/// Upper bound on how long [`oauth_loopback_cancel`] waits for the accept
/// loop to close the port. The loop selects on the cancel signal, so in
/// practice this is a single scheduler hop.
const CANCEL_STOP_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Serialize)]
pub struct OauthLoopbackStart {
    /// Random opaque id the caller passes to [`oauth_loopback_await`].
    pub listener_id: String,
    /// `http://127.0.0.1:<port><path>` — register this verbatim with the
    /// OAuth provider.
    pub redirect_uri: String,
    /// Just the port number, in case the caller wants to log it.
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct OauthLoopbackResult {
    /// Every query parameter from the redirect, surfaced verbatim. Most
    /// flows just look at `code`, `state`, and `error`.
    pub params: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct OauthLoopbackStartArgs {
    /// Path the OAuth server will redirect to, e.g. `/callback`. We
    /// match strictly: anything else returns 404 so a stray browser
    /// hit can't slip through.
    pub redirect_path: String,
    /// 0 / missing → DEFAULT_TIMEOUT_SECS.
    #[serde(default)]
    pub timeout_secs: u64,
    /// Optional human-readable label baked into the success page so the
    /// user knows which extension just authenticated.
    #[serde(default)]
    pub success_label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OauthLoopbackAwaitArgs {
    pub listener_id: String,
}

/// Why a listener stopped without delivering a callback.
#[derive(Debug)]
enum Failure {
    /// `oauth_loopback_cancel` stopped it, or nobody is left to receive the
    /// callback (the waiter went away).
    Cancelled,
    /// The configured timeout elapsed first.
    TimedOut,
    /// `accept()` failed; the listener can't continue.
    Listener(String),
}

impl Failure {
    fn into_error(self) -> AppError {
        match self {
            Failure::Cancelled => cancelled_error(),
            Failure::TimedOut => timed_out_error(),
            Failure::Listener(reason) => AppError::keyed(
                ErrorKind::Internal,
                "errors.oauth.listener_failed",
                &[("reason", &reason)],
            ),
        }
    }
}

/// Message of a `timeout` rejection. Fixed English in every locale: see the
/// module docs. Extensions match on "callback arrived" / "timed out".
const TIMED_OUT_MESSAGE: &str = "OAuth flow timed out before a callback arrived";

/// Message of a `cancelled` rejection. Fixed English in every locale: see
/// the module docs.
const CANCELLED_MESSAGE: &str = "OAuth listener was cancelled before a callback arrived";

fn cancelled_error() -> AppError {
    AppError::raw(ErrorKind::Cancelled, CANCELLED_MESSAGE)
}

fn timed_out_error() -> AppError {
    AppError::raw(ErrorKind::Timeout, TIMED_OUT_MESSAGE)
}

/// What the accept loop reports through the oneshot.
type Outcome = Result<OauthLoopbackResult, Failure>;

/// One registered listener. The entry lives from `start` until the flow is
/// awaited to completion, cancelled, or (never awaited) reaped.
struct PendingListener {
    /// The waiter's end of the outcome channel. [`oauth_loopback_await`]
    /// takes it but leaves the entry in place, so [`oauth_loopback_cancel`]
    /// can still reach the accept loop while the await is in flight.
    rx: Option<oneshot::Receiver<Outcome>>,
    /// Cancel signal for the accept loop. Sending `true` stops it; so does
    /// dropping this sender along with the entry.
    cancel: watch::Sender<bool>,
    /// Flips to `true` (or the sender drops) once the accept loop has
    /// exited and the port is closed.
    stopped: watch::Receiver<bool>,
    timeout: Duration,
}

/// Process-wide registry of in-flight listeners.
type Registry = HashMap<String, PendingListener>;

fn registry() -> MutexGuard<'static, Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    // Every critical section is a plain map operation, so a poisoned lock
    // can't hold a half-updated entry; recover it rather than wedging every
    // later sign-in.
    REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn make_listener_id() -> String {
    // Two random u64s give 128 bits of entropy — plenty for an opaque
    // session id that lives at most a few minutes. `rand::random()`
    // pulls from the thread-local OS-seeded RNG.
    let lo: u64 = rand::random();
    let hi: u64 = rand::random();
    format!("{:016x}{:016x}", hi, lo)
}

/// Open a single-shot HTTP listener and prepare it for [`oauth_loopback_await`].
#[tauri::command]
pub async fn oauth_loopback_start(
    args: OauthLoopbackStartArgs,
) -> Result<OauthLoopbackStart, AppError> {
    if !args.redirect_path.starts_with('/')
        || args.redirect_path.contains(' ')
        || args.redirect_path.contains('\n')
    {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.oauth.invalid_redirect_path",
            &[],
        ));
    }

    let timeout_secs = if args.timeout_secs == 0 {
        DEFAULT_TIMEOUT_SECS
    } else {
        args.timeout_secs.min(MAX_TIMEOUT_SECS)
    };
    let timeout = Duration::from_secs(timeout_secs);

    let bind_failed = |e: std::io::Error| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.oauth.bind_failed",
            &[("reason", &e.to_string())],
        )
    };
    // Bind to an OS-chosen ephemeral port on the loopback address.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(bind_failed)?;
    let port = listener.local_addr().map_err(bind_failed)?.port();

    let listener_id = make_listener_id();
    let redirect_uri = format!("http://127.0.0.1:{}{}", port, args.redirect_path);

    let (mut tx, rx) = oneshot::channel::<Outcome>();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (stopped_tx, stopped_rx) = watch::channel(false);
    let path = args.redirect_path;
    let label = args.success_label;

    registry().insert(
        listener_id.clone(),
        PendingListener {
            rx: Some(rx),
            cancel: cancel_tx,
            stopped: stopped_rx,
            timeout,
        },
    );

    let id = listener_id.clone();
    tokio::spawn(async move {
        let outcome = run_listener(
            listener,
            &path,
            label.as_deref(),
            timeout,
            cancel_rx,
            &mut tx,
        )
        .await;
        // `run_listener` owned the socket, so the port is already closed.
        let _ = stopped_tx.send(true);
        // Fails only if nobody can receive it any more — nothing to do.
        let _ = tx.send(outcome);

        // An outcome nobody has claimed yet (callback or timeout before the
        // extension awaited) waits a while, then the entry goes. A claimed
        // outcome has been delivered by now, and its waiter removed the
        // entry itself; removing again is a no-op.
        time::sleep(UNCLAIMED_OUTCOME_TTL).await;
        registry().remove(&id);
    });

    Ok(OauthLoopbackStart {
        listener_id,
        redirect_uri,
        port,
    })
}

/// Serve the listener until a callback arrives, the timeout elapses, the
/// flow is cancelled, or nobody is left to receive the result. Takes the
/// listener by value so the port is closed by the time this returns.
async fn run_listener(
    listener: TcpListener,
    path: &str,
    label: Option<&str>,
    timeout: Duration,
    mut cancel: watch::Receiver<bool>,
    tx: &mut oneshot::Sender<Outcome>,
) -> Outcome {
    tokio::select! {
        // Cancellation is checked first, so once it's requested a callback
        // that becomes ready in the same poll isn't served as a success.
        biased;
        // `wait_for` errors when the sender is dropped — the registry entry
        // is gone, which is a cancel too.
        _ = cancel.wait_for(|cancelled| *cancelled) => Err(Failure::Cancelled),
        // The receiver is gone (the awaiting command was dropped, or the
        // unclaimed entry was removed): there's no one to hand a callback
        // to, so don't keep a port open to tell the browser it worked.
        _ = tx.closed() => Err(Failure::Cancelled),
        _ = time::sleep(timeout) => Err(Failure::TimedOut),
        outcome = accept_callback(&listener, path, label) => outcome,
    }
}

/// Accept connections until one hits the registered path.
async fn accept_callback(listener: &TcpListener, path: &str, label: Option<&str>) -> Outcome {
    // Loop because the browser may make probe requests (favicon.ico, etc.)
    // before the real callback. We answer non-matching requests with 404
    // and keep waiting.
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| {
            log::warn!("oauth_loopback: accept failed: {}", e);
            Failure::Listener(e.to_string())
        })?;
        if let Some(params) = handle_one_request(stream, path, label).await {
            return Ok(OauthLoopbackResult { params });
        }
    }
}

/// Wait for the matching browser redirect. Resolves with the query
/// parameters, or rejects with a `timeout` / `cancelled` error (see the
/// module docs for the full list).
#[tauri::command]
pub async fn oauth_loopback_await(
    args: OauthLoopbackAwaitArgs,
) -> Result<OauthLoopbackResult, AppError> {
    let (rx, timeout) = {
        let mut reg = registry();
        let entry = reg.get_mut(&args.listener_id).ok_or_else(|| {
            AppError::keyed(ErrorKind::Internal, "errors.oauth.unknown_listener", &[])
        })?;
        let rx = entry.rx.take().ok_or_else(|| {
            AppError::keyed(ErrorKind::Internal, "errors.oauth.already_awaited", &[])
        })?;
        (rx, entry.timeout)
    };

    // The accept loop enforces the timeout and reports it as an outcome;
    // this outer bound only guards against a wedged listener task.
    let waited = time::timeout(timeout + Duration::from_secs(2), rx).await;
    // The id is finished whichever way this went. Dropping the entry also
    // drops the cancel sender, which stops a listener that's still running
    // after the backstop fired.
    registry().remove(&args.listener_id);

    match waited {
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(failure))) => Err(failure.into_error()),
        // The listener task ended without reporting (it panicked).
        Ok(Err(_)) => Err(cancelled_error()),
        Err(_) => Err(timed_out_error()),
    }
}

/// Cancel a listener. Idempotent; safe whether or not an
/// [`oauth_loopback_await`] is in flight. The accept loop selects on the
/// cancel signal, so the port is closed by the time this resolves, and an
/// in-flight await rejects with a `cancelled` error.
#[tauri::command]
pub async fn oauth_loopback_cancel(args: OauthLoopbackAwaitArgs) -> Result<(), AppError> {
    let Some(entry) = registry().remove(&args.listener_id) else {
        return Ok(());
    };
    let PendingListener {
        cancel,
        mut stopped,
        ..
    } = entry;
    let _ = cancel.send(true);
    // `wait_for` also returns (with an error) once the task has dropped its
    // sender, i.e. it's finished either way.
    let _ = time::timeout(CANCEL_STOP_WAIT, stopped.wait_for(|stopped| *stopped)).await;
    Ok(())
}

/// Read one HTTP request, decide whether it matches `expected_path`,
/// and respond with a friendly success/failure page if so. Returns the
/// parsed query params on a match, `None` otherwise (in which case the
/// caller keeps accepting).
async fn handle_one_request(
    mut stream: TcpStream,
    expected_path: &str,
    success_label: Option<&str>,
) -> Option<HashMap<String, String>> {
    // Read up to 8 KiB of the request — more than enough for any
    // sensible OAuth callback URL. Truncating is safe; we only need
    // the request-line.
    let mut buf = vec![0u8; 8 * 1024];
    let n = match time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => n,
        _ => {
            return None;
        }
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");
    // Expected format: `GET /callback?foo=bar HTTP/1.1`
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    if method != "GET" {
        let _ = write_response(
            &mut stream,
            405,
            "Method Not Allowed",
            "text/plain",
            b"Method Not Allowed",
        )
        .await;
        return None;
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };

    if path != expected_path {
        let _ = write_response(&mut stream, 404, "Not Found", "text/plain", b"Not Found").await;
        return None;
    }

    let mut params = HashMap::new();
    if !query.is_empty() {
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            params.insert(k.into_owned(), v.into_owned());
        }
    }

    let success = !params.contains_key("error");
    let title = if success {
        "Authorized"
    } else {
        "Authorization failed"
    };
    // Both the query params and the label (extension-supplied via
    // oauth_loopback_start args) are untrusted — escape anything
    // interpolated into the HTML response.
    let label = html_escape(success_label.unwrap_or("Kage"));
    let body = if success {
        format!(
            "<!doctype html><html><head><meta charset=utf-8><title>{label} - Authorized</title>\
            <style>body{{font-family:-apple-system,Segoe UI,sans-serif;background:#0e0f17;color:#e6e6f0;\
            display:grid;place-items:center;min-height:100vh;margin:0;text-align:center;padding:20px;}}\
            .card{{background:#161826;border:1px solid rgba(255,255,255,.08);border-radius:12px;padding:32px;max-width:420px;}}\
            h1{{margin:0 0 8px;}}p{{color:#9aa0b4;margin:0 0 6px;}}</style></head><body><div class=card>\
            <h1>You're authorized</h1><p>{label} is now connected.</p><p>You can close this tab and return to Kage.</p></div></body></html>"
        )
    } else {
        let err = html_escape(
            params
                .get("error")
                .map(String::as_str)
                .unwrap_or("unknown_error"),
        );
        format!(
            "<!doctype html><html><head><meta charset=utf-8><title>{label} - Failed</title>\
            <style>body{{font-family:-apple-system,Segoe UI,sans-serif;background:#0e0f17;color:#e6e6f0;\
            display:grid;place-items:center;min-height:100vh;margin:0;text-align:center;padding:20px;}}\
            .card{{background:#161826;border:1px solid rgba(255,135,135,.4);border-radius:12px;padding:32px;max-width:420px;}}\
            h1{{margin:0 0 8px;color:#ff8e8e;}}p{{color:#9aa0b4;margin:0;}}</style></head><body><div class=card>\
            <h1>Authorization failed</h1><p>{err}</p><p>Close this tab and retry from {label}'s settings.</p></div></body></html>"
        )
    };

    let _ = write_response(
        &mut stream,
        200,
        title,
        "text/html; charset=utf-8",
        body.as_bytes(),
    )
    .await;
    Some(params)
}

/// Minimal HTML escaper for the loopback response pages. Covers body text
/// and quoted attribute contexts.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status, reason, content_type, body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn start(path: &str, timeout_secs: u64) -> OauthLoopbackStart {
        oauth_loopback_start(OauthLoopbackStartArgs {
            redirect_path: path.to_string(),
            timeout_secs,
            success_label: Some("Test".to_string()),
        })
        .await
        .expect("loopback should start")
    }

    fn id_args(id: &str) -> OauthLoopbackAwaitArgs {
        OauthLoopbackAwaitArgs {
            listener_id: id.to_string(),
        }
    }

    /// Play the browser: send `GET <target>` and return the raw response,
    /// or "" when the connection is refused or closed without one.
    async fn send_callback(port: u16, target: &str) -> String {
        let connect = TcpStream::connect(("127.0.0.1", port));
        let Ok(Ok(mut stream)) = time::timeout(Duration::from_secs(2), connect).await else {
            return String::new();
        };
        let request = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        if stream.write_all(request.as_bytes()).await.is_err() {
            return String::new();
        }
        let mut buf = Vec::new();
        let _ = time::timeout(Duration::from_secs(2), stream.read_to_end(&mut buf)).await;
        String::from_utf8_lossy(&buf).into_owned()
    }

    type Waiter = tokio::task::JoinHandle<Result<OauthLoopbackResult, AppError>>;

    /// Spawn an await and return once it has claimed the listener's
    /// receiver, i.e. the await is genuinely in flight.
    async fn spawn_waiter(id: &str) -> Waiter {
        let waiter = tokio::spawn(oauth_loopback_await(id_args(id)));
        for _ in 0..200 {
            if registry().get(id).is_some_and(|e| e.rx.is_none()) {
                return waiter;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the await never claimed the listener");
    }

    async fn join_err(waiter: Waiter) -> AppError {
        time::timeout(Duration::from_secs(3), waiter)
            .await
            .expect("the waiter should wake promptly")
            .expect("the waiter task should not panic")
            .expect_err("the await should reject")
    }

    #[tokio::test]
    async fn cancel_while_awaiting_wakes_the_waiter_with_cancelled() {
        let started = start("/t/cancel-in-flight", 30).await;
        let id = started.listener_id.clone();
        let waiter = spawn_waiter(&id).await;

        oauth_loopback_cancel(id_args(&id))
            .await
            .expect("cancel ok");

        let err = join_err(waiter).await;
        assert!(matches!(err.kind, ErrorKind::Cancelled), "{:?}", err.kind);
        assert_eq!(err.to_string(), CANCELLED_MESSAGE);
        assert!(!registry().contains_key(&id), "the entry should be gone");
    }

    #[tokio::test]
    async fn late_callback_after_cancel_is_not_authorized() {
        let started = start("/t/late", 30).await;
        let id = started.listener_id.clone();
        let waiter = spawn_waiter(&id).await;

        oauth_loopback_cancel(id_args(&id))
            .await
            .expect("cancel ok");

        // The user approves in the browser after Kage gave up: the port is
        // already closed, so the redirect is refused rather than answered
        // with "You're authorized".
        let response = send_callback(started.port, "/t/late?code=abc&state=s").await;
        assert!(
            !response.starts_with("HTTP/1.1 200"),
            "late callback was served: {response:?}"
        );
        assert!(!response.contains("authorized"), "{response:?}");

        let err = join_err(waiter).await;
        assert!(matches!(err.kind, ErrorKind::Cancelled), "{:?}", err.kind);
    }

    #[tokio::test]
    async fn cancel_before_await_stops_the_listener() {
        // No await yet, so the registry still holds the receiver. Cancel
        // must still shut the accept loop down.
        let started = start("/t/no-waiter", 30).await;
        let id = started.listener_id.clone();

        oauth_loopback_cancel(id_args(&id))
            .await
            .expect("cancel ok");

        let response = send_callback(started.port, "/t/no-waiter?code=abc").await;
        assert!(
            !response.starts_with("HTTP/1.1 200"),
            "callback after cancel was served: {response:?}"
        );
        let err = oauth_loopback_await(id_args(&id))
            .await
            .expect_err("await after cancel should reject");
        assert_eq!(err.key, "errors.oauth.unknown_listener");
        // Cancel is idempotent.
        oauth_loopback_cancel(id_args(&id))
            .await
            .expect("second cancel ok");
    }

    #[tokio::test]
    async fn timeout_is_reported_as_timeout_not_cancelled() {
        let started = start("/t/timeout", 1).await;
        let id = started.listener_id.clone();

        let err = time::timeout(Duration::from_secs(5), oauth_loopback_await(id_args(&id)))
            .await
            .expect("the await should resolve after the 1s timeout")
            .expect_err("the await should time out");
        assert!(matches!(err.kind, ErrorKind::Timeout), "{:?}", err.kind);
        assert_eq!(err.to_string(), TIMED_OUT_MESSAGE);

        // The port was released with the timeout.
        let response = send_callback(started.port, "/t/timeout?code=abc").await;
        assert!(!response.starts_with("HTTP/1.1 200"), "{response:?}");
    }

    #[tokio::test]
    async fn timeout_before_await_is_kept_for_the_waiter() {
        // The extension may only call await after the listener has already
        // given up; it should still learn *why*, not get "unknown id".
        let started = start("/t/timeout-unclaimed", 1).await;
        time::sleep(Duration::from_millis(1500)).await;
        let err = oauth_loopback_await(id_args(&started.listener_id))
            .await
            .expect_err("the await should report the timeout");
        assert!(matches!(err.kind, ErrorKind::Timeout), "{:?}", err.kind);
    }

    #[tokio::test]
    async fn a_second_concurrent_await_is_rejected() {
        let started = start("/t/double-await", 30).await;
        let id = started.listener_id.clone();
        let waiter = spawn_waiter(&id).await;

        let err = oauth_loopback_await(id_args(&id))
            .await
            .expect_err("a second await should reject");
        assert_eq!(err.key, "errors.oauth.already_awaited");

        // The first await is unaffected and still cancellable.
        oauth_loopback_cancel(id_args(&id))
            .await
            .expect("cancel ok");
        assert!(matches!(join_err(waiter).await.kind, ErrorKind::Cancelled));
    }

    #[tokio::test]
    async fn callback_while_awaiting_resolves_with_params() {
        let started = start("/t/ok", 30).await;
        let id = started.listener_id.clone();
        let waiter = spawn_waiter(&id).await;

        let response = send_callback(started.port, "/t/ok?code=abc&state=xyz").await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response:?}");

        let result = time::timeout(Duration::from_secs(3), waiter)
            .await
            .expect("resolves")
            .expect("no panic")
            .expect("callback params");
        assert_eq!(result.params.get("code").map(String::as_str), Some("abc"));
        assert_eq!(result.params.get("state").map(String::as_str), Some("xyz"));
        assert!(!registry().contains_key(&id));
    }

    #[tokio::test]
    async fn invalid_redirect_path_is_a_keyed_error() {
        let err = oauth_loopback_start(OauthLoopbackStartArgs {
            redirect_path: "no-slash".to_string(),
            timeout_secs: 5,
            success_label: None,
        })
        .await
        .expect_err("should reject");
        assert_eq!(err.key, "errors.oauth.invalid_redirect_path");
    }

    #[test]
    fn timeout_and_cancel_messages_stay_english_in_every_locale() {
        // Extensions written before errors carried a kind tell a timeout
        // apart by matching "…before a callback arrived" / "timed out" in
        // the message, so it must not be translated. Rendered per locale
        // through the catalog, the way serialisation does it.
        crate::i18n::init(Some("en"));
        for (err, text) in [
            (cancelled_error(), CANCELLED_MESSAGE),
            (timed_out_error(), TIMED_OUT_MESSAGE),
        ] {
            let params: Vec<(&str, &str)> = err
                .params
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            for lang in ["en", "de", "ja", "ar"] {
                assert_eq!(
                    crate::i18n::translate_in(lang, &err.key, &params),
                    text,
                    "{lang}"
                );
            }
        }
        assert!(CANCELLED_MESSAGE.contains("callback arrived"));
        assert!(TIMED_OUT_MESSAGE.contains("timed out"));
    }
}
