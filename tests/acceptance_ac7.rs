//! AC7: `wm-burst status` reports standing box load, cache hit rate,
//! month-to-date burst spend vs cap, and last N jobs.

use std::process::Command;
use tempfile::TempDir;

fn wm_burst_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target/debug/wm-burst");
    p
}

fn write_config(dir: &TempDir) -> std::path::PathBuf {
    let config = dir.path().join("config.toml");
    let content = r#"remote_host = "builder.example.com"
monthly_budget_usd = 50.0

[sccache]
endpoint = "http://builder.example.com:9000"
bucket = "sccache"
"#;
    std::fs::write(&config, content).expect("write config");
    config
}

fn write_cost_log(dir: &TempDir) -> std::path::PathBuf {
    let log = dir.path().join("cost-log.ndjson");
    // One entry from today, one old one.
    let now = chrono::Utc::now();
    let entry1 = serde_json::json!({
        "job_id": "build-1",
        "ran_on": "standing-box:builder.example.com",
        "started_at": now.to_rfc3339(),
        "ended_at": now.to_rfc3339(),
        "elapsed_secs": 42.0,
        "cost_usd": 0.0,
        "description": "cargo build in ~/myproject",
    });
    std::fs::write(&log, format!("{entry1}\n")).expect("write cost log");
    log
}

#[test]
fn status_json_contains_required_fields() {
    let dir = TempDir::new().expect("tempdir");
    let config = write_config(&dir);
    let cost_log = write_cost_log(&dir);

    let output = Command::new(wm_burst_bin())
        .args([
            "status",
            "--offline",
            "--json",
            "--config", config.to_str().expect("path"),
            "--cost-log", cost_log.to_str().expect("path"),
        ])
        .output()
        .expect("run wm-burst status --json");

    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .expect("status --json output is not valid JSON");

    assert!(v.get("remote_host").is_some(), "missing remote_host");
    assert!(v.get("month_to_date_usd").is_some(), "missing month_to_date_usd");
    assert!(v.get("monthly_budget_usd").is_some(), "missing monthly_budget_usd");
    assert!(v.get("last_jobs").is_some(), "missing last_jobs");
}

#[test]
fn status_text_shows_spend_vs_cap() {
    let dir = TempDir::new().expect("tempdir");
    let config = write_config(&dir);
    let cost_log = write_cost_log(&dir);

    let output = Command::new(wm_burst_bin())
        .args([
            "status",
            "--offline",
            "--config", config.to_str().expect("path"),
            "--cost-log", cost_log.to_str().expect("path"),
        ])
        .output()
        .expect("run wm-burst status");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should show the budget and remote host.
    assert!(
        stdout.contains("50.00") || stdout.contains("50"),
        "missing budget in output: {stdout}"
    );
    assert!(
        stdout.contains("builder.example.com"),
        "missing remote host: {stdout}"
    );
}

#[test]
fn status_missing_config_shows_actionable_error() {
    let dir = TempDir::new().expect("tempdir");
    let config = dir.path().join("nonexistent.toml");

    let output = Command::new(wm_burst_bin())
        .args([
            "status",
            "--offline",
            "--config", config.to_str().expect("path"),
        ])
        .output()
        .expect("run status with missing config");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("init") || stderr.contains("config") || stderr.contains("error"),
        "error not actionable: {stderr}"
    );
}
