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
