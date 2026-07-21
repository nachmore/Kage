//! Activity Tracker — polls the foreground window periodically and stores
//! app usage data (time per app, context switches, focus streaks).
//!
//! Data is stored in a SQLite database in the user's config directory.
//! The tracker runs as a background tokio task, controlled via start/stop.
//!
//! # Concurrency
//!
//! The DB connection lives behind a `std::sync::Mutex` (not `tokio::sync::Mutex`)
//! because all SQLite operations are synchronous/blocking. The lock is never held
//! across an `.await`. Actual DB work (inserts, queries) runs inside
//! `tokio::task::spawn_blocking` so the async scheduler keeps running while
//! SQLite does disk I/O.

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Local};
use log::{debug, info, warn};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;

mod report;

pub use report::get_report;
use report::prettify_process_name;

/// How often to poll the foreground window (in seconds)
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityReport {
    /// Time range label (e.g. "Today", "This Week")
    pub period: String,
    /// Total tracked time in seconds
    pub total_seconds: u64,
    /// Per-app breakdown, sorted by duration descending
    pub apps: Vec<AppUsage>,
    /// Number of context switches (app changes)
    pub context_switches: u64,
    /// Longest uninterrupted focus streak in seconds
    pub longest_streak_seconds: u64,
    /// App with the longest streak
    pub longest_streak_app: String,
    /// Per-month rollup, oldest first. Populated only when the period
    /// spans more than one calendar month (year / all) — long ranges
    /// read as a handful of compact month summaries instead of a wall
    /// of app rows. Empty for today/week/month.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub months: Vec<MonthUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthUsage {
    /// Human label, e.g. "Mar 2026".
    pub label: String,
    /// "YYYY-MM" sort key (also useful to clients).
    pub month: String,
    pub total_seconds: u64,
    /// Top apps for the month, capped at 3 — enough to characterise the
    /// month in one line without re-listing the whole table.
    pub top_apps: Vec<MonthAppUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthAppUsage {
    pub display_name: String,
    pub seconds: u64,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppUsage {
    pub process_name: String,
    pub display_name: String,
    pub seconds: u64,
    pub percentage: f64,
    pub switches_to: u64,
    /// For browsers: breakdown by website/page title
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<SiteUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteUsage {
    pub site: String,
    pub seconds: u64,
    pub percentage: f64,
}

// ---------------------------------------------------------------------------
// Tracker state (shared across start/stop/query)
// ---------------------------------------------------------------------------

pub struct ActivityTrackerState {
    running: AtomicBool,
    /// SQLite connection. Guarded by a sync mutex because all rusqlite calls
    /// are blocking — we never want to hold this across an `.await`. DB work
    /// happens inside `spawn_blocking`, not on the async scheduler.
    db: Mutex<Option<Connection>>,
    poll_interval_secs: Mutex<u64>,
}

impl Default for ActivityTrackerState {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            db: Mutex::new(None),
            poll_interval_secs: Mutex::new(DEFAULT_POLL_INTERVAL_SECS),
        }
    }
}

impl ActivityTrackerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Database setup
// ---------------------------------------------------------------------------

fn db_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kage");
    std::fs::create_dir_all(&dir).ok();
    dir.join("activity.db")
}

/// How long to keep activity rows. The tracker inserts one row every poll
/// interval (~5s) while running, so without pruning the table grows without
/// bound — after a year that's millions of rows, and `build_report` for "All
/// Time" walks every one. 90 days is plenty for the reports the UI offers and
/// matches the frecency cutoff elsewhere.
const RETENTION_DAYS: i64 = 90;

/// How long to keep the per-day rollup rows that pruning produces. Two
/// years covers every report the UI offers (year + all-time) at a cost
/// of a few rows per app per day — thousands of rows, not millions.
const ROLLUP_RETENTION_DAYS: i64 = 730;

fn init_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS activity_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            process_name TEXT NOT NULL,
            window_title TEXT NOT NULL,
            duration_secs INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_activity_timestamp ON activity_log(timestamp);
        CREATE INDEX IF NOT EXISTS idx_activity_process ON activity_log(process_name);
        CREATE TABLE IF NOT EXISTS activity_daily (
            day TEXT NOT NULL,
            process_name TEXT NOT NULL,
            duration_secs INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (day, process_name)
        );
        ",
    )?;
    prune_old_rows(conn);
    Ok(())
}

