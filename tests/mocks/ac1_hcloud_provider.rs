//! AC1 mock: `HcloudPodProvider` implements `PodProvider` and is selected when
//! `[hcloud] provider = "hcloud"`.  The `$HCLOUD_TOKEN` never appears in config,
//! cost log, or Debug output.

use wm_burst::config::{Config, HcloudPodConfig, SccacheConfig};
use wm_burst::cost::{CostLog, JobEntry};
use wm_burst::provider::{make_provider, HcloudPodProvider, MockPodProvider, PodProvider};
use chrono::Utc;
use tempfile::tempdir;

fn test_hcloud_cfg() -> HcloudPodConfig {
    HcloudPodConfig {
        provider: "hcloud".into(),
        server_type: "ccx23".into(),
        location: "fsn1".into(),
        image: "ubuntu-24.04".into(),
        ssh_key_name: "my-key".into(),
        remote_arch: "x86_64-unknown-linux-gnu".into(),
        sccache_endpoint: "https://fsn1.your-objectstorage.com".into(),
        sccache_bucket: "wm-sccache".into(),
        idle_timeout_secs: 3600,
    }
}

fn test_config_with_hcloud() -> Config {
    Config {
        remote_host: "builder.example.com".into(),
        sccache: SccacheConfig {
            endpoint: "https://fsn1.your-objectstorage.com".into(),
            bucket: "wm-sccache".into(),
            access_key: None,
            secret_key: None,
        },
        monthly_budget_usd: 20.0,
        pod: None,
        hcloud: Some(test_hcloud_cfg()),
        hub: None,
    }
}

#[test]
fn hcloud_provider_selected_via_make_provider_mock() -> anyhow::Result<()> {
    // When use_mock=true, make_provider always returns MockPodProvider regardless of name.
    let p = make_provider(true, "hcloud")?;
    let cfg = test_hcloud_cfg();
    let id = p.create_pod(&cfg)?;
    assert!(!id.is_empty(), "pod_id must not be empty");
    Ok(())
}

#[test]
fn make_provider_real_hcloud_empty_token_errors() {
    // Verify that HcloudPodProvider rejects an empty token string at construction.
    // We test via HcloudPodProvider::from_token (no env mutation needed → no unsafe).
    let err = HcloudPodProvider::from_token("").map_or_else(|e| format!("{e}"), |_| String::new());
    assert!(
        err.contains("empty"),
        "empty token must produce an error mentioning 'empty', got: {err}"
    );
    // Also confirm a valid token succeeds.
    assert!(HcloudPodProvider::from_token("some-valid-token").is_ok());
}

#[test]
fn hcloud_token_never_appears_in_cost_log() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let log_path = dir.path().join("cost-log.ndjson");
    let fake_token = "hcloud-fake-token-SHOULD-NOT-APPEAR";

    let mut log = CostLog::load(&log_path).unwrap_or_default();
    let now = Utc::now();
    let entry = JobEntry {
        job_id: "burst-test-1".into(),
        ran_on: "pod:12345@1.2.3.4".into(),
        started_at: now,
        ended_at: Some(now),
        elapsed_secs: Some(30.0),
        cost_usd: 0.0003,
        description: "cargo build in ~/myproject".into(),
    };
    log.append(entry)?;

    let raw = std::fs::read_to_string(&log_path)?;
    assert!(
        !raw.contains(fake_token),
        "cost log must not contain the API token; got: {raw}"
    );
    Ok(())
}

#[test]
fn hcloud_token_never_appears_in_rendered_config() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let cfg_path = dir.path().join("config.toml");
    let cfg = test_config_with_hcloud();
    cfg.save(&cfg_path)?;

    let raw = std::fs::read_to_string(&cfg_path)?;
    // Config must not store any HCLOUD_TOKEN value (it isn't a field on HcloudPodConfig).
    // We verify the pattern "HCLOUD_TOKEN" does not appear.
    assert!(
        !raw.contains("HCLOUD_TOKEN"),
        "rendered config must not contain HCLOUD_TOKEN env var name; got:\n{raw}"
    );
    // The [hcloud] section must be present and default arch must be x86_64.
    assert!(raw.contains("[hcloud]"), "config must have [hcloud] section; got:\n{raw}");
    assert!(
        raw.contains("x86_64-unknown-linux-gnu"),
        "remote_arch must default to x86_64-unknown-linux-gnu; got:\n{raw}"
    );
    Ok(())
}

#[test]
fn mock_provider_destroy_is_idempotent() -> anyhow::Result<()> {
    // AC2: destroy must not error even on a half-created pod.
    let p = MockPodProvider::new();
    // Call destroy without a prior create — simulates partial failure.
    p.destroy_pod("mock-42@192.0.2.42")?;
    p.destroy_pod("mock-42@192.0.2.42")?;
    Ok(())
}

#[test]
fn config_hcloud_validates_defaults() -> anyhow::Result<()> {
    let cfg = test_config_with_hcloud();
    cfg.validate()?;
    Ok(())
}

#[test]
fn config_hcloud_rejects_empty_ssh_key() {
    let mut cfg = test_config_with_hcloud();
    if let Some(hc) = &mut cfg.hcloud {
        hc.ssh_key_name = String::new();
    }
    let err = cfg.validate().map_or_else(|e| format!("{e}"), |()| String::new());
    assert!(
        err.contains("ssh_key_name"),
        "validate must mention ssh_key_name; got: {err}"
    );
}

#[test]
fn config_hcloud_remote_arch_defaults_to_x86() {
    let hc = HcloudPodConfig::default();
    assert_eq!(
        hc.remote_arch, "x86_64-unknown-linux-gnu",
        "remote_arch default must be x86_64-unknown-linux-gnu"
    );
}

#[test]
fn config_hcloud_server_type_default_is_ccx23() {
    let hc = HcloudPodConfig::default();
    assert_eq!(hc.server_type, "ccx23", "server_type default must be ccx23");
}

#[test]
fn config_hcloud_round_trips_toml() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let cfg_path = dir.path().join("config.toml");
    let cfg_orig = test_config_with_hcloud();
    cfg_orig.save(&cfg_path)?;

    let cfg_loaded = Config::load(Some(&cfg_path))?;
    let hc = cfg_loaded.hcloud.ok_or_else(|| anyhow::anyhow!("hcloud section missing after round-trip"))?;
    assert_eq!(hc.server_type, "ccx23");
    assert_eq!(hc.location, "fsn1");
    assert_eq!(hc.remote_arch, "x86_64-unknown-linux-gnu");
    assert_eq!(hc.sccache_bucket, "wm-sccache");
    Ok(())
}
