//! Pod provider abstraction and implementations.
//!
//! This module defines the [`PodProvider`] trait and ships two implementations:
//!
//! * [`HcloudPodProvider`] — real Hetzner Cloud API (selected when `[pod] provider = "hcloud"`).
//!   Requires `$HCLOUD_TOKEN` in the environment.  The token is never written to logs or config.
//! * [`MockPodProvider`] — no-op implementation for tests and the `--mock` flag.

use anyhow::{Context, Result};

use crate::config::HcloudPodConfig;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over ephemeral pod providers.
pub trait PodProvider {
    /// Create a pod and return its opaque string ID.
    ///
    /// # Errors
    /// Returns an error if the provider API call fails or the response is malformed.
    fn create_pod(&self, cfg: &HcloudPodConfig) -> Result<String>;

    /// Run a shell command inside `pod_id` and wait for completion.
    ///
    /// Returns `Ok(())` on exit code 0, `Err` otherwise.
    ///
    /// # Errors
    /// Returns an error if the SSH connection fails or the command exits non-zero.
    fn run_job(&self, pod_id: &str, ip: &str, cmd: &str, max_secs: u64) -> Result<()>;

    /// Destroy the pod identified by `pod_id`.
    ///
    /// Must be idempotent: safe to call even when `pod_id` is already gone or
    /// was only half-created.
    ///
    /// # Errors
    /// Returns an error only if the destroy API call fails *and* the server may
    /// still be running (i.e. a 404 / already-gone response is treated as success).
    fn destroy_pod(&self, pod_id: &str) -> Result<()>;
}

// ---------------------------------------------------------------------------
// HcloudPodProvider
// ---------------------------------------------------------------------------

/// Hetzner Cloud implementation of [`PodProvider`].
///
/// Uses the hcloud REST API v1 (`api.hetzner.cloud/v1`).
/// The API token is read once from `$HCLOUD_TOKEN` at construction time and
/// **never** surfaced to logs, config, or error messages.
pub struct HcloudPodProvider {
    // Token stored as bytes to make it marginally harder to accidentally Debug-print.
    token: Box<str>,
}

impl HcloudPodProvider {
    /// Construct by reading `$HCLOUD_TOKEN` from the environment.
    ///
    /// # Errors
    /// Returns an error if `$HCLOUD_TOKEN` is not set or is empty.
    pub fn from_env() -> Result<Self> {
        let token = std::env::var("HCLOUD_TOKEN")
            .context("$HCLOUD_TOKEN is not set; export your Hetzner Cloud API token")?;
        Self::from_token(&token)
    }

    /// Construct from a token string directly (useful for testing the empty-token guard).
    ///
    /// # Errors
    /// Returns an error if `token` is empty or whitespace-only.
    pub fn from_token(token: &str) -> Result<Self> {
        if token.trim().is_empty() {
            anyhow::bail!("HCLOUD_TOKEN is empty; provide a valid Hetzner Cloud API token");
        }
        Ok(Self { token: token.into() })
    }

    /// Bearer-auth header value — used in HTTP calls but never logged.
    fn auth_header(&self) -> String {
        // Intentionally not public and never added to log/display paths.
        format!("Bearer {}", self.token)
    }

    /// Call the hcloud v1 API with a JSON body, returning the response body string.
    ///
    /// This is a thin wrapper over `curl` to avoid adding an HTTP-client dependency
    /// for this first implementation.  A future iteration can swap to `reqwest`.
    ///
    /// # Errors
    /// Returns an error if `curl` fails or the HTTP response indicates an error.
    fn hcloud_post(&self, path: &str, body: &str) -> Result<String> {
        let url = format!("https://api.hetzner.cloud/v1{path}");
        let auth = self.auth_header();
        let output = std::process::Command::new("curl")
            .args([
                "-sS",
                "-X", "POST",
                "-H", "Content-Type: application/json",
                "-H", &format!("Authorization: {auth}"),
                "-w", "\n__HTTP_STATUS__%{http_code}",
                "-d", body,
                &url,
            ])
            .output()
            .context("failed to spawn curl for hcloud API")?;

        let raw = String::from_utf8_lossy(&output.stdout).into_owned();
        let (body_part, status_part) = Self::split_curl_status(&raw);
        let status: u16 = status_part
            .trim()
            .parse()
            .unwrap_or(0);
        if !(200..300).contains(&status) {
            anyhow::bail!(
                "hcloud API POST {path} returned HTTP {status}; body: {}",
                // Trim to avoid giant JSON blobs in errors.
                body_part.chars().take(400).collect::<String>()
            );
        }
        Ok(body_part.to_owned())
    }

