//! Configuration file management for wm-burst.
//!
//! Config lives at `~/.config/wm-burst/config.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level wm-burst configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// SSH alias or host for the dedicated builder.
    pub remote_host: String,
    /// sccache shared object store configuration.
    pub sccache: SccacheConfig,
    /// Monthly burst-budget cap in USD.
    pub monthly_budget_usd: f64,
    /// Optional pod provider configuration.
    pub pod: Option<PodConfig>,
}

/// sccache shared object store configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct SccacheConfig {
    /// `S3`-compatible endpoint URL (e.g. `http://builder-box:9000`).
    pub endpoint: String,
    /// Bucket name.
    pub bucket: String,
    /// `AWS_ACCESS_KEY_ID` equivalent.
    // NOTE: access_key/secret_key excluded from Debug to avoid leaking credentials via {:?}.
    pub access_key: Option<String>,
    /// `AWS_SECRET_ACCESS_KEY` equivalent.
    pub secret_key: Option<String>,
}

impl std::fmt::Debug for SccacheConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SccacheConfig")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("access_key", &self.access_key.as_ref().map(|_| "<redacted>"))
            .field("secret_key", &self.secret_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Optional ephemeral pod provider configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct PodConfig {
    /// Provider name: `runpod`, `vast`, or `hetzner-cloud`.
    pub provider: String,
    /// Provider API key (stored in config; user's responsibility to protect the file).
    // NOTE: api_key is intentionally excluded from Debug to prevent leaking secrets via {:?}.
    pub api_key: Option<String>,
    /// Idle timeout in seconds before the pod is torn down.
    pub idle_timeout_secs: u64,
}

impl std::fmt::Debug for PodConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PodConfig")
            .field("provider", &self.provider)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .finish()
    }
}

impl Config {
    /// Default path: `~/.config/wm-burst/config.toml`.
    ///
    /// # Errors
    /// Returns an error if the home directory cannot be determined.
    pub fn default_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        Ok(home.join(".config").join("wm-burst").join("config.toml"))
    }

    /// Load configuration from `path`, or from the default path if `path` is `None`.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or if the TOML is invalid.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let p = match path {
            Some(p) => p.to_owned(),
            None => Self::default_path()?,
        };
        let raw = std::fs::read_to_string(&p)
            .with_context(|| format!("cannot read config at {}", p.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("invalid config at {}: check field names and types", p.display()))
    }

    /// Save configuration to `path`, creating parent directories as needed.
    ///
    /// # Errors
    /// Returns an error if the parent directory cannot be created or if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create config dir {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self).context("cannot serialize config")?;
        std::fs::write(path, raw)
            .with_context(|| format!("cannot write config to {}", path.display()))
    }

    /// Validate that all required fields are non-empty / sane.
    ///
    /// # Errors
    /// Returns an error describing which field is missing or invalid.
    pub fn validate(&self) -> Result<()> {
        if self.remote_host.trim().is_empty() {
            anyhow::bail!("remote_host must not be empty");
        }
        if self.sccache.endpoint.trim().is_empty() {
            anyhow::bail!("sccache.endpoint must not be empty");
        }
        if self.sccache.bucket.trim().is_empty() {
            anyhow::bail!("sccache.bucket must not be empty");
        }
        if self.monthly_budget_usd < 0.0 {
            anyhow::bail!("monthly_budget_usd must be >= 0");
        }
        if let Some(pod) = &self.pod {
            if pod.provider.trim().is_empty() {
                anyhow::bail!("pod.provider must not be empty when pod section is present");
            }
            if !["runpod", "vast", "hetzner-cloud"].contains(&pod.provider.as_str()) {
                anyhow::bail!(
                    "pod.provider '{}' is unknown; valid values: runpod, vast, hetzner-cloud",
                    pod.provider
                );
            }
        }
        Ok(())
    }
}
