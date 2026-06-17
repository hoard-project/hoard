//! Standalone test binary for the Hoard upload pipeline.
//!
//! Usage: hoard-test-upload --file <path> --url <http://host:port/bucket/key>

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    url: String,
    #[arg(long, default_value = "application/octet-stream")]
    content_type: String,
    #[arg(long)]
    no_tls_verify: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let file_size = std::fs::metadata(&args.file)
        .with_context(|| format!("Cannot stat {:?}", args.file))?
        .len();
    let file_name = args
        .file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("═══ Hoard Upload Pipeline Test ═══");
    println!("  File:     {file_name} ({file_size} bytes)");
    println!("  Target:   {}", args.url);

    let data = std::fs::read(&args.file).context("Cannot read file")?;
    let client = if args.no_tls_verify {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .context("reqwest")?
    } else {
        reqwest::Client::new()
    };

    println!("  → Uploading...");
    let response = client
        .put(&args.url)
        .header("Content-Type", &args.content_type)
        .header("Content-Length", file_size.to_string())
        .body(data.clone())
        .send()
        .await
        .context("PUT failed")?;

    let status = response.status();
    let etag = response
        .headers()
        .get("etag")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("none");
    println!("  ← HTTP {status}  ETag: {etag}");

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Upload failed: HTTP {status}: {body}");
    }

    // Verify
    println!("  → Verifying...");
    let dl = client
        .get(&args.url)
        .send()
        .await
        .context("GET failed")?
        .bytes()
        .await
        .context("body read failed")?;
    if dl.as_ref() == data.as_slice() {
        let md5 = format!("{:x}", md5::compute(&data));
        println!("  ✅ PASS  {file_name:<30} {file_size:>8} bytes  md5={md5}");
        Ok(())
    } else {
        anyhow::bail!("Content mismatch: orig={} dl={}", data.len(), dl.len());
    }
}
