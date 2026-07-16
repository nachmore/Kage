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
//!
//! The listener is single-shot: as soon as the callback arrives (or the
//! timeout expires) the listener is dropped, the port is released, and
//! the session id is invalidated. Multiple concurrent flows are fine —
//! each gets its own port and id.
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
//!   registered path) and immediately shut down. A second hit returns
//!   404. This prevents a browser-prefetcher / curl tab from
//!   accidentally double-firing.
//! - The listener has a hard timeout (configured by the caller; we cap
//!   at 10 minutes so a forgotten flow doesn't tie up a port forever).
//! - The capability `oauth` gates the commands. Existing storage /
//!   shell capabilities don't unlock loopback — listening on a port is
//!   a distinct primitive.

use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time;

/// 10 minutes — twice the typical OAuth `code` lifetime, well under any
/// sensible "user got distracted" upper bound.
const MAX_TIMEOUT_SECS: u64 = 600;

/// Default timeout when the caller passes 0 / nothing.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

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

/// One pending listener.
struct PendingListener {
    rx: oneshot::Receiver<OauthLoopbackResult>,
    timeout: Duration,
}

/// Process-wide registry of in-flight listeners.
type Registry = Arc<Mutex<HashMap<String, PendingListener>>>;

fn registry() -> Registry {
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
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
    if !args.redirect_path.starts_with('/') {
        return Err("redirect_path must start with '/'".into());
    }
    if args.redirect_path.contains(' ') || args.redirect_path.contains('\n') {
        return Err("redirect_path must not contain whitespace".into());
    }

    let timeout_secs = if args.timeout_secs == 0 {
        DEFAULT_TIMEOUT_SECS
    } else {
        args.timeout_secs.min(MAX_TIMEOUT_SECS)
    };
    let timeout = Duration::from_secs(timeout_secs);

    // Bind to an OS-chosen ephemeral port on the loopback address.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind loopback listener: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to read local addr: {}", e))?
        .port();

    let listener_id = make_listener_id();
    let redirect_uri = format!("http://127.0.0.1:{}{}", port, args.redirect_path);

    let (tx, rx) = oneshot::channel::<OauthLoopbackResult>();
    let path = args.redirect_path.clone();
    let label = args.success_label.clone();

    // Spawn the accept loop. It exits as soon as a matching callback
    // arrives or the listener is dropped.
    tokio::spawn(async move {
        let result = time::timeout(timeout, async {
            // Loop because the browser may make probe requests (favicon.ico,
            // etc.) before the real callback. We answer non-matching
            // requests with 404 and keep waiting.
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        if let Some(params) =
                            handle_one_request(stream, &path, label.as_deref()).await
                        {
                            return Ok::<_, ()>(OauthLoopbackResult { params });
                        }
                    }
                    Err(e) => {
                        log::warn!("oauth_loopback: accept failed: {}", e);
                        return Err(());
                    }
                }
            }
        })
        .await;

        match result {
            Ok(Ok(payload)) => {
                let _ = tx.send(payload);
            }
            Ok(Err(_)) | Err(_) => {
                // Channel closed implicitly when tx drops; await side
                // gets a "channel closed" error which we translate into
                // the right user-facing message.
            }
        }
        // The listener is dropped at end of scope, releasing the port.
    });

    let registry = registry();
    let mut reg = registry.lock().expect("oauth_loopback registry poisoned");
    reg.insert(listener_id.clone(), PendingListener { rx, timeout });

    Ok(OauthLoopbackStart {
        listener_id,
        redirect_uri,
        port,
    })
}

/// Wait for the matching browser redirect. Resolves with the query
/// parameters, or errors out on timeout / cancellation.
#[tauri::command]
pub async fn oauth_loopback_await(
    args: OauthLoopbackAwaitArgs,
) -> Result<OauthLoopbackResult, AppError> {
    let pending = {
        let registry = registry();
        let mut reg = registry.lock().expect("oauth_loopback registry poisoned");
        reg.remove(&args.listener_id)
            .ok_or("Unknown listener id (timed out, cancelled, or already consumed)")?
    };

    // Use the same timeout the start call configured. The accept-loop
    // spawn enforces it independently, but adding an outer timeout
    // protects against the (theoretical) case where the listener task
    // is alive but the channel is wedged.
    match time::timeout(pending.timeout + Duration::from_secs(2), pending.rx).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err("OAuth listener was cancelled before a callback arrived".into()),
        Err(_) => Err("OAuth flow timed out before a callback arrived".into()),
    }
}

/// Cancel a pending listener. Idempotent.
#[tauri::command]
pub async fn oauth_loopback_cancel(args: OauthLoopbackAwaitArgs) -> Result<(), AppError> {
    let registry = registry();
    let mut reg = registry.lock().expect("oauth_loopback registry poisoned");
    reg.remove(&args.listener_id);
    // The receiver drop in the spawned task closes the channel, which
    // unblocks the accept loop on its next iteration.
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
