//! `hoard` daemon CLI — the main entry point for the backup daemon.

#![deny(unsafe_code)]

use clap::Parser;

/// Hoard backup daemon — eBPF-based zero-copy SQLite to S3.
///
/// Available modes:
///   --mode standalone   Single-machine: SIGTERM/CLI triggers
///   --mode nomad        Nomad cluster: SSE Drain events + Prestart restore
#[derive(Parser, Debug)]
#[command(name = "hoard", version, about, long_about = None)]
pub struct HoardCli {
    #[command(flatten)]
    pub config: crate::config::Config,
}
