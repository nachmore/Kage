//! Per-extension usage, breakers and pauses, kept on this machine in
//! `<data_local_dir>/kage/agent-calls-usage.json`
//! (docs/design/EXTENSION_AGENT_CALLS.md §5.6, §5.11, §5.13).
//!
//! Counts only, never text. Days are local calendar dates. A day's counts
//! are kept until the daily telemetry event has reported them, so a
//! restart doesn't lose them. Never backed up, never synced.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::AgentCallOutcome;

/// File format version.
const VERSION: u32 = 1;
/// Days kept per extension once reported (today and the unreported ones
/// are always kept).
const KEEP_DAYS: i64 = 3;

/// Why an extension is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// The failure breaker: several failures in a row.
    Failures,
    /// The tool breaker: tool attempts today.
    ToolAttempt,
    /// The user pressed **Stop now**.
    Stopped,
}

/// One extension's counts for one local day.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayUsage {
    /// `YYYY-MM-DD`, local.
    pub date: String,
    #[serde(default)]
    pub calls: u32,
    /// Prompt and reply characters.
    #[serde(default)]
    pub chars: u64,
    #[serde(default)]
    pub ok: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub cancelled: u32,
    #[serde(default)]
    pub tool_attempts: u32,
    /// The daily telemetry event has counted this day.
    #[serde(default)]
    pub reported: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionUsage {
    #[serde(default)]
    pub days: Vec<DayUsage>,
    /// Failures in a row (timeout, `not_json`, `invalid_response`,
    /// `response_too_large`, `agent_error`). A success resets it.
    #[serde(default)]
    pub failures_in_row: u32,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<PauseReason>,
}

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageBook {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub extensions: BTreeMap<String, ExtensionUsage>,
    /// **Stop now** for every extension. RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_paused_until: Option<String>,
}

/// What a finished call adds to the counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tally {
    Ok,
    Failed,
    Cancelled,
}

/// What [`UsageBook::charge`] did to the breakers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerChange {
    None,
    Opened(PauseReason),
}

pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

