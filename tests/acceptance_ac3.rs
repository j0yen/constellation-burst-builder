//! AC3: `wm-burst doctor` hard-fails when remote rustc set does not match
//! rust-toolchain.toml, printing the exact local-vs-remote mismatch,
//! and passes when they match.
//!
//! Tested via the public toolchain module — no real SSH required.

use wm_burst::toolchain::{check_toolchain_match, ToolchainChannel};

#[test]
fn doctor_fails_on_toolchain_mismatch_with_diagnostic() {
    // Simulate a project pinned to 1.85.0 but remote reports 1.88.0.
    let local_channel = ToolchainChannel("1.85.0".into());
    let remote_rustc_output = "rustc 1.88.0 (hash 2025-06-01)";

    let err = check_toolchain_match(&local_channel, remote_rustc_output)
        .expect_err("expected mismatch error");
    let msg = format!("{err:#}");

    // Must mention both local and remote in the diagnostic.
    assert!(
        msg.contains("1.85.0"),
        "error must include local version; got: {msg}"
    );
    assert!(
        msg.contains("1.88.0"),
        "error must include remote version; got: {msg}"
    );
}

#[test]
fn doctor_passes_when_toolchain_matches() {
    let local_channel = ToolchainChannel("1.85.0".into());
    let remote_rustc_output = "rustc 1.85.0 (9fcaa977d 2025-01-10)";

    check_toolchain_match(&local_channel, remote_rustc_output)
        .expect("expected match to succeed");
}

#[test]
fn doctor_passes_with_partial_channel_notation() {
    // Local channel can be "1.85" without ".0".
    let local_channel = ToolchainChannel("1.85".into());
    let remote_rustc_output = "rustc 1.85.0 (9fcaa977d 2025-01-10)";

    check_toolchain_match(&local_channel, remote_rustc_output)
        .expect("partial channel '1.85' should match '1.85.0'");
}

#[test]
fn doctor_fails_on_nightly_vs_stable() {
    let local_channel = ToolchainChannel("nightly".into());
    let remote_rustc_output = "rustc 1.85.0 (9fcaa977d 2025-01-10)";

    let err = check_toolchain_match(&local_channel, remote_rustc_output)
        .expect_err("nightly vs stable should mismatch");
    let msg = format!("{err}");
    assert!(msg.contains("nightly"), "must mention local in error: {msg}");
}
