//! Requests waiting for the user's answer. Each prompt answers only its own
//! request, and only from the window it was shown in.

use crate::lock_ext::LockExt;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex};

/// Prompts waiting at once, at most; more are refused at once.
pub const MAX_PENDING: usize = 8;

/// The user's answer to one prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    RunOnce,
    /// Run, and from now on run commands starting with this prefix
    /// (already validated and saved).
    Always(String),
    Deny,
    /// The window showing it closed: nobody can answer it any more.
    WindowClosed,
    /// Kage is quitting.
    Shutdown,
}

/// What a window may know about a pending request before answering it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInfo {
    pub command: String,
    /// "Always allow …" may be offered (and accepted) for it.
    pub can_always: bool,
    /// It has no hidden characters, so it may run from Kage at all.
    pub plain: bool,
}

struct Pending {
    window: String,
    info: PendingInfo,
    payload: serde_json::Value,
    tx: mpsc::SyncSender<Answer>,
}

/// Why an answer was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerError {
    /// Answered, withdrawn or timed out already (or never existed).
    NotPending,
    /// Shown in another window.
    WrongWindow,
}

#[derive(Default)]
pub struct Approvals {
    pending: Mutex<HashMap<String, Pending>>,
}

impl Approvals {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait for an answer to request `id`, shown in `window` with
    /// `payload`. `None` when [`MAX_PENDING`] are already waiting.
    pub fn register(
        &self,
        id: &str,
        window: &str,
        info: PendingInfo,
        payload: serde_json::Value,
    ) -> Option<mpsc::Receiver<Answer>> {
        let mut pending = self.pending.lock_or_recover();
        if pending.len() >= MAX_PENDING || pending.contains_key(id) {
            return None;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        pending.insert(
            id.to_string(),
            Pending {
                window: window.to_string(),
                info,
                payload,
                tx,
            },
        );
        Some(rx)
    }

    /// What `window` needs to check an answer to `id`.
    pub fn info_for(&self, id: &str, window: &str) -> Result<PendingInfo, AnswerError> {
        let pending = self.pending.lock_or_recover();
        let p = pending.get(id).ok_or(AnswerError::NotPending)?;
        if p.window != window {
            return Err(AnswerError::WrongWindow);
        }
        Ok(p.info.clone())
    }

    /// Answer request `id` from `window`: once, and only from the window
    /// it was shown in.
    pub fn answer(&self, id: &str, window: &str, answer: Answer) -> Result<(), AnswerError> {
        let p = {
            let mut pending = self.pending.lock_or_recover();
            match pending.get(id) {
                None => return Err(AnswerError::NotPending),
                Some(p) if p.window != window => return Err(AnswerError::WrongWindow),
                Some(_) => {}
            }
            pending.remove(id).ok_or(AnswerError::NotPending)?
        };
        p.tx.send(answer).map_err(|_| AnswerError::NotPending)
    }

    /// Stop waiting for `id` (timed out, or the agent went away). Returns
    /// whether it was still waiting.
    pub fn withdraw(&self, id: &str) -> bool {
        self.pending.lock_or_recover().remove(id).is_some()
    }

    /// The prompts waiting in `window`, oldest first by id, for a window
    /// that reloaded to show again.
    pub fn payloads_for(&self, window: &str) -> Vec<serde_json::Value> {
        let pending = self.pending.lock_or_recover();
        let mut items: Vec<(&String, &Pending)> =
            pending.iter().filter(|(_, p)| p.window == window).collect();
        items.sort_by(|a, b| a.0.cmp(b.0));
        items.into_iter().map(|(_, p)| p.payload.clone()).collect()
    }

    /// `window` closed: its waiting requests get [`Answer::WindowClosed`].
    pub fn close_window(&self, window: &str) {
        let closed: Vec<Pending> = {
            let mut pending = self.pending.lock_or_recover();
            let ids: Vec<String> = pending
                .iter()
                .filter(|(_, p)| p.window == window)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| pending.remove(id)).collect()
        };
        for p in closed {
            let _ = p.tx.send(Answer::WindowClosed);
        }
    }

