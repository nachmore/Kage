//! Automation scheduler — manages triggers for automations (macros).
//!
//! Runs as a background tokio task. Handles:
//! - Schedule-based triggers (interval, daily, weekdays)
//! - Signal-based triggers (from extensions or system events)
//! - Battery-aware throttling

use crate::config::{AutomationTrigger, AutomationPowerConfig, MacroConfig};
use crate::os::power::{PowerState, get_power_state};
use log::{info, debug};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// A signal emitted by an extension or the system.
#[derive(Debug, Clone)]
pub struct AutomationSignal {
    pub name: String,
    pub data: Option<serde_json::Value>,
}

/// Max number of signals the scheduler will buffer before applying backpressure.
/// A misbehaving extension flooding signals faster than the scheduler can consume
/// them will have the oldest/newest dropped rather than ballooning memory.
const SIGNAL_CHANNEL_CAPACITY: usize = 256;

/// Manages automation scheduling and signal dispatch.
pub struct AutomationScheduler {
    /// Channel to send signals into the scheduler
    signal_tx: mpsc::Sender<AutomationSignal>,
    /// Shared config reference
    config: Arc<Mutex<crate::config::Config>>,
    /// Track last run times for schedule-based automations
    last_runs: Arc<Mutex<HashMap<String, chrono::DateTime<chrono::Local>>>>,
    /// Whether the scheduler is running
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl AutomationScheduler {
    pub fn new(config: Arc<Mutex<crate::config::Config>>) -> (Self, mpsc::Receiver<AutomationSignal>) {
        let (tx, rx) = mpsc::channel(SIGNAL_CHANNEL_CAPACITY);
        (AutomationScheduler {
            signal_tx: tx,
            config,
            last_runs: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }, rx)
    }

    /// Get a sender handle for emitting signals (clone-friendly).
    pub fn signal_sender(&self) -> mpsc::Sender<AutomationSignal> {
        self.signal_tx.clone()
    }

    /// Start the scheduler loop. Call from a tokio::spawn.
    pub async fn run(&self, mut signal_rx: mpsc::Receiver<AutomationSignal>, app_handle: tauri::AppHandle) {
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);
        crate::os::set_current_thread_name("automation");
        info!("[Automation] Scheduler started");

        let mut schedule_interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                _ = schedule_interval.tick() => {
                    self.check_schedules(&app_handle).await;
                }
                signal = signal_rx.recv() => {
                    match signal {
                        Some(sig) => self.handle_signal(&sig, &app_handle).await,
                        None => {
                            info!("[Automation] Signal channel closed, stopping scheduler");
                            break;
                        }
                    }
                }
            }
        }

        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Check all schedule-based automations and fire any that are due.
    async fn check_schedules(&self, app_handle: &tauri::AppHandle) {
        let (macros, power_config) = {
            let config = self.config.lock().unwrap();
            (config.macros.clone(), config.automation_power.clone())
        };

        let power_state = get_power_state();
        let multiplier = get_throttle_multiplier(&power_config, power_state);

        let now = chrono::Local::now();

        for mac in &macros {
            if !mac.enabled { continue; }
            if let AutomationTrigger::Schedule { ref interval, .. } = mac.trigger {
                if interval.is_empty() { continue; }
                let interval_secs = parse_interval(interval);
                if interval_secs == 0 { continue; }

                // Apply battery throttle
                let effective_interval = (interval_secs as f32 * multiplier) as i64;

                let should_run = {
                    let last_runs = self.last_runs.lock().unwrap();
                    match last_runs.get(&mac.name) {
                        Some(last) => (now - *last).num_seconds() >= effective_interval,
                        None => true,
                    }
                };

                if should_run {
                    // Check daily/weekday time constraints
                    if !check_time_constraint(interval, &now) { continue; }

                    info!("[Automation] Schedule trigger firing: {}", mac.name);
                    self.last_runs.lock().unwrap().insert(mac.name.clone(), now);
                    fire_automation(app_handle, mac, None);
                }
            }
        }
    }

    /// Handle an incoming signal and fire matching automations.
    async fn handle_signal(&self, signal: &AutomationSignal, app_handle: &tauri::AppHandle) {
        let (macros, power_config) = {
            let config = self.config.lock().unwrap();
            (config.macros.clone(), config.automation_power.clone())
        };

        let power_state = get_power_state();
        if power_config.disable_signals_on_low_battery && power_state == PowerState::LowBattery {
            debug!("[Automation] Skipping signal '{}' — low battery", signal.name);
            return;
        }

        for mac in &macros {
            if !mac.enabled { continue; }
            if let AutomationTrigger::Signal { signal: ref sig_name, ref filter } = mac.trigger {
                if sig_name == &signal.name {
                    // Check filter if present
                    if let Some(f) = filter {
                        if !f.is_empty() {
                            let data_str = signal.data.as_ref()
                                .map(|d| d.to_string())
                                .unwrap_or_default();
                            if !data_str.to_lowercase().contains(&f.to_lowercase()) {
                                continue;
                            }
                        }
                    }
                    info!("[Automation] Signal trigger firing: {} (signal: {})", mac.name, signal.name);
                    fire_automation(app_handle, mac, signal.data.clone());
                }
            }
        }
    }
}

