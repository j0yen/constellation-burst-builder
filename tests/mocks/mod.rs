//! Mock tests for hardware/network-deferred acceptance criteria.
//!
//! These run under `cargo test` by default. Each mock exercises the same
//! public API surface and asserts the same invariants as the real AC test
//! would, using in-process fakes rather than live SSH/sccache/provider calls.

pub mod ac1_hcloud_provider;
pub mod ac2;
pub mod ac4;
pub mod ac5;
