//! Type-state upload pipeline with compile-time operation ordering.
//!
//! The pipeline enforces the correct sequence of operations through
//! Rust's type system. Each state transition returns a new type;
//! methods that don't apply to the current state don't exist.
//!
//! ```ignore
//! UploadPipeline::new(file, file_size, s3_key, db_path)
//!     .wal_checkpoint()?         // Pending → Checkpointed
//!     .presign(&s3)?             // Checkpointed → Presigned
//!     .connect(host, port)?      // Presigned → Connected
//!     .write_header(tls_keys)?   // Connected → (HeaderWritten, SocketFd)
//!     .0.sendfile_body(&sock)?  // HeaderWritten → BodyTransmitted
//!     .shutdown_and_read(sock)?; // BodyTransmitted → UploadOutcome
//! ```

#![deny(unsafe_code)]

use crate::fd::{FileFd, SocketFd};
use crate::ffi;
use crate::s3::VerifiedS3Backend;
use crate::upload::outcome::UploadOutcome;
use anyhow::{Context, Result};
use std::marker::PhantomData;
use std::net::TcpStream;
use std::path::PathBuf;

// ── State tokens (zero-size, compile-time only) ─────────────────

pub enum Pending {}
pub enum Checkpointed {}
pub enum Presigned {}
pub enum Connected {}
pub enum HeaderWritten {}
pub enum BodyTransmitted {}

// ── Pipeline struct ──────────────────────────────────────────────

/// The upload pipeline, parameterized by state `S`.
pub struct UploadPipeline<S> {
    file: FileFd,
    file_size: u64,
    s3_key: String,
    db_path: PathBuf,
    presigned_url: Option<String>,
    _state: PhantomData<S>,
}

// ── Pending state ────────────────────────────────────────────────

impl UploadPipeline<Pending> {
    /// Create a new upload pipeline in the initial state.
    pub fn new(file: FileFd, file_size: u64, s3_key: String, db_path: PathBuf) -> Self {
        Self {
            file,
            file_size,
            s3_key,
            db_path,
            presigned_url: None,
            _state: PhantomData,
        }
    }

    /// Perform WAL checkpoint with exponential backoff + PASSIVE fallback (§五.2).
    ///
    /// If the file is not a SQLite database, the checkpoint is silently skipped
    /// and the pipeline proceeds directly to the presign stage. This allows Hoard
    /// to back up any file type, not just SQLite databases.
    pub fn wal_checkpoint(self) -> Result<UploadPipeline<Checkpointed>> {
        // Open the file as SQLite; if it's not a valid database, skip checkpoint.
        match rusqlite::Connection::open(&self.db_path) {
            Ok(conn) => {
                // Stage 1: TRUNCATE with exponential backoff (5 attempts)
                for attempt in 0..5 {
                    match conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                        Ok(_) => {
                            tracing::info!(attempt, "WAL checkpoint TRUNCATE succeeded");
                            break;
                        }
                        Err(ref e) if is_busy(e) => {
                            let wait_ms = 100 * 2u64.pow(attempt);
                            tracing::warn!(attempt, wait_ms, "checkpoint BUSY, retrying");
                            std::thread::sleep(std::time::Duration::from_millis(wait_ms));
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(?e, "TRUNCATE failed, falling back to PASSIVE checkpoint");
                            conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
                                .context("PASSIVE checkpoint also failed")?;
                            break;
                        }
                    }
                }
            }
            Err(_) => {
                tracing::debug!(
                    path = %self.db_path.display(),
                    "not a SQLite database, skipping WAL checkpoint"
                );
            }
        }

        Ok(UploadPipeline {
            file: self.file,
            file_size: self.file_size,
            s3_key: self.s3_key,
            db_path: self.db_path,
            presigned_url: None,
            _state: PhantomData,
        })
    }
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                ..
            },
            _,
        )
    )
}

// ── Checkpointed state ───────────────────────────────────────────

impl UploadPipeline<Checkpointed> {
    /// Obtain a pre-signed PUT URL from S3 backend.
    pub async fn presign(mut self, s3: &VerifiedS3Backend) -> Result<UploadPipeline<Presigned>> {
        let url = s3
            .presign_put(&self.s3_key, std::time::Duration::from_secs(300))
            .await
            .context("S3 presign failed")?;
        self.presigned_url = Some(url);
        Ok(UploadPipeline {
            file: self.file,
            file_size: self.file_size,
            s3_key: self.s3_key,
            db_path: self.db_path,
            presigned_url: self.presigned_url,
            _state: PhantomData,
        })
    }
}

