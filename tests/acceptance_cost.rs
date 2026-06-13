//! Acceptance tests for `wm-burst cost` (harbor-thrift).
//!
//! AC1: cargo build + test pass (structural — covered by being compiled at all)
//! AC2: fixture log with UP/DOWN + HUB-UP parses to burst_usd > 0 and hub_standing_usd > 0
//! AC3: still-running hub (HUB-UP, no HUB-DOWN) charged from HUB-UP timestamp to injected now
//! AC4: month_projection_usd = month-to-date + rate × remaining hours (deterministic)
//! AC5: over_cap true iff projection > budget; exit non-zero when over_cap
//! AC6: --json round-trip
//! AC7: unknown server type uses conservative default rate; report notes "rate estimated"
//! AC8: burst-only log (no HUB-* lines) → hub_standing_usd == 0

use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use wm_burst::cost::{CostLog, JobEntry, compute_cost_report};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_burst_entry(cost: f64, ts: chrono::DateTime<Utc>) -> JobEntry {
    JobEntry {
        job_id: format!("burst-{}", ts.timestamp()),
        ran_on: "pod:test".into(),
        started_at: ts,
        ended_at: Some(ts + chrono::Duration::seconds(60)),
        elapsed_secs: Some(60.0),
        cost_usd: cost,
        description: "burst job".into(),
    }
}

fn make_hub_up_entry(hub_id: &str, server_type: &str, ts: chrono::DateTime<Utc>) -> JobEntry {
    JobEntry {
        job_id: format!("HUB-UP id={hub_id} type={server_type} ip=192.0.2.1"),
        ran_on: format!("hub:{hub_id}"),
        started_at: ts,
        ended_at: None,
        elapsed_secs: None,
        cost_usd: 0.0,
        description: format!("HUB-UP id={hub_id} type={server_type} ip=192.0.2.1"),
    }
}

fn make_hub_down_entry(hub_id: &str, ts: chrono::DateTime<Utc>) -> JobEntry {
    JobEntry {
        job_id: format!("HUB-DOWN id={hub_id}"),
        ran_on: format!("hub:{hub_id}"),
        started_at: ts,
        ended_at: Some(ts),
        elapsed_secs: Some(0.0),
        cost_usd: 0.0,
        description: format!("HUB-DOWN id={hub_id}"),
    }
}

// A "now" fixed in the middle of the month: 2025-06-15 12:00:00 UTC
fn fixed_now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0)
        .single()
        .expect("fixed_now")
}

fn month_start() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0)
        .single()
        .expect("month_start")
}

// ---------------------------------------------------------------------------
// AC2: fixture with burst + HUB-UP/HUB-DOWN → both streams > 0
// ---------------------------------------------------------------------------

#[test]
fn ac2_burst_and_hub_both_nonzero() {
    let now = fixed_now();

    // Burst: 2 USD earlier this month.
    let burst_ts = month_start() + chrono::Duration::days(2);

    // Hub: up 10 days ago, down 5 days ago = 5 days * cpx11 rate.
    let hub_up_ts = now - chrono::Duration::days(10);
    let hub_down_ts = now - chrono::Duration::days(5);

    let mut log = CostLog::default();
    log.entries_mut().push(make_burst_entry(2.0, burst_ts));
    log.entries_mut().push(make_hub_up_entry("h1", "cpx11", hub_up_ts));
    log.entries_mut().push(make_hub_down_entry("h1", hub_down_ts));

    let report = compute_cost_report(&log, now, 100.0);

    assert!(report.burst_usd > 0.0, "burst_usd must be > 0, got {}", report.burst_usd);
    assert!(
        report.hub_standing_usd > 0.0,
        "hub_standing_usd must be > 0, got {}",
        report.hub_standing_usd
    );
    assert!(report.month_to_date_usd > 0.0, "month_to_date_usd must be > 0");
}

// ---------------------------------------------------------------------------
// AC3: still-running hub charged from HUB-UP to injected now
// ---------------------------------------------------------------------------

#[test]
fn ac3_still_running_hub_charged_to_now() {
    let now = fixed_now();

    // Hub came up exactly 2 hours before now.
    let hub_up_ts = now - chrono::Duration::hours(2);

    let mut log = CostLog::default();
    log.entries_mut().push(make_hub_up_entry("h2", "cpx11", hub_up_ts));
    // No HUB-DOWN — still running.

    let report = compute_cost_report(&log, now, 100.0);

    // cpx11 rate = $0.005/hr; 2 hours = $0.010
    let expected = 2.0 * 0.005;
    assert!(
        (report.hub_standing_usd - expected).abs() < 0.0001,
        "expected hub_standing_usd ≈ {expected:.4}, got {:.4}",
        report.hub_standing_usd
    );
}

