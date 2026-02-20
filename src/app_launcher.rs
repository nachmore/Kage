use anyhow::{Context, Result};
use log::info;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Application {
    pub name: String,
    pub path: PathBuf,
    pub aliases: Vec<String>,
}

pub struct AppLauncher {
    app_registry: HashMap<String, Application>,
}

impl AppLauncher {
    pub fn new() -> Result<Self> {
        let mut launcher = Self {
            app_registry: HashMap::new(),
        };
        launcher.refresh_registry()?;
        Ok(launcher)
    }

    /// Refresh the application registry by scanning the system
    pub fn refresh_registry(&mut self) -> Result<()> {
        info!("Refreshing application registry");
        self.app_registry.clear();

        #[cfg(target_os = "windows")]
        self.scan_windows_apps()?;

        #[cfg(target_os = "macos")]
        self.scan_macos_apps()?;

        #[cfg(target_os = "linux")]
        self.scan_linux_apps()?;

        info!("Application registry refreshed: {} apps found", self.app_registry.len());
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn scan_windows_apps(&mut self) -> Result<()> {
        use std::fs;
        use winreg::enums::*;
        use winreg::RegKey;

        // Scan Start Menu
        if let Some(start_menu) = dirs::data_dir() {
            let start_menu_path = start_menu.join("Microsoft\\Windows\\Start Menu\\Programs");
            if start_menu_path.exists() {
                self.scan_directory_for_shortcuts(&start_menu_path)?;
            }
        }

        // Scan Common Start Menu
        let common_start_menu = PathBuf::from("C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs");
        if common_start_menu.exists() {
            self.scan_directory_for_shortcuts(&common_start_menu)?;
        }

        // Scan registry for installed applications
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        if let Ok(uninstall_key) = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall") {
            for subkey_name in uninstall_key.enum_keys().filter_map(|k| k.ok()) {
                if let Ok(subkey) = uninstall_key.open_subkey(&subkey_name) {
                    if let Ok(display_name) = subkey.get_value::<String, _>("DisplayName") {
                        if let Ok(install_location) = subkey.get_value::<String, _>("InstallLocation") {
                            // Try to find an executable
                            let install_path = PathBuf::from(&install_location);
                            if install_path.exists() {
                                if let Ok(entries) = fs::read_dir(&install_path) {
                                    for entry in entries.filter_map(|e| e.ok()) {
                                        let path = entry.path();
                                        if path.extension().and_then(|s| s.to_str()) == Some("exe") {
                                            self.add_application(display_name.clone(), path);
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

    #[cfg(target_os = "windows")]
    fn scan_directory_for_shortcuts(&mut self, dir: &PathBuf) -> Result<()> {
        use std::fs;

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                
                if path.is_dir() {
                    // Recursively scan subdirectories
                    self.scan_directory_for_shortcuts(&path)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("lnk") {
                    // Parse .lnk file to get target
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        // For now, just store the shortcut path
                        // In a full implementation, we'd resolve the .lnk target
                        self.add_application(name.to_string(), path);
                    }
                }
            }
        }

        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn scan_macos_apps(&mut self) -> Result<()> {
        use std::fs;

        let applications_dir = PathBuf::from("/Applications");
        if applications_dir.exists() {
            if let Ok(entries) = fs::read_dir(&applications_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("app") {
                        if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                            self.add_application(name.to_string(), path);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn scan_linux_apps(&mut self) -> Result<()> {
        use std::fs;

        // Scan .desktop files in standard locations
        let desktop_dirs = vec![
            PathBuf::from("/usr/share/applications"),
            PathBuf::from("/usr/local/share/applications"),
        ];

        if let Some(home) = dirs::home_dir() {
            desktop_dirs.push(home.join(".local/share/applications"));
        }

        for dir in desktop_dirs {
            if dir.exists() {
                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
                            if let Ok(content) = fs::read_to_string(&path) {
                                // Parse .desktop file
                                let mut name = None;
                                let mut exec = None;

                                for line in content.lines() {
                                    if line.starts_with("Name=") {
                                        name = Some(line[5..].to_string());
                                    } else if line.starts_with("Exec=") {
                                        exec = Some(line[5..].to_string());
                                    }
                                }

                                if let (Some(name), Some(exec)) = (name, exec) {
                                    self.add_application(name, PathBuf::from(exec));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn add_application(&mut self, name: String, path: PathBuf) {
        let normalized_name = name.to_lowercase();
        
        // Generate aliases (lowercase, without spaces, etc.)
        let mut aliases = vec![normalized_name.clone()];
        let no_spaces = normalized_name.replace(" ", "");
        if no_spaces != normalized_name {
            aliases.push(no_spaces);
        }

        let app = Application {
            name: name.clone(),
            path,
            aliases,
        };

        self.app_registry.insert(normalized_name, app);
    }

    /// Find applications matching the query using fuzzy matching
    pub fn find_app(&self, query: &str) -> Vec<Application> {
        let query_lower = query.to_lowercase();
        let mut matches = Vec::new();

        for app in self.app_registry.values() {
            // Exact match
            if app.aliases.iter().any(|alias| alias == &query_lower) {
                matches.push((app.clone(), 100));
                continue;
            }

            // Starts with match
            if app.aliases.iter().any(|alias| alias.starts_with(&query_lower)) {
                matches.push((app.clone(), 90));
                continue;
            }

            // Contains match
            if app.aliases.iter().any(|alias| alias.contains(&query_lower)) {
                matches.push((app.clone(), 70));
                continue;
            }

            // Fuzzy match (simple Levenshtein-like)
            for alias in &app.aliases {
                let similarity = self.calculate_similarity(&query_lower, alias);
                if similarity > 60 {
                    matches.push((app.clone(), similarity));
                    break;
                }
            }
        }

        // Sort by score (highest first)
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        // Return top matches
        matches.into_iter().take(5).map(|(app, _)| app).collect()
    }

    /// Simple similarity calculation (0-100)
    fn calculate_similarity(&self, s1: &str, s2: &str) -> u32 {
        if s1 == s2 {
            return 100;
        }

        let len1 = s1.len();
        let len2 = s2.len();

        if len1 == 0 || len2 == 0 {
            return 0;
        }

        // Count matching characters in order
        let mut matches = 0;
        let mut j = 0;

        for c1 in s1.chars() {
            for (i, c2) in s2.chars().enumerate().skip(j) {
                if c1 == c2 {
                    matches += 1;
                    j = i + 1;
                    break;
                }
            }
        }

        // Calculate percentage
        let max_len = len1.max(len2);
        (matches * 100 / max_len) as u32
    }

    /// Launch an application
    pub fn launch(&self, app: &Application) -> Result<()> {
        info!("Launching application: {} at {:?}", app.name, app.path);

        #[cfg(target_os = "windows")]
        {
            // On Windows, use cmd /c start to launch
            Command::new("cmd")
                .args(&["/C", "start", "", app.path.to_str().unwrap()])
                .spawn()
                .context("Failed to launch application")?;
        }

        #[cfg(target_os = "macos")]
        {
            // On macOS, use open command
            Command::new("open")
                .arg(&app.path)
                .spawn()
                .context("Failed to launch application")?;
        }

        #[cfg(target_os = "linux")]
        {
            // On Linux, execute directly or use xdg-open
            if app.path.extension().and_then(|s| s.to_str()) == Some("desktop") {
                Command::new("xdg-open")
                    .arg(&app.path)
                    .spawn()
                    .context("Failed to launch application")?;
            } else {
                Command::new(&app.path)
                    .spawn()
                    .context("Failed to launch application")?;
            }
        }

        Ok(())
    }
}
