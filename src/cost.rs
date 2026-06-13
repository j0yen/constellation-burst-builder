//! Cost tracking and budget enforcement for burst pods.
//!
//! Every pod lifecycle (create/run/destroy + cost estimate) is appended to
//! a cost log. Exceeding the monthly cap blocks new pods.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A single job entry in the cost log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEntry {
    /// Unique job identifier.
    pub job_id: String,
    /// Where the job ran: `"standing-box"`, `"pod:<id>"`, or `"local"`.
    pub ran_on: String,
    /// ISO 8601 start timestamp.
    pub started_at: DateTime<Utc>,
    /// ISO 8601 end timestamp (`None` if still running).
    pub ended_at: Option<DateTime<Utc>>,
    /// Elapsed seconds (`None` if still running).
    pub elapsed_secs: Option<f64>,
    /// Estimated cost in USD (`0.0` for standing-box/local, non-zero for pods).
    pub cost_usd: f64,
    /// Human-readable description of the job.
    pub description: String,
}

impl JobEntry {
    /// Compute elapsed seconds if both timestamps are present.
    #[must_use]
    #[allow(dead_code)] // used in mock tests (tests/mocks/ac5.rs)
    pub fn compute_elapsed(&self) -> Option<f64> {
        self.ended_at.map(|end| {
            let millis = (end - self.started_at).num_milliseconds();
            // Allow small rounding; milliseconds fit in f64 precision for any reasonable duration.
            // Allow float arithmetic: time-to-secs conversion.
            #[allow(
                clippy::cast_precision_loss,
                clippy::float_arithmetic,
                clippy::as_conversions
            )]
            { millis as f64 / 1000.0 }
        })
    }
}

/// Persistent cost log stored as newline-delimited JSON.
#[derive(Debug, Default)]
pub struct CostLog {
    entries: Vec<JobEntry>,
    path: PathBuf,
}

impl CostLog {
    /// Default path: `~/.local/share/wm-burst/cost-log.ndjson`.
    ///
    /// # Errors
    /// Returns an error if the home directory cannot be determined.
    pub fn default_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        Ok(home.join(".local").join("share").join("wm-burst").join("cost-log.ndjson"))
    }

    /// Load a cost log from `path`.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or if any line is invalid JSON.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                entries: Vec::new(),
                path: path.to_owned(),
            });
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read cost log at {}", path.display()))?;
        let entries: Result<Vec<JobEntry>, _> = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect();
        let entries = entries.context("invalid cost log entry; log may be corrupted")?;
        Ok(Self { entries, path: path.to_owned() })
    }

    /// Append a job entry to the log (appends one line to the file).
    ///
    /// # Errors
    /// Returns an error if the parent directory cannot be created or if the file cannot be written.
    pub fn append(&mut self, entry: JobEntry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create cost log dir {}", parent.display()))?;
        }
        let line = serde_json::to_string(&entry).context("cannot serialize job entry")?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("cannot open cost log at {}", self.path.display()))?;
        writeln!(f, "{line}").context("cannot write to cost log")?;
        self.entries.push(entry);
        Ok(())
    }

    /// Sum of pod costs in the current calendar month (UTC).
    #[must_use]
    pub fn month_to_date_usd(&self) -> f64 {
        let now = Utc::now();
        self.entries
            .iter()
            .filter(|e| {
                e.started_at.year() == now.year() && e.started_at.month() == now.month()
            })
            .map(|e| e.cost_usd)
            .sum()
    }

    /// Last `n` job entries (most recent last).
    #[must_use]
    pub fn last_n(&self, n: usize) -> &[JobEntry] {
        let len = self.entries.len();
        if n >= len {
            &self.entries
        } else {
            // len > n, so len - n is in range [0, len); get(start..) is always Some.
            self.entries.get(len - n..).unwrap_or(&self.entries)
        }
    }

    /// All entries (for status display).
    #[must_use]
    #[allow(dead_code)] // used in tests
    pub fn entries(&self) -> &[JobEntry] {
        &self.entries
    }

    /// Mutable access to entries (used in property tests only).
    #[must_use]
    #[allow(dead_code)] // used in proptest_invariants.rs
    pub fn entries_mut(&mut self) -> &mut Vec<JobEntry> {
        &mut self.entries
    }
}

// ---------------------------------------------------------------------------
// Hub standing-cost tracking (harbor-thrift)
// ---------------------------------------------------------------------------

/// Per-type hourly rate in USD for Hetzner server types.
///
/// Returns a conservative default rate for unknown types.
#[must_use]
pub fn hourly_rate_usd(server_type: &str) -> f64 {
    match server_type {
        "cx22"  => 0.006,
        "cpx11" => 0.005,
        "ccx53" => 0.028,
        _       => 0.030,
    }
}

