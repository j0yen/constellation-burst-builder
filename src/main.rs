//! wm-burst — burst heavy Rust compiles and CPU jobs to a dedicated cloud box.
//!
//! Zero-mesh rung: ssh + sccache + config file. No NATS, no dispatch coordinator.
//! Guardrail: refuses to build remotely when toolchain doesn't match `rust-toolchain.toml`.

// CLI binary: printing to stdout/stderr is intentional in command handlers.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod config;
mod cost;
mod provider;
mod toolchain;

/// Burst heavy Rust compiles and CPU jobs to a dedicated cloud box via ssh+sccache.
#[derive(Parser, Debug)]
#[command(name = "wm-burst", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Write or edit ~/.config/wm-burst/config.toml.
    Init(commands::init::InitArgs),
    /// Generate and apply an Ansible playbook to provision a fresh builder host.
    Provision(commands::provision::ProvisionArgs),
    /// Verify remote reachability, toolchain match, sccache writability, and cache hit rate.
    Doctor(commands::doctor::DoctorArgs),
    /// Run a cargo build remotely with `RUSTC_WRAPPER=sccache`; stream output and stats.
    Build(commands::build::BuildArgs),
    /// Run a non-cargo CPU job on the remote box; stream output and propagate exit code.
    Exec(commands::exec::ExecArgs),
    /// Manage ephemeral burst pods (up|down).
    Pod(commands::pod::PodArgs),
    /// Manage the permanent hub (up|down|status) — long-lived box for standing workloads.
    Hub(commands::hub::HubArgs),
    /// Show standing box load, cache hit rate, month-to-date spend, and last N jobs.
    Status(commands::status::StatusArgs),
    /// Show cost breakdown: burst spend, hub standing charge, month-to-date, and projection.
    Cost(commands::cost::CostArgs),
}

fn main() -> std::process::ExitCode {
    // SIGPIPE: reset disposition so piped consumers (e.g. `wm-burst status | head`)
    // don't cause a panic. Must be first side-effect in main().
    sigpipe::reset();

    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<std::process::ExitCode> {
    match cli.command {
        Commands::Init(args) => commands::init::run(&args),
        Commands::Provision(args) => commands::provision::run(&args),
        Commands::Doctor(args) => commands::doctor::run(&args),
        Commands::Build(args) => commands::build::run(&args),
        Commands::Exec(args) => commands::exec::run(&args),
        Commands::Pod(args) => commands::pod::run(args),
        Commands::Hub(args) => commands::hub::run(args),
        Commands::Status(args) => commands::status::run(&args),
        Commands::Cost(args) => commands::cost::run(&args),
    }
}
