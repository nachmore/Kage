// Windows startup management via registry

use log::{info, warn};

const STARTUP_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const STARTUP_APP_NAME: &str = "Kiro Assistant";

pub fn get_startup_enabled_impl() -> bool {
    if let Ok(hkcu) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(STARTUP_KEY_PATH, winreg::enums::KEY_READ)
    {
        let val: Result<String, _> = hkcu.get_value(STARTUP_APP_NAME);
        return val.is_ok();
    }
    false
}

pub fn set_startup_enabled_impl(enabled: bool) {
    let exe = std::env::current_exe().unwrap_or_default();
    if enabled {
        if let Ok(hkcu) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey_with_flags(STARTUP_KEY_PATH, winreg::enums::KEY_WRITE)
        {
            if let Err(e) = hkcu.set_value(STARTUP_APP_NAME, &exe.to_string_lossy().to_string()) {
                warn!("Failed to set startup registry entry: {}", e);
            } else {
                info!("Startup registry entry added");
            }
        }
    } else if let Ok(hkcu) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(STARTUP_KEY_PATH, winreg::enums::KEY_WRITE)
    {
        if let Err(e) = hkcu.delete_value(STARTUP_APP_NAME) {
            warn!("Failed to remove startup registry entry: {}", e);
        } else {
            info!("Startup registry entry removed");
        }
    }
}
