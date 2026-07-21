use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use rusqlite::Connection;
use std::sync::Arc;

use super::{build_month_buckets, ActivityReport, ActivityTrackerState, AppUsage, SiteUsage};
use crate::lock_ext::LockExt;

pub async fn get_report(state: &Arc<ActivityTrackerState>, period: &str) -> Result<ActivityReport> {
    let state = Arc::clone(state);
    let period = period.to_string();

    tokio::task::spawn_blocking(move || build_report(&state, &period))
        .await
        .context("Report task panicked")?
}

/// Blocking report builder. Runs inside `spawn_blocking` from `get_report`.
fn build_report(state: &ActivityTrackerState, period: &str) -> Result<ActivityReport> {
    let db_guard = state.db.lock_or_recover();
    let conn = db_guard.as_ref().context("Activity tracker not started")?;

    let now = Local::now();
    let (start_date, period_label) = match period {
        "today" => (now.date_naive(), "Today".to_string()),
        "week" => {
            let start = now.date_naive()
                - ChronoDuration::days(now.weekday().num_days_from_monday() as i64);
            (start, "This Week".to_string())
        }
        "month" => {
            let start =
                NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap_or(now.date_naive());
            (start, "This Month".to_string())
        }
        "year" => {
            let start = NaiveDate::from_ymd_opt(now.year(), 1, 1).unwrap_or(now.date_naive());
            (start, "This Year".to_string())
        }
        _ => {
            let start = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
            (start, "All Time".to_string())
        }
    };

    let start_str = start_date.format("%Y-%m-%d").to_string();
    let mut stmt = conn.prepare(
        "SELECT process_name, SUM(total) as total, SUM(entries) as entries FROM (
             SELECT process_name, SUM(duration_secs) as total, COUNT(*) as entries
             FROM activity_log WHERE timestamp >= ?1 GROUP BY process_name
             UNION ALL
             SELECT process_name, SUM(duration_secs) as total, 0 as entries
             FROM activity_daily WHERE day >= ?1 GROUP BY process_name
         )
         GROUP BY process_name
         ORDER BY total DESC",
    )?;

    let mut apps: Vec<AppUsage> = Vec::new();
    let mut total_seconds: u64 = 0;
    let rows = stmt.query_map(rusqlite::params![start_str], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u64>(1)?,
            row.get::<_, u64>(2)?,
        ))
    })?;
    for row in rows {
        let (process_name, seconds, switches_to) = row?;
        total_seconds += seconds;
        apps.push(AppUsage {
            display_name: prettify_process_name(&process_name),
            process_name,
            seconds,
            percentage: 0.0,
            switches_to,
            sites: Vec::new(),
        });
    }
    for app in &mut apps {
        app.percentage = if total_seconds > 0 {
            (app.seconds as f64 / total_seconds as f64) * 100.0
        } else {
            0.0
        };
    }

    populate_browser_sites(conn, &start_str, &mut apps)?;

    let mut switch_stmt = conn.prepare(
        "SELECT process_name FROM activity_log WHERE timestamp >= ?1 ORDER BY timestamp",
    )?;
    let processes: Vec<String> = switch_stmt
        .query_map(rusqlite::params![start_str], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    let (context_switches, longest_streak, longest_streak_app) =
        calculate_streaks(&processes, *state.poll_interval_secs.lock_or_recover());

    let months = if matches!(period, "year" | "all") {
        build_month_buckets(conn, &start_str)?
    } else {
        Vec::new()
    };

    Ok(ActivityReport {
        period: period_label,
        total_seconds,
        apps,
        context_switches,
        longest_streak_seconds: longest_streak,
        longest_streak_app: prettify_process_name(&longest_streak_app),
        months,
    })
}

fn populate_browser_sites(conn: &Connection, start_str: &str, apps: &mut [AppUsage]) -> Result<()> {
    if !apps.iter().any(|app| is_browser(&app.process_name)) {
        return Ok(());
    }
    let mut site_stmt = conn.prepare(
        "SELECT process_name, window_title, SUM(duration_secs) as total
         FROM activity_log
         WHERE timestamp >= ?1
         GROUP BY process_name, window_title
         ORDER BY process_name, total DESC",
    )?;
    let site_rows = site_stmt.query_map(rusqlite::params![start_str], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u64>(2)?,
        ))
    })?;
    let mut site_map =
        std::collections::HashMap::<String, std::collections::HashMap<String, u64>>::new();
    for row in site_rows {
        let (process, title, secs) = row?;
        if is_browser(&process) {
            let site = extract_site_from_title(&title, &process);
            *site_map
                .entry(process)
                .or_default()
                .entry(site)
                .or_default() += secs;
        }
    }
    for app in apps {
        if let Some(sites) = site_map.remove(&app.process_name) {
            let app_total = app.seconds.max(1);
            let mut site_list: Vec<SiteUsage> = sites
                .into_iter()
                .map(|(site, seconds)| SiteUsage {
                    site,
                    seconds,
                    percentage: (seconds as f64 / app_total as f64) * 100.0,
                })
                .collect();
            site_list.sort_by_key(|site| std::cmp::Reverse(site.seconds));
            site_list.truncate(10);
            app.sites = site_list;
        }
    }
    Ok(())
}

