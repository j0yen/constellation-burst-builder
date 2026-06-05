//! `wm-burst build` — run cargo build remotely with `RUSTC_WRAPPER=sccache`.
//!
//! Streams output locally and reports where it ran + cache hit/miss counts + elapsed.
//! With `--burst`, spins up an ephemeral hcloud pod, runs the build there, and tears
//! it down on completion — the on-demand Hetzner Cloud burst flow.

use anyhow::{Context, Result};
use clap::Args;
use std::time::Instant;

use crate::config::Config;
use crate::cost::{check_budget, CostLog, JobEntry};
use crate::provider;

/// Arguments for `wm-burst build`.
#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Path to config file (default: `~/.config/wm-burst/config.toml`).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Remote path to the project to build (must be pre-synced).
    #[arg(long)]
    pub remote_path: Option<String>,

    /// Extra arguments passed to `cargo build`.
    #[arg(last = true)]
    pub cargo_args: Vec<String>,

    /// Path to cost log (default: `~/.local/share/wm-burst/cost-log.ndjson`).
    #[arg(long)]
    pub cost_log: Option<std::path::PathBuf>,

    /// Burst mode: spin up an ephemeral hcloud pod, build there, tear it down.
    /// Requires `[hcloud]` config and `$HCLOUD_TOKEN` in environment.
    #[arg(long)]
    pub burst: bool,

    /// Use mocked provider for `--burst` (for tests; no real API calls).
    #[arg(long, hide = true)]
    pub mock: bool,
}

/// Run the `build` subcommand.
///
/// # Errors
/// Returns an error if config is missing, remote build fails, or cost log write fails.
pub fn run(args: &BuildArgs) -> Result<std::process::ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };
    let cfg = Config::load(Some(&config_path))?;
    cfg.validate()?;

    if args.burst {
        return run_burst_build(args, &cfg);
    }

    run_standing_build(args, &cfg)
}

/// Burst-build path: spin up an ephemeral hcloud pod, build there, tear it down.
fn run_burst_build(args: &BuildArgs, cfg: &Config) -> Result<std::process::ExitCode> {
    let hcfg = cfg.hcloud.as_ref().context(
        "no [hcloud] section in config; run `wm-burst init --pod-provider hcloud` \
         or add [hcloud] manually to ~/.config/wm-burst/config.toml",
    )?;

    let log_path = match &args.cost_log {
        Some(p) => p.clone(),
        None => CostLog::default_path()?,
    };
    let mut log = CostLog::load(&log_path).unwrap_or_default();
    check_budget(&log, cfg.monthly_budget_usd)?;

    let remote_path = args.remote_path.as_deref().unwrap_or("~/current-project");
    let cargo_cmd = if args.cargo_args.is_empty() {
        "cargo build".to_owned()
    } else {
        format!("cargo build {}", args.cargo_args.join(" "))
    };

    let remote_cmd = format!(
        "cd {remote_path} && \
         RUSTC_WRAPPER=sccache \
         SCCACHE_ENDPOINT={endpoint} \
         SCCACHE_BUCKET={bucket} \
         {cargo_cmd} && sccache --show-stats",
        endpoint = hcfg.sccache_endpoint,
        bucket = hcfg.sccache_bucket,
    );

    eprintln!(
        "[build] --burst: creating hcloud pod (type={} location={} image={})",
        hcfg.server_type, hcfg.location, hcfg.image
    );

    let p = provider::make_provider(args.mock, &hcfg.provider)?;
    let start = Instant::now();

    let pod_id = p.create_pod(hcfg)?;
    let ip = pod_id.split('@').nth(1).unwrap_or("unknown").to_owned();
    eprintln!("[build] pod created: {pod_id} ip={ip}");

    let job_result = p.run_job(&pod_id, &ip, &remote_cmd, 3600);
    let teardown = p.destroy_pod(&pod_id);

    let elapsed = start.elapsed().as_secs_f64();

    // CCX23 is ~€0.0295/hr; use a rounded constant for the estimate.
    #[allow(clippy::float_arithmetic)]
    let cost_usd = 0.03_f64 * elapsed / 3600.0;

    eprintln!(
        "[build] --burst lifecycle: pod={pod_id} elapsed={elapsed:.1}s cost_usd=${cost_usd:.4}"
    );

    if let Err(e) = teardown {
        eprintln!("[build] WARN: teardown failed for pod {pod_id}: {e}");
    }

    let now = chrono::Utc::now();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::float_arithmetic, clippy::as_conversions)]
    let elapsed_ms = (elapsed * 1000.0) as i64;
    let entry = JobEntry {
        job_id: format!("burst-build-{}", now.timestamp()),
        ran_on: format!("pod:{pod_id}"),
        started_at: now - chrono::Duration::milliseconds(elapsed_ms),
        ended_at: Some(now),
        elapsed_secs: Some(elapsed),
        cost_usd,
        description: format!("burst cargo build in {remote_path}"),
    };
    log.append(entry).context("cannot write cost log")?;

    println!(
        "month-to-date spend: ${:.2} / ${:.2} cap",
        log.month_to_date_usd(),
        cfg.monthly_budget_usd
    );

    job_result.map(|()| std::process::ExitCode::SUCCESS)
}

