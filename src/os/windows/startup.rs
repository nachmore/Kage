// Windows startup management.
//
// Two mechanisms, preferred order:
//
//  1. A logon-triggered Scheduled Task (`Kage Autorun for <user>`).
//     Run-key entries sit in Windows' startup queue, which is
//     deliberately staggered and CPU/IO-throttled after logon (and on
//     Win11 subject to startup-app dispositioning), so Run-key apps can
//     appear tens of seconds after the desktop. A logon trigger fires
//     as soon as the session starts — the same mechanism PowerToys
//     uses. The task XML overrides two scheduler defaults that are
//     wrong for a long-lived GUI app: base priority (default 7 =
//     below-normal, and it sticks for the process lifetime) and
//     ExecutionTimeLimit (default 72h — the scheduler would kill Kage
//     after 3 days of uptime).
//
//  2. The classic HKCU Run key — fallback when task registration is
//     blocked (some managed/locked-down environments deny schtasks).
//
// Enable prefers the task and removes any Run-key entry so the app
// never double-launches; disable removes both. The uninstaller mirrors
// this (Run-key delete in the Tauri NSIS template, task delete in
// NSIS_HOOK_POSTUNINSTALL in src-tauri/windows/hooks.nsh — keep the
// task name there in sync with TASK_NAME_PREFIX).

use log::{debug, info, warn};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

const STARTUP_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const STARTUP_APP_NAME: &str = "Kage";

/// Task names live in a single namespace shared by every user's tasks
/// in the scheduler root folder, so the name must be per-user for
/// per-user installs on a shared machine (PowerToys does the same).
const TASK_NAME_PREFIX: &str = "Kage Autorun for ";

fn current_user() -> Option<String> {
    std::env::var("USERNAME").ok().filter(|s| !s.is_empty())
}

fn task_name() -> Option<String> {
    current_user().map(|u| format!("{TASK_NAME_PREFIX}{u}"))
}

