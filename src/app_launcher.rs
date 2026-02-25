use anyhow::Result;
use log::info;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::os;

#[derive(Debug, Clone, Serialize)]
pub struct Application {
    pub name: String,
    pub path: PathBuf,
    pub aliases: Vec<String>,
    pub icon_base64: Option<String>,
    pub emoji_icon: Option<String>,
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

        // Use OS abstraction layer to scan applications
        let apps = os::scan_applications()?;
        
        for app_info in apps {
            self.add_application(app_info.name, app_info.path, app_info.icon_path, app_info.emoji_icon, app_info.icon_data);
        }

        info!("Application registry refreshed: {} apps found", self.app_registry.len());
        Ok(())
    }

    fn add_application(&mut self, name: String, path: PathBuf, icon_path: Option<String>, emoji_icon: Option<String>, icon_data: Option<String>) {
        let normalized_name = name.to_lowercase();
        
        // Generate aliases (lowercase, without spaces, etc.)
        let mut aliases = vec![normalized_name.clone()];
        let no_spaces = normalized_name.replace(" ", "");
        if no_spaces != normalized_name {
            aliases.push(no_spaces);
        }

        // Extract icon from path using OS abstraction
        let icon_base64 = icon_path.and_then(|path| {
            os::extract_icon_base64(&path)
        });

        // Use pre-computed icon data if available, otherwise extract from path
        let final_icon = if icon_data.is_some() {
            icon_data
        } else {
            icon_base64
        };

        let app = Application {
            name: name.clone(),
            path,
            aliases,
            icon_base64: final_icon,
            emoji_icon,
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
        os::launch_application(&app.path)
    }
}