/// Standing-box build path (legacy; SSH to a fixed `remote_host`).
fn run_standing_build(args: &BuildArgs, cfg: &Config) -> Result<std::process::ExitCode> {
    let remote_path = args
        .remote_path
        .as_deref()
        .unwrap_or("~/current-project");

    let cargo_cmd = if args.cargo_args.is_empty() {
        "cargo build".to_owned()
    } else {
        format!("cargo build {}", args.cargo_args.join(" "))
    };

    let remote_cmd = format!(
        "cd {remote_path} && RUSTC_WRAPPER=sccache SCCACHE_ENDPOINT={endpoint} SCCACHE_BUCKET={bucket} {cargo_cmd} && sccache --show-stats",
        endpoint = cfg.sccache.endpoint,
        bucket = cfg.sccache.bucket,
    );

    eprintln!("[build] running on {} ...", cfg.remote_host);
    let start = Instant::now();

    let output = std::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            &cfg.remote_host,
            &remote_cmd,
        ])
        .output()
        .context("failed to spawn ssh for remote build")?;

    let elapsed = start.elapsed().as_secs_f64();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    print!("{stdout}");
    eprint!("{stderr}");

    let (hits, misses) = parse_sccache_stats(&stdout);
    println!("\n--- wm-burst build summary ---");
    println!("ran on:      {}", cfg.remote_host);
    println!("elapsed:     {elapsed:.1}s");
    println!("cache hits:  {hits}");
    println!("cache misses:{misses}");

    // Log the job (best-effort).
    let _ = log_build_job(args.cost_log.as_ref(), &cfg.remote_host, remote_path, elapsed);

    if output.status.success() {
        Ok(std::process::ExitCode::SUCCESS)
    } else {
        Err(anyhow::anyhow!("remote build failed (exit {})", output.status))
    }
}

fn log_build_job(
    cost_log_path: Option<&std::path::PathBuf>,
    remote_host: &str,
    remote_path: &str,
    elapsed: f64,
) -> Result<()> {
    let log_path = match cost_log_path {
        Some(p) => p.clone(),
        None => CostLog::default_path()?,
    };
    let mut log = CostLog::load(&log_path).unwrap_or_default();
    let now = chrono::Utc::now();
    // Safe: elapsed is a wall-clock duration in seconds from Instant, so it is
    // always positive and <1e9s, which fits in i64 milliseconds.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::float_arithmetic, clippy::as_conversions)]
    let elapsed_millis = (elapsed * 1000.0) as i64;
    let entry = JobEntry {
        job_id: format!("build-{}", now.timestamp()),
        ran_on: format!("standing-box:{remote_host}"),
        started_at: now - chrono::Duration::milliseconds(elapsed_millis),
        ended_at: Some(now),
        elapsed_secs: Some(elapsed),
        cost_usd: 0.0,
        description: format!("cargo build in {remote_path}"),
    };
    log.append(entry).context("cannot write cost log")
}

/// Parse hit and miss counts from `sccache --show-stats` output.
/// Returns `(hits, misses)`.
#[must_use]
pub fn parse_sccache_stats(stats_output: &str) -> (u64, u64) {
    let mut hits = 0u64;
    let mut misses = 0u64;
    for line in stats_output.lines() {
        if line.contains("Cache hits") {
            if let Some(n) = extract_first_number(line) {
                hits = n;
            }
        } else if line.contains("Cache misses") {
            if let Some(n) = extract_first_number(line) {
                misses = n;
            }
        }
    }
    (hits, misses)
}

fn extract_first_number(s: &str) -> Option<u64> {
    s.split_whitespace()
        .find_map(|tok| tok.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stats_extracts_hits_and_misses() {
        let sample = "Cache hits           42  (78.5%)\nCache misses         12  (21.5%)\n";
        let (hits, misses) = parse_sccache_stats(sample);
        assert_eq!(hits, 42);
        assert_eq!(misses, 12);
    }

    #[test]
    fn parse_stats_returns_zeros_on_empty() {
        let (hits, misses) = parse_sccache_stats("");
        assert_eq!(hits, 0);
        assert_eq!(misses, 0);
    }
}
