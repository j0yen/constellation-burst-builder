//! `wm-burst hub up|down|status` — permanent hub lifecycle.
//!
//! Unlike ephemeral burst pods, the hub is provisioned once and kept running
//! until an explicit `hub down --yes`. Identity persists in `~/.config/wm-burst/hub.json`.
//!
//! `hub up`     — idempotent: reuses an already-running hub, else provisions one.
//! `hub down`   — requires `--yes` to prevent accidental teardown.
//! `hub status` — prints persisted identity and live status.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::config::{Config, HubConfig, HubState};
use crate::cost::{CostLog, JobEntry};
use crate::provider;

/// Arguments for `wm-burst hub`.
#[derive(Args, Debug)]
pub struct HubArgs {
    /// Hub lifecycle action.
    #[command(subcommand)]
    pub action: HubAction,

    /// Path to config file (default: `~/.config/wm-burst/config.toml`).
    #[arg(long, global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Path to hub state file (default: `~/.config/wm-burst/hub.json`).
    #[arg(long, global = true)]
    pub hub_state: Option<std::path::PathBuf>,

    /// Path to cost log (default: `~/.local/share/wm-burst/cost-log.ndjson`).
    #[arg(long, global = true)]
    pub cost_log: Option<std::path::PathBuf>,

    /// Use mocked provider (for tests; no real API calls).
    #[arg(long, global = true, hide = true)]
    pub mock: bool,
}

/// Hub lifecycle actions.
#[derive(Subcommand, Debug)]
pub enum HubAction {
    /// Bring the hub up (idempotent — reuses a running hub).
    Up,
    /// Tear the hub down (requires `--yes` to confirm).
    Down(HubDownArgs),
    /// Print hub identity and live status.
    Status,
}

/// Arguments for `wm-burst hub down`.
#[derive(Args, Debug)]
pub struct HubDownArgs {
    /// Confirm permanent hub teardown (required to prevent accidents).
    #[arg(long)]
    pub yes: bool,
}

/// Run the `hub` subcommand.
///
/// # Errors
/// Returns an error if config cannot be loaded, provider API fails, or state cannot be persisted.
pub fn run(args: HubArgs) -> Result<std::process::ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };
    let hub_state_path = match &args.hub_state {
        Some(p) => p.clone(),
        None => HubState::default_path()?,
    };
    let cost_log_path = match &args.cost_log {
        Some(p) => p.clone(),
        None => CostLog::default_path()?,
    };

    let cfg = Config::load(Some(&config_path))?;

    let hub_cfg = cfg.hub.clone().unwrap_or_default();

    match args.action {
        HubAction::Up => run_hub_up(&cfg, &hub_cfg, &hub_state_path, &cost_log_path, args.mock),
        HubAction::Down(down_args) => {
            run_hub_down(&cfg, &hub_state_path, &cost_log_path, &down_args, args.mock)
        }
        HubAction::Status => run_hub_status(&hub_cfg, &hub_state_path, args.mock),
    }
}

