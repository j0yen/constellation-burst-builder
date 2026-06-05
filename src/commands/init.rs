//! `wm-burst init` — write or show `~/.config/wm-burst/config.toml`.

use anyhow::{Context, Result};
use clap::Args;

use crate::config::{Config, PodConfig, SccacheConfig};

/// Arguments for `wm-burst init`.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// SSH alias or hostname of the dedicated builder.
    #[arg(long)]
    pub remote_host: Option<String>,

    /// sccache S3-compatible endpoint URL.
    #[arg(long)]
    pub sccache_endpoint: Option<String>,

    /// sccache bucket name.
    #[arg(long)]
    pub sccache_bucket: Option<String>,

    /// Monthly burst-budget cap in USD.
    #[arg(long, default_value = "50.0")]
    pub monthly_budget_usd: f64,

    /// Pod provider: runpod, vast, or hetzner-cloud.
    #[arg(long)]
    pub pod_provider: Option<String>,

    /// Show current config without modifying it.
    #[arg(long, short = 's')]
    pub show: bool,

    /// Path to config file (default: ~/.config/wm-burst/config.toml).
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,
}

/// Run the `init` subcommand.
///
/// # Errors
/// Returns an error if the config cannot be read, written, or validated.
pub fn run(args: InitArgs) -> Result<std::process::ExitCode> {
    let config_path = match &args.config {
        Some(p) => p.clone(),
        None => Config::default_path()?,
    };

    if args.show {
        // Load and pretty-print existing config.
        let cfg = Config::load(Some(&config_path)).with_context(|| {
            format!(
                "no config found at {}; run `wm-burst init --remote-host <host> --sccache-endpoint <url> --sccache-bucket <bucket>` to create one",
                config_path.display()
            )
        })?;
        cfg.validate()?;
        let raw = toml::to_string_pretty(&cfg).context("cannot serialise config")?;
        println!("{raw}");
        return Ok(std::process::ExitCode::SUCCESS);
    }

    // Build or merge config.
    let cfg = if config_path.exists() {
        // Load existing and patch fields that were provided.
        let mut existing = Config::load(Some(&config_path))?;
        if let Some(h) = args.remote_host {
            existing.remote_host = h;
        }
        if let Some(e) = args.sccache_endpoint {
            existing.sccache.endpoint = e;
        }
        if let Some(b) = args.sccache_bucket {
            existing.sccache.bucket = b;
        }
        existing.monthly_budget_usd = args.monthly_budget_usd;
        if let Some(provider) = args.pod_provider {
            let pod = existing.pod.get_or_insert(PodConfig {
                provider: String::new(),
                api_key: None,
                idle_timeout_secs: 300,
            });
            pod.provider = provider;
        }
        existing
    } else {
        // Create new config; require the mandatory fields.
        let remote_host = args.remote_host.context(
            "missing --remote-host; provide the SSH alias or hostname of the builder",
        )?;
        let sccache_endpoint = args.sccache_endpoint.context(
            "missing --sccache-endpoint; provide the S3-compatible endpoint URL (e.g. http://builder:9000)",
        )?;
        let sccache_bucket = args
            .sccache_bucket
            .context("missing --sccache-bucket; provide the bucket name")?;
        let pod = args.pod_provider.map(|provider| PodConfig {
            provider,
            api_key: None,
            idle_timeout_secs: 300,
        });
        Config {
            remote_host,
            sccache: SccacheConfig {
                endpoint: sccache_endpoint,
                bucket: sccache_bucket,
                access_key: None,
                secret_key: None,
            },
            monthly_budget_usd: args.monthly_budget_usd,
            pod,
        }
    };

    cfg.validate()?;
    cfg.save(&config_path)?;
    println!(
        "config written to {}",
        config_path.display()
    );
    Ok(std::process::ExitCode::SUCCESS)
}
