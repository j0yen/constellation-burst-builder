//! Toolchain version detection and mismatch checking.
//!
//! Reads the local `rust-toolchain.toml` channel and compares it to a
//! remote `rustc --version` string. Hard-fails on any mismatch so that
//! cross-machine toolchain drift cannot silently corrupt builds.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// The channel field from `rust-toolchain.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainChannel(pub String);

/// Parsed `[toolchain]` section of `rust-toolchain.toml`.
#[derive(Debug, Deserialize)]
struct RustToolchainFile {
    toolchain: ToolchainSection,
}

#[derive(Debug, Deserialize)]
struct ToolchainSection {
    channel: String,
}

/// Read the channel from `rust-toolchain.toml` at `project_root`.
///
/// Returns `Err` if the file is absent or unparseable.
///
/// # Errors
/// Returns an error if the file cannot be read or if the TOML is invalid.
pub fn read_local_channel(project_root: &Path) -> Result<ToolchainChannel> {
    let path = project_root.join("rust-toolchain.toml");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let parsed: RustToolchainFile =
        toml::from_str(&raw).with_context(|| format!("invalid rust-toolchain.toml at {}", path.display()))?;
    Ok(ToolchainChannel(parsed.toolchain.channel))
}

/// Parse a `rustc --version` output line (e.g. `rustc 1.85.0 (9fcaa977d 2025-01-10)`)
/// and return the version component (e.g. `1.85.0`).
///
/// Returns `None` if the line doesn't match the expected format.
#[must_use]
pub fn parse_rustc_version(rustc_version_output: &str) -> Option<String> {
    // Expect: "rustc X.Y.Z (hash date)"
    let line = rustc_version_output.trim();
    let after_rustc = line.strip_prefix("rustc ")?;
    // version is the first token
    let version = after_rustc.split_whitespace().next()?;
    Some(version.to_owned())
}

/// Toolchain mismatch diagnostic.
#[derive(Debug, Clone)]
pub struct ToolchainMismatch {
    /// Local channel from `rust-toolchain.toml`.
    pub local: String,
    /// Remote version string from `rustc --version`.
    pub remote: String,
}

impl std::fmt::Display for ToolchainMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "toolchain mismatch: local rust-toolchain.toml requires '{}' but remote reports '{}'",
            self.local, self.remote
        )
    }
}

/// Check that `remote_rustc_version` (output of `rustc --version`) is
/// consistent with `local_channel` from `rust-toolchain.toml`.
///
/// The local channel may be a full version (`1.85.0`) or a partial (`1.85`).
/// We consider them matching if the remote version *starts with* the local
/// channel string after stripping any trailing `.0` from the channel.
///
/// Returns `Ok(())` on match, `Err(ToolchainMismatch)` on mismatch,
/// and `Err(_)` if the remote version string is unparseable.
///
/// # Errors
/// Returns an error if the remote version string cannot be parsed, or if
/// the local and remote toolchain versions do not match.
pub fn check_toolchain_match(
    local_channel: &ToolchainChannel,
    remote_rustc_output: &str,
) -> Result<()> {
    let remote_version =
        parse_rustc_version(remote_rustc_output).context("cannot parse remote rustc --version output")?;

    let local = &local_channel.0;

    // Normalise: strip trailing ".0" so "1.85.0" == "1.85".
    let local_norm = local.strip_suffix(".0").unwrap_or(local.as_str());
    let remote_norm = remote_version
        .strip_suffix(".0")
        .unwrap_or(remote_version.as_str());

    if local_norm == remote_norm || remote_norm.starts_with(&format!("{local_norm}.")) {
        return Ok(());
    }

    Err(anyhow::anyhow!(
        "{}",
        ToolchainMismatch {
            local: local.clone(),
            remote: remote_version,
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_version_matches() {
        let ch = ToolchainChannel("1.85.0".into());
        check_toolchain_match(&ch, "rustc 1.85.0 (9fcaa977d 2025-01-10)").unwrap();
    }

    #[test]
    fn partial_channel_matches_full_version() {
        let ch = ToolchainChannel("1.85".into());
        check_toolchain_match(&ch, "rustc 1.85.0 (9fcaa977d 2025-01-10)").unwrap();
    }

    #[test]
    fn mismatch_returns_error() {
        let ch = ToolchainChannel("1.85.0".into());
        let err = check_toolchain_match(&ch, "rustc 1.88.0 (hash 2025-06-01)").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("1.85.0"), "expected local version in error: {msg}");
        assert!(msg.contains("1.88.0"), "expected remote version in error: {msg}");
    }

    #[test]
    fn unparseable_remote_returns_error() {
        let ch = ToolchainChannel("1.85.0".into());
        let err = check_toolchain_match(&ch, "not a rustc version line").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("parse"), "expected parse error: {msg}");
    }

    #[test]
    fn nightly_vs_stable_mismatch() {
        let ch = ToolchainChannel("nightly".into());
        let err = check_toolchain_match(&ch, "rustc 1.85.0 (hash 2025-01-10)").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("nightly"), "error should mention local: {msg}");
    }
}