/// Whether a server type is a known type with a fixed rate.
#[must_use]
pub fn is_known_server_type(server_type: &str) -> bool {
    matches!(server_type, "cx22" | "cpx11" | "ccx53")
}

/// A hub lifecycle span parsed from the cost log.
#[derive(Debug, Clone)]
pub struct HubEntry {
    /// When the hub came up.
    pub up_at: DateTime<Utc>,
    /// When the hub went down (`None` = still running).
    pub down_at: Option<DateTime<Utc>>,
    /// Server type extracted from the `HUB-UP` log line.
    pub server_type: String,
}

/// Full cost report for the current calendar month.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CostReport {
    /// Total burst pod spend this month (USD).
    pub burst_usd: f64,
    /// Hub standing charge this month (USD).
    pub hub_standing_usd: f64,
    /// Month-to-date total (burst + hub standing).
    pub month_to_date_usd: f64,
    /// Projected month-end total (month-to-date + hub rate × remaining hours).
    pub month_projection_usd: f64,
    /// Budget cap from config.
    pub budget_usd: f64,
    /// True if `month_projection_usd > budget_usd`.
    pub over_cap: bool,
    /// Non-empty if any hub entry used an estimated rate (unknown server type).
    pub rate_notes: Vec<String>,
}

/// Parse `cost.log` entries into `HubEntry` spans.
///
/// Matches `HUB-UP` entries with subsequent `HUB-DOWN` entries by hub id.
/// A `HUB-UP` with no matching `HUB-DOWN` has `down_at = None`.
#[must_use]
pub fn parse_hub_entries(entries: &[JobEntry]) -> Vec<HubEntry> {
    // First pass: collect all HUB-UP entries.
    let mut pending: Vec<(String, DateTime<Utc>, String)> = Vec::new(); // (hub_id, up_at, server_type)
    let mut completed: Vec<HubEntry> = Vec::new();

    for entry in entries {
        if entry.job_id.starts_with("HUB-UP") {
            // Extract hub_id and server_type from job_id: "HUB-UP id=<id> type=<t> ip=<ip>"
            let hub_id = extract_field(&entry.job_id, "id=").unwrap_or_default();
            let server_type = extract_field(&entry.job_id, "type=").unwrap_or("unknown".into());
            pending.push((hub_id, entry.started_at, server_type));
        } else if entry.job_id.starts_with("HUB-DOWN") {
            // "HUB-DOWN id=<id>"
            let down_id = extract_field(&entry.job_id, "id=").unwrap_or_default();
            // Match against a pending HUB-UP.
            if let Some(idx) = pending.iter().position(|(id, _, _)| *id == down_id) {
                let (_, up_at, server_type) = pending.remove(idx);
                completed.push(HubEntry { up_at, down_at: Some(entry.started_at), server_type });
            }
        }
    }

    // Remaining pending HUB-UP entries have no matching HUB-DOWN (still running).
    for (_, up_at, server_type) in pending {
        completed.push(HubEntry { up_at, down_at: None, server_type });
    }

    completed
}

