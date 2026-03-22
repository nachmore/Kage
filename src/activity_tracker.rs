//! Activity Tracker — polls the foreground window periodically and stores
//! app usage data (time per app, context switches, focus streaks).
//!
//! Data is stored in a SQLite database in the user's config directory.
//! The tracker runs as a background tokio task, controlled via start/stop.

use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate, Duration as ChronoDuration};
use log::{info, debug};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppUsage {
    pub process_name: String,
    pub display_name: String,
    pub seconds: u64,
    pub percentage: f64,
    pub switches_to: u64,
}

// ---------------------------------------------------------------------------
// Tracker state (shared across start/stop/query)
// ---------------------------------------------------------------------------

pub struct ActivityTrackerState {
    running: AtomicBool,
    db: Mutex<Option<Connection>>,
    poll_interval_secs: std::sync::Mutex<u64>,
}

impl ActivityTrackerState {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            db: Mutex::new(None),
            poll_interval_secs: std::sync::Mutex::new(DEFAULT_POLL_INTERVAL_SECS),
        }
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
        .join("kiro-assistant");
    std::fs::create_dir_all(&dir).ok();
    dir.join("activity.db")
}

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
        "
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Start / Stop
// ---------------------------------------------------------------------------

pub async fn start_tracker(state: &Arc<ActivityTrackerState>, poll_interval: Option<u64>) -> Result<()> {
    if state.running.load(Ordering::Relaxed) {
        return Ok(()); // Already running
    }

    if let Some(interval) = poll_interval {
        *state.poll_interval_secs.lock().unwrap() = interval.max(2);
    }

    // Open DB
    let conn = Connection::open(db_path()).context("Failed to open activity database")?;
    init_db(&conn)?;
    *state.db.lock().await = Some(conn);

    state.running.store(true, Ordering::Relaxed);
    info!("[ActivityTracker] Started (poll every {}s)", state.poll_interval_secs.lock().unwrap());

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
    let mut last_process = String::new();
    let mut last_poll = std::time::Instant::now();

    while state.running.load(Ordering::Relaxed) {
        let interval = *state.poll_interval_secs.lock().unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        if !state.running.load(Ordering::Relaxed) { break; }

        let now = std::time::Instant::now();
        let elapsed_secs = now.duration_since(last_poll).as_secs().max(1);
        last_poll = now;

        // Get foreground window
        let info = crate::os::window_list::get_foreground_window_info();
        let (title, process) = match info {
            Some((t, p)) if !p.is_empty() => (t, p),
            _ => continue,
        };

        debug!("[ActivityTracker] Active: {} ({})", process, title);

        // Record to DB
        let db_guard = state.db.lock().await;
        if let Some(ref conn) = *db_guard {
            let timestamp = Local::now().to_rfc3339();
            let _ = conn.execute(
                "INSERT INTO activity_log (timestamp, process_name, window_title, duration_secs) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![timestamp, process, title, elapsed_secs],
            );
        }

        if process != last_process {
            last_process = process;
        }
    }
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

pub async fn get_report(state: &Arc<ActivityTrackerState>, period: &str) -> Result<ActivityReport> {
    let db_guard = state.db.lock().await;
    let conn = db_guard.as_ref().context("Activity tracker not started")?;

    let now = Local::now();
    let (start_date, period_label) = match period {
        "today" => (now.date_naive(), "Today".to_string()),
        "week" => {
            let start = now.date_naive() - ChronoDuration::days(now.weekday().num_days_from_monday() as i64);
            (start, "This Week".to_string())
        }
        "month" => {
            let start = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap_or(now.date_naive());
            (start, "This Month".to_string())
        }
        "all" | _ => {
            let start = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
            (start, "All Time".to_string())
        }
    };

    let start_str = start_date.format("%Y-%m-%d").to_string();

    // Per-app totals
    let mut stmt = conn.prepare(
        "SELECT process_name, SUM(duration_secs) as total, COUNT(*) as entries
         FROM activity_log
         WHERE timestamp >= ?1
         GROUP BY process_name
         ORDER BY total DESC"
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
        });
    }

    // Calculate percentages
    for app in &mut apps {
        app.percentage = if total_seconds > 0 {
            (app.seconds as f64 / total_seconds as f64) * 100.0
        } else {
            0.0
        };
    }

    // Context switches: count consecutive process changes
    let mut switch_stmt = conn.prepare(
        "SELECT process_name FROM activity_log WHERE timestamp >= ?1 ORDER BY timestamp"
    )?;
    let processes: Vec<String> = switch_stmt
        .query_map(rusqlite::params![start_str], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut context_switches: u64 = 0;
    let mut longest_streak: u64 = 0;
    let mut longest_streak_app = String::new();
    let mut current_streak: u64 = 0;
    let mut current_app = String::new();
    let poll_interval = *state.poll_interval_secs.lock().unwrap();

    for p in &processes {
        if *p == current_app {
            current_streak += poll_interval;
        } else {
            if current_streak > longest_streak && !current_app.is_empty() {
                longest_streak = current_streak;
                longest_streak_app = current_app.clone();
            }
            current_app = p.clone();
            current_streak = poll_interval;
            if !current_app.is_empty() {
                context_switches += 1;
            }
        }
    }
    // Check last streak
    if current_streak > longest_streak && !current_app.is_empty() {
        longest_streak = current_streak;
        longest_streak_app = current_app;
    }

    Ok(ActivityReport {
        period: period_label,
        total_seconds,
        apps,
        context_switches,
        longest_streak_seconds: longest_streak,
        longest_streak_app: prettify_process_name(&longest_streak_app),
    })
}

fn prettify_process_name(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "code" | "code - insiders" => "VS Code".to_string(),
        "chrome" => "Chrome".to_string(),
        "firefox" => "Firefox".to_string(),
        "msedge" => "Edge".to_string(),
        "explorer" => "File Explorer".to_string(),
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
