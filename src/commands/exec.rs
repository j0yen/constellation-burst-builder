//! `wm-burst exec -- <cmd>` — run a non-cargo CPU job on the remote box.
//!
//! Streams stdout/stderr locally and propagates the exit code faithfully.

use anyhow::{Context, Result};
use clap::Args;

use crate::config::Config;
use crate::cost::{CostLog, JobEntry};

/// Arguments for `wm-burst exec`.
#[derive(Args, Debug)]
pub struct ExecArgs {
    /// Path to config file (default: `~/.config/wm-burst/config.toml`).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Path to cost log (default: `~/.local/share/wm-burst/cost-log.ndjson`).
    #[arg(long)]
    pub cost_log: Option<std::path::PathBuf>,

    /// Command to run on the remote box (everything after `--`).
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

/// Run the `exec` subcommand.
///
/// # Errors
/// Returns an error if config is missing or the SSH invocation fails to spawn.
pub fn run(args: &ExecArgs) -> Result<std::process::ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };
    let cfg = Config::load(Some(&config_path))?;
    cfg.validate()?;

    if args.command.is_empty() {
        anyhow::bail!("no command specified; usage: wm-burst exec -- <command> [args...]");
    }

    let remote_cmd = args.command.join(" ");
    eprintln!("[exec] running on {}: {remote_cmd}", cfg.remote_host);

    let start = std::time::Instant::now();

    // Stream stdout/stderr via inherited handles.
    let status = std::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=10",
            &cfg.remote_host,
            &remote_cmd,
        ])
        .status()
        .context("failed to spawn ssh for remote exec")?;

    let elapsed = start.elapsed().as_secs_f64();

    // Log the job (best-effort — don't mask the real exit code).
    let _ = log_exec_job(args.cost_log.as_ref(), &cfg.remote_host, &remote_cmd, elapsed);

    eprintln!("[exec] finished in {elapsed:.1}s (exit: {status})");

    // Propagate exit code exactly. Unix codes are 0-255; values outside that
    // range are clamped. Negative codes (killed by signal) map to 1.
    let code = status.code().unwrap_or(1);
    let exit_byte: u8 = if code <= 0 {
        1
    } else if code > 255 {
        255
    } else {
        // SAFETY: code is 1..=255, fits in u8.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation, clippy::as_conversions)]
        { code as u8 }
    };
    Ok(std::process::ExitCode::from(exit_byte))
}

fn log_exec_job(
    cost_log_path: Option<&std::path::PathBuf>,
    remote_host: &str,
    remote_cmd: &str,
    elapsed: f64,
) -> Result<()> {
    let log_path = match cost_log_path {
        Some(p) => p.clone(),
        None => CostLog::default_path()?,
    };
    let mut log = CostLog::load(&log_path).unwrap_or_default();
    let now = chrono::Utc::now();
    // Safe: elapsed is a wall-clock duration, always positive and < 1e9s.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::float_arithmetic, clippy::as_conversions)]
    let elapsed_millis = (elapsed * 1000.0) as i64;
    let entry = JobEntry {
        job_id: format!("exec-{}", now.timestamp()),
        ran_on: format!("standing-box:{remote_host}"),
        started_at: now - chrono::Duration::milliseconds(elapsed_millis),
        ended_at: Some(now),
        elapsed_secs: Some(elapsed),
        cost_usd: 0.0,
        description: format!("exec: {remote_cmd}"),
    };
    log.append(entry).context("cannot write cost log")
}