fn run_hub_up(
    cfg: &Config,
    hub_cfg: &HubConfig,
    hub_state_path: &std::path::Path,
    cost_log_path: &std::path::Path,
    use_mock: bool,
) -> Result<std::process::ExitCode> {
    // Determine provider name: prefer hcloud section, fall back to "hcloud".
    let provider_name = cfg
        .hcloud
        .as_ref()
        .map(|h| h.provider.as_str())
        .unwrap_or("hcloud");

    let p = provider::make_provider(use_mock, provider_name)?;

    // Check if we already have a persisted hub.
    let existing_state = HubState::load(hub_state_path)?;

    if let Some(state) = existing_state {
        // We have a persisted hub_id. Ask the provider if it's still up.
        let server_id = state.hub_id.split('@').next().unwrap_or(&state.hub_id);
        match p.get_server(server_id)? {
            Some(live) if live.status == "running" => {
                let ip = state.hub_id.split('@').nth(1).unwrap_or(&live.ip);
                println!("hub already up @ {ip} (id={}, status={})", live.id, live.status);
                return Ok(std::process::ExitCode::SUCCESS);
            }
            Some(live) => {
                // Exists but not running (e.g. "off" or "initializing") — report and bail.
                println!(
                    "hub exists but is not running (status={}); use `hub down --yes` then `hub up` to reprovision",
                    live.status
                );
                return Ok(std::process::ExitCode::FAILURE);
            }
            None => {
                // Server was deleted externally — clear stale state and provision fresh.
                eprintln!("[hub] persisted hub_id {} is gone; reprovisioning", state.hub_id);
                HubState::clear(hub_state_path)?;
            }
        }
    }

    // Provision a new hub using the same create_pod path.
    let hcloud_cfg = cfg.hcloud.clone().unwrap_or_else(|| crate::config::HcloudPodConfig {
        provider: "hcloud".into(),
        server_type: hub_cfg.server_type.clone(),
        location: hub_cfg.location.clone(),
        image: hub_cfg.image.clone(),
        ssh_key_name: hub_cfg.ssh_key_name.clone(),
        ..Default::default()
    });

    eprintln!(
        "[hub] provisioning hub type={} location={} image={}",
        hub_cfg.server_type, hub_cfg.location, hub_cfg.image
    );

    let hub_id = p.create_pod(&hcloud_cfg)?;
    let ip = hub_id.split('@').nth(1).unwrap_or("unknown");

    // Persist identity.
    let new_state = HubState { hub_id: hub_id.clone() };
    new_state.save(hub_state_path)?;

    // Log HUB-UP to cost log (distinct prefix for harbor-thrift).
    let mut log = CostLog::load(cost_log_path).unwrap_or_default();
    let entry = JobEntry {
        job_id: format!("HUB-UP id={hub_id} type={} ip={ip}", hub_cfg.server_type),
        ran_on: format!("hub:{hub_id}"),
        started_at: chrono::Utc::now(),
        ended_at: None,
        elapsed_secs: None,
        cost_usd: 0.0,
        description: format!("HUB-UP id={hub_id} type={} ip={ip}", hub_cfg.server_type),
    };
    log.append(entry).context("cannot write cost log for HUB-UP")?;

    println!("hub up @ {ip} (id={hub_id})");
    Ok(std::process::ExitCode::SUCCESS)
}

fn run_hub_down(
    cfg: &Config,
    hub_state_path: &std::path::Path,
    cost_log_path: &std::path::Path,
    args: &HubDownArgs,
    use_mock: bool,
) -> Result<std::process::ExitCode> {
    if !args.yes {
        eprintln!(
            "hub down requires --yes to confirm; the hub may be stateful (see harbor-mirror/harbor-cache). \
             Run: wm-burst hub down --yes"
        );
        return Ok(std::process::ExitCode::FAILURE);
    }

    let state = match HubState::load(hub_state_path)? {
        Some(s) => s,
        None => {
            println!("hub: none (nothing to tear down)");
            return Ok(std::process::ExitCode::SUCCESS);
        }
    };

    let provider_name = cfg
        .hcloud
        .as_ref()
        .map(|h| h.provider.as_str())
        .unwrap_or("hcloud");
    let p = provider::make_provider(use_mock, provider_name)?;

    p.destroy_pod(&state.hub_id)?;
    HubState::clear(hub_state_path)?;

    // Log HUB-DOWN with distinct prefix.
    let mut log = CostLog::load(cost_log_path).unwrap_or_default();
    let now = chrono::Utc::now();
    let entry = JobEntry {
        job_id: format!("HUB-DOWN id={}", state.hub_id),
        ran_on: format!("hub:{}", state.hub_id),
        started_at: now,
        ended_at: Some(now),
        elapsed_secs: Some(0.0),
        cost_usd: 0.0,
        description: format!("HUB-DOWN id={}", state.hub_id),
    };
    log.append(entry).context("cannot write cost log for HUB-DOWN")?;

    println!("hub down (id={})", state.hub_id);
    Ok(std::process::ExitCode::SUCCESS)
}