    /// Kage is quitting: every waiting request gets [`Answer::Shutdown`].
    pub fn shutdown_all(&self) {
        let drained: Vec<Pending> = self
            .pending
            .lock_or_recover()
            .drain()
            .map(|(_, p)| p)
            .collect();
        for p in drained {
            let _ = p.tx.send(Answer::Shutdown);
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.pending.lock_or_recover().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(cmd: &str) -> PendingInfo {
        PendingInfo {
            command: cmd.into(),
            can_always: true,
            plain: true,
        }
    }

    #[test]
    fn each_prompt_answers_only_its_own_request_once() {
        let a = Approvals::new();
        let rx1 = a
            .register("r1", "main", info("one"), serde_json::json!(1))
            .unwrap();
        let rx2 = a
            .register("r2", "main", info("two"), serde_json::json!(2))
            .unwrap();
        a.answer("r1", "main", Answer::Deny).unwrap();
        assert_eq!(rx1.try_recv(), Ok(Answer::Deny));
        assert!(rx2.try_recv().is_err(), "r2 is still waiting");
        assert_eq!(
            a.answer("r1", "main", Answer::RunOnce),
            Err(AnswerError::NotPending)
        );
        a.answer("r2", "main", Answer::RunOnce).unwrap();
        assert_eq!(rx2.try_recv(), Ok(Answer::RunOnce));
    }

    #[test]
    fn only_the_window_it_was_shown_in_may_answer() {
        let a = Approvals::new();
        let rx = a
            .register("r1", "chat-1", info("x"), serde_json::json!({}))
            .unwrap();
        assert_eq!(
            a.answer("r1", "floating", Answer::RunOnce),
            Err(AnswerError::WrongWindow)
        );
        assert_eq!(a.info_for("r1", "floating"), Err(AnswerError::WrongWindow));
        assert!(rx.try_recv().is_err());
        assert_eq!(a.info_for("r1", "chat-1").unwrap().command, "x");
        a.answer("r1", "chat-1", Answer::RunOnce).unwrap();
        assert_eq!(rx.try_recv(), Ok(Answer::RunOnce));
    }

    #[test]
    fn a_withdrawn_request_cannot_be_answered() {
        let a = Approvals::new();
        let _rx = a
            .register("r1", "main", info("x"), serde_json::json!({}))
            .unwrap();
        assert!(a.withdraw("r1"));
        assert!(!a.withdraw("r1"));
        assert_eq!(
            a.answer("r1", "main", Answer::RunOnce),
            Err(AnswerError::NotPending)
        );
    }

    #[test]
    fn the_queue_is_bounded() {
        let a = Approvals::new();
        let mut keep = Vec::new();
        for i in 0..MAX_PENDING {
            keep.push(
                a.register(&format!("r{i}"), "main", info("x"), serde_json::json!(i))
                    .unwrap(),
            );
        }
        assert!(a
            .register("one-more", "main", info("x"), serde_json::json!(0))
            .is_none());
        assert_eq!(a.payloads_for("main").len(), MAX_PENDING);
        assert!(a.payloads_for("floating").is_empty());
    }

    #[test]
    fn a_closed_window_ends_only_its_own_prompts() {
        let a = Approvals::new();
        let rx_chat = a
            .register("r1", "chat-1", info("x"), serde_json::json!({}))
            .unwrap();
        let rx_float = a
            .register("r2", "floating", info("y"), serde_json::json!({}))
            .unwrap();
        a.close_window("chat-1");
        assert_eq!(rx_chat.try_recv(), Ok(Answer::WindowClosed));
        assert!(rx_float.try_recv().is_err());
        assert_eq!(a.payloads_for("floating").len(), 1);
    }

    #[test]
    fn shutdown_answers_everyone() {
        let a = Approvals::new();
        let rx = a
            .register("r1", "main", info("x"), serde_json::json!({}))
            .unwrap();
        a.shutdown_all();
        assert_eq!(rx.try_recv(), Ok(Answer::Shutdown));
        assert!(a.is_empty());
    }
}
