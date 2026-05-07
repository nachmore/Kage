// Windows power state detection via GetSystemPowerStatus.

use crate::os::power::PowerState;

pub fn get_power_state_impl() -> PowerState {
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
