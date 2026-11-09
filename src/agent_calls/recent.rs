//! The last few prompts and replies of this session, in memory only, for
//! "Show what was sent" (docs/design/EXTENSION_AGENT_CALLS.md §4.2, §5.13).
//! Never written to disk, never logged.

use serde::Serialize;
use std::collections::VecDeque;

/// One call's exact prompt and reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestDetail {
    pub call_id: String,
    pub extension_id: String,
    pub purpose: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// RFC 3339, when the call ended.
    pub at: String,
    /// The framed prompt, exactly as sent.
    pub prompt: String,
    /// The reply as received (empty if none came back).
    pub reply: String,
}

/// A ring of the last `cap` calls.
#[derive(Debug, Default)]
pub struct Recent {
    cap: usize,
    items: VecDeque<RequestDetail>,
}

impl Recent {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            items: VecDeque::with_capacity(cap),
        }
    }

    pub fn push(&mut self, detail: RequestDetail) {
        if self.cap == 0 {
            return;
        }
        while self.items.len() >= self.cap {
            self.items.pop_front();
        }
        self.items.push_back(detail);
    }

    /// The call, if it is one of this extension's kept calls.
    pub fn get(&self, extension_id: &str, call_id: &str) -> Option<&RequestDetail> {
        self.items
            .iter()
            .find(|d| d.call_id == call_id && d.extension_id == extension_id)
    }

    /// Ids of this extension's kept calls, newest first.
    pub fn ids_for(&self, extension_id: &str) -> Vec<String> {
        self.items
            .iter()
            .rev()
            .filter(|d| d.extension_id == extension_id)
            .map(|d| d.call_id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(id: &str, ext: &str) -> RequestDetail {
        RequestDetail {
            call_id: id.into(),
            extension_id: ext.into(),
            purpose: "organize".into(),
            label: None,
            at: "2026-11-09T10:00:00Z".into(),
            prompt: format!("prompt {id}"),
            reply: format!("reply {id}"),
        }
    }

    #[test]
    fn keeps_the_last_n_and_binds_ids_to_their_extension() {
        let mut r = Recent::new(3);
        for i in 0..5 {
            r.push(detail(&format!("c{i}"), if i % 2 == 0 { "a" } else { "b" }));
        }
        assert!(r.get("a", "c0").is_none(), "pushed out");
        assert_eq!(r.get("a", "c4").unwrap().prompt, "prompt c4");
        assert!(r.get("b", "c4").is_none(), "another extension's id");
        assert_eq!(r.ids_for("a"), vec!["c4", "c2"]);
        assert_eq!(r.ids_for("b"), vec!["c3"]);
    }
}