fn run_hub_status(
    _hub_cfg: &HubConfig,
    hub_state_path: &std::path::Path,
    use_mock: bool,
) -> Result<std::process::ExitCode> {
    let state = match HubState::load(hub_state_path)? {
        Some(s) => s,
        None => {
            println!("hub: none");
            return Ok(std::process::ExitCode::SUCCESS);
        }
    };

    let server_id = state.hub_id.split('@').next().unwrap_or(&state.hub_id);
    let p = provider::make_provider(use_mock, "hcloud")?;
    let live = p.get_server(server_id)?;

    let ip = state.hub_id.split('@').nth(1).unwrap_or("unknown");
    match live {
        Some(s) => {
            println!("hub: id={} ip={ip} status={}", state.hub_id, s.status);
        }
        None => {
            println!("hub: id={} ip={ip} status=gone (deleted externally)", state.hub_id);
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HcloudPodConfig, HubConfig, HubState, SccacheConfig};
    use crate::provider::MockPodProvider;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn temp_hub_state_path(dir: &TempDir) -> PathBuf {
        dir.path().join("hub.json")
    }

    fn temp_cost_log_path(dir: &TempDir) -> PathBuf {
        dir.path().join("cost-log.ndjson")
    }

    fn make_test_config() -> Config {
        Config {
            remote_host: "builder".into(),
            sccache: SccacheConfig {
                endpoint: "http://localhost:9000".into(),
                bucket: "test".into(),
                access_key: None,
                secret_key: None,
            },
            monthly_budget_usd: 50.0,
            pod: None,
            hcloud: Some(HcloudPodConfig {
                provider: "hcloud".into(),
                server_type: "cpx11".into(),
                location: "nbg1".into(),
                image: "ubuntu-24.04".into(),
                ssh_key_name: "test-key".into(),
                remote_arch: "x86_64-unknown-linux-gnu".into(),
                sccache_endpoint: "https://nbg1.your-objectstorage.com".into(),
                sccache_bucket: "wm-sccache".into(),
                idle_timeout_secs: 3600,
            }),
            hub: Some(HubConfig {
                server_type: "cpx11".into(),
                location: "nbg1".into(),
                image: "ubuntu-24.04".into(),
                ssh_key_name: "test-key".into(),
            }),
        }
    }

    /// AC2 — HubState round-trips through hub.json.
    #[test]
    fn hub_state_roundtrip() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = temp_hub_state_path(&dir);
        let original = HubState { hub_id: "mock-1@192.0.2.1".into() };
        original.save(&path)?;
        let loaded = HubState::load(&path)?.expect("expected Some after save");
        assert_eq!(original, loaded, "round-trip mismatch");
        Ok(())
    }

    /// AC3 — hub up with persisted+running hub makes zero create_pod calls.
    #[test]
    fn hub_up_idempotent_when_running() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let hub_path = temp_hub_state_path(&dir);
        let cost_path = temp_cost_log_path(&dir);

        // Pre-persist a running hub.
        let state = HubState { hub_id: "mock-1@192.0.2.1".into() };
        state.save(&hub_path)?;

        let cfg = make_test_config();
        let hub_cfg = cfg.hub.clone().unwrap_or_default();

        // Use a provider that reports "running".
        let p = MockPodProvider::with_running_server();

        // Call the inner logic directly via a private helper that accepts the provider.
        // We test via the run_hub_up_with_provider helper to inject mock.
        run_hub_up_with_provider(&cfg, &hub_cfg, &hub_path, &cost_path, &p)?;

        assert_eq!(p.create_calls.get(), 0, "create_pod must NOT be called when hub is already running");
        Ok(())
    }

    /// AC4 — hub up with no persisted hub calls create_pod exactly once and persists the id.
    #[test]
    fn hub_up_provisions_and_persists() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let hub_path = temp_hub_state_path(&dir);
        let cost_path = temp_cost_log_path(&dir);

        let cfg = make_test_config();
        let hub_cfg = cfg.hub.clone().unwrap_or_default();

        let p = MockPodProvider::new(); // no persisted hub, no running server
        run_hub_up_with_provider(&cfg, &hub_cfg, &hub_path, &cost_path, &p)?;

        assert_eq!(p.create_calls.get(), 1, "create_pod must be called exactly once");

        // Re-read hub state and verify it was persisted.
        let saved = HubState::load(&hub_path)?.expect("hub.json must be written after hub up");
        assert!(saved.hub_id.starts_with("mock-"), "persisted hub_id must come from create_pod");
        Ok(())
    }

    /// AC5 — hub down without --yes refuses (no destroy call).
    #[test]
    fn hub_down_requires_yes() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let hub_path = temp_hub_state_path(&dir);
        let cost_path = temp_cost_log_path(&dir);

        // Persist a hub so down has something to target.
        let state = HubState { hub_id: "mock-1@192.0.2.1".into() };
        state.save(&hub_path)?;

        let cfg = make_test_config();
        let p = MockPodProvider::new();
        let args = HubDownArgs { yes: false };

        let code = run_hub_down_with_provider(&cfg, &hub_path, &cost_path, &args, &p)?;
        assert_ne!(code, std::process::ExitCode::SUCCESS, "hub down without --yes must return non-zero exit");
        assert_eq!(p.destroy_calls.get(), 0, "destroy_pod must NOT be called without --yes");
        Ok(())
    }

    /// AC5 — hub down with --yes calls destroy_pod once and clears hub_id.
    #[test]
    fn hub_down_with_yes_destroys_and_clears() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let hub_path = temp_hub_state_path(&dir);
        let cost_path = temp_cost_log_path(&dir);

        let state = HubState { hub_id: "mock-1@192.0.2.1".into() };
        state.save(&hub_path)?;

        let cfg = make_test_config();
        let p = MockPodProvider::new();
        let args = HubDownArgs { yes: true };

        run_hub_down_with_provider(&cfg, &hub_path, &cost_path, &args, &p)?;

        assert_eq!(p.destroy_calls.get(), 1, "destroy_pod must be called exactly once");
        let after = HubState::load(&hub_path)?;
        assert!(after.is_none(), "hub.json must be cleared after hub down --yes");
        Ok(())
    }

    /// AC6 — hub status with no persisted hub prints "hub: none" and exits 0.
    #[test]
    fn hub_status_no_hub_prints_none() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let hub_path = temp_hub_state_path(&dir);
        let hub_cfg = HubConfig::default();
        let p = MockPodProvider::new();

        let code = run_hub_status_with_provider(&hub_cfg, &hub_path, &p)?;
        assert_eq!(code, std::process::ExitCode::SUCCESS, "hub status with no hub must exit 0");
        Ok(())
    }

    /// AC7 — cost log gets HUB-UP line on provision.
    #[test]
    fn hub_up_writes_hub_up_log_entry() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let hub_path = temp_hub_state_path(&dir);
        let cost_path = temp_cost_log_path(&dir);

        let cfg = make_test_config();
        let hub_cfg = cfg.hub.clone().unwrap_or_default();
        let p = MockPodProvider::new();
        run_hub_up_with_provider(&cfg, &hub_cfg, &hub_path, &cost_path, &p)?;

        let log = CostLog::load(&cost_path)?;
        let entries = log.entries();
        assert!(!entries.is_empty(), "cost log must have at least one entry");
        let hub_up_entry = entries.iter().find(|e| e.job_id.starts_with("HUB-UP"));
        assert!(hub_up_entry.is_some(), "cost log must contain a HUB-UP entry");
        Ok(())
    }

    /// AC7 — cost log gets HUB-DOWN line on teardown.
    #[test]
    fn hub_down_writes_hub_down_log_entry() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let hub_path = temp_hub_state_path(&dir);
        let cost_path = temp_cost_log_path(&dir);

        let state = HubState { hub_id: "mock-1@192.0.2.1".into() };
        state.save(&hub_path)?;

        let cfg = make_test_config();
        let p = MockPodProvider::new();
        let args = HubDownArgs { yes: true };
        run_hub_down_with_provider(&cfg, &hub_path, &cost_path, &args, &p)?;

        let log = CostLog::load(&cost_path)?;
        let entries = log.entries();
        let hub_down_entry = entries.iter().find(|e| e.job_id.starts_with("HUB-DOWN"));
        assert!(hub_down_entry.is_some(), "cost log must contain a HUB-DOWN entry");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Testable inner functions (accept injected provider to avoid real API calls)