// ── Presigned state ──────────────────────────────────────────────

impl UploadPipeline<Presigned> {
    /// Connect to S3 endpoint. Connection is established in `write_header()`.
    pub async fn connect(self, _host: &str, _port: u16) -> Result<UploadPipeline<Connected>> {
        // The real TCP connection is made in write_header() from the presigned URL.
        // This pass-through exists to preserve the type-state machine.
        Ok(UploadPipeline {
            file: self.file,
            file_size: self.file_size,
            s3_key: self.s3_key,
            db_path: self.db_path,
            presigned_url: self.presigned_url,
            _state: PhantomData,
        })
    }
}

// ── Connected state ──────────────────────────────────────────────

impl UploadPipeline<Connected> {
    /// Write the HTTP PUT header to the socket. Enables kTLS if keys provided.
    /// Returns pipeline in `HeaderWritten` state and the `SocketFd`.
    pub fn write_header(
        self,
        tls_keys: Option<&ffi::TlsKeys>,
    ) -> Result<(UploadPipeline<HeaderWritten>, SocketFd)> {
        // Extract host[:port] from presigned URL, fallback to localhost:443
        let host_port = self
            .presigned_url
            .as_ref()
            .and_then(|u| u.split('/').nth(2))
            .unwrap_or("localhost:443");
        let stream = TcpStream::connect(host_port)?;
        let sock = SocketFd::from(stream);

        // Enable kTLS if keys are provided
        if let Some(keys) = tls_keys {
            ffi::enable_ktls(sock.as_raw_fd(), ffi::TlsCipher::Aes128Gcm, keys)?;
        }

        // Build the HTTP PUT request header
        let url = self
            .presigned_url
            .as_deref()
            .context("presigned URL not set — call .presign() first")?;

        let host_port = url.split('/').nth(2).unwrap_or("localhost");
        let path: String = url.split('/').skip(3).collect::<Vec<_>>().join("/");
        let path = format!("/{path}");

        let header = format!(
            "PUT {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
            self.file_size
        );

        ffi::write(sock.as_raw_fd(), header.as_bytes())?;
        tracing::debug!(header_len = header.len(), "HTTP header written");

        Ok((
            UploadPipeline {
                file: self.file,
                file_size: self.file_size,
                s3_key: self.s3_key,
                db_path: self.db_path,
                presigned_url: self.presigned_url,
                _state: PhantomData,
            },
            sock,
        ))
    }
}

// ── HeaderWritten state ──────────────────────────────────────────

impl UploadPipeline<HeaderWritten> {
    /// Send the file body using zero-copy sendfile. EAGAIN handled internally.
    pub fn sendfile_body(self, sock: &SocketFd) -> Result<UploadPipeline<BodyTransmitted>> {
        let sent =
            ffi::sendfile_loop(sock, &self.file, self.file_size).context("sendfile failed")?;

        tracing::info!(
            bytes_sent = sent,
            file_size = self.file_size,
            "sendfile body complete"
        );

        Ok(UploadPipeline {
            file: self.file,
            file_size: self.file_size,
            s3_key: self.s3_key,
            db_path: self.db_path,
            presigned_url: self.presigned_url,
            _state: PhantomData,
        })
    }
}

// ── BodyTransmitted state ────────────────────────────────────────

impl UploadPipeline<BodyTransmitted> {
    /// Shutdown write side and read HTTP response. Consumes SocketFd.
    pub fn shutdown_and_read(self, sock: SocketFd) -> Result<UploadOutcome> {
        let raw_fd = sock.into_raw_fd();

        // Give MinIO a moment to process the body before shutdown
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Shutdown write side (send EOF)
        ffi::shutdown(raw_fd, libc::SHUT_WR)?;

        // Read HTTP response with timeout
        let mut buf = [0u8; 4096];
        let n = ffi::read(raw_fd, &mut buf)?;
        let response = String::from_utf8_lossy(&buf[..n]);

        // Parse HTTP status line
        let status_code = response
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(500);

        // Extract ETag header
        let etag = response
            .lines()
            .find(|l| l.to_lowercase().starts_with("etag:"))
            .map(|l| l[5..].trim().trim_matches('"').to_string());

        if (200..300).contains(&status_code) {
            tracing::info!(status_code, ?etag, "upload successful");
        } else {
            tracing::error!(status_code, response = %response, "upload failed");
        }

        // Close the FD (SocketFd was consumed via into_raw_fd())
        ffi::close_fd(raw_fd);

        Ok(UploadOutcome::success(status_code, etag))
    }
}
