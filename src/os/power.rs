// Cross-platform power/battery status detection

/// Power status of the system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerState {
    /// Running on AC power (plugged in)
    AC,
    /// Running on battery
    Battery,
    /// Running on battery with low charge (< 20%)
    LowBattery,
    /// Cannot determine power state
    Unknown,
}

/// Get the current power state.
pub fn get_power_state() -> PowerState {
    #[cfg(target_os = "windows")]
    {
        get_power_state_windows()
    }

    #[cfg(target_os = "macos")]
    {
        // TODO: Use IOPSCopyPowerSourcesInfo
        PowerState::AC
    }

    #[cfg(target_os = "linux")]
    {
        get_power_state_linux()
    }
}

#[cfg(target_os = "windows")]
fn get_power_state_windows() -> PowerState {
    use windows::Win32::System::Power::GetSystemPowerStatus;
    use windows::Win32::System::Power::SYSTEM_POWER_STATUS;

    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe {
        if GetSystemPowerStatus(&mut status).is_ok() {
            // ACLineStatus: 0 = offline (battery), 1 = online (AC)
            if status.ACLineStatus == 1 {
                return PowerState::AC;
            }
            // BatteryLifePercent: 0-100, 255 = unknown
            if status.BatteryLifePercent != 255 && status.BatteryLifePercent < 20 {
                return PowerState::LowBattery;
            }
            return PowerState::Battery;
        }
    }
    PowerState::Unknown
}

#[cfg(target_os = "linux")]
fn get_power_state_linux() -> PowerState {
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
                if pct < 20 { return PowerState::LowBattery; }
            }
        }
        return PowerState::Battery;
    }

    // No battery found — likely a desktop
    PowerState::AC
}
