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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::lock_ext::LockExt;

mod report;
mod switches;

pub use report::get_report;
use report::prettify_process_name;

/// How often to poll the foreground window (in seconds)
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
/// Bounds on the poll interval a caller can ask for. The Focus Tracker's
/// settings offer 2 to 60 s. The upper bound also caps what one row can be
/// credited (see `switches::MAX_ROW_SECS`).
const MIN_POLL_INTERVAL_SECS: u64 = 2;
const MAX_POLL_INTERVAL_SECS: u64 = 60;

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
    /// Moves from one app to a different app in the period. The first app
    /// isn't one, and neither is the first app after a break (sleep, lock).
    /// Kage's own windows are seen through. Rules in `switches.rs`.
    pub context_switches: u64,
    /// Longest unbroken run of one app, in credited seconds
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
    /// Sessions: how many separate times this app was in front, one per
    /// unbroken run of it. Unlike `context_switches` this includes the
    /// first app of the period and the first app after a break, so a day
    /// spent in one app is one session. The sum over apps is
    /// `context_switches` plus the number of fresh starts. The Focus
    /// Tracker shows it as "N sessions".
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
    /// Bumped by every start and stop. A poll loop owns the generation it
    /// was spawned with and exits once that's no longer current, so a loop
    /// still asleep from before a quick stop/start can't run alongside the
    /// new one and double-count every tick.
    generation: AtomicU64,
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
            generation: AtomicU64::new(0),
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

    /// Atomically flip `running` false → true and return the new
    /// generation, or `None` if a tracker is already running. Every
    /// extension window calls start at launch, so a plain load-then-store
    /// let several callers through and spawn one poll loop each.
    fn claim_start(&self) -> Option<u64> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(self.generation.fetch_add(1, Ordering::AcqRel) + 1)
    }

    /// Flip `running` true → false and retire the current generation.
    /// Returns false if the tracker wasn't running.
    fn release(&self) -> bool {
        if !self.running.swap(false, Ordering::AcqRel) {
            return false;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        true
    }

    /// True while the loop spawned for `generation` should keep polling.
    fn is_current(&self, generation: u64) -> bool {
        self.running.load(Ordering::Acquire)
            && self.generation.load(Ordering::Acquire) == generation
    }
}

/// Seconds to credit for one tick. `Instant` keeps counting through system
/// sleep (QueryPerformanceCounter on Windows) and runtime stalls, so an
/// unbounded delta would charge a whole night to whatever app was in front
/// at resume. A gap well past the poll interval is treated as a resume and
/// credited one interval; normal jitter is capped at two.
fn credited_secs(elapsed: std::time::Duration, interval_secs: u64) -> u64 {
    let interval = interval_secs.max(1);
    let elapsed = elapsed.as_secs();
    if elapsed > interval * 3 {
        interval
    } else {
        elapsed.clamp(1, interval * 2)
    }
}

// ---------------------------------------------------------------------------
// Database setup
// ---------------------------------------------------------------------------

fn db_path() -> PathBuf {
    let dir = crate::paths::config_dir()
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

/// Columns added to `activity_daily` after it first shipped.
/// `CREATE TABLE IF NOT EXISTS` leaves an existing table as it is, so
/// [`add_missing_daily_columns`] adds them. Rows rolled up before these
/// existed keep the default 0, because the ordering they'd need is gone.
const DAILY_ADDED_COLUMNS: [(&str, &str); 3] = [
    ("switches", "INTEGER NOT NULL DEFAULT 0"),
    ("sessions", "INTEGER NOT NULL DEFAULT 0"),
    ("longest_streak_secs", "INTEGER NOT NULL DEFAULT 0"),
];

fn init_db(conn: &Connection, poll_interval: u64) -> Result<()> {
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
            switches INTEGER NOT NULL DEFAULT 0,
            sessions INTEGER NOT NULL DEFAULT 0,
            longest_streak_secs INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (day, process_name)
        );
        ",
    )?;
    add_missing_daily_columns(conn)?;
    prune_old_rows(conn, poll_interval);
    Ok(())
}

fn add_missing_daily_columns(conn: &Connection) -> Result<()> {
    for (column, decl) in DAILY_ADDED_COLUMNS {
        let present = conn
            .prepare("SELECT 1 FROM pragma_table_info('activity_daily') WHERE name = ?1")?
            .exists([column])?;
        if !present {
            conn.execute_batch(&format!(
                "ALTER TABLE activity_daily ADD COLUMN {column} {decl}"
            ))
            .with_context(|| format!("Failed to add activity_daily.{column}"))?;
        }
    }
    Ok(())
}

