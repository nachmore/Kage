// Linux power state via /sys/class/power_supply/BAT0/.

use crate::os::power::PowerState;

pub fn get_power_state_impl() -> PowerState {
    // Read from /sys/class/power_supply/BAT0/
    let status_path = "/sys/class/power_supply/BAT0/status";
    let capacity_path = "/sys/class/power_supply/BAT0/capacity";

    let status = std::fs::read_to_string(status_path).unwrap_or_default();
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

    // No battery found — likely a desktop
    PowerState::AC
}
