//! `wm-burst doctor` — verify remote reachability, toolchain match, and sccache.
//!
//! Hard-fails when the remote rustc set does not match `rust-toolchain.toml`,
//! printing the exact local-vs-remote mismatch. This is the guardrail against
//! cross-machine toolchain drift that has produced spurious compile failures before.

use anyhow::{Context, Result};
use clap::Args;

use crate::config::Config;
use crate::toolchain::{check_toolchain_match, read_local_channel};

/// Arguments for `wm-burst doctor`.
#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Path to config file (default: `~/.config/wm-burst/config.toml`).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Project root to read `rust-toolchain.toml` from (default: current directory).
    #[arg(long, default_value = ".")]
    pub project_root: std::path::PathBuf,

    /// Skip the live SSH/sccache checks (toolchain check only).
    /// Useful for unit tests.
    #[arg(long, hide = true)]
    pub offline: bool,
}

/// Run the `doctor` subcommand.
///
/// # Errors
/// Returns an error if any check fails (e.g. toolchain mismatch, SSH unreachable).
pub fn run(args: &DoctorArgs) -> Result<std::process::ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };
    let cfg = Config::load(Some(&config_path))?;
    cfg.validate()?;

    let mut any_fail = false;

    // 1. Toolchain check.
    eprintln!("[doctor] reading local rust-toolchain.toml ...");
    let local_channel = read_local_channel(&args.project_root)
        .context("cannot read rust-toolchain.toml; run doctor from a project root")?;
    eprintln!("[doctor] local channel: {}", local_channel.0);

    if args.offline {
        eprintln!("[doctor] offline mode — skipping SSH/sccache checks");
        println!("toolchain (offline): local = {}", local_channel.0);
        return Ok(std::process::ExitCode::SUCCESS);
    }

    // 2. Remote SSH reachability + rustc version.
    eprintln!("[doctor] checking remote {} ...", cfg.remote_host);
    match run_ssh_command(&cfg.remote_host, "rustc --version") {
        Ok(output) => {
            eprintln!("[doctor] remote rustc: {output}");
            match check_toolchain_match(&local_channel, &output) {
                Ok(()) => {
                    println!("[doctor] toolchain: OK (local={}, remote={})", local_channel.0, output.trim());
                }
                Err(e) => {
                    eprintln!("[doctor] FAIL: {e}");
                    any_fail = true;
                }
            }
        }
        Err(e) => {
            eprintln!("[doctor] FAIL: cannot reach remote {}: {e}", cfg.remote_host);
            any_fail = true;
        }
    }

    // 3. sccache bucket writability (simple probe via sccache --show-stats over SSH).
    eprintln!("[doctor] checking sccache bucket writability ...");
    let sccache_check_cmd = format!(
        "SCCACHE_ENDPOINT={} SCCACHE_BUCKET={} sccache --show-stats",
        cfg.sccache.endpoint, cfg.sccache.bucket
    );
    match run_ssh_command(&cfg.remote_host, &sccache_check_cmd) {
        Ok(output) => {
            let hit_rate = parse_cache_hit_rate(&output);
            println!("[doctor] sccache: OK (hit_rate={hit_rate:.1}%)");
        }
        Err(e) => {
            eprintln!("[doctor] WARN: sccache check failed: {e}");
            // Advisory only — sccache may not be running yet after fresh provision.
        }
    }

    if any_fail {
        Err(anyhow::anyhow!("one or more doctor checks failed; see output above"))
    } else {
        println!("[doctor] all checks passed");
        Ok(std::process::ExitCode::SUCCESS)
    }
}

/// Run a command on the remote host via SSH and return stdout.
///
/// # Errors
/// Returns an error if SSH cannot be spawned or exits non-zero.
pub fn run_ssh_command(host: &str, cmd: &str) -> Result<String> {
    let output = std::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=10",
            host,
            cmd,
        ])
        .output()
        .with_context(|| format!("failed to spawn ssh to {host}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ssh {host} `{cmd}` exited {}: {stderr}", output.status);
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse a rough cache hit-rate percentage from `sccache --show-stats` output.
/// Returns `0.0` if not found.
fn parse_cache_hit_rate(stats_output: &str) -> f64 {
    // sccache --show-stats prints lines like "Cache hits  123  (45.6%)"
    for line in stats_output.lines() {
        if line.contains("Cache hits") {
            if let Some(pct_start) = line.find('(') {
                if let Some(pct_end) = line.find('%') {
                    let pct_str = line[pct_start + 1..pct_end].trim();
                    if let Ok(pct) = pct_str.parse::<f64>() {
                        return pct;
                    }
                }
            }
        }
    }
    0.0
}
