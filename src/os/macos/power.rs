// macOS power state — stub.
//
// A real implementation would call IOPSCopyPowerSourcesInfo /
// IOPSGetProvidingPowerSourceType from IOKit. Until that exists, return
// AC (the safe default — assume desktop or plugged-in laptop) and warn
// once so the missing behaviour is visible.

use crate::os::power::PowerState;
use std::sync::OnceLock;

pub fn get_power_state_impl() -> PowerState {
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        log::warn!(
            "power: macOS implementation not yet available — defaulting to PowerState::AC. \
             IOPSCopyPowerSourcesInfo integration is a follow-up."
        );
    });
    PowerState::AC
}