// ---------------------------------------------------------------------------
// AC4: month_projection_usd is deterministic with fixed now + known rate
// ---------------------------------------------------------------------------

#[test]
fn ac4_month_projection_deterministic() {
    // now = 2025-06-15 12:00:00 UTC
    // June has 30 days; remaining = 15 days 12 hours = 372 hours
    let now = fixed_now();
    let hub_up_ts = now - chrono::Duration::hours(1);

    let mut log = CostLog::default();
    log.entries_mut().push(make_hub_up_entry("h3", "cpx11", hub_up_ts));

    let report = compute_cost_report(&log, now, 100.0);

    // hub_standing_usd = 1hr × $0.005 = $0.005
    // remaining hours = from 2025-06-15 12:00 to 2025-07-01 00:00
    //   = 15 days 12 hours = 372 hours
    // month_projection = 0.005 + 0.005 * 372 = 0.005 + 1.860 = 1.865
    let expected_hub_mtd = 1.0 * 0.005;
    let expected_remaining_hrs = 372.0_f64;
    let expected_projection = expected_hub_mtd + 0.005 * expected_remaining_hrs;

    assert!(
        (report.hub_standing_usd - expected_hub_mtd).abs() < 0.001,
        "hub_standing_usd mismatch: expected {expected_hub_mtd:.4}, got {:.4}",
        report.hub_standing_usd
    );
    assert!(
        (report.month_projection_usd - expected_projection).abs() < 0.1,
        "month_projection_usd mismatch: expected ≈ {expected_projection:.4}, got {:.4}",
        report.month_projection_usd
    );
}

// ---------------------------------------------------------------------------
// AC5: over_cap when projection > budget
// ---------------------------------------------------------------------------

#[test]
fn ac5_over_cap_when_projection_exceeds_budget() {
    let now = fixed_now();
    // Hub up since start of month — ensures large projection.
    let hub_up_ts = month_start();

    let mut log = CostLog::default();
    log.entries_mut().push(make_hub_up_entry("h4", "cx22", hub_up_ts));

    // Budget of 0.001 — anything will exceed it.
    let report = compute_cost_report(&log, now, 0.001);
    assert!(
        report.over_cap,
        "over_cap must be true when projection > budget; projection={:.4} budget={:.4}",
        report.month_projection_usd,
        report.budget_usd
    );
}

#[test]
fn ac5_not_over_cap_when_under_budget() {
    let now = fixed_now();

    let log = CostLog::default();

    // Budget of 1000 — empty log won't exceed.
    let report = compute_cost_report(&log, now, 1000.0);
    assert!(!report.over_cap, "over_cap must be false on empty log with large budget");
}

// ---------------------------------------------------------------------------
// AC6: --json round-trip (unit test: serde round-trip of CostReport)
// ---------------------------------------------------------------------------

#[test]
fn ac6_json_round_trip() {
    let now = fixed_now();
    let hub_up_ts = now - chrono::Duration::hours(3);

    let mut log = CostLog::default();
    log.entries_mut().push(make_burst_entry(1.5, month_start() + chrono::Duration::days(1)));
    log.entries_mut().push(make_hub_up_entry("h5", "cpx11", hub_up_ts));

    let original = compute_cost_report(&log, now, 50.0);

    // Serialize then deserialize.
    let json = serde_json::to_string(&original).expect("serialize");
    let parsed: wm_burst::cost::CostReport = serde_json::from_str(&json).expect("deserialize");

    assert!((original.burst_usd - parsed.burst_usd).abs() < 1e-9, "burst_usd mismatch after round-trip");
    assert!(
        (original.hub_standing_usd - parsed.hub_standing_usd).abs() < 1e-9,
        "hub_standing_usd mismatch after round-trip"
    );
    assert!((original.month_to_date_usd - parsed.month_to_date_usd).abs() < 1e-9);
    assert!((original.month_projection_usd - parsed.month_projection_usd).abs() < 1e-9);
    assert!((original.budget_usd - parsed.budget_usd).abs() < 1e-9);
    assert_eq!(original.over_cap, parsed.over_cap);
}

// ---------------------------------------------------------------------------
// AC7: unknown server type uses conservative rate, notes "rate estimated"
// ---------------------------------------------------------------------------

