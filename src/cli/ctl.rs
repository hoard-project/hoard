//! `hoardctl` CLI — control tool for the Hoard daemon.
//!
//! Usage:
//!   hoardctl flush <service>        Trigger immediate upload
//!   hoardctl status <service>       Query daemon status
//!   hoardctl restore --key=<s3> --output=<path>  Restore from S3

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Hoard control tool — manage the backup daemon.
#[derive(Parser, Debug)]
#[command(name = "hoardctl", version, about)]
pub struct CtlCli {
    #[command(subcommand)]
    pub command: CtlCommand,
}

#[derive(Subcommand, Debug)]
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

/// Run the hoardctl CLI.
pub async fn run() -> Result<()> {
    let cli = CtlCli::parse();

    match cli.command {
        CtlCommand::Flush { service } => {
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
        }
        CtlCommand::Status { service } => {
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
        }
        CtlCommand::Restore {
            key,
            output,
            s3_endpoint,
            s3_region,
            s3_bucket,
            s3_access_key,
            s3_secret_key,
        } => {
            // Validate output path — prevent path traversal
            if output.is_absolute() || output.to_string_lossy().contains("..") {
                anyhow::bail!("output path must be relative and cannot contain '..'");
            }

            let s3 = crate::s3::S3Backend::new(
                s3_access_key,
                s3_secret_key,
                s3_region,
                s3_endpoint,
                s3_bucket,
                false, // no_sign — restore always uses SigV4
            );
            let verified = s3.verify().await?;
            let data = verified.get_object(&key).await?;

            // Decompress if .zst suffix
            let output_data = if key.ends_with(".zst") {
                zstd::decode_all(&data[..])?
            } else {
                data
            };

            std::fs::write(&output, &output_data)
                .with_context(|| format!("failed to write {}", output.display()))?;

            println!("Restored {} → {}", key, output.display());
        }
    }

    Ok(())
}
