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

#[derive(Default)]
pub struct AppLauncher {
    app_registry: HashMap<String, Application>,
}

impl AppLauncher {
    /// Build an empty launcher. The expensive registry scan happens later
    /// in `build_registry`, which runs on a background thread and is
    /// applied via `apply_registry`. Construction itself can't fail.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a new application registry by scanning the system.
    /// This is the expensive part (registry reads, icon extraction, etc.)
    /// and does not require `&self`, so it can run without holding the lock.
    pub fn build_registry() -> Result<HashMap<String, Application>> {
        info!("Scanning application registry");
        let mut registry = HashMap::new();
        let apps = os::scan_applications()?;

        for app_info in apps {
            let normalized_name = app_info.name.to_lowercase();

            let mut aliases = vec![normalized_name.clone()];
            let no_spaces = normalized_name.replace(" ", "");
            if no_spaces != normalized_name {
                aliases.push(no_spaces);
            }

            let icon_base64 = app_info
                .icon_path
                .and_then(|path| os::extract_icon_base64(&path));
            let final_icon = if app_info.icon_data.is_some() {
                app_info.icon_data
            } else {
                icon_base64
            };

            let app = Application {
                name: app_info.name,
                path: app_info.path,
                aliases,
                icon_base64: final_icon,
                emoji_icon: app_info.emoji_icon,
            };
            registry.insert(normalized_name, app);
        }

        info!("Application scan complete: {} apps found", registry.len());
        Ok(registry)
    }

    /// Replace the current registry with a pre-built one.
    /// This is cheap — just a pointer swap.
    pub fn apply_registry(&mut self, registry: HashMap<String, Application>) {
        self.app_registry = registry;
    }

    /// The registry entry for exactly this display name (case-insensitive),
    /// as (display name, path). No fuzzy matching: callers acting on an app
    /// (the result menu) must hit the app the user saw, or nothing.
    pub fn get_by_name(&self, name: &str) -> Option<(String, PathBuf)> {
        let app = self.app_registry.get(&name.trim().to_lowercase())?;
        app.name
            .trim()
            .eq_ignore_ascii_case(name.trim())
            .then(|| (app.name.clone(), app.path.clone()))
    }

    /// The scanned icon of the app with exactly this display name
    /// (case-insensitive), if it has one.
    pub fn icon_by_name(&self, name: &str) -> Option<String> {
        let app = self.app_registry.get(&name.trim().to_lowercase())?;
        if !app.name.trim().eq_ignore_ascii_case(name.trim()) {
            return None;
        }
        app.icon_base64.clone().filter(|i| !i.is_empty())
    }

    /// Drop the app at `path` right away (it was just moved to the Trash),
    /// so it stops showing before the install watcher's rescan lands.
    pub fn remove_app_at(&mut self, path: &std::path::Path) {
        self.app_registry.retain(|_, app| app.path != path);
    }

    /// Find applications matching the query using fuzzy matching.
    ///
    /// Scores by reference and clones only the final top 5.
    /// `Application.icon_base64` holds a full base64 PNG (KBs–tens of KBs
    /// per app), and this runs per keystroke while holding the launcher
    /// mutex — cloning every match just to discard all but 5 moved
    /// megabytes per keystroke on short queries.
    pub fn find_app(&self, query: &str) -> Vec<Application> {
        let query_lower = query.to_lowercase();
        let mut matches: Vec<(&Application, u32)> = Vec::new();

        for app in self.app_registry.values() {
            // Exact match
            if app.aliases.iter().any(|alias| alias == &query_lower) {
                matches.push((app, 100));
                continue;
            }

            // Starts with match
            if app
                .aliases
                .iter()
                .any(|alias| alias.starts_with(&query_lower))
            {
                matches.push((app, 90));
                continue;
            }

            // Contains match
            if app.aliases.iter().any(|alias| alias.contains(&query_lower)) {
                matches.push((app, 70));
                continue;
            }

            // Fuzzy match (simple Levenshtein-like)
            for alias in &app.aliases {
                let similarity = self.calculate_similarity(&query_lower, alias);
                if similarity > 60 {
                    matches.push((app, similarity));
                    break;
                }
            }
        }

        // Sort by score (highest first)
        matches.sort_by_key(|m| std::cmp::Reverse(m.1));

        // Clone only the top matches
        matches
            .into_iter()
            .take(5)
            .map(|(app, _)| app.clone())
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher_with(apps: &[(&str, &str)]) -> AppLauncher {
        let mut l = AppLauncher::new();
        l.apply_registry(
            apps.iter()
                .map(|(name, path)| {
                    (
                        name.to_lowercase(),
                        Application {
                            name: name.to_string(),
                            path: PathBuf::from(path),
                            aliases: vec![name.to_lowercase()],
                            icon_base64: None,
                            emoji_icon: None,
                        },
                    )
                })
                .collect(),
        );
        l
    }

    #[test]
    fn get_by_name_is_exact_not_fuzzy() {
        let l = launcher_with(&[("Slack", "/Applications/Slack.app")]);
        assert_eq!(
            l.get_by_name("slack"),
            Some(("Slack".into(), PathBuf::from("/Applications/Slack.app")))
        );
        assert_eq!(l.get_by_name("Slac"), None);
        assert_eq!(l.get_by_name(""), None);
    }

    #[test]
    fn icon_by_name_is_exact_and_skips_apps_without_an_icon() {
        let mut l = launcher_with(&[
            ("Google Chrome", "/Applications/Google Chrome.app"),
            ("Notes", "/System/Applications/Notes.app"),
        ]);
        let mut registry = std::mem::take(&mut l.app_registry);
        registry.get_mut("google chrome").unwrap().icon_base64 =
            Some("data:image/png;base64,AA".into());
        registry.get_mut("notes").unwrap().icon_base64 = Some(String::new());
        l.apply_registry(registry);
        assert_eq!(
            l.icon_by_name("google chrome").as_deref(),
            Some("data:image/png;base64,AA")
        );
        assert_eq!(l.icon_by_name("Google"), None);
        assert_eq!(l.icon_by_name("Notes"), None);
        assert_eq!(l.icon_by_name(""), None);
    }

    #[test]
    fn remove_app_at_drops_only_that_path() {
        let mut l = launcher_with(&[
            ("Slack", "/Applications/Slack.app"),
            ("Notes", "/System/Applications/Notes.app"),
        ]);
        l.remove_app_at(std::path::Path::new("/Applications/Slack.app"));
        assert_eq!(l.get_by_name("Slack"), None);
        assert!(l.get_by_name("Notes").is_some());
    }
}
