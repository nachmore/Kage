// macOS side of `os::kage_target`: which process is Kage, which receives
// keyboard input, and which owns the window under a point.

use accessibility_sys as ax;
use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    CGWindowListCopyWindowInfo,
};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};

fn executable_path(pid: u32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: a buffer of the size passed.
    let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

pub fn is_kage_process_impl(pid: u32) -> bool {
    if executable_path(pid).is_some_and(|p| crate::os::kage_target::is_kage_executable(&p)) {
        return true;
    }
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
        .and_then(|app| app.bundleIdentifier())
        .is_some_and(|id| id.to_string() == crate::os::kage_target::KAGE_BUNDLE_ID)
}

/// The active app, and whatever the accessibility API says holds keyboard
/// focus (a key non-activating panel can take keystrokes while another app
/// is frontmost).
pub fn keyboard_target_pids_impl() -> Vec<u32> {
    let mut out = Vec::new();
    if let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() {
        out.push(app.processIdentifier() as u32);
    }
    // SAFETY: the system-wide element and every copied value are released;
    // AXUIElementGetPid only reads.
    unsafe {
        let system = ax::AXUIElementCreateSystemWide();
        if system.is_null() {
            return out;
        }
        ax::AXUIElementSetMessagingTimeout(system, 0.5);
        for attr in [
            ax::kAXFocusedApplicationAttribute,
            ax::kAXFocusedUIElementAttribute,
        ] {
            let name = CFString::new(attr);
            let mut value: CFTypeRef = std::ptr::null();
            let err =
                ax::AXUIElementCopyAttributeValue(system, name.as_concrete_TypeRef(), &mut value);
            if err == ax::kAXErrorSuccess && !value.is_null() {
                let mut pid: ax::pid_t = 0;
                if ax::AXUIElementGetPid(value as ax::AXUIElementRef, &mut pid)
                    == ax::kAXErrorSuccess
                    && pid > 0
                {
                    out.push(pid as u32);
                }
                CFRelease(value);
            }
        }
        CFRelease(system as CFTypeRef);
    }
    out
}

/// Owners of the on-screen windows at (`x`, `y`), front to back, down to
/// and including the first regular (layer 0) window — what a click there
/// can land on. Kage's floating window sits above layer 0, so it counts.
pub fn window_owners_at_impl(x: i32, y: i32) -> Vec<u32> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    // SAFETY: a +1 array, wrapped so it is released.
    let windows: CFArray<CFDictionary<CFString, CFType>> = unsafe {
        let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID);
        if raw.is_null() {
            return Vec::new();
        }
        CFArray::wrap_under_create_rule(raw as *const _)
    };
    let key_bounds = CFString::from_static_string("kCGWindowBounds");
    let key_pid = CFString::from_static_string("kCGWindowOwnerPID");
    let key_layer = CFString::from_static_string("kCGWindowLayer");
    let key_alpha = CFString::from_static_string("kCGWindowAlpha");
    let number = |dict: &CFDictionary<CFString, CFType>, key: &CFString| {
        dict.find(key)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_f64())
    };
    let (px, py) = (f64::from(x), f64::from(y));
    let mut out = Vec::new();
    for info in windows.iter() {
        if number(&info, &key_alpha).is_some_and(|a| a <= 0.0) {
            continue;
        }
        let Some(bounds) = info.find(&key_bounds) else {
            continue;
        };
        if bounds.type_of() != CFDictionary::<CFString, CFType>::type_id() {
            continue;
        }
        // SAFETY: checked to be a CFDictionary; retained by the wrapper.
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_CFTypeRef() as CFDictionaryRef) };
        let field = |k: &str| number(&bounds, &CFString::new(k));
        let (Some(wx), Some(wy), Some(ww), Some(wh)) =
            (field("X"), field("Y"), field("Width"), field("Height"))
        else {
            continue;
        };
        if px < wx || py < wy || px >= wx + ww || py >= wy + wh {
            continue;
        }
        let Some(pid) = number(&info, &key_pid).map(|p| p as u32) else {
            continue;
        };
        out.push(pid);
        if number(&info, &key_layer).is_some_and(|l| l == 0.0) {
            break;
        }
    }
    out
}
