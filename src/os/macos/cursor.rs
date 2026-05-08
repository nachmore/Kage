// macOS cursor position detection.
//
// Uses CGEventSource::location via a fresh HID-system-state source. This
// reads the current cursor position directly from the event subsystem
// without allocating an NSEvent or requiring the main thread — so it's
// safe to call from background threads (the floating window positioning
// code on Kage runs from a tokio task, not from AppKit's main run loop).
//
// Coordinate system note: AppKit APIs (NSEvent.mouseLocation) return
// bottom-origin coordinates where y=0 is the bottom of the primary display.
// CGEvent APIs (what we use here) return top-origin coordinates matching
// every other platform Kage runs on — no Y-flip required at the callsite.

use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

pub fn get_cursor_position_impl() -> Option<(i32, i32)> {
    // HIDSystemState is the lowest-level source — it reflects where the
    // HID driver thinks the cursor is, matching what a user sees on
    // screen. CombinedSessionState would also work but involves an
    // extra hop through the session layer.
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;

    // Creating a non-keyboard, non-mouse event with the default source
    // is the documented trick for reading the current cursor without
    // synthesizing any input.
    let event = CGEvent::new(source).ok()?;
    let point = event.location();

    // CGPoint is f64; truncate to i32 matching the cross-platform signature.
    // Rounding would be slightly more accurate but at i32 pixel resolution
    // the difference is invisible.
    Some((point.x as i32, point.y as i32))
}
