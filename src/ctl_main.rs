//! Entry point for the `hoardctl` control tool.
//!
//! This is a separate binary that communicates with the hoard daemon
//! via Unix domain socket.

#![deny(unsafe_code)]

use anyhow::Result;
use tracing_subscriber::fmt;

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("hoardctl=info")),
        )
        .try_init()
        .ok();

    hoard::cli::ctl::run().await
}
