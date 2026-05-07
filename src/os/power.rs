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
    crate::os::platform::power::get_power_state_impl()
}
