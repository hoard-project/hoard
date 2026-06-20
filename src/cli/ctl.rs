//! `hoardctl` CLI — control tool for the Hoard daemon.
//!
//! Usage:
//!   `hoardctl flush SERVICE`       — Trigger immediate upload
//!   `hoardctl status SERVICE`      — Query daemon status
//!   `hoardctl restore --key=S3_KEY --output=PATH`  — Restore from S3

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Hoard control tool — manage the backup daemon.
#[derive(Parser, Debug)]
#[command(name = "hoardctl", version, about)]
#[allow(dead_code)]
pub struct CtlCli {
    #[command(subcommand)]
    pub command: CtlCommand,
}

#[derive(Subcommand, Debug)]
#[allow(dead_code)]
pub enum CtlCommand {
    /// Trigger an immediate backup upload
    Flush {
        /// Service name to flush
        service: String,
    },
    /// Query daemon status
    Status {
        /// Service name to query
        service: String,
    },
    /// Restore a backup from S3 to local filesystem
    Restore {
        /// S3 key of the backup object
        #[arg(long)]
        key: String,
        /// Output file path (must be under watch-path)
        #[arg(long)]
        output: PathBuf,
        /// S3 endpoint
        #[arg(long, env = "HOARD_S3_ENDPOINT")]
        s3_endpoint: String,
        /// S3 region
        #[arg(long, env = "HOARD_S3_REGION", default_value = "us-east-1")]
        s3_region: String,
        /// S3 bucket
        #[arg(long, env = "HOARD_S3_BUCKET")]
        s3_bucket: String,
        /// S3 access key
        #[arg(long, env = "HOARD_S3_ACCESS_KEY", hide_env_values = true)]
        s3_access_key: String,
        /// S3 secret key
        #[arg(long, env = "HOARD_S3_SECRET_KEY", hide_env_values = true)]
        s3_secret_key: String,
    },
}

/// Send a flush command to the daemon via Unix socket.
pub async fn run_flush(service: &str) -> Result<()> {
    let sock = PathBuf::from(format!("/run/hoard/{service}.sock"));
    let stream = tokio::net::UnixStream::connect(&sock)
        .await
        .with_context(|| {
            format!(
                "failed to connect to {}: hoard daemon not running?",
                sock.display()
            )
        })?;

    stream.writable().await?;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (rx, mut tx) = stream.into_split();
    tx.write_all(b"flush\n").await?;
    tx.shutdown().await?;

    let mut response = String::new();
    BufReader::new(rx).read_line(&mut response).await?;
    print!("{response}");
    Ok(())
}

/// Send a status query to the daemon via Unix socket.
pub async fn run_status(service: &str) -> Result<()> {
    let sock = PathBuf::from(format!("/run/hoard/{service}.sock"));
    let stream = tokio::net::UnixStream::connect(&sock)
        .await
        .with_context(|| format!("failed to connect to {}", sock.display()))?;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (rx, mut tx) = stream.into_split();
    tx.write_all(b"status\n").await?;
    tx.shutdown().await?;

    let mut response = String::new();
    BufReader::new(rx).read_line(&mut response).await?;
    print!("{response}");
    Ok(())
}
