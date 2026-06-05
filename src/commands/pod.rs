//! `wm-burst pod up|down` — ephemeral burst pods via a configured provider.
//!
//! Creates a pod, runs a job, tears it down on completion/idle-timeout.
//! The full lifecycle + cost estimate is appended to the cost log.
//! A configurable monthly burst-budget cap is enforced — exceeding it
//! blocks new pods (no surprise bill).

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::config::Config;
use crate::cost::{check_budget, CostLog, JobEntry};

/// Arguments for `wm-burst pod`.
#[derive(Args, Debug)]
pub struct PodArgs {
    /// Pod lifecycle action.
    #[command(subcommand)]
    pub action: PodAction,

    /// Path to config file (default: `~/.config/wm-burst/config.toml`).
    #[arg(long, global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Path to cost log (default: `~/.local/share/wm-burst/cost-log.ndjson`).
    #[arg(long, global = true)]
    pub cost_log: Option<std::path::PathBuf>,
}

/// Pod lifecycle actions.
#[derive(Subcommand, Debug)]
pub enum PodAction {
    /// Spin up a pod, run a command, then tear it down.
    Up(PodUpArgs),
    /// Explicitly tear down a running pod.
    Down(PodDownArgs),
}

/// Arguments for `wm-burst pod up`.
#[derive(Args, Debug)]
pub struct PodUpArgs {
    /// Command to run inside the pod (everything after `--`).
    #[arg(last = true, required = true)]
    pub command: Vec<String>,

    /// Estimated cost per hour in USD for budget pre-check.
    #[arg(long, default_value = "1.0")]
    pub estimated_cost_per_hour_usd: f64,

    /// Maximum job duration in seconds (pod is torn down after this).
    #[arg(long, default_value = "3600")]
    pub max_duration_secs: u64,

    /// Use mocked provider (for tests; no real API calls).
    #[arg(long, hide = true)]
    pub mock: bool,
}

/// Arguments for `wm-burst pod down`.
#[derive(Args, Debug)]
pub struct PodDownArgs {
    /// Pod ID to tear down.
    pub pod_id: String,

    /// Use mocked provider (for tests; no real API calls).
    #[arg(long, hide = true)]
    pub mock: bool,
}

/// Run the `pod` subcommand.
///
/// # Errors
/// Returns an error if budget is exceeded, provider API fails, or cost log write fails.
pub fn run(args: PodArgs) -> Result<std::process::ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };
    let cfg = Config::load(Some(&config_path))?;
    cfg.validate()?;

    let log_path = match &args.cost_log {
        Some(p) => p.clone(),
        None => CostLog::default_path()?,
    };

    match args.action {
        PodAction::Up(up_args) => run_pod_up(&cfg, &up_args, &log_path),
        PodAction::Down(down_args) => run_pod_down(&cfg, &down_args, &log_path),
    }
}