pub fn format_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn date_key(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

impl UsageBook {
    /// Read the file. Missing → empty. Corrupt → empty, with a warning;
    /// the next save replaces it.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                super::warn_line(format!(
                    "Background AI usage file unreadable, starting at zero: {e}"
                ));
                return Self::default();
            }
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(book) => book,
            Err(e) => {
                super::warn_line(format!(
                    "Background AI usage file is corrupt, starting at zero: {e}"
                ));
                Self::default()
            }
        }
    }

    /// Write the file atomically, owner-only.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut book = self.clone();
        book.version = VERSION;
        let json = serde_json::to_vec_pretty(&book).map_err(std::io::Error::other)?;
        crate::atomic_write::write(path, json, crate::atomic_write::Perms::Private)
    }

    pub fn day(&self, ext: &str, date: NaiveDate) -> DayUsage {
        let key = date_key(date);
        self.extensions
            .get(ext)
            .and_then(|e| e.days.iter().find(|d| d.date == key))
            .cloned()
            .unwrap_or(DayUsage {
                date: key,
                ..Default::default()
            })
    }

    fn day_mut(&mut self, ext: &str, date: NaiveDate) -> &mut DayUsage {
        let key = date_key(date);
        let e = self.extensions.entry(ext.to_string()).or_default();
        if let Some(i) = e.days.iter().position(|d| d.date == key) {
            &mut e.days[i]
        } else {
            e.days.push(DayUsage {
                date: key,
                ..Default::default()
            });
            e.days.last_mut().expect("just pushed")
        }
    }

    /// Calls every extension sent on `date`.
    pub fn global_calls(&self, date: NaiveDate) -> u32 {
        let key = date_key(date);
        self.extensions
            .values()
            .flat_map(|e| e.days.iter())
            .filter(|d| d.date == key)
            .map(|d| d.calls)
            .sum()
    }

    /// The pause in effect for `ext` at `now`: the later of its own pause
    /// and a pause of every extension.
    pub fn pause(&self, ext: &str, now: DateTime<Utc>) -> Option<(DateTime<Utc>, PauseReason)> {
        let own = self.extensions.get(ext).and_then(|e| {
            let until = parse_time(e.paused_until.as_deref()?)?;
            (until > now).then_some((until, e.pause_reason.unwrap_or(PauseReason::Stopped)))
        });
        let all = self
            .all_paused_until
            .as_deref()
            .and_then(parse_time)
            .filter(|t| *t > now)
            .map(|t| (t, PauseReason::Stopped));
        match (own, all) {
            (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    /// Pause `ext` until `until` (never shortens a longer pause).
    pub fn set_pause(&mut self, ext: &str, until: DateTime<Utc>, reason: PauseReason) {
        let e = self.extensions.entry(ext.to_string()).or_default();
        let longer = e
            .paused_until
            .as_deref()
            .and_then(parse_time)
            .is_none_or(|t| t < until);
        if longer {
            e.paused_until = Some(format_time(until));
            e.pause_reason = Some(reason);
        }
    }

    /// Pause every extension until `until`.
    pub fn set_pause_all(&mut self, until: DateTime<Utc>) {
        let longer = self
            .all_paused_until
            .as_deref()
            .and_then(parse_time)
            .is_none_or(|t| t < until);
        if longer {
            self.all_paused_until = Some(format_time(until));
        }
    }

    /// Count a call that was sent and update the breakers. `chars` is what
    /// was sent plus what came back. `midnight` is when the tool breaker's
    /// pause ends.
    pub fn charge(
        &mut self,
        ext: &str,
        date: NaiveDate,
        outcome: AgentCallOutcome,
        chars: u64,
        now: DateTime<Utc>,
        midnight: DateTime<Utc>,
    ) -> BreakerChange {
        let tool_attempts = {
            let day = self.day_mut(ext, date);
            day.calls += 1;
            day.chars += chars;
            match tally(outcome) {
                Tally::Ok => day.ok += 1,
                Tally::Failed => day.failed += 1,
                Tally::Cancelled => day.cancelled += 1,
            }
            if outcome == AgentCallOutcome::ToolAttempted {
                day.tool_attempts += 1;
            }
            day.tool_attempts
        };
        let e = self.extensions.entry(ext.to_string()).or_default();
        if outcome == AgentCallOutcome::Ok {
            e.failures_in_row = 0;
        } else if counts_as_failure(outcome) {
            e.failures_in_row += 1;
        }
        if outcome == AgentCallOutcome::ToolAttempted
            && tool_attempts >= super::limits::TOOL_BREAKER_THRESHOLD
        {
            self.set_pause(ext, midnight, PauseReason::ToolAttempt);
            return BreakerChange::Opened(PauseReason::ToolAttempt);
        }
        let e = self.extensions.entry(ext.to_string()).or_default();
        if e.failures_in_row >= super::limits::FAILURE_BREAKER_THRESHOLD {
            e.failures_in_row = 0;
            let until = now + chrono::Duration::minutes(super::limits::FAILURE_PAUSE_MINUTES);
            self.set_pause(ext, until, PauseReason::Failures);
            return BreakerChange::Opened(PauseReason::Failures);
        }
        BreakerChange::None
    }

    /// Past days with calls that telemetry hasn't counted yet, marked as
    /// counted. Then old counted days are dropped.
    pub fn take_unreported(&mut self, today: NaiveDate) -> Vec<(String, DayUsage)> {
        let today_key = date_key(today);
        let mut out = Vec::new();
        for (ext, e) in self.extensions.iter_mut() {
            for d in e.days.iter_mut() {
                if d.date < today_key && !d.reported && d.calls > 0 {
                    d.reported = true;
                    out.push((ext.clone(), d.clone()));
                }
            }
        }
        self.prune(today);
        out
    }

    /// Drop counted or empty days older than [`KEEP_DAYS`], and extensions
    /// with nothing left.
    pub fn prune(&mut self, today: NaiveDate) {
        let oldest = date_key(today - chrono::Duration::days(KEEP_DAYS));
        let today_key = date_key(today);
        for e in self.extensions.values_mut() {
            e.days.retain(|d| {
                d.date == today_key
                    || (d.date >= oldest && d.calls > 0)
                    || (!d.reported && d.calls > 0)
            });
        }
        let now_key = today_key;
        self.extensions.retain(|_, e| {
            !e.days.is_empty()
                || e.failures_in_row > 0
                || e.paused_until
                    .as_deref()
                    .is_some_and(|t| t >= now_key.as_str())
        });
    }
}

/// The outcomes that count toward the failure breaker.
pub fn counts_as_failure(outcome: AgentCallOutcome) -> bool {
    matches!(
        outcome,
        AgentCallOutcome::Timeout
            | AgentCallOutcome::NotJson
            | AgentCallOutcome::InvalidResponse
            | AgentCallOutcome::ResponseTooLarge
            | AgentCallOutcome::AgentError
    )
}

pub fn tally(outcome: AgentCallOutcome) -> Tally {
    match outcome {
        AgentCallOutcome::Ok => Tally::Ok,
        AgentCallOutcome::Cancelled => Tally::Cancelled,
        _ => Tally::Failed,
    }
}

/// `<data_local_dir>/kage/agent-calls-usage.json`.
pub fn default_path() -> Option<PathBuf> {
    Some(
        crate::paths::data_local_dir()?
            .join("kage")
            .join("agent-calls-usage.json"),
    )
}

/// `<data_local_dir>/kage/machine-id`.
pub fn default_machine_id_path() -> Option<PathBuf> {
    Some(
        crate::paths::data_local_dir()?
            .join("kage")
            .join("machine-id"),
    )
}

/// This machine's random id (128 bit, hex), created on first use. `None`
/// path, or a file that can't be read or written, gives a fresh random id
/// for this run.
pub fn machine_id(path: Option<&Path>) -> String {
    let fresh = || {
        format!(
            "{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        )
    };
    let Some(path) = path else {
        return fresh();
    };
    if let Ok(text) = std::fs::read_to_string(path) {
        let id = text.trim();
        if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return id.to_ascii_lowercase();
        }
    }
    let id = fresh();
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| crate::atomic_write::write(path, &id, crate::atomic_write::Perms::Private));
    if let Err(e) = saved {
        super::warn_line(format!(
            "Could not save the machine id for background AI: {e}"
        ));
    }
    id
}

/// First 16 hex digits of SHA-256(machine id + extension id): stable per
/// machine and extension, and not linkable across extensions.
pub fn machine_tag(machine_id: &str, extension_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{machine_id}{extension_id}").as_bytes());
    hex::encode(digest)[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn t(s: &str) -> DateTime<Utc> {
        parse_time(s).unwrap()
    }

    #[test]
    fn charge_counts_by_outcome() {
        let mut b = UsageBook::default();
        let now = t("2026-11-09T10:00:00Z");
        let mid = t("2026-11-10T00:00:00Z");
        let day = d("2026-11-09");
        b.charge("mm", day, AgentCallOutcome::Ok, 100, now, mid);
        b.charge("mm", day, AgentCallOutcome::Cancelled, 10, now, mid);
        b.charge("mm", day, AgentCallOutcome::ConnectionLost, 5, now, mid);
        let u = b.day("mm", day);
        assert_eq!(
            (u.calls, u.chars, u.ok, u.cancelled, u.failed),
            (3, 115, 1, 1, 1)
        );
        assert_eq!(b.global_calls(day), 3);
        b.charge("other", day, AgentCallOutcome::Ok, 1, now, mid);
        assert_eq!(b.global_calls(day), 4);
        assert_eq!(b.global_calls(d("2026-11-10")), 0);
    }

    #[test]
    fn three_failures_open_the_breaker_and_success_resets() {
        let mut b = UsageBook::default();
        let now = t("2026-11-09T10:00:00Z");
        let mid = t("2026-11-10T00:00:00Z");
        let day = d("2026-11-09");
        assert_eq!(
            b.charge("mm", day, AgentCallOutcome::Timeout, 1, now, mid),
            BreakerChange::None
        );
        assert_eq!(
            b.charge("mm", day, AgentCallOutcome::NotJson, 1, now, mid),
            BreakerChange::None
        );
        // A success resets the count.
        b.charge("mm", day, AgentCallOutcome::Ok, 1, now, mid);
        b.charge("mm", day, AgentCallOutcome::InvalidResponse, 1, now, mid);
        // Cancellations and lost connections don't count either way.
        b.charge("mm", day, AgentCallOutcome::Cancelled, 1, now, mid);
        b.charge("mm", day, AgentCallOutcome::ConnectionLost, 1, now, mid);
        b.charge("mm", day, AgentCallOutcome::ResponseTooLarge, 1, now, mid);
        assert!(b.pause("mm", now).is_none());
        assert_eq!(
            b.charge("mm", day, AgentCallOutcome::AgentError, 1, now, mid),
            BreakerChange::Opened(PauseReason::Failures)
        );
        let (until, why) = b.pause("mm", now).unwrap();
        assert_eq!(until, t("2026-11-09T10:30:00Z"));
        assert_eq!(why, PauseReason::Failures);
        assert!(b.pause("mm", t("2026-11-09T10:30:00Z")).is_none());
        assert!(b.pause("other", now).is_none());
    }

    #[test]
    fn two_tool_attempts_pause_until_midnight() {
        let mut b = UsageBook::default();
        let now = t("2026-11-09T10:00:00Z");
        let mid = t("2026-11-10T00:00:00Z");
        let day = d("2026-11-09");
        assert_eq!(
            b.charge("mm", day, AgentCallOutcome::ToolAttempted, 1, now, mid),
            BreakerChange::None
        );
        assert_eq!(
            b.charge("mm", day, AgentCallOutcome::ToolAttempted, 1, now, mid),
            BreakerChange::Opened(PauseReason::ToolAttempt)
        );
        assert_eq!(b.pause("mm", now), Some((mid, PauseReason::ToolAttempt)));
        assert_eq!(b.day("mm", day).tool_attempts, 2);
        // A new day starts the count again.
        let next = d("2026-11-10");
        assert_eq!(
            b.charge(
                "mm",
                next,
                AgentCallOutcome::ToolAttempted,
                1,
                mid,
                t("2026-11-11T00:00:00Z")
            ),
            BreakerChange::None
        );
    }

    #[test]
    fn pauses_take_the_later_end() {
        let mut b = UsageBook::default();
        let now = t("2026-11-09T10:00:00Z");
        b.set_pause("mm", t("2026-11-09T12:00:00Z"), PauseReason::ToolAttempt);
        b.set_pause("mm", t("2026-11-09T11:00:00Z"), PauseReason::Stopped);
        assert_eq!(b.pause("mm", now).unwrap().1, PauseReason::ToolAttempt);
        b.set_pause_all(t("2026-11-09T13:00:00Z"));
        assert_eq!(
            b.pause("mm", now),
            Some((t("2026-11-09T13:00:00Z"), PauseReason::Stopped))
        );
        assert!(b.pause("never-seen", now).is_some());
    }

    #[test]
    fn unreported_days_are_taken_once_and_pruned() {
        let mut b = UsageBook::default();
        let now = t("2026-11-09T10:00:00Z");
        let mid = t("2026-11-10T00:00:00Z");
        b.charge("mm", d("2026-11-08"), AgentCallOutcome::Ok, 5_000, now, mid);
        b.charge("mm", d("2026-11-09"), AgentCallOutcome::Ok, 1, now, mid);
        let taken = b.take_unreported(d("2026-11-09"));
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].0, "mm");
        assert_eq!(taken[0].1.date, "2026-11-08");
        assert!(b.take_unreported(d("2026-11-09")).is_empty());
        // Today is never reported early.
        let taken = b.take_unreported(d("2026-11-10"));
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].1.date, "2026-11-09");
        // Old counted days go.
        b.take_unreported(d("2026-11-20"));
        assert!(!b.extensions.contains_key("mm"));
    }

    #[test]
    fn file_round_trip_and_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kage").join("agent-calls-usage.json");
        assert_eq!(UsageBook::load(&path), UsageBook::default());
        let mut b = UsageBook::default();
        b.charge(
            "mm",
            d("2026-11-09"),
            AgentCallOutcome::Ok,
            42,
            t("2026-11-09T10:00:00Z"),
            t("2026-11-10T00:00:00Z"),
        );
        b.set_pause_all(t("2026-11-09T11:00:00Z"));
        b.save(&path).unwrap();
        let back = UsageBook::load(&path);
        assert_eq!(back.extensions, b.extensions);
        assert_eq!(back.all_paused_until, b.all_paused_until);
        assert_eq!(back.version, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(UsageBook::load(&path), UsageBook::default());
    }

    #[test]
    fn machine_id_is_created_once_and_tags_differ_per_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kage").join("machine-id");
        let id = machine_id(Some(&path));
        assert_eq!(id.len(), 32);
        assert_eq!(machine_id(Some(&path)), id);
        let a = machine_tag(&id, "mindmap");
        let b = machine_tag(&id, "todo");
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
        assert_eq!(machine_tag(&id, "mindmap"), a);
        assert_ne!(machine_tag("0".repeat(32).as_str(), "mindmap"), a);
        std::fs::write(&path, "garbage").unwrap();
        let replaced = machine_id(Some(&path));
        assert_ne!(replaced, id);
        assert_eq!(machine_id(Some(&path)), replaced);
    }
}
