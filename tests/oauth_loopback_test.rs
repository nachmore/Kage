// Integration test for the OAuth loopback listener.
//
// We can't call the `#[tauri::command]` wrappers directly without a
// running Tauri runtime, so the test goes through the same surface
// extensions use: bind a listener via the public start fn (which the
// command thinly wraps), simulate the browser hitting the redirect by
// firing a plain TCP request to the captured port, and verify the
// await call returns the parsed query params.

use kage::commands::oauth::{
    oauth_loopback_await, oauth_loopback_cancel, oauth_loopback_start, OauthLoopbackAwaitArgs,
    OauthLoopbackStartArgs,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn loopback_receives_callback_and_returns_params() {
    let started = oauth_loopback_start(OauthLoopbackStartArgs {
        redirect_path: "/test/cb".to_string(),
        timeout_secs: 10,
        success_label: Some("Test".to_string()),
    })
    .await
    .expect("loopback should start");

    assert!(started.redirect_uri.starts_with("http://127.0.0.1:"));
    assert!(started.redirect_uri.ends_with("/test/cb"));
    assert_eq!(started.listener_id.len(), 32);

    // Simulate the browser hitting the redirect with code + state.
    let listener_addr = format!("127.0.0.1:{}", started.port);
    let mut stream = TcpStream::connect(&listener_addr)
        .await
        .expect("should connect to loopback");
    let req = "GET /test/cb?code=abc123&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
    stream
        .write_all(req.as_bytes())
        .await
        .expect("write request");
    // Read the response so the server-side write completes before we close.
    let mut buf = vec![0u8; 4096];
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;

    let result = oauth_loopback_await(OauthLoopbackAwaitArgs {
        listener_id: started.listener_id,
    })
    .await
    .expect("await should resolve");

    assert_eq!(result.params.get("code"), Some(&"abc123".to_string()));
    assert_eq!(result.params.get("state"), Some(&"xyz".to_string()));
}

#[tokio::test]
async fn loopback_404s_non_matching_paths_then_succeeds_on_match() {
    let started = oauth_loopback_start(OauthLoopbackStartArgs {
        redirect_path: "/spotify/cb".to_string(),
        timeout_secs: 10,
        success_label: None,
    })
    .await
    .expect("loopback should start");

    let listener_addr = format!("127.0.0.1:{}", started.port);

    // First a probe request the browser might make. Should 404 and the
    // listener stays alive.
    {
        let mut stream = TcpStream::connect(&listener_addr).await.unwrap();
        stream
            .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256];
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap_or(0);
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.starts_with("HTTP/1.1 404"),
            "expected 404, got: {}",
            response.lines().next().unwrap_or("")
        );
    }

    // Now the real callback.
    {
        let mut stream = TcpStream::connect(&listener_addr).await.unwrap();
        stream
            .write_all(b"GET /spotify/cb?code=ok HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let _ = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
    }

    let result = oauth_loopback_await(OauthLoopbackAwaitArgs {
        listener_id: started.listener_id,
    })
    .await
    .expect("await should resolve");
    assert_eq!(result.params.get("code"), Some(&"ok".to_string()));
}

#[tokio::test]
async fn loopback_await_after_cancel_returns_error() {
    let started = oauth_loopback_start(OauthLoopbackStartArgs {
        redirect_path: "/x".to_string(),
        timeout_secs: 10,
        success_label: None,
    })
    .await
    .expect("loopback should start");

    let listener_id = started.listener_id.clone();
    oauth_loopback_cancel(OauthLoopbackAwaitArgs {
        listener_id: listener_id.clone(),
    })
    .await
    .expect("cancel ok");

    let result = oauth_loopback_await(OauthLoopbackAwaitArgs { listener_id }).await;
    assert!(result.is_err(), "await should fail after cancel");
}

#[tokio::test]
async fn loopback_rejects_bad_path() {
    let result = oauth_loopback_start(OauthLoopbackStartArgs {
        redirect_path: "no-leading-slash".to_string(),
        timeout_secs: 10,
        success_label: None,
    })
    .await;
    assert!(result.is_err());
}