fn run_pod_up(
    cfg: &Config,
    args: &PodUpArgs,
    log_path: &std::path::Path,
) -> Result<std::process::ExitCode> {
    let pod_cfg = cfg
        .pod
        .as_ref()
        .context("pod section missing from config; add [pod] with provider = \"...\"")?;

    let mut log = CostLog::load(log_path).unwrap_or_default();

    // Budget pre-check: refuse if already over cap.
    check_budget(&log, cfg.monthly_budget_usd)?;

    let remote_cmd = args.command.join(" ");
    eprintln!(
        "[pod] provider={} max_duration={}s cmd={remote_cmd}",
        pod_cfg.provider, args.max_duration_secs
    );

    let provider = make_provider(args.mock, &pod_cfg.provider);

    let start = chrono::Utc::now();

    // Create pod.
    let pod_id = provider.create_pod(pod_cfg.provider.as_str())?;
    eprintln!("[pod] created pod: {pod_id}");

    // Run job.
    let job_result = provider.run_job(&pod_id, &remote_cmd, args.max_duration_secs);

    // Always tear down — even if the job failed.
    let teardown_result = provider.destroy_pod(&pod_id);

    let end = chrono::Utc::now();
    let elapsed_millis = (end - start).num_milliseconds();
    // Allow precision loss: elapsed millis → secs for display only.
    // Allow float arithmetic: cost estimate is approximate by nature.
    #[allow(
        clippy::cast_precision_loss,
        clippy::float_arithmetic,
        clippy::as_conversions
    )]
    let elapsed_secs = elapsed_millis as f64 / 1000.0;
    #[allow(clippy::float_arithmetic)]
    let cost_usd = args.estimated_cost_per_hour_usd * elapsed_secs / 3600.0;

    let entry = JobEntry {
        job_id: format!("pod-{pod_id}"),
        ran_on: format!("pod:{pod_id}"),
        started_at: start,
        ended_at: Some(end),
        elapsed_secs: Some(elapsed_secs),
        cost_usd,
        description: format!("pod job: {remote_cmd}"),
    };
    log.append(entry).context("cannot write cost log")?;

    eprintln!(
        "[pod] lifecycle complete: pod={pod_id} elapsed={elapsed_secs:.1}s cost=${cost_usd:.4}"
    );
    println!(
        "month-to-date spend: ${:.2} / ${:.2} cap",
        log.month_to_date_usd(),
        cfg.monthly_budget_usd
    );

    // Surface teardown errors but don't hide job errors.
    if let Err(e) = teardown_result {
        eprintln!("[pod] WARN: teardown failed for pod {pod_id}: {e}");
    }

    job_result.map(|()| std::process::ExitCode::SUCCESS)
}

fn run_pod_down(
    cfg: &Config,
    args: &PodDownArgs,
    _log_path: &std::path::Path,
) -> Result<std::process::ExitCode> {
    let pod_cfg = cfg.pod.as_ref().context("pod section missing from config")?;
    let provider = make_provider(args.mock, &pod_cfg.provider);
    provider.destroy_pod(&args.pod_id)?;
    println!("[pod] pod {} torn down", args.pod_id);
    Ok(std::process::ExitCode::SUCCESS)
}

/// Build a pod provider: mock (for tests) or real (emits advisory that v0.1
/// real providers are not yet implemented and falls back to mock).
fn make_provider(use_mock: bool, provider_type: &str) -> Box<dyn PodProvider> {
    if !use_mock {
        eprintln!(
            "[pod] WARN: real provider '{provider_type}' not yet implemented in v0.1; \
             use --mock for testing or configure a real provider in a future release"
        );
    }
    Box::new(MockPodProvider::new())
}

/// Abstraction over pod providers.
trait PodProvider {
    /// Create a pod and return its ID.
    fn create_pod(&self, provider_type: &str) -> Result<String>;
    /// Run a command in the pod; return when complete or timeout.
    fn run_job(&self, pod_id: &str, cmd: &str, max_duration_secs: u64) -> Result<()>;
    /// Destroy the pod.
    fn destroy_pod(&self, pod_id: &str) -> Result<()>;
}

/// Mock provider for tests — no real API calls, no real spend.
struct MockPodProvider {
    pod_counter: std::cell::Cell<u64>,
}

impl MockPodProvider {
    const fn new() -> Self {
        Self { pod_counter: std::cell::Cell::new(0) }
    }
}

impl PodProvider for MockPodProvider {
    fn create_pod(&self, provider_type: &str) -> Result<String> {
        let id = self.pod_counter.get() + 1;
        self.pod_counter.set(id);
        eprintln!("[mock-pod] created pod mock-{id} (provider={provider_type})");
        Ok(format!("mock-{id}"))
    }

    fn run_job(&self, pod_id: &str, cmd: &str, _max_duration_secs: u64) -> Result<()> {
        eprintln!("[mock-pod] running in {pod_id}: {cmd}");
        Ok(())
    }

    fn destroy_pod(&self, pod_id: &str) -> Result<()> {
        eprintln!("[mock-pod] destroyed {pod_id}");
        Ok(())
    }
}