/// Extract `value` from a `key=value` pair in a space-delimited string.
/// Stops at the next space or end of string.
fn extract_field(s: &str, prefix: &str) -> Option<String> {
    let start = s.find(prefix)? + prefix.len();
    let rest = &s[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

/// Compute a `CostReport` from the entries in a cost log.
///
/// `now` is injected for determinism in tests.
///
/// # Errors
/// Never fails (returns a zeroed report on an empty log), but signature uses
/// `Result` for forward compatibility.
pub fn compute_cost_report(
    log: &CostLog,
    now: DateTime<Utc>,
    budget_usd: f64,
) -> CostReport {
    use chrono::{Datelike, NaiveDate, TimeZone};

    // Burst spend: sum cost_usd for non-hub entries in the current calendar month.
    let burst_usd: f64 = log
        .entries()
        .iter()
        .filter(|e| {
            !e.job_id.starts_with("HUB-UP") && !e.job_id.starts_with("HUB-DOWN")
        })
        .filter(|e| e.started_at.year() == now.year() && e.started_at.month() == now.month())
        .map(|e| e.cost_usd)
        .sum();

    // Hub standing cost.
    let hub_entries = parse_hub_entries(log.entries());

    let mut hub_standing_usd: f64 = 0.0;
    let mut hub_hourly_rate: f64 = 0.0;
    let mut rate_notes: Vec<String> = Vec::new();

    // Month boundaries (UTC).
    let month_start = Utc
        .from_utc_datetime(
            &NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                .unwrap_or(NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap_or_default())
                .and_hms_opt(0, 0, 0)
                .unwrap_or_default(),
        );
    // Days in the current month: try day 31, then 30, then 28 as fallback.
    let days_in_month = days_in_month(now.year(), now.month());
    let month_end = month_start + chrono::Duration::days(i64::from(days_in_month));

    for hub in &hub_entries {
        // Only count time within the current calendar month.
        let effective_start = hub.up_at.max(month_start);
        let effective_end = hub.down_at.unwrap_or(now).min(month_end);

        if effective_end <= effective_start {
            continue;
        }

        let rate = hourly_rate_usd(&hub.server_type);
        if !is_known_server_type(&hub.server_type) {
            rate_notes.push(format!("rate estimated for type={}", hub.server_type));
        }

        // Clamp to within this month.
        let secs = (effective_end - effective_start).num_seconds().max(0);
        // Allow float arithmetic for time-to-cost conversion.
        #[allow(clippy::float_arithmetic, clippy::cast_precision_loss, clippy::as_conversions)]
        let cost = (secs as f64 / 3600.0) * rate;
        hub_standing_usd += cost;

        // For projection: use the rate of still-running hubs.
        if hub.down_at.is_none() {
            hub_hourly_rate += rate;
        }
    }

    let month_to_date_usd = burst_usd + hub_standing_usd;

    // Remaining hours in the month.
    let remaining_secs = (month_end - now).num_seconds().max(0);
    #[allow(clippy::float_arithmetic, clippy::cast_precision_loss, clippy::as_conversions)]
    let remaining_hours = remaining_secs as f64 / 3600.0;

    #[allow(clippy::float_arithmetic)]
    let month_projection_usd = month_to_date_usd + hub_hourly_rate * remaining_hours;

    let over_cap = month_projection_usd > budget_usd;

    // Deduplicate rate_notes.
    rate_notes.dedup();

    CostReport {
        burst_usd,
        hub_standing_usd,
        month_to_date_usd,
        month_projection_usd,
        budget_usd,
        over_cap,
        rate_notes,
    }
}

/// Number of days in a given calendar month.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) {
                29
            } else {
                28
            }
        }
        _ => 30, // unreachable for valid months
    }
}

/// Check whether a new pod job is allowed under `monthly_budget_usd`.
///
/// Returns `Ok(())` if the current month-to-date spend is below the cap,
/// or `Err` with a budget-exceeded message.
///
/// # Errors
/// Returns an error if the monthly budget cap has been reached or exceeded.
pub fn check_budget(log: &CostLog, monthly_budget_usd: f64) -> Result<()> {
    let spent = log.month_to_date_usd();
    if spent >= monthly_budget_usd {
        anyhow::bail!(
            "monthly burst budget exceeded: spent ${spent:.2} of ${monthly_budget_usd:.2} cap; \
             adjust monthly_budget_usd in config or wait until next month"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(cost: f64, days_ago: i64) -> JobEntry {
        let started = Utc::now() - chrono::Duration::days(days_ago);
        JobEntry {
            job_id: "test".into(),
            ran_on: "pod:test".into(),
            started_at: started,
            ended_at: Some(started + chrono::Duration::seconds(60)),
            elapsed_secs: Some(60.0),
            cost_usd: cost,
            description: "test job".into(),
        }
    }

    #[test]
    fn month_to_date_sums_current_month_only() {
        let mut log = CostLog::default();
        log.entries.push(make_entry(5.0, 0));    // this month
        log.entries.push(make_entry(3.0, 1));    // this month
        log.entries.push(make_entry(100.0, 45)); // last month (>31 days ago)
        let mtd = log.month_to_date_usd();
        // 5 + 3 = 8; the 100 from 45 days ago is excluded
        assert!((mtd - 8.0).abs() < 0.001, "expected 8.0, got {mtd}");
    }

    #[test]
    fn budget_check_allows_under_cap() {
        let log = CostLog::default();
        check_budget(&log, 50.0).unwrap();
    }

    #[test]
    fn budget_check_blocks_at_cap() {
        let mut log = CostLog::default();
        log.entries.push(make_entry(50.0, 0));
        let err = check_budget(&log, 50.0).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("exceeded"), "expected budget exceeded: {msg}");
    }

    #[test]
    fn last_n_returns_tail() {
        let mut log = CostLog::default();
        for i in 0..5_u32 {
            log.entries.push(make_entry(f64::from(i), 0));
        }
        let tail = log.last_n(3);
        assert_eq!(tail.len(), 3);
        assert!((tail[0].cost_usd - 2.0).abs() < 0.001);
    }
}
