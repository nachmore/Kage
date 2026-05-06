//! Dedicated single-threaded worker for Windows UI Automation calls.
//!
//! UI Automation lives in a COM apartment, and `UiaElement` (from the
//! `uiautomation` crate) is `!Send`. The accessibility provider keeps a
//! `thread_local!` registry of native handles keyed by the ephemeral IDs
//! it hands back to the LLM. Pre-2026-05 the public functions were called
//! directly from whatever thread happened to be invoking them — Tauri's
//! `spawn_blocking` pool in the main app, the stdin loop in the MCP
//! binary. The MCP binary is single-threaded so it accidentally got the
//! right behaviour. The main app only ever called `get_ui_tree`, so the
//! resolve-side APIs (`click_element`, `set_element_value`, …) didn't
//! exercise the broken path either. But the moment any future caller
//! used `spawn_blocking` to register IDs in one task and resolve them in
//! another, the registry would silently come up empty: each blocking-pool
//! worker has its own `thread_local!` slot.
//!
//! Routing every call through this worker fixes the threading model:
//!
//! - One dedicated thread (`acp-uia-worker`) owns the COM apartment, the
//!   cached `UIAutomation` + `UITreeWalker`, and the element registry.
//! - Callers send a `Job` over a bounded channel and block on a oneshot
//!   response. Two simultaneous callers serialise on the channel — that's
//!   actually required by COM apartment semantics, not a downside.
//! - Other Windows subsystems (calendar, clipboard history, file search)
//!   are not routed through here. They have their own apartment
//!   affinities and need their own dedicated threads if they're going to
//!   be hot. A single shared worker for everything would let a 300ms
//!   calendar query block a 50ms accessibility tool call.
//!
//! Cached COM objects also remove the per-call `UIAutomation::new()`
//! cost (which is `CoCreateInstance` for `CUIAutomation`) — the audit
//! flagged that as a separate perf concern but the worker pattern
//! resolves it as a free byproduct.

use std::sync::mpsc;
use std::sync::OnceLock;

use uiautomation::core::{UIAutomation, UITreeWalker};

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

/// State that lives for the lifetime of the worker thread. Cached on
/// first use — `UIAutomation::new()` calls `CoCreateInstance` for
/// `CUIAutomation`, which is the expensive part. After this, every job
/// just borrows the cached objects.
pub(super) struct WorkerState {
    pub(super) automation: UIAutomation,
    pub(super) walker: UITreeWalker,
}

/// Bounded channel — caller blocks if the worker is already deep in a
/// long UIA call. Buffer big enough for a small queue, small enough that
/// runaway callers don't pile up unbounded work.
const CHANNEL_CAPACITY: usize = 16;

static SENDER: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();

/// Lazily start the worker thread. Returns the channel sender; on first
/// call this also boots the COM apartment, creates the cached `UIAutomation`
/// + walker, and spawns the loop. Subsequent calls just clone the sender.
///
/// If worker startup fails (e.g. `UIAutomation::new()` errors), every
/// future job submission gets the same error back through its reply
/// channel — see how `start_worker` handles a failed init.
fn ensure_worker() -> &'static mpsc::SyncSender<Job> {
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(CHANNEL_CAPACITY);

        let _ = std::thread::Builder::new()
            .name("acp-uia-worker".into())
            .spawn(move || run_worker(rx));

        tx
    })
}

/// The worker thread's main loop. Initialises COM as STA, builds the
/// cached UIA objects, then drains the job channel until all senders go
/// away (process shutdown).
fn run_worker(rx: mpsc::Receiver<Job>) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    let state = match build_worker_state() {
        Ok(s) => s,
        Err(e) => {
            log::error!("UIA worker init failed: {} — accessibility calls will fail", e);
            // Drain jobs and reply with the init error so callers don't hang.
            for job in rx.iter() {
                reply_init_error(job, &e);
            }
            return;
        }
    };

    for job in rx.iter() {
        dispatch(&state, job);
    }
}

fn build_worker_state() -> Result<WorkerState, String> {
    let automation = UIAutomation::new().map_err(|e| format!("UIA init: {}", e))?;
    let walker = automation
        .get_control_view_walker()
        .map_err(|e| format!("Walker: {}", e))?;
    Ok(WorkerState { automation, walker })
}

/// Run the requested job and send the result through its reply channel.
/// Each variant maps to the matching `_inner` function in `accessibility`,
/// which contains the real UIA logic. The worker thread is the only one
/// that ever calls those, so the `thread_local!` registry stays consistent.
fn dispatch(state: &WorkerState, job: Job) {
    match job {
        Job::GetUiTree { window_title, max_depth, include_invisible, reply } => {
            let result = acc::get_ui_tree_inner(state, window_title.as_deref(), max_depth, include_invisible);
            let _ = reply.send(result);
        }
        Job::FindElements { params, reply } => {
            let result = acc::find_elements_inner(state, &params);
            let _ = reply.send(result);
        }
        Job::GetFocusedElement { reply } => {
            let result = acc::get_focused_element_inner(state);
            let _ = reply.send(result);
        }
        Job::ListAccessibleWindows { title_filter, reply } => {
            let result = acc::list_accessible_windows_inner(state, title_filter.as_deref());
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
        Job::SetElementValue { element_id, value, reply } => {
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
        Job::ScrollElement { element_id, direction, amount, reply } => {
            let result = acc::scroll_element_inner(&element_id, &direction, amount);
            let _ = reply.send(result);
        }
        Job::GetElementText { element_id, reply } => {
            let result = acc::get_element_text_inner(&element_id);
            let _ = reply.send(result);
        }
        Job::GetElementChildren { element_id, max_depth, reply } => {
            let result = acc::get_element_children_inner(state, &element_id, max_depth);
            let _ = reply.send(result);
        }
    }
}

/// On worker init failure, every queued job receives the same init error
/// rather than blocking forever waiting for a reply.
fn reply_init_error(job: Job, err: &str) {
    let msg = format!("UIA worker unavailable: {}", err);
    match job {
        Job::GetUiTree { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::FindElements { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::GetFocusedElement { reply } => { let _ = reply.send(Err(msg)); }
        Job::ListAccessibleWindows { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::ClickElement { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::FocusElement { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::SetElementValue { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::ToggleElement { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::SelectElement { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::ExpandElement { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::CollapseElement { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::ScrollElement { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::GetElementText { reply, .. } => { let _ = reply.send(Err(msg)); }
        Job::GetElementChildren { reply, .. } => { let _ = reply.send(Err(msg)); }
    }
}

/// Submit a job to the worker and block on the reply. If the worker has
/// gone away (only possible at process teardown), returns the same error
/// shape callers already handle.
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
