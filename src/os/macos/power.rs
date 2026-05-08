// macOS power state detection via IOKit's IOPowerSources APIs.
//
// Hand-rolled FFI rather than pulling in the `iokit-sys` crate — we only
// need three symbols and the CF types we already depend on through
// `core-foundation`. Keeping the FFI surface narrow makes upgrades easy.
//
// IOKit's power source model:
// - `IOPSCopyPowerSourcesInfo` — returns an opaque "blob" of the current
//   power-source snapshot (one CFDictionary per source: internal battery,
//   external UPS, etc.).
// - `IOPSGetProvidingPowerSourceType` — peeks at that blob and reports
//   which type is currently providing power: `"AC Power"`, `"Battery
//   Power"`, or `"Off Line"`. This is the fast path.
// - `IOPSCopyPowerSourcesList` — returns the full list of source dicts;
//   we dip into the first internal battery's dict to pull out current
//   capacity % for the LowBattery threshold.
//
// Thresholds match the Windows implementation: < 20% capacity while on
// battery is `LowBattery` so the automation scheduler can throttle the
// same way on both platforms.

use crate::os::power::PowerState;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFRelease, CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

// IOKit power-source FFI. The `#[link]` attribute ties us to the
// IOKit framework at link time — Tauri's bundling will resolve this
// on any macOS 10.2+ system, which is every system we care about.
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFStringRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: *const c_void) -> CFTypeRef;
}

// IOPS dictionary keys and provider values. These are C string literals
// in Apple's headers; we wrap them in CFStrings at call time.
const KEY_CURRENT_CAPACITY: &str = "Current Capacity";
const KEY_MAX_CAPACITY: &str = "Max Capacity";
const KEY_TYPE: &str = "Type";
const VALUE_INTERNAL_BATTERY: &str = "InternalBattery";
const PROVIDER_AC: &str = "AC Power";
const PROVIDER_BATTERY: &str = "Battery Power";

/// Low-battery threshold (percent). Matches the Windows implementation so
/// automation throttling behaves the same on both platforms.
const LOW_BATTERY_THRESHOLD: f64 = 20.0;

pub fn get_power_state_impl() -> PowerState {
    // Take the power-source snapshot. Empty blob (NULL) means IOKit
    // couldn't enumerate — treat as Unknown rather than assuming AC,
    // otherwise automation would never throttle on a system with broken
    // power reporting.
    let blob = unsafe { IOPSCopyPowerSourcesInfo() };
    if blob.is_null() {
        return PowerState::Unknown;
    }

    // Ensure the blob is released even on early return. The
    // `IOPSCopy*` APIs follow the Copy rule so we own the retain.
    let _blob_guard = CFReleaseGuard(blob);

    // Fast path: what type of source is providing power right now?
    let provider = unsafe {
        let cf_str = IOPSGetProvidingPowerSourceType(blob);
        if cf_str.is_null() {
            return PowerState::Unknown;
        }
        // This string is owned by the blob per Apple's Get rule —
        // don't release it separately. `wrap_under_get_rule` expresses
        // exactly that retain semantics.
        let s: CFString = TCFType::wrap_under_get_rule(cf_str);
        s.to_string()
    };

    match provider.as_str() {
        PROVIDER_AC => PowerState::AC,
        PROVIDER_BATTERY => {
            // On battery — check capacity for the LowBattery threshold.
            match internal_battery_capacity_pct(blob) {
                Some(pct) if pct < LOW_BATTERY_THRESHOLD => PowerState::LowBattery,
                Some(_) => PowerState::Battery,
                None => {
                    // Couldn't read capacity — report plain Battery so
                    // automation knows we're at least not on AC.
                    PowerState::Battery
                }
            }
        }
        _ => PowerState::Unknown,
    }
}

/// Read the capacity percentage of the first internal battery in the
/// blob's power-source list. Returns None if there's no internal battery
/// or the capacity fields are missing.
fn internal_battery_capacity_pct(blob: CFTypeRef) -> Option<f64> {
    let list_ref = unsafe { IOPSCopyPowerSourcesList(blob) };
    if list_ref.is_null() {
        return None;
    }
    let list: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(list_ref) };

    let key_current = CFString::from_static_string(KEY_CURRENT_CAPACITY);
    let key_max = CFString::from_static_string(KEY_MAX_CAPACITY);
    let key_type = CFString::from_static_string(KEY_TYPE);

    for ps in list.iter() {
        let ps_ref = ps.as_concrete_TypeRef();
        // GetPowerSourceDescription follows the Get rule — don't release.
        let desc_ref = unsafe { IOPSGetPowerSourceDescription(blob, ps_ref) };
        if desc_ref.is_null() {
            continue;
        }
        let desc: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(desc_ref as *const _) };

        // Filter to the internal battery — skip UPS, external batteries, etc.
        let ty = desc
            .find(&key_type)
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if ty != VALUE_INTERNAL_BATTERY {
            continue;
        }

        let current = desc
            .find(&key_current)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_f64())?;
        let max = desc
            .find(&key_max)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_f64())?;
        if max <= 0.0 {
            return None;
        }

        return Some((current / max) * 100.0);
    }

    None
}

/// RAII guard that releases a CFTypeRef on drop. Used for the
/// IOPSCopyPowerSourcesInfo blob which we own under the Copy rule but
/// can't wrap in a `core_foundation::CFType` without knowing its concrete
/// type (it's an opaque handle, not a normal CF object).
struct CFReleaseGuard(CFTypeRef);

impl Drop for CFReleaseGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_battery_threshold_is_twenty_percent() {
        // Windows impl hard-codes 20%; make sure we stay in sync.
        assert_eq!(LOW_BATTERY_THRESHOLD, 20.0);
    }

    #[test]
    fn get_power_state_returns_a_known_variant() {
        // Can't assert the exact state (varies by whether CI is on a
        // laptop), but we should never panic and should never return
        // something outside the enum's valid variants.
        let state = get_power_state_impl();
        assert!(matches!(
            state,
            PowerState::AC | PowerState::Battery | PowerState::LowBattery | PowerState::Unknown
        ));
    }
}