/// Retention pass, run once at DB open. Fine-grained rows older than
/// [`RETENTION_DAYS`] are ROLLED UP into `activity_daily` before being
/// deleted, so long-range reports (year / all-time) keep the same
/// figures as the fine window: per-day, per-app time, switches into the
/// app, its sessions, and its longest streak (see `switches.rs`). Without
/// the switch figures,
/// a year report would count switches over the last 90 days only while
/// its time covered the whole year. What the rollup drops: window titles
/// (site breakdowns) and row ordering. A streak running across a prune
/// boundary is split in two (one extra session), and the switch between
/// the last rolled-up row and the first fine row isn't counted (at most
/// one per Kage launch, on data 90+ days old). Marker rows (empty process
/// name, 0 s) aren't rolled up. Rollup rows older than
/// [`ROLLUP_RETENTION_DAYS`] age out too.
///
/// The rollup and the delete commit together, so a crash between them
/// can't count a day twice. Best effort: a failure here just means
/// tables are larger than intended, so we log and carry on rather than
/// failing DB init.
fn prune_old_rows(conn: &Connection, poll_interval: u64) {
    let cutoff_dt = Local::now() - ChronoDuration::days(RETENTION_DAYS);
    let cutoff = cutoff_dt.to_rfc3339();

    match roll_up_and_delete(conn, &cutoff, poll_interval) {
        Ok(n) if n > 0 => info!(
            "[ActivityTracker] rolled up + pruned {} row(s) older than {} days",
            n, RETENTION_DAYS
        ),
        Ok(_) => {}
        Err(e) => warn!(
            "[ActivityTracker] rollup failed: {} — skipping prune so no data is lost",
            e
        ),
    }

    // Age out ancient rollup rows.
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

/// Fold every `activity_log` row older than `cutoff` into `activity_daily`,
/// then delete them, in one transaction. Returns the number of fine rows
/// deleted.
fn roll_up_and_delete(conn: &Connection, cutoff: &str, poll_interval: u64) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;

    // Switches and streaks need the rows in order, so they're computed here
    // rather than in SQL. Each streak is one session, filed under the day
    // it started, and its switch (if any) happened on that day too.
    #[derive(Default)]
    struct DayStats {
        switches: u64,
        sessions: u64,
        longest: u64,
    }
    let mut per_day: std::collections::HashMap<(String, String), DayStats> =
        std::collections::HashMap::new();
    switches::for_each_streak(&tx, "timestamp < ?1", cutoff, poll_interval, |streak| {
        let day = per_day.entry((streak.day, streak.app)).or_default();
        day.switches += u64::from(streak.switched_in);
        day.sessions += 1;
        day.longest = day.longest.max(streak.secs);
    })?;

    // Time per day and app. substr(ts,1,10) is the "YYYY-MM-DD" prefix of
    // the RFC3339 timestamp. UPSERT so a re-run (or rows landing on a day
    // that already has rollup) adds. Marker rows carry no time.
    tx.execute(
        "INSERT INTO activity_daily (day, process_name, duration_secs)
         SELECT substr(timestamp, 1, 10), process_name, SUM(duration_secs)
         FROM activity_log WHERE timestamp < ?1 AND process_name <> ''
         GROUP BY substr(timestamp, 1, 10), process_name
         ON CONFLICT(day, process_name)
         DO UPDATE SET duration_secs = duration_secs + excluded.duration_secs",
        rusqlite::params![cutoff],
    )?;

    {
        let mut upsert = tx.prepare(
            "INSERT INTO activity_daily (day, process_name, switches, sessions, longest_streak_secs)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(day, process_name) DO UPDATE SET
                 switches = switches + excluded.switches,
                 sessions = sessions + excluded.sessions,
                 longest_streak_secs = MAX(longest_streak_secs, excluded.longest_streak_secs)",
        )?;
        for ((day, process_name), stats) in &per_day {
            upsert.execute(rusqlite::params![
                day,
                process_name,
                stats.switches,
                stats.sessions,
                stats.longest
            ])?;
        }
    }

    let deleted = tx.execute(
        "DELETE FROM activity_log WHERE timestamp < ?1",
        rusqlite::params![cutoff],
    )?;
    tx.commit()?;
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Start / Stop
// ---------------------------------------------------------------------------

pub async fn start_tracker(
    state: &Arc<ActivityTrackerState>,
    poll_interval: Option<u64>,
) -> Result<()> {
    // Claim before any await so concurrent starts can't all pass the check.
    let Some(generation) = state.claim_start() else {
        return Ok(()); // Already running
    };

    if let Some(interval) = poll_interval {
        *state.poll_interval_secs.lock_or_recover() =
            interval.clamp(MIN_POLL_INTERVAL_SECS, MAX_POLL_INTERVAL_SECS);
    }
    let interval = *state.poll_interval_secs.lock_or_recover();

    // Open + init DB on the blocking pool (file I/O + schema create).
    let opened = match tokio::task::spawn_blocking(move || -> Result<Connection> {
        let conn = Connection::open(db_path()).context("Failed to open activity database")?;
        init_db(&conn, interval)?;
        Ok(conn)
    })
    .await
    {
        Ok(result) => result,
        Err(e) => Err(anyhow::Error::new(e).context("DB init task panicked")),
    };
    let conn = match opened {
        Ok(conn) => conn,
        Err(e) => {
            // Undo the claim so a later start can retry — unless a stop
            // (and maybe a newer start) already superseded this one.
            if state.is_current(generation) {
                state.release();
            }
            return Err(e);
        }
    };

    *state.db.lock_or_recover() = Some(conn);

    info!(
        "[ActivityTracker] Started (poll every {}s)",
        state.poll_interval_secs.lock_or_recover()
    );

    // Spawn background poller
    let state_clone = Arc::clone(state);
    tokio::spawn(async move {
        poll_loop(state_clone, generation).await;
    });

    Ok(())
}

pub async fn stop_tracker(state: &Arc<ActivityTrackerState>) {
    if state.release() {
        info!("[ActivityTracker] Stopped");
    }
}

/// One `activity_log` row: `(process_name, window_title, duration_secs)`.
type TickRow = (String, String, u64);

/// The row one tick stores, or `None` to store nothing.
///
/// - An app in front: its row, credited `credited` seconds.
/// - Kage's own window or transient system UI in front: a marker row,
///   empty process name and title, 0 s. It adds no time anywhere, but it
///   tells `switches.rs` the machine was awake and in use, so a minute in
///   Kage's chat doesn't read as a break. The platform layer returns `None`
///   for our own windows on Windows (by PID) and on a bundled macOS build
///   (by name), so `own_window_in_front` asks it separately.
///   `is_own_process_name` also catches an unbundled macOS dev binary,
///   which reports itself as lowercase `kage`.
/// - No window, or a window with no process name: nothing. That's what
///   the locked desktop on Windows looks like, so the gap it leaves is a
///   break.
fn row_for_tick(
    foreground: Option<(String, String)>,
    own_window_in_front: impl FnOnce() -> bool,
    credited: u64,
) -> Option<TickRow> {
    let marker = || Some((String::new(), String::new(), 0));
    match foreground {
        Some((_, process))
            if is_system_noise(&process) || switches::is_own_process_name(&process) =>
        {
            marker()
        }
        Some((title, process)) if !process.trim().is_empty() => Some((process, title, credited)),
        Some(_) => None,
        None if own_window_in_front() => marker(),
        None => None,
    }
}

async fn poll_loop(state: Arc<ActivityTrackerState>, generation: u64) {
    crate::os::set_current_thread_name("activity-tracker");
    let mut last_poll = std::time::Instant::now();

    while state.is_current(generation) {
        let interval = *state.poll_interval_secs.lock_or_recover();
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        if !state.is_current(generation) {
            break;
        }

        let now = std::time::Instant::now();
        let elapsed_secs = credited_secs(now.duration_since(last_poll), interval);
        last_poll = now;

        let Some((process, title, credited)) = row_for_tick(
            crate::os::window_list::get_foreground_window_info(),
            crate::os::window_list::foreground_is_own_process,
            elapsed_secs,
        ) else {
            continue;
        };

        if process.is_empty() {
            debug!("[ActivityTracker] Kage or system UI in front");
        } else {
            debug!("[ActivityTracker] Active: {} ({})", process, title);
        }

        // Record to DB on the blocking pool so SQLite's disk I/O doesn't
        // stall the async scheduler. The lock is a std::sync::Mutex and is
        // only held for the duration of the insert.
        let state_for_insert = Arc::clone(&state);
        let insert_result = tokio::task::spawn_blocking(move || {
            let guard = state_for_insert.db.lock_or_recover();
            if let Some(ref conn) = *guard {
                let timestamp = Local::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![timestamp, process, title, credited],
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
             FROM activity_log WHERE timestamp >= ?1 AND process_name <> ''
             GROUP BY month, process_name
             UNION ALL
             SELECT substr(day, 1, 7) as month, process_name,
                    SUM(duration_secs) as total
             FROM activity_daily WHERE day >= ?1 AND process_name <> ''
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
mod lifecycle_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn second_start_claim_is_rejected_while_running() {
        let state = ActivityTrackerState::new();
        let first = state.claim_start().expect("first claim wins");
        assert!(
            state.claim_start().is_none(),
            "concurrent start must not spawn a second loop"
        );
        assert!(state.is_current(first));
    }

    #[test]
    fn stale_loop_is_retired_by_quick_stop_start() {
        let state = ActivityTrackerState::new();
        let old = state.claim_start().unwrap();
        assert!(state.release());
        let new = state.claim_start().unwrap();
        // The old loop wakes to running == true but must still exit.
        assert!(!state.is_current(old));
        assert!(state.is_current(new));
    }

    #[test]
    fn release_when_stopped_is_a_noop() {
        let state = ActivityTrackerState::new();
        assert!(!state.release());
    }

    #[test]
    fn credited_secs_passes_normal_ticks_through() {
        assert_eq!(credited_secs(Duration::from_secs(5), 5), 5);
        assert_eq!(credited_secs(Duration::from_millis(200), 5), 1);
        assert_eq!(credited_secs(Duration::from_secs(12), 5), 10);
    }

    #[test]
    fn credited_secs_does_not_charge_a_sleep_gap() {
        // Lid closed overnight: ~15h between ticks.
        assert_eq!(credited_secs(Duration::from_secs(54_000), 5), 5);
    }

    #[test]
    fn no_tick_is_credited_more_than_max_row_secs() {
        // switches.rs reads anything above this as an older build's resume.
        for interval in MIN_POLL_INTERVAL_SECS..=MAX_POLL_INTERVAL_SECS {
            for elapsed in (0..=4 * MAX_POLL_INTERVAL_SECS).chain([54_000]) {
                let credited = credited_secs(Duration::from_secs(elapsed), interval);
                assert!(
                    credited <= switches::MAX_ROW_SECS,
                    "{interval}s / {elapsed}s"
                );
            }
        }
    }

    fn fg(title: &str, process: &str) -> Option<(String, String)> {
        Some((title.to_string(), process.to_string()))
    }

    fn marker() -> Option<TickRow> {
        Some((String::new(), String::new(), 0))
    }

    #[test]
    fn an_app_in_front_stores_its_row() {
        let row = row_for_tick(fg("main.rs", "code"), || panic!("not asked"), 5);
        assert_eq!(row, Some(("code".into(), "main.rs".into(), 5)));
    }

    #[test]
    fn kage_and_system_ui_store_a_marker() {
        for process in ["kage", "Kage", "SearchHost", "ShellExperienceHost"] {
            assert_eq!(
                row_for_tick(fg("x", process), || false, 5),
                marker(),
                "{process}"
            );
        }
        // Our own window, which the platform layer reports as None.
        assert_eq!(row_for_tick(None, || true, 5), marker());
    }

    #[test]
    fn no_window_stores_nothing() {
        assert_eq!(row_for_tick(None, || false, 5), None);
        // A window whose process couldn't be named.
        assert_eq!(row_for_tick(fg("title", ""), || true, 5), None);
        assert_eq!(row_for_tick(fg("title", "  "), || true, 5), None);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    #[test]
    fn prune_removes_only_rows_older_than_retention() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();

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

        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM activity_log", [], |r| r.get(0))
            .unwrap();
        // The old row is gone; the recent and just-inside rows survive.
        assert_eq!(remaining, 2);
    }

    #[test]
    fn prune_rolls_old_rows_into_daily_before_deleting() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();

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

        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);

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
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();

        let old_dt = Local::now() - ChronoDuration::days(RETENTION_DAYS + 5);
        let insert = |secs: u64| {
            conn.execute(
                "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, 'code', 't', ?2)",
                rusqlite::params![old_dt.to_rfc3339(), secs],
            )
            .unwrap();
        };

        insert(5);
        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);
        insert(7); // arrives late for the same old day (e.g. clock skew)
        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);

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

    fn noon_days_ago(days: i64) -> chrono::DateTime<Local> {
        (Local::now() - ChronoDuration::days(days))
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .earliest()
            .unwrap()
    }

    fn insert_row(conn: &Connection, ts: chrono::DateTime<Local>, process: &str, secs: u64) {
        conn.execute(
            "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, ?2, 't', ?3)",
            rusqlite::params![ts.to_rfc3339(), process, secs],
        )
        .unwrap();
    }

    /// Rows 5 s apart from noon `days` ago, each credited 5 s. An empty
    /// name is a marker row (0 s).
    fn seed_old_run(conn: &Connection, days: i64, apps: &[&str]) {
        let noon = noon_days_ago(days);
        for (i, app) in apps.iter().enumerate() {
            let ts = noon + ChronoDuration::seconds(i as i64 * 5);
            insert_row(conn, ts, app, if app.is_empty() { 0 } else { 5 });
        }
    }

    /// `(duration_secs, switches, sessions, longest_streak_secs)`.
    fn init_mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();
        conn
    }

    fn daily_stats(conn: &Connection, process: &str) -> (u64, u64, u64, u64) {
        conn.query_row(
            "SELECT duration_secs, switches, sessions, longest_streak_secs
             FROM activity_daily WHERE process_name = ?1",
            [process],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    }

    #[test]
    fn prune_keeps_switches_and_longest_streak_per_day() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();
        seed_old_run(
            &conn,
            RETENTION_DAYS + 5,
            &["code", "code", "code", "slack", "code", "code"],
        );

        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);

        // code: first app (no switch, one session), then one switch back
        // from slack (a second session).
        assert_eq!(daily_stats(&conn, "code"), (25, 1, 2, 15));
        assert_eq!(daily_stats(&conn, "slack"), (5, 1, 1, 5));
    }

    #[test]
    fn prune_reads_an_older_builds_overnight_credit_as_a_break() {
        let conn = init_mem_db();
        // Before the credit cap, the first row after a night's sleep held
        // the whole night.
        let noon = noon_days_ago(RETENTION_DAYS + 5);
        let at = |secs: i64| noon + ChronoDuration::seconds(secs);
        insert_row(&conn, at(0), "code", 5);
        insert_row(&conn, at(5), "code", 5);
        insert_row(&conn, at(54_005), "code", 54_000);
        insert_row(&conn, at(54_010), "code", 5);

        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);

        let (switches, sessions, longest): (u64, u64, u64) = conn
            .query_row(
                "SELECT SUM(switches), SUM(sessions), MAX(longest_streak_secs)
                 FROM activity_daily WHERE process_name = 'code'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((switches, sessions), (0, 2));
        // Two 10 s streaks, not one of 54,015 s kept for two years.
        assert_eq!(longest, 10);
    }

    #[test]
    fn prune_drops_marker_rows_but_keeps_the_switch_they_bridge() {
        let conn = init_mem_db();
        let mut seq = vec!["code", "code"];
        seq.extend([""; 12]);
        seq.push("chrome");
        seed_old_run(&conn, RETENTION_DAYS + 5, &seq);

        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);

        let markers: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM activity_daily WHERE process_name = ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(markers, 0);
        assert_eq!(daily_stats(&conn, "chrome"), (5, 1, 1, 5));
    }

    #[test]
    fn prune_reruns_accumulate_switches_and_keep_the_longest_streak() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();
        seed_old_run(
            &conn,
            RETENTION_DAYS + 5,
            &["code", "slack", "code", "code"],
        );
        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);
        // A second launch rolls up a later stretch of the same day.
        seed_old_run(&conn, RETENTION_DAYS + 5, &["slack", "code"]);
        prune_old_rows(&conn, DEFAULT_POLL_INTERVAL_SECS);

        let (_, code_switches, code_sessions, code_longest) = daily_stats(&conn, "code");
        assert_eq!(code_switches, 2);
        assert_eq!(code_sessions, 3);
        assert_eq!(code_longest, 10);
        let (_, slack_switches, slack_sessions, _) = daily_stats(&conn, "slack");
        assert_eq!(slack_switches, 1);
        assert_eq!(slack_sessions, 2);
    }

    #[test]
    fn init_db_adds_switch_columns_to_an_existing_rollup_table() {
        let conn = Connection::open_in_memory().unwrap();
        // activity_daily as the first rollup build created it.
        conn.execute_batch(
            "CREATE TABLE activity_daily (
                day TEXT NOT NULL,
                process_name TEXT NOT NULL,
                duration_secs INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (day, process_name)
            );
            INSERT INTO activity_daily VALUES ('2026-01-10', 'code', 3600);",
        )
        .unwrap();

        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();
        // Opening again is a no-op, not a duplicate-column error.
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();

        assert_eq!(daily_stats(&conn, "code"), (3600, 0, 0, 0));
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
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();

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
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();
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
        init_db(&conn, DEFAULT_POLL_INTERVAL_SECS).unwrap();
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