#[test]
fn ac7_unknown_server_type_uses_conservative_rate_and_notes() {
    let now = fixed_now();
    let hub_up_ts = now - chrono::Duration::hours(1);

    let mut log = CostLog::default();
    log.entries_mut().push(make_hub_up_entry("h6", "cx999-turbo", hub_up_ts));

    let report = compute_cost_report(&log, now, 100.0);

    // Default rate = $0.030/hr; 1 hour = $0.030
    let expected = 1.0 * 0.030;
    assert!(
        (report.hub_standing_usd - expected).abs() < 0.001,
        "hub_standing_usd with unknown type: expected ≈ {expected:.4}, got {:.4}",
        report.hub_standing_usd
    );

    assert!(
        report.rate_notes.iter().any(|n| n.contains("rate estimated for type=cx999-turbo")),
        "rate_notes must mention 'rate estimated for type=cx999-turbo'; got: {:?}",
        report.rate_notes
    );
}

// ---------------------------------------------------------------------------
// AC8: burst-only log → hub_standing_usd == 0 (no regression)
// ---------------------------------------------------------------------------

#[test]
fn ac8_burst_only_log_hub_standing_zero() {
    let now = fixed_now();
    let ts = month_start() + chrono::Duration::days(1);

    let mut log = CostLog::default();
    log.entries_mut().push(make_burst_entry(3.0, ts));
    log.entries_mut().push(make_burst_entry(2.0, ts + chrono::Duration::hours(1)));

    let report = compute_cost_report(&log, now, 100.0);

    assert!(
        report.hub_standing_usd.abs() < 1e-9,
        "hub_standing_usd must be 0.0 for burst-only log; got {}",
        report.hub_standing_usd
    );
    assert!(
        (report.burst_usd - 5.0).abs() < 0.001,
        "burst_usd must be 5.0; got {}",
        report.burst_usd
    );
    assert!(report.rate_notes.is_empty(), "rate_notes must be empty for burst-only log");
}

// ---------------------------------------------------------------------------
// CLI smoke tests (require built binary)
// ---------------------------------------------------------------------------

fn wm_burst_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target/debug/wm-burst");
    p
}

/// Write a minimal config.toml in the tempdir.
fn write_config(dir: &TempDir) -> std::path::PathBuf {
    let config_path = dir.path().join("config.toml");
    let contents = r#"
remote_host = "builder.example.com"
monthly_budget_usd = 50.0

[sccache]
endpoint = "http://localhost:9000"
bucket = "test"
"#;
    std::fs::write(&config_path, contents).expect("write config");
    config_path
}

/// Write a cost log with a single burst entry (no hub entries).
fn write_burst_log(dir: &TempDir, now: chrono::DateTime<Utc>) -> std::path::PathBuf {
    let log_path = dir.path().join("cost-log.ndjson");
    let entry = JobEntry {
        job_id: "burst-1".into(),
        ran_on: "pod:test".into(),
        started_at: now - chrono::Duration::days(1),
        ended_at: Some(now - chrono::Duration::hours(23)),
        elapsed_secs: Some(3600.0),
        cost_usd: 1.23,
        description: "test burst".into(),
    };
    let line = serde_json::to_string(&entry).expect("serialize");
    std::fs::write(&log_path, format!("{line}\n")).expect("write log");
    log_path
}

#[test]
fn cli_cost_exits_zero_under_budget() {
    let dir = TempDir::new().expect("tempdir");
    let config = write_config(&dir);
    let log_path = write_burst_log(&dir, chrono::Utc::now());

    let output = std::process::Command::new(wm_burst_bin())
        .args([
            "cost",
            "--config",
            config.to_str().expect("config path"),
            "--cost-log",
            log_path.to_str().expect("log path"),
        ])
        .output()
        .expect("run wm-burst cost");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "expected exit 0 under budget; stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("burst spend"), "stdout must contain 'burst spend'");
}

#[test]
fn cli_cost_json_output_is_valid_json() {
    let dir = TempDir::new().expect("tempdir");
    let config = write_config(&dir);
    let log_path = write_burst_log(&dir, chrono::Utc::now());

    let output = std::process::Command::new(wm_burst_bin())
        .args([
            "cost",
            "--json",
            "--config",
            config.to_str().expect("config path"),
            "--cost-log",
            log_path.to_str().expect("log path"),
        ])
        .output()
        .expect("run wm-burst cost --json");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("output must be valid JSON");
    assert!(parsed.get("burst_usd").is_some(), "JSON must have burst_usd field");
    assert!(parsed.get("hub_standing_usd").is_some(), "JSON must have hub_standing_usd field");
    assert!(parsed.get("over_cap").is_some(), "JSON must have over_cap field");
}
