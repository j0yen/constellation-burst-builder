//! wm-burst library — public API surface used by integration and mock tests.
//!
//! The main binary is `src/main.rs`; this module re-exports the components
//! needed by `tests/` without crossing the binary boundary.

// This is a CLI application library surface (for integration tests only).
// Print to stdout/stderr is intentional in command handlers.
// `unreachable_pub` is noisy in an application crate where the binary is the primary consumer.
#![allow(clippy::print_stdout, clippy::print_stderr, unreachable_pub)]

pub mod commands;
pub mod config;
pub mod cost;
pub mod provider;
pub mod toolchain;
