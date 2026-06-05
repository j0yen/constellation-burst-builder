//! `wm-burst status` — show standing box load, cache hit rate,
//! month-to-date burst spend vs cap, and last N jobs.

use anyhow::{Context, Result};
use clap::Args;

use crate::config::Config;
use crate::cost::{CostLog, JobEntry};

/// Arguments for `wm-burst status`.
#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Path to config file (default: `~/.config/wm-burst/config.toml`).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Path to cost log (default: `~/.local/share/wm-burst/cost-log.ndjson`).
    #[arg(long)]
    pub cost_log: Option<std::path::PathBuf>,

    /// Number of recent jobs to display.
    #[arg(long, short = 'n', default_value = "10")]
    pub last_n: usize,

    /// Output JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,

    /// Skip live SSH probe (for offline/test use).
    #[arg(long, hide = true)]
    pub offline: bool,
}

/// Remote load averages from `/proc/loadavg`.
struct LoadAvg {
    /// 1-minute load average.
    one: Option<f64>,
    /// 5-minute load average.
    five: Option<f64>,
    /// 15-minute load average.
    fifteen: Option<f64>,
}

/// Budget summary.
struct Budget {
    spent: f64,
    cap: f64,
}

impl Budget {
    #[allow(clippy::float_arithmetic)]
    fn remaining(&self) -> f64 {
        if self.cap > self.spent { self.cap - self.spent } else { 0.0 }
    }
}

/// Run the `status` subcommand.
///
/// # Errors
/// Returns an error if config or cost log cannot be read.
pub fn run(args: &StatusArgs) -> Result<std::process::ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };

    let cfg = Config::load(Some(&config_path)).with_context(|| {
        format!(
            "cannot load config from {}; run `wm-burst init` first",
            config_path.display()
        )
    })?;
    cfg.validate()?;

    let log_path = match &args.cost_log {
        Some(p) => p.clone(),
        None => CostLog::default_path()?,
    };
    let log = CostLog::load(&log_path).unwrap_or_default();

    let budget = Budget { spent: log.month_to_date_usd(), cap: cfg.monthly_budget_usd };
    let last_jobs = log.last_n(args.last_n);
    let load = if args.offline {
        LoadAvg { one: None, five: None, fifteen: None }
    } else {
        probe_remote_load(&cfg.remote_host)
    };

    if args.json {
        print_json(&cfg.remote_host, &load, &budget, last_jobs)?;
    } else {
        print_text(&cfg.remote_host, &load, &budget, last_jobs);
    }

    Ok(std::process::ExitCode::SUCCESS)
}

fn print_json(
    remote_host: &str,
    load: &LoadAvg,
    budget: &Budget,
    last_jobs: &[JobEntry],
) -> Result<()> {
    let jobs_json: Vec<serde_json::Value> = last_jobs
        .iter()
        .map(|e| {
            serde_json::json!({
                "job_id": e.job_id,
                "ran_on": e.ran_on,
                "started_at": e.started_at.to_rfc3339(),
                "elapsed_secs": e.elapsed_secs,
                "cost_usd": e.cost_usd,
                "description": e.description,
            })
        })
        .collect();

    let out = serde_json::json!({
        "remote_host": remote_host,
        "load_1m": load.one,
        "load_5m": load.five,
        "load_15m": load.fifteen,
        "month_to_date_usd": budget.spent,
        "monthly_budget_usd": budget.cap,
        "budget_remaining_usd": budget.remaining(),
        "last_jobs": jobs_json,
    });
    println!("{}", serde_json::to_string_pretty(&out).context("cannot serialize status")?);
    Ok(())
}

fn print_text(
    remote_host: &str,
    load: &LoadAvg,
    budget: &Budget,
    last_jobs: &[JobEntry],
) {
    println!("=== wm-burst status ===");
    println!("remote host:     {remote_host}");
    match (load.one, load.five, load.fifteen) {
        (Some(l1), Some(l5), Some(l15)) => {
            println!("load average:    {l1:.2} {l5:.2} {l15:.2} (1m 5m 15m)");
        }
        _ => println!("load average:    (offline or unavailable)"),
    }
    println!(
        "burst spend:     ${:.2} / ${:.2} (${:.2} remaining)",
        budget.spent,
        budget.cap,
        budget.remaining()
    );
    println!();
    if last_jobs.is_empty() {
        println!("no jobs recorded yet");
    } else {
        println!(
            "{:<25} {:<30} {:>8}  {:>8}  description",
            "job_id", "ran_on", "elapsed", "cost"
        );
        println!("{}", "-".repeat(100));
        for job in last_jobs {
            let elapsed = job
                .elapsed_secs
                .map_or_else(|| "-".into(), |s| format!("{s:.1}s"));
            let cost = format!("${:.4}", job.cost_usd);
            println!(
                "{:<25} {:<30} {:>8}  {:>8}  {}",
                job.job_id, job.ran_on, elapsed, cost, job.description
            );
        }
    }
}

/// Probe the standing box's load average via SSH.
fn probe_remote_load(host: &str) -> LoadAvg {
    let output = std::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=5",
            host,
            "cat /proc/loadavg",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return LoadAvg { one: None, five: None, fifteen: None },
    };

    let s = String::from_utf8_lossy(&output.stdout);
    // /proc/loadavg: "0.00 0.01 0.05 1/150 12345"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 3 {
        return LoadAvg { one: None, five: None, fifteen: None };
    }

    LoadAvg {
        one: parts.first().and_then(|v| v.parse::<f64>().ok()),
        five: parts.get(1).and_then(|v| v.parse::<f64>().ok()),
        fifteen: parts.get(2).and_then(|v| v.parse::<f64>().ok()),
    }
}
