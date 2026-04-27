//! Lightweight in-process mock ACP TCP server for integration tests.
//!
//! Spins up a TCP listener on an ephemeral port, accepts exactly one
//! connection, and runs a programmable request handler in a background
//! thread. Handlers can:
//!   - Reply to requests by JSON id.
//!   - Push notifications independently of any incoming request.
//!   - Signal test code via the recorded request log.
//!
//! The server shuts down cleanly on drop: the listener is closed, the
//! handler thread sees a read-EOF, and the test continues.

#![allow(dead_code)] // Used across multiple integration test crates

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Canned behavior for a mock server: answer each `method` with a
/// result value, or send an error, or respond to any method matching
/// a predicate. Tests build one of these, install it, and drive a
/// real AcpClient against the listener's address.
pub struct MockResponder {
    /// For each recognized method, either a JSON result or an error.
    pub replies: Vec<(&'static str, Reply)>,
}

pub enum Reply {
    Ok(Value),
    Err { code: i32, message: &'static str },
    /// Never respond — used to test timeouts.
    Drop,
}

pub struct MockAcpServer {
    /// Port the listener bound to. Pass this to AcpClient.
    pub port: u16,
    /// Every request line the server saw, in arrival order.
    pub requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    notifier: Arc<Mutex<Option<std::sync::mpsc::Sender<String>>>>,
}

impl MockAcpServer {
    /// Bind to an ephemeral port on 127.0.0.1 and start accepting one
    /// connection. Returns immediately; the connection handler runs
    /// on a background thread.
    pub fn start(responder: MockResponder) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        listener.set_nonblocking(false).unwrap();
        let port = listener.local_addr().unwrap().port();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (notif_tx, notif_rx) = std::sync::mpsc::channel::<String>();
        let notifier = Arc::new(Mutex::new(Some(notif_tx)));

        let requests_clone = requests.clone();
        let stop_clone = stop.clone();
        let handle = thread::spawn(move || {
            // Accept with a short timeout so we can honor `stop`.
            listener.set_nonblocking(true).unwrap();
            let stream = loop {
                if stop_clone.load(Ordering::SeqCst) {
                    return;
                }
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(e) => {
                        eprintln!("mock-acp: accept failed: {}", e);
                        return;
                    }
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
            handle_connection(stream, responder, requests_clone, stop_clone, notif_rx);
        });

        MockAcpServer { port, requests, stop, handle: Some(handle), notifier }
    }

    /// Send a notification (no id) to the connected client at any time.
    pub fn push_notification(&self, notif: Value) {
        let line = serde_json::to_string(&notif).expect("serialize notification");
        if let Some(tx) = self.notifier.lock().unwrap().as_ref() {
            let _ = tx.send(line);
        }
    }

    /// Wait up to `timeout` for at least `n` requests to have arrived.
    /// Returns the snapshot of requests seen; test should assert on it.
    pub fn wait_for_requests(&self, n: usize, timeout: Duration) -> Vec<Value> {
        let start = Instant::now();
        loop {
            let snap = self.requests.lock().unwrap().clone();
            if snap.len() >= n {
                return snap;
            }
            if start.elapsed() >= timeout {
                return snap;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for MockAcpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Drop notifier so the send loop in handle_connection exits.
        *self.notifier.lock().unwrap() = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    responder: MockResponder,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    notif_rx: std::sync::mpsc::Receiver<String>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }

        // Drain any pending notifications we've been asked to push.
        while let Ok(line) = notif_rx.try_recv() {
            if writeln!(stream, "{}", line).is_err() {
                return;
            }
            let _ = stream.flush();
        }

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return, // EOF — client disconnected
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => return,
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        requests.lock().unwrap().push(request.clone());

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let reply = responder.replies.iter().find(|(m, _)| *m == method);
        match reply {
            Some((_, Reply::Ok(val))) => {
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": val,
                });
                if writeln!(stream, "{}", response).is_err() {
                    return;
                }
                let _ = stream.flush();
            }
            Some((_, Reply::Err { code, message })) => {
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": code, "message": message }
                });
                if writeln!(stream, "{}", response).is_err() {
                    return;
                }
                let _ = stream.flush();
            }
            Some((_, Reply::Drop)) | None => {
                // Silently drop — client will hit its own timeout.
            }
        }
    }
}
