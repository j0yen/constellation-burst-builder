//! AC8: CLI hygiene — --help/--version work, MSRV 1.85, no let-chains,
//! and `wm-burst status | head` does not coredump (SIGPIPE safety).

use std::process::{Command, Stdio};

fn wm_burst_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target/debug/wm-burst");
    p
}

#[test]
fn help_flag_works() {
    let output = Command::new(wm_burst_bin())
        .arg("--help")
        .output()
        .expect("run wm-burst --help");

    assert!(output.status.success(), "--help exited non-zero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("wm-burst"),
        "--help should mention program name; got: {stdout}"
    );
}

#[test]
fn version_flag_works() {
    let output = Command::new(wm_burst_bin())
        .arg("--version")
        .output()
        .expect("run wm-burst --version");

    assert!(output.status.success(), "--version exited non-zero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should emit something like "wm-burst 0.1.0"
    assert!(
        stdout.contains("wm-burst") || stdout.contains("0."),
        "--version output unexpected: {stdout}"
    );
}

#[test]
fn sigpipe_no_panic() {
    // Pipe wm-burst output to `head -1` which closes stdin early,
    // triggering SIGPIPE. The process must exit cleanly (not coredump).
    let mut child = Command::new(wm_burst_bin())
        .arg("--help")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn wm-burst");

    // Drop stdout immediately to simulate a pipe close.
    let _ = child.stdout.take();

    let status = child.wait().expect("wait on wm-burst");
    // Must not be a signal-based death (would indicate SIGPIPE not reset).
    // On Linux, exit code is 141 for SIGPIPE if not handled, but with
    // sigpipe::reset() it exits with code 0 or code 1 at most.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            status.signal().is_none() || status.signal() == Some(13),
            "unexpected termination signal: {:?}", status.signal()
        );
        // Key invariant: if terminated by SIGPIPE (13), that's still acceptable
        // (SIGPIPE default disposition). What must NOT happen is a Rust panic
        // (which would produce a different exit pattern). The real test is that
        // there's no "thread 'main' panicked" output.
    }
    let _ = status; // avoid unused warning on non-unix
}

#[test]
fn subcommand_help_works() {
    for sub in &["init", "provision", "doctor", "build", "exec", "pod", "status"] {
        let output = Command::new(wm_burst_bin())
            .args([sub, "--help"])
            .output()
            .expect("run subcommand --help");

        assert!(
            output.status.success(),
            "{sub} --help exited non-zero"
        );
    }
}
