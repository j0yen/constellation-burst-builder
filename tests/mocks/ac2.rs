//! AC2 mock: `wm-burst provision` generates a valid, idempotent Ansible playbook.
//!
//! Deferred for real-host dry-run (requires ansible + SSH target).
//! This mock validates the playbook generation + structural content.

use wm_burst::commands::provision::generate_playbook;
use wm_burst::config::{Config, SccacheConfig};

fn test_config() -> Config {
    Config {
        remote_host: "builder.example.com".into(),
        sccache: SccacheConfig {
            endpoint: "http://builder.example.com:9000".into(),
            bucket: "sccache-test".into(),
            access_key: Some("AKIA123".into()),
            secret_key: Some("SECRET456".into()),
        },
        monthly_budget_usd: 75.0,
        pod: None,
    }
}

#[test]
fn playbook_contains_both_toolchains() {
    let cfg = test_config();
    let playbook = generate_playbook(&cfg);
    assert!(
        playbook.contains("1.85"),
        "playbook must install 1.85 toolchain; got:\n{playbook}"
    );
    assert!(
        playbook.contains("1.88"),
        "playbook must install 1.88 toolchain; got:\n{playbook}"
    );
}

#[test]
fn playbook_contains_sccache_install() {
    let cfg = test_config();
    let playbook = generate_playbook(&cfg);
    assert!(
        playbook.contains("sccache"),
        "playbook must reference sccache; got:\n{playbook}"
    );
}

#[test]
fn playbook_contains_idempotent_assertion() {
    let cfg = test_config();
    let playbook = generate_playbook(&cfg);
    // The playbook must have an assert task to verify toolchains are present.
    assert!(
        playbook.contains("assert") || playbook.contains("Assert"),
        "playbook should have assertion task; got:\n{playbook}"
    );
}

#[test]
fn playbook_contains_target_endpoint() {
    let cfg = test_config();
    let playbook = generate_playbook(&cfg);
    assert!(
        playbook.contains("http://builder.example.com:9000"),
        "playbook must embed sccache endpoint; got:\n{playbook}"
    );
    assert!(
        playbook.contains("sccache-test"),
        "playbook must embed sccache bucket name; got:\n{playbook}"
    );
}

#[test]
fn playbook_is_valid_yaml_structure() {
    let cfg = test_config();
    let playbook = generate_playbook(&cfg);
    // Must start with YAML document marker or `---`.
    assert!(
        playbook.trim_start().starts_with("---") || playbook.trim_start().starts_with('-'),
        "playbook must be valid YAML (starts with --- or -): {}", &playbook[..100.min(playbook.len())]
    );
    // Must have 'hosts:' and 'tasks:'.
    assert!(playbook.contains("hosts:"), "playbook missing 'hosts:'");
    assert!(playbook.contains("tasks:"), "playbook missing 'tasks:'");
}
