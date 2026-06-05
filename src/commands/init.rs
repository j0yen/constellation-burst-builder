//! `wm-burst init` — write or show `~/.config/wm-burst/config.toml`.

use anyhow::{Context, Result};
use clap::Args;

use crate::config::{Config, HcloudPodConfig, PodConfig, SccacheConfig};

/// Arguments for `wm-burst init`.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// SSH alias or hostname of the dedicated builder (legacy standing-box).
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

    /// Pod provider: `hcloud` (Hetzner Cloud), `runpod`, or `vast`.
    /// Use `hcloud` for the on-demand x86 burst flow.
    #[arg(long)]
    pub pod_provider: Option<String>,

    /// Hetzner Cloud server type (default: `ccx23`).
    #[arg(long)]
    pub hcloud_server_type: Option<String>,

    /// Hetzner Cloud datacenter location (default: `fsn1`).
    #[arg(long)]
    pub hcloud_location: Option<String>,

    /// Hetzner Cloud snapshot/image name or ID (default: `ubuntu-24.04`).
    #[arg(long)]
    pub hcloud_image: Option<String>,

    /// Name of the SSH key registered in your hcloud project.
    #[arg(long)]
    pub hcloud_ssh_key: Option<String>,

    /// Remote Rust target triple (default: `x86_64-unknown-linux-gnu`).
    #[arg(long)]
    pub hcloud_remote_arch: Option<String>,

    /// sccache endpoint for the Hetzner Object Storage bucket.
    #[arg(long)]
    pub hcloud_sccache_endpoint: Option<String>,

    /// sccache bucket name on Hetzner Object Storage.
    #[arg(long)]
    pub hcloud_sccache_bucket: Option<String>,

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
pub fn run(args: &InitArgs) -> Result<std::process::ExitCode> {
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
        if let Some(ref h) = args.remote_host {
            existing.remote_host.clone_from(h);
        }
        if let Some(ref e) = args.sccache_endpoint {
            existing.sccache.endpoint.clone_from(e);
        }
        if let Some(ref b) = args.sccache_bucket {
            existing.sccache.bucket.clone_from(b);
        }
        existing.monthly_budget_usd = args.monthly_budget_usd;
        if let Some(provider) = &args.pod_provider {
            let pod = existing.pod.get_or_insert(PodConfig {
                provider: String::new(),
                api_key: None,
                idle_timeout_secs: 300,
            });
            pod.provider.clone_from(provider);
        }
        // Patch hcloud section if any hcloud flags were provided.
        patch_hcloud(&mut existing, args);
        existing
    } else {
        // Create new config; require the mandatory fields.
        let remote_host = args.remote_host.clone().unwrap_or_default();
        let sccache_endpoint = args
            .sccache_endpoint
            .clone()
            .unwrap_or_else(|| "https://fsn1.your-objectstorage.com".into());
        let sccache_bucket = args
            .sccache_bucket
            .clone()
            .unwrap_or_else(|| "wm-sccache".into());
        let pod = args.pod_provider.as_deref().map(|provider| PodConfig {
            provider: provider.to_owned(),
            api_key: None,
            idle_timeout_secs: 300,
        });
        let mut cfg = Config {
            remote_host,
            sccache: SccacheConfig {
                endpoint: sccache_endpoint,
                bucket: sccache_bucket,
                access_key: None,
                secret_key: None,
            },
            monthly_budget_usd: args.monthly_budget_usd,
            pod,
            hcloud: None,
        };
        patch_hcloud(&mut cfg, args);
        cfg
    };

    cfg.validate()?;
    cfg.save(&config_path)?;
    println!(
        "config written to {}",
        config_path.display()
    );
    Ok(std::process::ExitCode::SUCCESS)
}

/// Apply any `--hcloud-*` flags onto `cfg.hcloud`, creating the section if any flag is present.
fn patch_hcloud(cfg: &mut Config, args: &InitArgs) {
    let any_hcloud = args.hcloud_server_type.is_some()
        || args.hcloud_location.is_some()
        || args.hcloud_image.is_some()
        || args.hcloud_ssh_key.is_some()
        || args.hcloud_remote_arch.is_some()
        || args.hcloud_sccache_endpoint.is_some()
        || args.hcloud_sccache_bucket.is_some()
        || args.pod_provider.as_deref() == Some("hcloud");

    if !any_hcloud {
        return;
    }

    let hc = cfg.hcloud.get_or_insert_with(HcloudPodConfig::default);

    if let Some(v) = &args.hcloud_server_type {
        hc.server_type.clone_from(v);
    }
    if let Some(v) = &args.hcloud_location {
        hc.location.clone_from(v);
    }
    if let Some(v) = &args.hcloud_image {
        hc.image.clone_from(v);
    }
    if let Some(v) = &args.hcloud_ssh_key {
        hc.ssh_key_name.clone_from(v);
    }
    if let Some(v) = &args.hcloud_remote_arch {
        hc.remote_arch.clone_from(v);
    }
    if let Some(v) = &args.hcloud_sccache_endpoint {
        hc.sccache_endpoint.clone_from(v);
    }
    if let Some(v) = &args.hcloud_sccache_bucket {
        hc.sccache_bucket.clone_from(v);
    }
}
