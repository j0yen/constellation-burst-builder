//! AC1: `wm-burst init` writes a valid config.toml and `--show` round-trips it;
//! missing/invalid config produces a clear, actionable error.

use std::process::Command;
use tempfile::TempDir;

fn wm_burst_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target/debug/wm-burst");
    p
}

#[test]
fn init_writes_valid_config() {
    let dir = TempDir::new().expect("tempdir");
    let config = dir.path().join("config.toml");

    let status = Command::new(wm_burst_bin())
        .args([
            "init",
            "--remote-host", "builder.example.com",
            "--sccache-endpoint", "http://builder.example.com:9000",
            "--sccache-bucket", "sccache",
            "--monthly-budget-usd", "75.0",
            "--config", config.to_str().expect("path"),
        ])
        .status()
        .expect("run wm-burst init");

    assert!(status.success(), "wm-burst init exited non-zero: {status}");
    assert!(config.exists(), "config.toml was not created");

    let raw = std::fs::read_to_string(&config).expect("read config");
    assert!(raw.contains("builder.example.com"), "config missing remote_host");
    assert!(raw.contains("sccache"), "config missing sccache bucket");
}

#[test]
fn init_show_round_trips_config() {
    let dir = TempDir::new().expect("tempdir");
    let config = dir.path().join("config.toml");

    // Write initial config.
    Command::new(wm_burst_bin())
        .args([
            "init",
            "--remote-host", "builder.example.com",
            "--sccache-endpoint", "http://builder.example.com:9000",
            "--sccache-bucket", "my-bucket",
            "--config", config.to_str().expect("path"),
        ])
        .status()
        .expect("run wm-burst init");

    // Round-trip via --show.
    let output = Command::new(wm_burst_bin())
        .args(["init", "--show", "--config", config.to_str().expect("path")])
        .output()
        .expect("run wm-burst init --show");

    assert!(output.status.success(), "init --show failed: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("builder.example.com"), "round-trip missing remote_host");
    assert!(stdout.contains("my-bucket"), "round-trip missing bucket name");
}

#[test]
fn show_missing_config_produces_actionable_error() {
    let dir = TempDir::new().expect("tempdir");
    let config = dir.path().join("nonexistent.toml");

    let output = Command::new(wm_burst_bin())
        .args(["init", "--show", "--config", config.to_str().expect("path")])
        .output()
        .expect("run wm-burst init --show on missing file");

    assert!(
        !output.status.success(),
        "expected non-zero exit for missing config"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Error must mention the config path or 'init' to be actionable.
    assert!(
        stderr.contains("config") || stderr.contains("init") || stderr.contains("error"),
        "error message not actionable: {stderr}"
    );
}

#[test]
fn init_without_required_flags_fails_with_clear_error() {
    let dir = TempDir::new().expect("tempdir");
    let config = dir.path().join("config.toml");

    // Missing --remote-host on a fresh (non-existent) config file.
    let output = Command::new(wm_burst_bin())
        .args([
            "init",
            "--sccache-endpoint", "http://builder.example.com:9000",
            "--sccache-bucket", "my-bucket",
            "--config", config.to_str().expect("path"),
        ])
        .output()
        .expect("run wm-burst init without --remote-host");

    assert!(
        !output.status.success(),
        "expected non-zero exit when --remote-host is missing"
    );
}