    /// Delete request against the hcloud API.
    ///
    /// # Errors
    /// Returns an error if `curl` fails, *unless* the status is 404 (already gone).
    fn hcloud_delete(&self, path: &str) -> Result<()> {
        let url = format!("https://api.hetzner.cloud/v1{path}");
        let auth = self.auth_header();
        let output = std::process::Command::new("curl")
            .args([
                "-sS",
                "-X", "DELETE",
                "-H", &format!("Authorization: {auth}"),
                "-w", "\n__HTTP_STATUS__%{http_code}",
                &url,
            ])
            .output()
            .context("failed to spawn curl for hcloud API DELETE")?;

        let raw = String::from_utf8_lossy(&output.stdout).into_owned();
        let (_body, status_part) = Self::split_curl_status(&raw);
        let status: u16 = status_part.trim().parse().unwrap_or(0);
        // 200, 204, or 404 (already gone) are all acceptable.
        if (200..300).contains(&status) || status == 404 {
            return Ok(());
        }
        anyhow::bail!("hcloud API DELETE {path} returned HTTP {status}");
    }

    fn split_curl_status(raw: &str) -> (&str, &str) {
        const MARKER: &str = "\n__HTTP_STATUS__";
        raw.rfind(MARKER).map_or((raw, "0"), |idx| (&raw[..idx], &raw[idx + MARKER.len()..]))
    }

    /// Extract a string field `key` from a flat JSON object (best-effort, no deps).
    ///
    /// Returns `None` if the key is not found.
    #[must_use]
    fn json_str_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
        // Pattern: `"key":"value"` or `"key": "value"` (handles whitespace around `:`).
        let needle = format!("\"{key}\"");
        let pos = json.find(needle.as_str())?;
        let after_key = json.get(pos + needle.len()..)?;
        // Skip `:`  and optional whitespace.
        let after_colon = after_key.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
        if after_colon.starts_with('"') {
            let inner = after_colon.get(1..)?;
            let end = inner.find('"')?;
            inner.get(..end)
        } else {
            None
        }
    }

    /// Extract the top-level numeric `id` from hcloud JSON response.
    #[must_use]
    fn json_num_field(json: &str, key: &str) -> Option<u64> {
        let needle = format!("\"{key}\"");
        let pos = json.find(needle.as_str())?;
        let after = json.get(pos + needle.len()..)?;
        let after_colon = after.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
        // Read digits.
        let digits: String = after_colon.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }
}

impl PodProvider for HcloudPodProvider {
    fn create_pod(&self, cfg: &HcloudPodConfig) -> Result<String> {
        eprintln!(
            "[hcloud] creating server type={} location={} image={} arch={}",
            cfg.server_type, cfg.location, cfg.image, cfg.remote_arch
        );

        // Build a minimal cloud-init user-data that exports sccache env from instance metadata.
        // The API token is NOT included here — the pod uses its own credentials for the
        // object store, configured separately via the bucket env vars.
        let user_data = format!(
            "#!/bin/bash\nexport RUSTC_WRAPPER=sccache\nexport SCCACHE_ENDPOINT={endpoint}\nexport SCCACHE_BUCKET={bucket}\n",
            endpoint = cfg.sccache_endpoint,
            bucket = cfg.sccache_bucket,
        );

        // Determine whether `image` looks like a numeric snapshot ID or a name.
        let image_field = if cfg.image.chars().all(|c| c.is_ascii_digit()) {
            format!("\"image\": {}", cfg.image)
        } else {
            format!("\"image\": \"{}\"", cfg.image)
        };

        let body = format!(
            r#"{{"name":"wm-burst-{ts}","server_type":"{stype}","location":"{loc}",{image},"ssh_keys":["{ssh_key}"],"user_data":{ud_json}}}"#,
            ts = chrono::Utc::now().timestamp(),
            stype = cfg.server_type,
            loc = cfg.location,
            image = image_field,
            ssh_key = cfg.ssh_key_name,
            ud_json = serde_json::to_string(&user_data)
                .unwrap_or_else(|_| "\"\"".to_owned()),
        );

        let response = self.hcloud_post("/servers", &body)?;

        // Parse server id from `{"server":{"id":12345,...},...}`.
        let id = Self::json_num_field(&response, "id")
            .context("hcloud create response missing 'id' field")?;

        // Parse the public IPv4 from `{"server":{"public_net":{"ipv4":{"ip":"1.2.3.4"}}}}`.
        // We store the IP inside the returned pod_id string for use in run_job.
        let ip = Self::json_str_field(&response, "ip").unwrap_or("unknown");

        let pod_id = format!("{id}@{ip}");
        eprintln!("[hcloud] server created: id={id} ip={ip}");
        Ok(pod_id)
    }

