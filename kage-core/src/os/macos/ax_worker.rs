//! Dedicated single-threaded worker for macOS Accessibility (AX) calls.
//!
//! Same rationale as `src/os/windows/uia_worker.rs`: the native element
//! type (`AXUIElementRef`) is a raw `CFTypeRef` with non-trivial lifetime
//! requirements, and the accessibility provider keeps a `thread_local!`
//! registry of retained handles keyed by the ephemeral IDs it hands back
//! to the LLM. Calling from a `spawn_blocking` pool would give each
//! worker its own `thread_local!` slot, so IDs registered on one thread
//! wouldn't resolve on another.
//!
//! One dedicated thread (`acp-ax-worker`) owns the registry. Jobs come
//! in over a bounded sync channel; each carries a oneshot reply. Two
//! simultaneous callers serialise on the channel — and that serialisation
//! is actually desirable because AX is happier with serial access (Apple
//! doesn't document strict thread-safety guarantees beyond "use the
//! same runloop").
//!
//! Cached state on the worker: nothing. Unlike Windows UIA, there's no
//! expensive `CoCreateInstance` equivalent — `AXUIElementCreateSystemWide`
//! and `AXUIElementCreateApplication` are cheap, so each job builds the
//! handles it needs.

use std::sync::mpsc;
use std::sync::OnceLock;

use crate::computer_control::tree::UIElement;
use crate::os::accessibility::{AccessibleWindowInfo, FindElementsParams};

use super::accessibility as acc;

/// One unit of work for the worker thread. Each variant carries its
/// arguments plus a oneshot reply channel so the caller blocks until the
/// worker has produced a result.
pub(super) enum Job {
    GetUiTree {
        window_title: Option<String>,
        max_depth: usize,
        include_invisible: bool,
        reply: mpsc::SyncSender<Result<UIElement, String>>,
    },
    FindElements {
        params: FindElementsParams,
        reply: mpsc::SyncSender<Result<Vec<UIElement>, String>>,
    },
    GetFocusedElement {
        reply: mpsc::SyncSender<Result<Option<UIElement>, String>>,
    },
    ListAccessibleWindows {
        title_filter: Option<String>,
        reply: mpsc::SyncSender<Result<Vec<AccessibleWindowInfo>, String>>,
    },
    ClickElement {
        element_id: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    FocusElement {
        element_id: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    SetElementValue {
        element_id: String,
        value: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    ToggleElement {
        element_id: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    SelectElement {
        element_id: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    ExpandElement {
        element_id: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    CollapseElement {
        element_id: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    ScrollElement {
        element_id: String,
        direction: String,
        amount: f64,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    GetElementText {
        element_id: String,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    GetElementChildren {
        element_id: String,
        max_depth: usize,
        reply: mpsc::SyncSender<Result<UIElement, String>>,
    },
}

const CHANNEL_CAPACITY: usize = 16;

static SENDER: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();

/// Lazily start the worker thread on first use.
fn ensure_worker() -> &'static mpsc::SyncSender<Job> {
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(CHANNEL_CAPACITY);
        if let Err(e) = std::thread::Builder::new()
            .name("acp-ax-worker".into())
            .spawn(move || run_worker(rx))
        {
            log::error!(
                "Failed to spawn AX worker thread: {} — all accessibility calls will fail",
                e
            );
        }
        tx
    })
}

/// Worker loop. Drains the job channel until every sender is dropped
/// (process shutdown).
fn run_worker(rx: mpsc::Receiver<Job>) {
    for job in rx.iter() {
        dispatch(job);
    }
}

fn dispatch(job: Job) {
    match job {
        Job::GetUiTree {
            window_title,
            max_depth,
            include_invisible,
            reply,
        } => {
            let result =
                acc::get_ui_tree_inner(window_title.as_deref(), max_depth, include_invisible);
            let _ = reply.send(result);
        }
        Job::FindElements { params, reply } => {
            let result = acc::find_elements_inner(&params);
            let _ = reply.send(result);
        }
        Job::GetFocusedElement { reply } => {
            let result = acc::get_focused_element_inner();
            let _ = reply.send(result);
        }
        Job::ListAccessibleWindows {
            title_filter,
            reply,
        } => {
            let result = acc::list_accessible_windows_inner(title_filter.as_deref());
            let _ = reply.send(result);
        }
        Job::ClickElement { element_id, reply } => {
            let result = acc::click_element_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::FocusElement { element_id, reply } => {
            let result = acc::focus_element_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::SetElementValue {
            element_id,
            value,
            reply,
        } => {
            let result = acc::set_element_value_inner(&element_id, &value);
            let _ = reply.send(result);
        }
        Job::ToggleElement { element_id, reply } => {
            let result = acc::toggle_element_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::SelectElement { element_id, reply } => {
            let result = acc::select_element_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::ExpandElement { element_id, reply } => {
            let result = acc::expand_element_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::CollapseElement { element_id, reply } => {
            let result = acc::collapse_element_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::ScrollElement {
            element_id,
            direction,
            amount,
            reply,
        } => {
            let result = acc::scroll_element_inner(&element_id, &direction, amount);
            let _ = reply.send(result);
        }
        Job::GetElementText { element_id, reply } => {
            let result = acc::get_element_text_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::GetElementChildren {
            element_id,
            max_depth,
            reply,
        } => {
            let result = acc::get_element_children_inner(&element_id, max_depth);
            let _ = reply.send(result);
        }
    }
}

/// Submit a job to the worker and block on the reply.
pub(super) fn submit<R>(
    build_job: impl FnOnce(mpsc::SyncSender<R>) -> Job,
    not_running: impl FnOnce() -> R,
) -> R {
    let tx = ensure_worker();
    let (reply_tx, reply_rx) = mpsc::sync_channel::<R>(1);
    let job = build_job(reply_tx);
    if tx.send(job).is_err() {
        return not_running();
    }
    match reply_rx.recv() {
        Ok(r) => r,
        Err(_) => not_running(),
    }
}
