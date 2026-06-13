//! `wm-burst cost` — show cost breakdown: burst spend, hub standing charge,
//! month-to-date, month projection, and whether the budget cap is breached.
//!
//! Exits non-zero when the monthly projection exceeds the budget cap.

use anyhow::{Context, Result};
use clap::Args;
use std::process::ExitCode;

use crate::config::Config;
use crate::cost::{CostLog, CostReport, compute_cost_report};

/// Arguments for `wm-burst cost`.
#[derive(Args, Debug)]
pub struct CostArgs {
    /// Emit JSON instead of the human-readable report.
    #[arg(long)]
    pub json: bool,

    /// Path to the config file (default: `~/.config/wm-burst/config.toml`).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Path to the cost log (default: `~/.local/share/wm-burst/cost-log.ndjson`).
    #[arg(long)]
    pub cost_log: Option<std::path::PathBuf>,
}

/// Run the `cost` subcommand.
///
/// # Errors
/// Returns an error if config or cost log cannot be loaded.
pub fn run(args: &CostArgs) -> Result<ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };
    let cost_log_path = match &args.cost_log {
        Some(p) => p.clone(),
        None => CostLog::default_path()?,
    };

    let cfg = Config::load(Some(&config_path))?;
    let log = CostLog::load(&cost_log_path)
        .with_context(|| format!("cannot load cost log at {}", cost_log_path.display()))?;

    let now = chrono::Utc::now();
    let report = compute_cost_report(&log, now, cfg.monthly_budget_usd);

    print_report(&report, args.json)?;

    if report.over_cap {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// Print `report` in human or JSON form.
///
/// # Errors
/// Returns a serialization error for JSON output.
pub fn print_report(report: &CostReport, json: bool) -> Result<()> {
    if json {
        let s = serde_json::to_string_pretty(report).context("cannot serialize CostReport")?;
        println!("{s}");
    } else {
        println!("wm-burst cost report");
        println!("  burst spend (this month):  ${:.4}", report.burst_usd);
        println!("  hub standing charge:       ${:.4}", report.hub_standing_usd);
        println!("  month-to-date:             ${:.4}", report.month_to_date_usd);
        println!("  projected month-end:       ${:.4}", report.month_projection_usd);
        println!("  budget cap:                ${:.4}", report.budget_usd);
        for note in &report.rate_notes {
            println!("  NOTE: {note}");
        }
        if report.over_cap {
            println!(
                "WARN: projected month-end ${:.4} exceeds budget cap ${:.4}",
                report.month_projection_usd, report.budget_usd
            );
        }
    }
    Ok(())
}
