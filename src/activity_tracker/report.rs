use anyhow::{Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;

use super::switches::{for_each_streak, SwitchSummary};
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
    let poll_interval = *state.poll_interval_secs.lock_or_recover();
    report_for(conn, period, poll_interval)
}

fn report_for(conn: &Connection, period: &str, poll_interval: u64) -> Result<ActivityReport> {
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

    // Switches, sessions and streaks over the fine rows, in sample order.
    let mut fine = SwitchSummary::default();
    for_each_streak(
        conn,
        "timestamp >= ?1",
        &start_str,
        poll_interval,
        |streak| fine.add(streak),
    )?;

    // Days older than the fine window were rolled up with their switches,
    // sessions and longest streak per app, so those figures cover the same
    // days as the time figures. Days rolled up before those columns existed
    // carry 0: their time still counts, their switches are gone. Marker rows
    // (empty process name, 0 s) aren't apps.
    let mut stmt = conn.prepare(
        "SELECT process_name, SUM(total) as total, SUM(switches) as switches,
                SUM(sessions) as sessions FROM (
             SELECT process_name, SUM(duration_secs) as total, 0 as switches, 0 as sessions
             FROM activity_log WHERE timestamp >= ?1 AND process_name <> ''
             GROUP BY process_name
             UNION ALL
             SELECT process_name, SUM(duration_secs) as total, SUM(switches) as switches,
                    SUM(sessions) as sessions
             FROM activity_daily WHERE day >= ?1 AND process_name <> ''
             GROUP BY process_name
         )
         GROUP BY process_name
         ORDER BY total DESC",
    )?;

    let mut apps: Vec<AppUsage> = Vec::new();
    let mut total_seconds: u64 = 0;
    let mut context_switches = fine.context_switches;
    let rows = stmt.query_map(rusqlite::params![start_str], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u64>(1)?,
            row.get::<_, u64>(2)?,
            row.get::<_, u64>(3)?,
        ))
    })?;
    for row in rows {
        let (process_name, seconds, rolled_up_switches, rolled_up_sessions) = row?;
        total_seconds += seconds;
        context_switches += rolled_up_switches;
        // `switches_to` carries sessions (see AppUsage).
        let switches_to =
            rolled_up_sessions + fine.sessions.get(&process_name).copied().unwrap_or(0);
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

    let (mut longest_streak, mut longest_streak_app) =
        (fine.longest_streak_secs, fine.longest_streak_app);
    if let Some((app, secs)) = longest_rolled_up_streak(conn, &start_str)? {
        if secs > longest_streak {
            longest_streak = secs;
            longest_streak_app = app;
        }
    }

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

/// The longest streak stored with the rolled-up days in range, if any.
fn longest_rolled_up_streak(conn: &Connection, start_str: &str) -> Result<Option<(String, u64)>> {
    Ok(conn
        .query_row(
            "SELECT process_name, longest_streak_secs FROM activity_daily
             WHERE day >= ?1 AND longest_streak_secs > 0
             ORDER BY longest_streak_secs DESC LIMIT 1",
            rusqlite::params![start_str],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .optional()?)
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

#[cfg(test)]
mod tests {
    use super::super::{init_db, prune_old_rows, RETENTION_DAYS};
    use super::*;

    const POLL: u64 = 5;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn, POLL).unwrap();
        conn
    }

    /// Fine rows 5 s apart from `start`, each credited 5 s, except that an
    /// empty name is a marker row (0 s), as the poll loop stores for Kage.
    fn seed_fine(conn: &Connection, start: chrono::DateTime<chrono::FixedOffset>, apps: &[&str]) {
        for (i, app) in apps.iter().enumerate() {
            let ts = start + ChronoDuration::seconds(i as i64 * 5);
            let secs = if app.is_empty() { 0 } else { 5 };
            conn.execute(
                "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, ?2, 't', ?3)",
                rusqlite::params![ts.to_rfc3339(), app, secs],
            )
            .unwrap();
        }
    }

    fn march_morning() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339("2026-03-02T09:00:00+01:00").unwrap()
    }

    fn app<'a>(report: &'a ActivityReport, process: &str) -> &'a AppUsage {
        report
            .apps
            .iter()
            .find(|a| a.process_name == process)
            .unwrap_or_else(|| panic!("{process} missing from report"))
    }

    #[test]
    fn report_counts_switches_not_samples() {
        let conn = db();
        let mut seq = vec!["code"; 10];
        seq.extend([
            "slack", "slack", "code", "code", "code", "kage", "kage", "code",
        ]);
        seed_fine(&conn, march_morning(), &seq);

        let report = report_for(&conn, "all", POLL).unwrap();
        // code → slack → code. Kage in between is seen through.
        assert_eq!(report.context_switches, 2);
        // Sessions: code's first run, and its run after slack.
        assert_eq!(app(&report, "code").switches_to, 2);
        assert_eq!(app(&report, "slack").switches_to, 1);
        assert_eq!(app(&report, "kage").switches_to, 0);
        // One fresh start (the first sample) on top of the switches.
        let per_app: u64 = report.apps.iter().map(|a| a.switches_to).sum();
        assert_eq!(per_app, report.context_switches + 1);
        // Time is unchanged: every row still counts toward its app.
        assert_eq!(app(&report, "code").seconds, 70);
        assert_eq!(report.total_seconds, 90);
        assert_eq!(report.longest_streak_seconds, 50);
        assert_eq!(report.longest_streak_app, "VS Code");
    }

    #[test]
    fn report_adds_rolled_up_switches_and_streaks() {
        let conn = db();
        conn.execute_batch(
            "INSERT INTO activity_daily
                 (day, process_name, duration_secs, switches, sessions, longest_streak_secs) VALUES
                 ('2026-01-10', 'code', 3600, 4, 5, 1800),
                 ('2026-01-10', 'slack', 600, 5, 5, 300);",
        )
        .unwrap();
        seed_fine(&conn, march_morning(), &["code", "slack", "code"]);

        let report = report_for(&conn, "all", POLL).unwrap();
        // Sessions: rolled up plus fine (code twice, slack once).
        assert_eq!(app(&report, "code").switches_to, 5 + 2);
        assert_eq!(app(&report, "slack").switches_to, 5 + 1);
        assert_eq!(report.context_switches, 4 + 5 + 2);
        assert_eq!(report.longest_streak_seconds, 1800);
        assert_eq!(report.longest_streak_app, "VS Code");
    }

    #[test]
    fn legacy_rolled_up_days_keep_their_time_but_add_no_switches() {
        let conn = db();
        // A row rolled up before the switch columns existed.
        conn.execute(
            "INSERT INTO activity_daily (day, process_name, duration_secs) VALUES ('2026-01-10', 'code', 3600)",
            [],
        )
        .unwrap();
        seed_fine(&conn, march_morning(), &["slack", "code"]);

        let report = report_for(&conn, "all", POLL).unwrap();
        assert_eq!(app(&report, "code").seconds, 3605);
        assert_eq!(app(&report, "code").switches_to, 1);
        assert_eq!(report.context_switches, 1);
        assert_eq!(report.longest_streak_app, "Slack");
    }

    #[test]
    fn one_app_all_day_is_one_session_and_no_switches() {
        let conn = db();
        seed_fine(&conn, march_morning(), &["code"; 20]);

        let report = report_for(&conn, "all", POLL).unwrap();
        assert_eq!(report.context_switches, 0);
        assert_eq!(app(&report, "code").switches_to, 1);
    }

    #[test]
    fn marker_rows_bridge_kage_and_are_not_apps() {
        let conn = db();
        // A minute in Kage's chat between code and chrome.
        let mut seq = vec!["code", "code"];
        seq.extend([""; 12]);
        seq.extend(["chrome", "chrome"]);
        seed_fine(&conn, march_morning(), &seq);

        let report = report_for(&conn, "all", POLL).unwrap();
        assert_eq!(report.context_switches, 1);
        assert_eq!(app(&report, "chrome").switches_to, 1);
        assert!(report.apps.iter().all(|a| !a.process_name.is_empty()));
        assert_eq!(report.apps.len(), 2);
        assert_eq!(report.total_seconds, 20);
    }

    #[test]
    fn switches_survive_the_rollup() {
        let conn = db();
        let old = (Local::now() - ChronoDuration::days(RETENTION_DAYS + 10))
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .earliest()
            .unwrap()
            .fixed_offset();
        seed_fine(
            &conn,
            old,
            &["code", "code", "slack", "code", "lockapp", "code"],
        );
        let before = report_for(&conn, "all", POLL).unwrap();

        prune_old_rows(&conn, POLL);
        let fine_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM activity_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fine_left, 0);

        let after = report_for(&conn, "all", POLL).unwrap();
        assert_eq!(before.context_switches, 2);
        // code: the first run, after slack, and after the lock.
        assert_eq!(app(&before, "code").switches_to, 3);
        assert_eq!(after.context_switches, before.context_switches);
        assert_eq!(after.total_seconds, before.total_seconds);
        for process in ["code", "slack", "lockapp"] {
            assert_eq!(
                app(&after, process).switches_to,
                app(&before, process).switches_to,
                "{process}"
            );
        }
        assert_eq!(after.longest_streak_seconds, before.longest_streak_seconds);
        assert_eq!(after.longest_streak_app, before.longest_streak_app);
    }
}