fn calculate_streaks(processes: &[String], poll_interval: u64) -> (u64, u64, String) {
    let mut context_switches = 0;
    let mut longest_streak = 0;
    let mut longest_streak_app = String::new();
    let mut current_streak = 0;
    let mut current_app = String::new();
    for process in processes {
        if *process == current_app {
            current_streak += poll_interval;
        } else {
            if current_streak > longest_streak && !current_app.is_empty() {
                longest_streak = current_streak;
                longest_streak_app = current_app.clone();
            }
            current_app = process.clone();
            current_streak = poll_interval;
            if !current_app.is_empty() {
                context_switches += 1;
            }
        }
    }
    if current_streak > longest_streak && !current_app.is_empty() {
        longest_streak = current_streak;
        longest_streak_app = current_app;
    }
    (context_switches, longest_streak, longest_streak_app)
}

/// Check if a process name is a known browser.
fn is_browser(process_name: &str) -> bool {
    matches!(
        process_name.to_lowercase().as_str(),
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi" | "arc"
    )
}

/// Extract a site/page name from a browser window title.
/// Browser titles typically look like: "Page Title - Site Name - Google Chrome"
/// We strip the browser suffix and return the meaningful part.
fn extract_site_from_title(title: &str, process_name: &str) -> String {
    // Real-world browser window titles vary: some Edge builds emit a
    // U+200B zero-width space between "Microsoft" and "Edge" (encoded
    // as `\u{200B}` so it survives editors that strip invisibles), and
    // Firefox alternates between em-dash and en-dash separators across
    // versions.
    let suffixes = [
        " - Google Chrome",
        " - Microsoft\u{200B} Edge",
        " - Microsoft Edge",
        " — Mozilla Firefox",
        " - Mozilla Firefox",
        " - Brave",
        " - Opera",
        " - Vivaldi",
        " - Arc",
        " – Google Chrome",
        " – Microsoft Edge",
    ];

    let mut clean = title.to_string();
    for suffix in &suffixes {
        if let Some(pos) = clean.rfind(suffix) {
            clean = clean[..pos].to_string();
            break;
        }
    }

    // If the title still has " - " separators, take the last segment as the site name
    // e.g. "Some Page - YouTube" → "YouTube"
    if let Some(pos) = clean.rfind(" - ") {
        let site = clean[pos + 3..].trim();
        if !site.is_empty() && site.len() < 60 {
            return site.to_string();
        }
    }
    if let Some(pos) = clean.rfind(" — ") {
        let site = clean[pos + 5..].trim(); // " — " is 5 bytes in UTF-8
        if !site.is_empty() && site.len() < 60 {
            return site.to_string();
        }
    }

    // Fallback: use the full cleaned title, truncated
    let trimmed = clean.trim();
    if trimmed.len() > 50 {
        // Find a valid char boundary near 47 bytes
        let mut end = 47;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &trimmed[..end])
    } else if trimmed.is_empty() {
        prettify_process_name(process_name)
    } else {
        trimmed.to_string()
    }
}

pub(super) fn prettify_process_name(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "code" | "code - insiders" => "VS Code".to_string(),
        "chrome" => "Chrome".to_string(),
        "firefox" => "Firefox".to_string(),
        "msedge" => "Edge".to_string(),
        "explorer" => "File Explorer".to_string(),
        "lockapp" => "Screen Locked".to_string(),
        "slack" => "Slack".to_string(),
        "teams" => "Teams".to_string(),
        "discord" => "Discord".to_string(),
        "windowsterminal" => "Terminal".to_string(),
        "spotify" => "Spotify".to_string(),
        "outlook" => "Outlook".to_string(),
        "winword" => "Word".to_string(),
        "excel" => "Excel".to_string(),
        "powerpnt" => "PowerPoint".to_string(),
        "notepad" => "Notepad".to_string(),
        "notepad++" => "Notepad++".to_string(),
        _ => {
            // Capitalize first letter
            let mut c = name.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().to_string() + c.as_str(),
            }
        }
    }
}