/// Retention pass, run once at DB open. Fine-grained rows older than
/// [`RETENTION_DAYS`] are ROLLED UP into `activity_daily` (per-day,
/// per-app totals) before being deleted, so long-range reports (year /
/// all-time) keep their totals and per-month aggregation while the big
/// table stays bounded. What the rollup deliberately drops: window
/// titles (site breakdowns), row ordering (context switches / streaks)
/// — reports only compute those from the fine window anyway. Rollup
/// rows older than [`ROLLUP_RETENTION_DAYS`] age out too. Best effort —
/// a failure here just means tables are larger than intended, so we log
/// and carry on rather than failing DB init.
fn prune_old_rows(conn: &Connection) {
    let cutoff_dt = Local::now() - ChronoDuration::days(RETENTION_DAYS);
    let cutoff = cutoff_dt.to_rfc3339();

    // 1. Fold soon-to-be-deleted rows into the daily rollup. substr(ts,1,10)
    //    is the "YYYY-MM-DD" prefix of the RFC3339 timestamp. UPSERT so a
    //    re-run (or rows landing on a day that already has rollup) adds.
    let rolled = conn.execute(
        "INSERT INTO activity_daily (day, process_name, duration_secs)
         SELECT substr(timestamp, 1, 10), process_name, SUM(duration_secs)
         FROM activity_log WHERE timestamp < ?1
         GROUP BY substr(timestamp, 1, 10), process_name
         ON CONFLICT(day, process_name)
         DO UPDATE SET duration_secs = duration_secs + excluded.duration_secs",
        rusqlite::params![cutoff],
    );
    if let Err(e) = &rolled {
        warn!(
            "[ActivityTracker] rollup failed: {} — skipping prune so no data is lost",
            e
        );
        return;
    }

    // 2. Delete the fine rows we just rolled up.
    match conn.execute(
        "DELETE FROM activity_log WHERE timestamp < ?1",
        rusqlite::params![cutoff],
    ) {
        Ok(n) if n > 0 => info!(
            "[ActivityTracker] rolled up + pruned {} row(s) older than {} days",
            n, RETENTION_DAYS
        ),
        Ok(_) => {}
        Err(e) => warn!("[ActivityTracker] retention prune failed: {}", e),
    }

    // 3. Age out ancient rollup rows.
    let rollup_cutoff = (Local::now() - ChronoDuration::days(ROLLUP_RETENTION_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    if let Err(e) = conn.execute(
        "DELETE FROM activity_daily WHERE day < ?1",
        rusqlite::params![rollup_cutoff],
    ) {
        warn!("[ActivityTracker] rollup retention prune failed: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Start / Stop
// ---------------------------------------------------------------------------

pub async fn start_tracker(
    state: &Arc<ActivityTrackerState>,
    poll_interval: Option<u64>,
) -> Result<()> {
    if state.running.load(Ordering::Relaxed) {
        return Ok(()); // Already running
    }

    if let Some(interval) = poll_interval {
        *state.poll_interval_secs.lock_or_recover() = interval.max(2);
    }

    // Open + init DB on the blocking pool (file I/O + schema create).
    let conn = tokio::task::spawn_blocking(|| -> Result<Connection> {
        let conn = Connection::open(db_path()).context("Failed to open activity database")?;
        init_db(&conn)?;
        Ok(conn)
    })
    .await
    .context("DB init task panicked")??;

    *state.db.lock_or_recover() = Some(conn);

    state.running.store(true, Ordering::Relaxed);
    info!(
        "[ActivityTracker] Started (poll every {}s)",
        state.poll_interval_secs.lock_or_recover()
    );

    // Spawn background poller
    let state_clone = Arc::clone(state);
    tokio::spawn(async move {
        poll_loop(state_clone).await;
    });

    Ok(())
}

pub async fn stop_tracker(state: &Arc<ActivityTrackerState>) {
    if !state.running.load(Ordering::Relaxed) {
        return;
    }
    state.running.store(false, Ordering::Relaxed);
    info!("[ActivityTracker] Stopped");
}

async fn poll_loop(state: Arc<ActivityTrackerState>) {
    crate::os::set_current_thread_name("activity-tracker");
    let mut last_process = String::new();
    let mut last_poll = std::time::Instant::now();

    while state.running.load(Ordering::Relaxed) {
        let interval = *state.poll_interval_secs.lock_or_recover();
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        if !state.running.load(Ordering::Relaxed) {
            break;
        }

        let now = std::time::Instant::now();
        let elapsed_secs = now.duration_since(last_poll).as_secs().max(1);
        last_poll = now;

        // Get foreground window
        let info = crate::os::window_list::get_foreground_window_info();
        let (title, process) = match info {
            Some((t, p)) if !p.is_empty() => (t, p),
            _ => continue,
        };

        // Skip transient system UI processes (noise, not real app usage)
        if is_system_noise(&process) {
            continue;
        }

        debug!("[ActivityTracker] Active: {} ({})", process, title);

        // Record to DB on the blocking pool so SQLite's disk I/O doesn't
        // stall the async scheduler. The lock is a std::sync::Mutex and is
        // only held for the duration of the insert.
        let state_for_insert = Arc::clone(&state);
        let process_for_insert = process.clone();
        let title_for_insert = title;
        let insert_result = tokio::task::spawn_blocking(move || {
            let guard = state_for_insert.db.lock_or_recover();
            if let Some(ref conn) = *guard {
                let timestamp = Local::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![timestamp, process_for_insert, title_for_insert, elapsed_secs],
                )?;
            }
            Ok::<(), rusqlite::Error>(())
        })
        .await;

        if let Err(e) = insert_result {
            warn!("[ActivityTracker] insert task panicked: {}", e);
        } else if let Ok(Err(e)) = insert_result {
            warn!("[ActivityTracker] insert failed: {}", e);
        }

        if process != last_process {
            last_process = process;
        }
    }
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// Aggregate per-month, per-app totals from both tables and reduce each
/// month to a total + its top 3 apps. Skipped when the whole range holds
/// a single month — the flat report already covers it.
pub(super) fn build_month_buckets(conn: &Connection, start_str: &str) -> Result<Vec<MonthUsage>> {
    let mut stmt = conn.prepare(
        "SELECT month, process_name, SUM(total) as total FROM (
             SELECT substr(timestamp, 1, 7) as month, process_name,
                    SUM(duration_secs) as total
             FROM activity_log WHERE timestamp >= ?1
             GROUP BY month, process_name
             UNION ALL
             SELECT substr(day, 1, 7) as month, process_name,
                    SUM(duration_secs) as total
             FROM activity_daily WHERE day >= ?1
             GROUP BY month, process_name
         )
         GROUP BY month, process_name
         ORDER BY month ASC, total DESC",
    )?;

    // Rows arrive month-ascending, per-app-descending — so per month the
    // first 3 rows ARE the top apps.
    let mut months: Vec<MonthUsage> = Vec::new();
    let rows = stmt.query_map(rusqlite::params![start_str], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u64>(2)?,
        ))
    })?;

    for row in rows {
        let (month, process_name, seconds) = row?;
        if months.last().map(|m| m.month != month).unwrap_or(true) {
            months.push(MonthUsage {
                label: month_label(&month),
                month: month.clone(),
                total_seconds: 0,
                top_apps: Vec::new(),
            });
        }
        let bucket = months.last_mut().expect("pushed above");
        bucket.total_seconds += seconds;
        if bucket.top_apps.len() < 3 {
            bucket.top_apps.push(MonthAppUsage {
                display_name: prettify_process_name(&process_name),
                seconds,
                percentage: 0.0,
            });
        }
    }

    // Percentages need the final month totals, so a second pass.
    for m in &mut months {
        for app in &mut m.top_apps {
            app.percentage = if m.total_seconds > 0 {
                (app.seconds as f64 / m.total_seconds as f64) * 100.0
            } else {
                0.0
            };
        }
    }

    // A single-month range gains nothing from a one-entry breakdown.
    if months.len() < 2 {
        months.clear();
    }
    Ok(months)
}

/// "2026-03" → "Mar 2026". Falls back to the raw key on parse failure.
fn month_label(month_key: &str) -> String {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = month_key.splitn(2, '-');
    let year = parts.next().unwrap_or("");
    let m: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if (1..=12).contains(&m) && !year.is_empty() {
        format!("{} {}", NAMES[m - 1], year)
    } else {
        month_key.to_string()
    }
}

/// Transient system UI processes that aren't real app usage.
fn is_system_noise(process_name: &str) -> bool {
    matches!(
        process_name.to_lowercase().as_str(),
        "shellexperiencehost"
            | "searchhost"
            | "textinputhost"
            | "startmenuexperiencehost"
            | "searchui"
            | "cortana"
            | "gamebar"
            | "gamebarftserver"
    )
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    #[test]
    fn prune_removes_only_rows_older_than_retention() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let recent = Local::now().to_rfc3339();
        let old = (Local::now() - ChronoDuration::days(RETENTION_DAYS + 5)).to_rfc3339();
        let just_inside = (Local::now() - ChronoDuration::days(RETENTION_DAYS - 5)).to_rfc3339();
        for ts in [&recent, &old, &just_inside] {
            conn.execute(
                "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, 'p', 't', 5)",
                rusqlite::params![ts],
            )
            .unwrap();
        }

        prune_old_rows(&conn);

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM activity_log", [], |r| r.get(0))
            .unwrap();
        // The old row is gone; the recent and just-inside rows survive.
        assert_eq!(remaining, 2);
    }

    #[test]
    fn prune_rolls_old_rows_into_daily_before_deleting() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let old_dt = Local::now() - ChronoDuration::days(RETENTION_DAYS + 5);
        let old_day = old_dt.format("%Y-%m-%d").to_string();
        // Two rows for the same app on the same old day → one rollup row
        // with the summed duration.
        for secs in [5, 7] {
            conn.execute(
                "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, 'code', 't', ?2)",
                rusqlite::params![old_dt.to_rfc3339(), secs],
            )
            .unwrap();
        }

        prune_old_rows(&conn);

        let (day, total): (String, u64) = conn
            .query_row(
                "SELECT day, duration_secs FROM activity_daily WHERE process_name = 'code'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(day, old_day);
        assert_eq!(total, 12);
        // Fine rows are gone.
        let fine: i64 = conn
            .query_row("SELECT COUNT(*) FROM activity_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fine, 0);
    }

    #[test]
    fn prune_reruns_accumulate_into_existing_rollup_rows() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let old_dt = Local::now() - ChronoDuration::days(RETENTION_DAYS + 5);
        let insert = |secs: u64| {
            conn.execute(
                "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, 'code', 't', ?2)",
                rusqlite::params![old_dt.to_rfc3339(), secs],
            )
            .unwrap();
        };

        insert(5);
        prune_old_rows(&conn);
        insert(7); // arrives late for the same old day (e.g. clock skew)
        prune_old_rows(&conn);

        let total: u64 = conn
            .query_row(
                "SELECT duration_secs FROM activity_daily WHERE process_name = 'code'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // UPSERT accumulates instead of replacing or duplicating.
        assert_eq!(total, 12);
    }
}

#[cfg(test)]
mod month_bucket_tests {
    use super::*;

    fn seed(conn: &Connection, table: &str, key: &str, process: &str, secs: u64) {
        match table {
            "fine" => conn
                .execute(
                    "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, ?2, 't', ?3)",
                    rusqlite::params![format!("{key}T12:00:00+00:00"), process, secs],
                )
                .map(|_| ())
                .unwrap(),
            _ => conn
                .execute(
                    "INSERT INTO activity_daily (day, process_name, duration_secs) VALUES (?1, ?2, ?3)
                     ON CONFLICT(day, process_name) DO UPDATE SET duration_secs = duration_secs + excluded.duration_secs",
                    rusqlite::params![key, process, secs],
                )
                .map(|_| ())
                .unwrap(),
        }
    }

    #[test]
    fn buckets_merge_fine_and_rollup_tables_per_month() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Feb entirely from the rollup, Mar split across both tables.
        seed(&conn, "rollup", "2026-02-10", "code", 100);
        seed(&conn, "rollup", "2026-03-01", "code", 40);
        seed(&conn, "fine", "2026-03-02", "code", 60);
        seed(&conn, "fine", "2026-03-02", "chrome", 30);

        let months = build_month_buckets(&conn, "2026-01-01").unwrap();
        assert_eq!(months.len(), 2);

        assert_eq!(months[0].month, "2026-02");
        assert_eq!(months[0].label, "Feb 2026");
        assert_eq!(months[0].total_seconds, 100);

        assert_eq!(months[1].month, "2026-03");
        assert_eq!(months[1].total_seconds, 130);
        // Top apps ordered by per-month total: code 100 (40+60), chrome 30.
        assert_eq!(months[1].top_apps[0].seconds, 100);
        assert_eq!(months[1].top_apps[1].seconds, 30);
    }

    #[test]
    fn buckets_cap_top_apps_at_three() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        for (i, p) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            seed(&conn, "rollup", "2026-01-05", p, 100 - i as u64);
        }
        // Need a second month or the breakdown is suppressed entirely.
        seed(&conn, "rollup", "2026-02-05", "a", 10);

        let months = build_month_buckets(&conn, "2026-01-01").unwrap();
        assert_eq!(months[0].top_apps.len(), 3);
        // Percentages are of the month total (500-... seeded: 100+99+98+97+96=490).
        let pct_sum: f64 = months[0].top_apps.iter().map(|a| a.percentage).sum();
        assert!(pct_sum > 0.0 && pct_sum <= 100.0);
    }

    #[test]
    fn single_month_range_gets_no_buckets() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn, "fine", "2026-03-02", "code", 60);
        let months = build_month_buckets(&conn, "2026-01-01").unwrap();
        assert!(months.is_empty());
    }

    #[test]
    fn month_label_formats_and_falls_back() {
        assert_eq!(month_label("2026-03"), "Mar 2026");
        assert_eq!(month_label("2026-12"), "Dec 2026");
        assert_eq!(month_label("garbage"), "garbage");
    }
}
