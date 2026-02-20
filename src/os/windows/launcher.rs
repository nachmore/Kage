// Windows application launcher

use anyhow::{Context, Result};
use log::info;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use winreg::enums::*;
use winreg::RegKey;

use crate::os::launcher::AppInfo;

pub fn scan_applications_impl() -> Result<Vec<AppInfo>> {
    let mut apps = HashMap::new();
    
    // Scan Start Menu
    if let Some(start_menu) = dirs::data_dir() {
        let start_menu_path = start_menu.join("Microsoft\\Windows\\Start Menu\\Programs");
        if start_menu_path.exists() {
            scan_directory_for_shortcuts(&start_menu_path, &mut apps)?;
        }
    }
    
    // Scan Common Start Menu
    let common_start_menu = PathBuf::from("C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs");
    if common_start_menu.exists() {
        scan_directory_for_shortcuts(&common_start_menu, &mut apps)?;
    }
    
    // Scan registry for installed applications
    scan_registry_apps(&mut apps)?;
    
    Ok(apps.into_values().collect())
}

fn scan_directory_for_shortcuts(dir: &PathBuf, apps: &mut HashMap<String, AppInfo>) -> Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            
            if path.is_dir() {
                scan_directory_for_shortcuts(&path, apps)?;
            } else if path.extension().and_then(|s| s.to_str()) == Some("lnk") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    let icon_path = path.to_string_lossy().to_string();
                    let key = name.to_lowercase();
                    
                    if !apps.contains_key(&key) {
                        apps.insert(key, AppInfo {
                            name: name.to_string(),
                            path: path.clone(),
                            icon_path: Some(icon_path),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn scan_registry_apps(apps: &mut HashMap<String, AppInfo>) -> Result<()> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(uninstall_key) = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall") {
        for subkey_name in uninstall_key.enum_keys().filter_map(|k| k.ok()) {
            if let Ok(subkey) = uninstall_key.open_subkey(&subkey_name) {
                if let Ok(display_name) = subkey.get_value::<String, _>("DisplayName") {
                    if let Ok(install_location) = subkey.get_value::<String, _>("InstallLocation") {
                        let install_path = PathBuf::from(&install_location);
                        if install_path.exists() {
                            if let Ok(entries) = fs::read_dir(&install_path) {
                                for entry in entries.filter_map(|e| e.ok()) {
                                    let path = entry.path();
                                    if path.extension().and_then(|s| s.to_str()) == Some("exe") {
                                        let icon_path = path.to_string_lossy().to_string();
                                        let key = display_name.to_lowercase();
                                        
                                        if !apps.contains_key(&key) {
                                            apps.insert(key, AppInfo {
                                                name: display_name.clone(),
                                                path: path.clone(),
                                                icon_path: Some(icon_path),
                                            });
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn launch_application_impl(path: &PathBuf) -> Result<()> {
    info!("Launching Windows application at {:?}", path);
    Command::new("cmd")
        .args(&["/C", "start", "", path.to_str().unwrap()])
        .spawn()
        .context("Failed to launch application")?;
    Ok(())
}