/// Fire an automation by emitting a Tauri event that the frontend handles.
fn fire_automation(app_handle: &tauri::AppHandle, mac: &MacroConfig, data: Option<serde_json::Value>) {
    use tauri::Emitter;
    let payload = serde_json::json!({
        "name": mac.name,
        "icon": mac.icon,
        "steps": mac.steps,
        "output": mac.output,
        "trigger_data": data,
    });
    let _ = app_handle.emit("automation_triggered", payload);
}

/// Parse an interval string into the check frequency in seconds.
/// The scheduler checks at this frequency; time-based triggers do their own matching.
fn parse_interval(interval: &str) -> i64 {
    if interval.starts_with("every_") {
        let rest = &interval[6..];
        if let Some(m) = rest.strip_suffix('m') {
            return m.parse::<i64>().unwrap_or(0) * 60;
        }
        if let Some(h) = rest.strip_suffix('h') {
            return h.parse::<i64>().unwrap_or(0) * 3600;
        }
        if let Some(s) = rest.strip_suffix('s') {
            return s.parse::<i64>().unwrap_or(0);
        }
    }
    // Hourly: "hourly_N" or "hourly_N_at_MM"
    if interval.starts_with("hourly_") {
        let rest = &interval[7..];
        let hours_str = rest.split('_').next().unwrap_or("1");
        return hours_str.parse::<i64>().unwrap_or(1) * 3600;
    }
    // Daily, monthly, yearly — check every 60 seconds
    if interval.starts_with("daily_") || interval.starts_with("weekdays_")
        || interval.starts_with("monthly_") || interval.starts_with("yearly_") {
        return 60;
    }
    0
}

/// For daily/weekday schedules, check if the current time matches.
fn check_time_constraint(interval: &str, now: &chrono::DateTime<chrono::Local>) -> bool {
    if let Some(time_str) = interval.strip_prefix("daily_") {
        return check_time_match(time_str, now);
    }
    if let Some(time_str) = interval.strip_prefix("weekdays_") {
        use chrono::Datelike;
        let weekday = now.weekday();
        if weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun {
            return false;
        }
        return check_time_match(time_str, now);
    }
    true // non-time-constrained intervals always pass
}


fn check_time_match(time_str: &str, now: &chrono::DateTime<chrono::Local>) -> bool {
    use chrono::Timelike;
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() == 2 {
        if let (Ok(h), Ok(m)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            // Match within a 1-minute window
            return now.hour() == h && now.minute() == m;
        }
    }
    false
}

/// Get the throttle multiplier based on power state and config.
fn get_throttle_multiplier(config: &AutomationPowerConfig, state: PowerState) -> f32 {
    match config.mode.as_str() {
        "full" => 1.0,
        "saving" => config.low_battery_multiplier,
        _ => { // "auto"
            match state {
                PowerState::AC | PowerState::Unknown => 1.0,
                PowerState::Battery => config.battery_multiplier,
                PowerState::LowBattery => config.low_battery_multiplier,
            }
        }
    }
}

/// Tauri command: emit a signal from the frontend (extensions call this).
#[tauri::command]
pub async fn emit_automation_signal(
    name: String,
    data: Option<serde_json::Value>,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    if let Some(ref tx) = *state.automation_signal_tx.lock().unwrap() {
        // Use try_send so a flood of signals from a misbehaving extension drops
        // rather than blocking the Tauri IPC thread or growing memory.
        match tx.try_send(AutomationSignal { name: name.clone(), data }) {
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                log::warn!("[Automation] Signal channel full, dropping signal '{}'", name);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                log::debug!("[Automation] Signal channel closed, ignoring '{}'", name);
            }
        }
    }
    Ok(())
}

/// Tauri command: get current power state.
#[tauri::command]
pub async fn get_power_status() -> Result<serde_json::Value, String> {
    let state = get_power_state();
    Ok(serde_json::json!({
        "state": match state {
            PowerState::AC => "ac",
            PowerState::Battery => "battery",
            PowerState::LowBattery => "low_battery",
            PowerState::Unknown => "unknown",
        }
    }))
}

/// Tauri command: list available signals from all loaded extensions.
#[tauri::command]
pub async fn list_automation_signals() -> Result<Vec<serde_json::Value>, String> {
    // Built-in system signals
    let signals = vec![
        serde_json::json!({ "name": "system:clipboard_change", "description": "Clipboard content changed", "source": "System" }),
        serde_json::json!({ "name": "system:window_focus", "description": "A window gained focus", "source": "System" }),
        serde_json::json!({ "name": "system:idle_5m", "description": "System idle for 5 minutes", "source": "System" }),
        serde_json::json!({ "name": "system:resume", "description": "System resumed from sleep", "source": "System" }),
    ];
    // Extension signals are added dynamically by the frontend
    Ok(signals)
}
