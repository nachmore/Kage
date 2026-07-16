//! Batched delivery of streaming agent chunks to the UI.
//!
//! Pre-2026-05 the notification handler emitted one Tauri `message_chunk`
//! event per `agent_message_chunk` notification it received. With token-
//! level streaming that's hundreds-to-thousands of IPC roundtrips per
//! response, each costing a JSON serialize + IPC bridge crossing + frontend
//! handler invocation. WebView2's emit path also has no backpressure
//! signal — bursts pile up in Tauri's internal queue and the renderer
//! falls behind without anyone knowing.
//!
//! The notification handler now appends each chunk's delta into a per-
//! session `Mutex<HashMap<String, String>>`. A dedicated background thread
//! (spawned in `commands::messaging`) wakes every ~16ms, drains the map
//! atomically, and emits one `message_chunk` event per non-empty session
//! bucket. Visually identical streaming, two orders of magnitude fewer
//! IPC crossings.
//!
//! This module owns the pure drain-and-emit step — extracted from the
//! flush thread so the locking discipline is verifiable without spinning
//! up a Tauri AppHandle.

use std::collections::HashMap;
use std::sync::Mutex;

/// Ceiling on the total bytes parked in `pending` while emission is
/// failing. Under sustained emit failure the map is fed by every
/// `agent_message_chunk` and drained by nobody; without a cap it grows
/// for as long as the agent keeps streaming. 8 MiB is far beyond any
/// real response, so hitting it means emission has been broken for a
/// long time and the text is undeliverable anyway.
pub const MAX_PENDING_BYTES: usize = 8 * 1024 * 1024;

/// Drain the pending-chunks map under a brief critical section and call
/// `emit` once per non-empty session bucket. Returns `false` if any
/// `emit` call fails.
///
/// On emit failure the failed bucket and all not-yet-attempted buckets
/// are re-queued (prepended, so chunk order within a session is
/// preserved against deltas that arrived meanwhile) rather than dropped —
/// an emit `Err` can be transient (e.g. a `chat-<uuid>` webview torn
/// down mid-dispatch), and the flush thread retries next cycle. If the
/// re-queued total exceeds [`MAX_PENDING_BYTES`], the map is cleared to
/// bound memory; the text was undeliverable for that long anyway.
///
/// The drain uses `std::mem::take` so the lock is held only long enough
/// to swap the HashMap; emits happen outside the lock, meaning the
/// notification handler is never blocked on Tauri's IPC bridge.
pub fn drain_and_emit_pending<F>(pending: &Mutex<HashMap<String, String>>, mut emit: F) -> bool
where
    F: FnMut(&str, &str) -> Result<(), String>,
{
    let snapshot: HashMap<String, String> = {
        let mut guard = match pending.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_empty() {
            return true;
        }
        std::mem::take(&mut *guard)
    };

    let mut iter = snapshot.into_iter();
    for (session_id, text) in iter.by_ref() {
        if text.is_empty() {
            continue;
        }
        if emit(&session_id, &text).is_err() {
            // Re-queue this bucket and the rest so a transient emit
            // failure doesn't lose streamed text.
            let mut guard = match pending.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let mut requeued = text.len();
            let cur = guard.entry(session_id).or_default();
            cur.insert_str(0, &text);
            for (sid, t) in iter {
                requeued += t.len();
                let cur = guard.entry(sid).or_default();
                cur.insert_str(0, &t);
            }
            let total: usize = guard.values().map(|v| v.len()).sum();
            if total.max(requeued) > MAX_PENDING_BYTES {
                guard.clear();
            }
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Helper: build a fresh `Mutex<HashMap>` with the given entries.
    fn make_pending(entries: &[(&str, &str)]) -> Mutex<HashMap<String, String>> {
        let mut m = HashMap::new();
        for (k, v) in entries {
            m.insert((*k).to_string(), (*v).to_string());
        }
        Mutex::new(m)
    }

    #[test]
    fn drain_emits_one_event_per_session_bucket() {
        let pending = make_pending(&[("session-a", "hello"), ("session-b", "world")]);

        let emitted: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = emitted.clone();
        let alive = drain_and_emit_pending(&pending, move |sid, text| {
            captured
                .lock()
                .unwrap()
                .push((sid.to_string(), text.to_string()));
            Ok(())
        });

        assert!(alive);
        let mut events = emitted.lock().unwrap().clone();
        events.sort();
        assert_eq!(
            events,
            vec![
                ("session-a".to_string(), "hello".to_string()),
                ("session-b".to_string(), "world".to_string()),
            ]
        );
        // Map drained to empty so the next cycle starts fresh.
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn drain_skips_empty_buckets_and_returns_true_on_empty_map() {
        let pending = make_pending(&[]);
        let calls = Arc::new(Mutex::new(0));
        let counter = calls.clone();
        let alive = drain_and_emit_pending(&pending, move |_, _| {
            *counter.lock().unwrap() += 1;
            Ok(())
        });
        assert!(alive);
        assert_eq!(*calls.lock().unwrap(), 0, "empty map → no emit calls");
    }

    #[test]
    fn drain_returns_false_when_emit_fails() {
        let pending = make_pending(&[("s", "data")]);
        let alive = drain_and_emit_pending(&pending, |_, _| Err("boom".to_string()));
        assert!(!alive);
    }

    #[test]
    fn failed_emit_requeues_text_for_retry() {
        // A transient emit failure (webview torn down mid-dispatch) must
        // not lose streamed text — the bucket goes back into the map so
        // the next cycle retries it.
        let pending = make_pending(&[("s", "data")]);
        let alive = drain_and_emit_pending(&pending, |_, _| Err("transient".to_string()));
        assert!(!alive);
        assert_eq!(pending.lock().unwrap().get("s").unwrap(), "data");
    }

    #[test]
    fn failed_emit_prepends_before_newly_arrived_chunks() {
        // Chunks appended between the drain and the re-queue must come
        // AFTER the failed text so per-session ordering is preserved.
        let pending = make_pending(&[("s", "first ")]);
        let alive = drain_and_emit_pending(&pending, |_, _| {
            // Simulate the notification handler racing in a new delta
            // while the emit is failing.
            pending.lock().unwrap().insert("s".into(), "second".into());
            Err("transient".to_string())
        });
        assert!(!alive);
        assert_eq!(pending.lock().unwrap().get("s").unwrap(), "first second");
    }

    #[test]
    fn requeue_clears_map_beyond_byte_cap() {
        let big = "x".repeat(MAX_PENDING_BYTES + 1);
        let pending = make_pending(&[("s", &big)]);
        let alive = drain_and_emit_pending(&pending, |_, _| Err("down".to_string()));
        assert!(!alive);
        assert!(
            pending.lock().unwrap().is_empty(),
            "oversized undeliverable backlog must be dropped"
        );
    }

    #[test]
    fn appends_into_a_single_session_concatenate_in_one_emit() {
        // Models the notification handler appending several chunks for
        // the same session before the flush thread runs: the map only
        // holds one bucket per session, so the drain emits once with
        // the concatenated text.
        let pending = Mutex::new(HashMap::<String, String>::new());
        {
            let mut guard = pending.lock().unwrap();
            guard
                .entry("s".to_string())
                .or_default()
                .push_str("Hello, ");
            guard.entry("s".to_string()).or_default().push_str("world!");
        }

        let emitted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = emitted.clone();
        drain_and_emit_pending(&pending, move |_sid, text| {
            captured.lock().unwrap().push(text.to_string());
            Ok(())
        });

        let events = emitted.lock().unwrap().clone();
        assert_eq!(events, vec!["Hello, world!".to_string()]);
    }
}
