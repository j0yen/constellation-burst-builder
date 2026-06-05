//! AC6: Pod tier creates an ephemeral builder, runs a job, tears it down;
//! the full lifecycle + cost estimate is appended to the cost log;
//! monthly burst-budget cap is enforced — exceeding it blocks new pods.
//!
//! All tests use `--mock` (no real API spend).

use std::process::Command;
use tempfile::TempDir;

fn wm_burst_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target/debug/wm-burst");
    p
}

fn write_config(dir: &TempDir, budget: f64) -> std::path::PathBuf {
    let config = dir.path().join("config.toml");
    let content = format!(
        r#"remote_host = "builder.example.com"
monthly_budget_usd = {budget}

[sccache]
endpoint = "http://builder.example.com:9000"
bucket = "sccache"

[pod]
provider = "runpod"
idle_timeout_secs = 300
"#
    );
    std::fs::write(&config, content).expect("write config");
    config
}

#[test]
fn pod_up_completes_and_records_cost_log() {
    let dir = TempDir::new().expect("tempdir");
    let config = write_config(&dir, 100.0);
    let cost_log = dir.path().join("cost-log.ndjson");

    let output = Command::new(wm_burst_bin())
        .args([
            "pod", "up",
            "--mock",
            "--config", config.to_str().expect("path"),
            "--cost-log", cost_log.to_str().expect("path"),
            "--", "echo", "hello from pod",
        ])
        .output()
        .expect("run wm-burst pod up");

    assert!(
        output.status.success(),
        "pod up failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(cost_log.exists(), "cost log not written");

    let log_content = std::fs::read_to_string(&cost_log).expect("read cost log");
    assert!(
        log_content.contains("pod:mock-"),
        "cost log missing pod entry: {log_content}"
    );
}

#[test]
fn pod_up_blocked_when_budget_exceeded() {
    let dir = TempDir::new().expect("tempdir");
    // Set budget to 0 — any existing spend blocks.
    let config = write_config(&dir, 0.0);
    let cost_log = dir.path().join("cost-log.ndjson");

    let output = Command::new(wm_burst_bin())
        .args([
            "pod", "up",
            "--mock",
            "--config", config.to_str().expect("path"),
            "--cost-log", cost_log.to_str().expect("path"),
            "--", "echo", "should be blocked",
        ])
        .output()
        .expect("run wm-burst pod up with zero budget");

    assert!(
        !output.status.success(),
        "expected non-zero exit when budget is 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("budget") || stderr.contains("exceeded") || stderr.contains("cap"),
        "error must mention budget/cap: {stderr}"
    );
}

#[test]
fn pod_down_succeeds_with_mock() {
    let dir = TempDir::new().expect("tempdir");
    let config = write_config(&dir, 100.0);
    let cost_log = dir.path().join("cost-log.ndjson");

    let output = Command::new(wm_burst_bin())
        .args([
            "pod", "down",
            "--mock",
            "--config", config.to_str().expect("path"),
            "--cost-log", cost_log.to_str().expect("path"),
            "mock-42",
        ])
        .output()
        .expect("run wm-burst pod down");

    assert!(
        output.status.success(),
        "pod down failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