    fn run_job(&self, pod_id: &str, ip: &str, cmd: &str, max_secs: u64) -> Result<()> {
        eprintln!("[hcloud] running job on pod {pod_id} (ip={ip})");

        // Give the server a moment to boot if needed — a real implementation would
        // poll the hcloud server status endpoint; for this iter-1 scaffold we use
        // a simple SSH retry loop via ssh's ConnectTimeout.
        let ssh_target = format!("root@{ip}");
        let timeout_cmd = format!("timeout {max_secs} {cmd}");
        let output = std::process::Command::new("ssh")
            .args([
                "-o", "BatchMode=yes",
                "-o", "StrictHostKeyChecking=no",
                "-o", "ConnectTimeout=30",
                // Retry up to 5 times for boot.
                "-o", "ConnectionAttempts=5",
                &ssh_target,
                &timeout_cmd,
            ])
            .status()
            .context("failed to spawn ssh for remote job")?;

        if output.success() {
            Ok(())
        } else {
            anyhow::bail!("remote job on pod {pod_id} failed: {output}")
        }
    }

    fn destroy_pod(&self, pod_id: &str) -> Result<()> {
        // pod_id format is `<hcloud-server-id>@<ip>` or just `<id>` if ip was unknown.
        let server_id = pod_id.split('@').next().unwrap_or(pod_id);
        eprintln!("[hcloud] deleting server {server_id}");
        self.hcloud_delete(&format!("/servers/{server_id}"))
    }
}

