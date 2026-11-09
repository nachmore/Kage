//! The one-request slot (docs/design/EXTENSION_FILE_TEXT.md §5.1 step 5):
//! one `pick_file_text` request at a time in the whole app, and
//! `DIALOG_GAP` between one extension's dialogs. Memory only.

use super::Code;
use crate::lock_ext::LockExt;
use kage_core::file_text_wire::DIALOG_GAP;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Who holds the slot, and when each extension's last dialog closed.
#[derive(Debug, Default)]
pub struct Slot {
    state: Mutex<SlotState>,
}

#[derive(Debug, Default)]
struct SlotState {
    active: Option<(String, Arc<AtomicBool>)>,
    last_closed: HashMap<String, Instant>,
}

/// The app's slot.
pub fn slot() -> &'static Arc<Slot> {
    static SLOT: OnceLock<Arc<Slot>> = OnceLock::new();
    SLOT.get_or_init(Arc::default)
}

impl Slot {
    /// Takes the slot for `extension_id`: `busy` while any request runs,
    /// `too_soon` within `DIALOG_GAP` of this extension's last dialog.
    pub fn take(self: &Arc<Self>, extension_id: &str, now: Instant) -> Result<SlotGuard, Code> {
        let mut state = self.state.lock_or_recover();
        if state.active.is_some() {
            return Err(Code::Busy);
        }
        state
            .last_closed
            .retain(|_, at| now.saturating_duration_since(*at) < DIALOG_GAP);
        if state.last_closed.contains_key(extension_id) {
            return Err(Code::TooSoon);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        state.active = Some((extension_id.to_string(), cancel.clone()));
        Ok(SlotGuard {
            slot: self.clone(),
            extension_id: extension_id.to_string(),
            cancel,
        })
    }

    /// Cancels `extension_id`'s request if it is the one running. Returns
    /// whether it was.
    pub fn cancel(&self, extension_id: &str) -> bool {
        let state = self.state.lock_or_recover();
        match &state.active {
            Some((id, flag)) if id == extension_id => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }
}

/// The running request's hold on the slot. Dropping it frees the slot.
#[derive(Debug)]
pub struct SlotGuard {
    slot: Arc<Slot>,
    extension_id: String,
    cancel: Arc<AtomicBool>,
}

impl SlotGuard {
    /// Starts this extension's gap before its next dialog.
    pub fn dialog_closed(&self, now: Instant) {
        let mut state = self.slot.state.lock_or_recover();
        state.last_closed.insert(self.extension_id.clone(), now);
    }

    /// The request's cancel flag, set by `cancel_file_text`.
    pub fn flag(&self) -> &AtomicBool {
        &self.cancel
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut state = self.slot.state.lock_or_recover();
        if state
            .active
            .as_ref()
            .is_some_and(|(_, flag)| Arc::ptr_eq(flag, &self.cancel))
        {
            state.active = None;
        }
    }
}