/// `DOMAIN\user` for the task XML's trigger/principal, so the trigger
/// only fires for this user's logon (a bare LogonTrigger fires for
/// every user on the machine).
fn qualified_user() -> Option<String> {
    let user = current_user()?;
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => Some(format!("{d}\\{user}")),
        _ => Some(user),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Non-defaults worth calling out:
///  - RunLevel LeastPrivilege: no elevation needed to register, and an
///    elevated Kage would break drag-drop and kage:// deep-links from
///    non-elevated processes.
///  - Priority 5: scheduled tasks default to 7 (below-normal class).
///  - ExecutionTimeLimit PT0S: no limit — the 72h default would kill a
///    long-running instance.
///  - DisallowStartIfOnBatteries/StopIfGoingOnBatteries false: laptops
///    on battery still get their assistant.
fn build_task_xml(exe: &Path, user: &str) -> String {
    let dir = xml_escape(&exe_parent_lossy(exe).unwrap_or_default());
    let exe = xml_escape(&exe.to_string_lossy());
    let user = xml_escape(user);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts Kage when you log in. Created by Kage (Settings - System); removed by its uninstaller.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn exe_parent_lossy(exe: &Path) -> Option<String> {
    exe.parent().map(|p| p.to_string_lossy().into_owned())
}

fn schtasks(args: &[&str]) -> Option<std::process::Output> {
    Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| warn!("Failed to run schtasks: {}", e))
        .ok()
}

fn startup_task_exists() -> bool {
    let Some(name) = task_name() else {
        return false;
    };
    schtasks(&["/Query", "/TN", &name])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Register (or overwrite — `/F`) the logon task. Returns false when
/// registration is blocked so the caller can fall back to the Run key.
fn create_startup_task(exe: &Path) -> bool {
    let (Some(name), Some(user)) = (task_name(), qualified_user()) else {
        warn!("Cannot determine current user for startup task");
        return false;
    };
    let xml_path = std::env::temp_dir().join("kage-autorun-task.xml");
    if let Err(e) = std::fs::write(&xml_path, build_task_xml(exe, &user)) {
        warn!("Failed to write startup task XML: {}", e);
        return false;
    }
    let result = schtasks(&[
        "/Create",
        "/TN",
        &name,
        "/XML",
        &xml_path.to_string_lossy(),
        "/F",
    ]);
    let _ = std::fs::remove_file(&xml_path);
    match result {
        Some(o) if o.status.success() => {
            info!("Startup task registered: {}", name);
            true
        }
        Some(o) => {
            warn!(
                "schtasks /Create failed (falling back to Run key): {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        None => false,
    }
}

fn delete_startup_task() {
    let Some(name) = task_name() else { return };
    if let Some(o) = schtasks(&["/Delete", "/TN", &name, "/F"]) {
        if o.status.success() {
            info!("Startup task removed: {}", name);
        } else {
            // "The system cannot find the file specified" — wasn't there.
            debug!(
                "schtasks /Delete: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
    }
}

fn run_key_entry_exists() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(STARTUP_KEY_PATH, winreg::enums::KEY_READ)
        .and_then(|k| k.get_value::<String, _>(STARTUP_APP_NAME))
        .is_ok()
}

fn set_run_key_entry(exe: &Path) {
    match winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(STARTUP_KEY_PATH, winreg::enums::KEY_WRITE)
        .and_then(|k| k.set_value(STARTUP_APP_NAME, &exe.to_string_lossy().to_string()))
    {
        Ok(()) => info!("Startup Run-key entry added"),
        Err(e) => warn!("Failed to set startup registry entry: {}", e),
    }
}

fn delete_run_key_entry() {
    if let Ok(k) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(STARTUP_KEY_PATH, winreg::enums::KEY_WRITE)
    {
        match k.delete_value(STARTUP_APP_NAME) {
            Ok(()) => info!("Startup Run-key entry removed"),
            // Usually "value not found" — nothing to remove.
            Err(e) => debug!("Run-key entry not removed: {}", e),
        }
    }
}

pub fn get_startup_enabled_impl() -> bool {
    startup_task_exists() || run_key_entry_exists()
}

pub fn set_startup_enabled_impl(enabled: bool) {
    if !enabled {
        delete_startup_task();
        delete_run_key_entry();
        return;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    if create_startup_task(&exe) {
        // Migrate pre-task installs and prevent double-launch: the Run
        // key must not coexist with the task.
        delete_run_key_entry();
    } else {
        set_run_key_entry(&exe);
    }
}

/// Upgrade a Run-key-era autostart to the Scheduled Task in place.
/// See `os::migrate_startup_mechanism` for the rationale.
pub fn migrate_startup_mechanism_impl() {
    if run_key_entry_exists() && !startup_task_exists() {
        info!("Migrating autostart from Run key to Scheduled Task");
        set_startup_enabled_impl(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_escapes_special_characters() {
        let xml = build_task_xml(
            Path::new(r"C:\Tools & Apps\kage.exe"),
            r"DOMAIN\O'Brien <x>",
        );
        assert!(xml.contains(r"C:\Tools &amp; Apps\kage.exe"));
        assert!(xml.contains("DOMAIN\\O&apos;Brien &lt;x&gt;"));
        assert!(!xml.contains("O'Brien"));
    }

    #[test]
    fn task_xml_overrides_hostile_scheduler_defaults() {
        let xml = build_task_xml(Path::new(r"C:\kage\kage.exe"), r"D\u");
        // Below-normal-forever default priority.
        assert!(xml.contains("<Priority>5</Priority>"));
        // 72h default would kill the app after 3 days.
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        // Must never require elevation.
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        // Laptops on battery still start.
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains(r"<WorkingDirectory>C:\kage</WorkingDirectory>"));
    }

    #[test]
    fn task_name_matches_uninstaller_prefix() {
        // hooks.nsh deletes "Kage Autorun for $USERNAME" — the prefix
        // here and there must stay identical.
        assert_eq!(TASK_NAME_PREFIX, "Kage Autorun for ");
        if let Some(name) = task_name() {
            assert!(name.starts_with(TASK_NAME_PREFIX));
        }
    }
}
