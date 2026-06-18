//! # Hoard: eBPF + io_uring zero-copy SQLite backup daemon
//!
//! **Architecture:** see `hoard-architecture-final.md`
//!
//! Hoard monitors SQLite database files via eBPF tracepoints,
//! performs zero-copy uploads to S3 using sendfile + kTLS, and
//! supports both standalone and Nomad cluster deployment modes.
//!
//! ## Commands
//!
//! - `hoard` (default) — run the daemon
//! - `hoard restore` — bulk restore backups from S3
//!
//! ## Unsafe policy
//!
//! All unsafe code is isolated in `src/ffi.rs`. Every other module
//! begins with `#![deny(unsafe_code)]`. The total unsafe surface
//! is ≤ 120 lines.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::must_use_candidate)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::cast_possible_truncation)]
#![deny(clippy::cast_sign_loss)]

mod cli;
mod config;
mod ebpf;
mod fd;
mod ffi;
mod hoard;
mod s3;
mod trigger;
mod upload;

mod metrics;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, EnvFilter};

/// Hoard — eBPF SQLite backup daemon
#[derive(Parser, Debug)]
#[command(name = "hoard", version, about)]
struct Cli {
    /// Optional config file (TOML)
    #[arg(short, long, env = "HOARD_CONFIG")]
    config: Option<String>,

    /// Subcommand
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Restore backups from S3 to local filesystem
    Restore(cli::restore::RestoreArgs),
    /// Control the hoard daemon via Unix socket
    Ctl {
        #[command(subcommand)]
        action: CtlAction,
    },
}

#[derive(clap::Subcommand, Debug)]
enum CtlAction {
    /// Trigger immediate upload flush
    Flush {
        /// Service name
        service: String,
    },
    /// Query daemon status
    Status {
        /// Service name
        service: String,
    },
}

/// Entry point.
#[tokio::main]
async fn main() -> Result<()> {
    // Structured logging with env-filter support
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("hoard=info")),
        )
        .json()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Restore(args)) => {
            // Human-readable logging for restore
            fmt()
                .with_env_filter(
                    EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| EnvFilter::new("hoard=info")),
                )
                .try_init()
                .ok();
            cli::restore::run(args).await
        }
        Some(Command::Ctl { action }) => {
            // Forward to hoardctl logic
            match action {
                CtlAction::Flush { service } => {
                    cli::ctl::run_flush(&service).await
                }
                CtlAction::Status { service } => {
                    cli::ctl::run_status(&service).await
                }
            }
        }
        None => {
            // JSON logging for daemon
            fmt()
                .with_env_filter(
                    EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| EnvFilter::new("hoard=info")),
                )
                .json()
                .try_init()
                .ok();

            tracing::info!(version = env!("CARGO_PKG_VERSION"), "Hoard starting");

            let config = cli::parse_config()?;
            let state = hoard::HoardStopped::new(config);

            let attached = state.load_ebpf().await?;
            let ready = attached.activate().await?;

            ready.run().await
        }
    }
}
