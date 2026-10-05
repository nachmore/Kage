// Linux power state via /sys/class/power_supply/BAT0/.

use crate::os::power::PowerState;

pub fn get_power_state_impl() -> PowerState {
    // Read from /sys/class/power_supply/BAT0/
    let status_path = "/sys/class/power_supply/BAT0/status";
    let capacity_path = "/sys/class/power_supply/BAT0/capacity";

    // No battery at all: a desktop, so on mains power.
    let Ok(status) = std::fs::read_to_string(status_path) else {
        return PowerState::AC;
    };
    let status = status.trim();

    if status == "Charging" || status == "Full" || status == "Not charging" {
        return PowerState::AC;
    }

    if status == "Discharging" {
        if let Ok(cap) = std::fs::read_to_string(capacity_path) {
            if let Ok(pct) = cap.trim().parse::<u32>() {
                if pct < 20 {
                    return PowerState::LowBattery;
                }
            }
        }
        return PowerState::Battery;
    }

    // A battery is present but reports something else (the kernel's own
    // "Unknown", or an empty read): don't claim AC and run full-power work.
    PowerState::Unknown
}