// Prevent the token from ever appearing in debug output.
impl std::fmt::Debug for HcloudPodProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HcloudPodProvider")
            .field("token", &"<redacted>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// MockPodProvider
// ---------------------------------------------------------------------------

/// Mock provider for tests — no real API calls, no real spend.
pub struct MockPodProvider {
    pod_counter: std::cell::Cell<u64>,
}

impl MockPodProvider {
    /// Create a new mock provider.
    #[must_use]
    pub const fn new() -> Self {
        Self { pod_counter: std::cell::Cell::new(0) }
    }
}

impl Default for MockPodProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PodProvider for MockPodProvider {
    fn create_pod(&self, cfg: &HcloudPodConfig) -> Result<String> {
        let id = self.pod_counter.get() + 1;
        self.pod_counter.set(id);
        eprintln!(
            "[mock-pod] created pod mock-{id} (provider=hcloud type={} loc={})",
            cfg.server_type, cfg.location
        );
        Ok(format!("mock-{id}@192.0.2.{id}"))
    }

    fn run_job(&self, pod_id: &str, _ip: &str, cmd: &str, _max_secs: u64) -> Result<()> {
        eprintln!("[mock-pod] running in {pod_id}: {cmd}");
        Ok(())
    }

    fn destroy_pod(&self, pod_id: &str) -> Result<()> {
        eprintln!("[mock-pod] destroyed {pod_id}");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Construct the appropriate [`PodProvider`] based on config / flags.
///
/// When `use_mock` is `true`, always returns a [`MockPodProvider`] regardless
/// of config (used by tests and `--mock` CLI flag).
///
/// When `provider_name` is `"hcloud"`, reads `$HCLOUD_TOKEN` and returns an
/// [`HcloudPodProvider`].
///
/// # Errors
/// Returns an error if `provider_name` is `"hcloud"` and `$HCLOUD_TOKEN` is not set,
/// or if `provider_name` is an unknown value.
pub fn make_provider(use_mock: bool, provider_name: &str) -> Result<Box<dyn PodProvider>> {
    if use_mock {
        return Ok(Box::new(MockPodProvider::new()));
    }
    match provider_name {
        "hcloud" => {
            let p = HcloudPodProvider::from_env()?;
            Ok(Box::new(p))
        }
        other => anyhow::bail!(
            "unknown pod provider '{other}'; supported: hcloud. \
             Use --mock for testing without real infra."
        ),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HcloudPodConfig;

    fn test_hcloud_cfg() -> HcloudPodConfig {
        HcloudPodConfig {
            provider: "hcloud".into(),
            server_type: "ccx23".into(),
            location: "fsn1".into(),
            image: "ubuntu-24.04".into(),
            ssh_key_name: "my-key".into(),
            remote_arch: "x86_64-unknown-linux-gnu".into(),
            sccache_endpoint: "https://fsn1.your-objectstorage.com".into(),
            sccache_bucket: "wm-sccache".into(),
            idle_timeout_secs: 3600,
        }
    }

    #[test]
    fn mock_provider_create_returns_id() -> anyhow::Result<()> {
        let cfg = test_hcloud_cfg();
        let p = MockPodProvider::new();
        let id = p.create_pod(&cfg)?;
        assert!(id.starts_with("mock-"), "expected mock- prefix, got {id}");
        Ok(())
    }

    #[test]
    fn mock_provider_destroy_is_idempotent() -> anyhow::Result<()> {
        let p = MockPodProvider::new();
        // Destroy a pod that was never created — should not error.
        p.destroy_pod("mock-99@192.0.2.99")?;
        p.destroy_pod("mock-99@192.0.2.99")?;
        Ok(())
    }

    #[test]
    fn mock_provider_run_job_succeeds() -> anyhow::Result<()> {
        let cfg = test_hcloud_cfg();
        let p = MockPodProvider::new();
        let id = p.create_pod(&cfg)?;
        let ip = "192.0.2.1";
        p.run_job(&id, ip, "cargo build", 3600)?;
        Ok(())
    }

    #[test]
    fn make_provider_mock_flag_returns_mock() -> anyhow::Result<()> {
        // Should not error even with invalid provider name when use_mock=true.
        let p = make_provider(true, "hcloud")?;
        // Verify it behaves like a mock (no real calls).
        let cfg = test_hcloud_cfg();
        p.create_pod(&cfg)?;
        Ok(())
    }

    #[test]
    fn hcloud_json_str_field_extracts_value() {
        let json = r#"{"server":{"id":123,"name":"wm-burst-1"},"public_net":{"ipv4":{"ip":"1.2.3.4"}}}"#;
        let name = HcloudPodProvider::json_str_field(json, "name");
        assert_eq!(name, Some("wm-burst-1"));
        let ip = HcloudPodProvider::json_str_field(json, "ip");
        assert_eq!(ip, Some("1.2.3.4"));
    }

    #[test]
    fn hcloud_json_num_field_extracts_id() {
        let json = r#"{"server":{"id":98765,"name":"test"}}"#;
        let id = HcloudPodProvider::json_num_field(json, "id");
        assert_eq!(id, Some(98765));
    }

    #[test]
    fn make_provider_unknown_provider_errors() {
        let err = make_provider(false, "runpod");
        assert!(err.is_err(), "unknown provider should return Err");
        let msg = err.map_or_else(|e| format!("{e}"), |_| String::new());
        assert!(msg.contains("unknown pod provider"), "expected 'unknown pod provider': {msg}");
    }

    #[test]
    fn hcloud_token_not_in_debug() -> anyhow::Result<()> {
        let p = HcloudPodProvider::from_token("super-secret-token")?;
        let debug_str = format!("{p:?}");
        assert!(
            !debug_str.contains("super-secret-token"),
            "HCLOUD_TOKEN must not appear in Debug output: {debug_str}"
        );
        Ok(())
    }
}
