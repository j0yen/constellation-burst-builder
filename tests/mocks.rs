//! Test-binary entry point that compiles the hardware/network-deferred
//! AC mocks (AC2/AC4/AC5) so they actually run under `cargo test`.
//!
//! The mock modules live under `tests/mocks/` and exercise the same public
//! API surface as the real AC tests would, using in-process fakes rather
//! than live SSH/sccache/provider calls. Without this entry point Cargo
//! never compiles them (a bare `tests/mocks/` subdir is not a test target).
#[path = "mocks/mod.rs"]
mod mocks;