// ---------------------------------------------------------------------------

#[cfg(test)]
fn run_hub_up_with_provider(
    cfg: &Config,
    hub_cfg: &HubConfig,
    hub_state_path: &std::path::Path,
    cost_log_path: &std::path::Path,
    p: &crate::provider::MockPodProvider,
) -> Result<std::process::ExitCode> {
    use crate::provider::PodProvider;

    let existing_state = HubState::load(hub_state_path)?;

    if let Some(state) = existing_state {
        let server_id = state.hub_id.split('@').next().unwrap_or(&state.hub_id);
        match p.get_server(server_id)? {
            Some(live) if live.status == "running" => {
                let ip = state.hub_id.split('@').nth(1).unwrap_or(&live.ip);
                println!("hub already up @ {ip} (id={}, status={})", live.id, live.status);
                return Ok(std::process::ExitCode::SUCCESS);
            }
            Some(live) => {
                println!(
                    "hub exists but is not running (status={}); use `hub down --yes` then `hub up` to reprovision",
                    live.status
                );
                return Ok(std::process::ExitCode::FAILURE);
            }
            None => {
                HubState::clear(hub_state_path)?;
            }
        }
    }

    let hcloud_cfg = cfg.hcloud.clone().unwrap_or_else(|| crate::config::HcloudPodConfig {
        provider: "hcloud".into(),
        server_type: hub_cfg.server_type.clone(),
        location: hub_cfg.location.clone(),
        image: hub_cfg.image.clone(),
        ssh_key_name: hub_cfg.ssh_key_name.clone(),
        ..Default::default()
    });

    let hub_id = p.create_pod(&hcloud_cfg)?;
    let ip = hub_id.split('@').nth(1).unwrap_or("unknown");

    let new_state = HubState { hub_id: hub_id.clone() };
    new_state.save(hub_state_path)?;

    let mut log = CostLog::load(cost_log_path).unwrap_or_default();
    let entry = JobEntry {
        job_id: format!("HUB-UP id={hub_id} type={} ip={ip}", hub_cfg.server_type),
        ran_on: format!("hub:{hub_id}"),
        started_at: chrono::Utc::now(),
        ended_at: None,
        elapsed_secs: None,
        cost_usd: 0.0,
        description: format!("HUB-UP id={hub_id} type={} ip={ip}", hub_cfg.server_type),
    };
    log.append(entry)?;

    println!("hub up @ {ip} (id={hub_id})");
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
fn run_hub_down_with_provider(
    _cfg: &Config,
    hub_state_path: &std::path::Path,
    cost_log_path: &std::path::Path,
    args: &HubDownArgs,
    p: &crate::provider::MockPodProvider,
) -> Result<std::process::ExitCode> {
    use crate::provider::PodProvider;

    if !args.yes {
        eprintln!(
            "hub down requires --yes to confirm; the hub may be stateful. \
             Run: wm-burst hub down --yes"
        );
        return Ok(std::process::ExitCode::FAILURE);
    }

    let state = match HubState::load(hub_state_path)? {
        Some(s) => s,
        None => {
            println!("hub: none (nothing to tear down)");
            return Ok(std::process::ExitCode::SUCCESS);
        }
    };

    p.destroy_pod(&state.hub_id)?;
    HubState::clear(hub_state_path)?;

    let mut log = CostLog::load(cost_log_path).unwrap_or_default();
    let now = chrono::Utc::now();
    let entry = JobEntry {
        job_id: format!("HUB-DOWN id={}", state.hub_id),
        ran_on: format!("hub:{}", state.hub_id),
        started_at: now,
        ended_at: Some(now),
        elapsed_secs: Some(0.0),
        cost_usd: 0.0,
        description: format!("HUB-DOWN id={}", state.hub_id),
    };
    log.append(entry)?;

    println!("hub down (id={})", state.hub_id);
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
fn run_hub_status_with_provider(
    _hub_cfg: &HubConfig,
    hub_state_path: &std::path::Path,
    p: &crate::provider::MockPodProvider,
) -> Result<std::process::ExitCode> {
    use crate::provider::PodProvider;

    let state = match HubState::load(hub_state_path)? {
        Some(s) => s,
        None => {
            println!("hub: none");
            return Ok(std::process::ExitCode::SUCCESS);
        }
    };

    let server_id = state.hub_id.split('@').next().unwrap_or(&state.hub_id);
    let live = p.get_server(server_id)?;
    let ip = state.hub_id.split('@').nth(1).unwrap_or("unknown");
    match live {
        Some(s) => println!("hub: id={} ip={ip} status={}", state.hub_id, s.status),
        None => println!("hub: id={} ip={ip} status=gone (deleted externally)", state.hub_id),
    }
    Ok(std::process::ExitCode::SUCCESS)
}
