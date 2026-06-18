//! Upload retry with exponential backoff and dead-letter queue.
//!
//! Files that fail to upload are retried with backoff. Files that
//! exceed max attempts are moved to a dead-letter directory for
//! operator inspection.

#![deny(unsafe_code)]

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Configuration for upload retry behaviour.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of upload attempts per file.
    pub max_attempts: u32,
    /// Base delay before first retry.
    pub base_delay: Duration,
    /// Maximum delay between retries.
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
        }
    }
}

/// A file in the dead-letter queue.
#[derive(Debug, Clone)]
pub struct DeadLetter {
    pub original_path: PathBuf,
    pub attempts: u32,
    pub last_error: String,
}

/// Calculate exponential backoff delay: base * 2^(attempt-1), capped at max.
pub fn backoff_delay(config: &RetryConfig, attempt: u32) -> Duration {
    let delay = config
        .base_delay
        .checked_mul(2u32.saturating_pow(attempt.saturating_sub(1)))
        .unwrap_or(config.max_delay);
    std::cmp::min(delay, config.max_delay)
}

/// Write a dead-letter entry to the dead-letter directory.
///
/// Each entry is a plain-text file named `{timestamp}_{filename}.dead`
/// containing the original path, retry count, and last error.
pub fn write_dead_letter(dead_letter_dir: &Path, entry: &DeadLetter) -> Result<()> {
    std::fs::create_dir_all(dead_letter_dir)?;

    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let basename = entry
        .original_path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("unknown"));
    let filename = format!("{}_{}.dead", ts, basename);
    let dest = dead_letter_dir.join(filename);

    let content = format!(
        "original_path: {}\nattempts: {}\nlast_error: {}\n",
        entry.original_path.display(),
        entry.attempts,
        entry.last_error,
    );

    std::fs::write(&dest, content)?;
    tracing::warn!(
        path = %entry.original_path.display(),
        attempts = entry.attempts,
        dest = %dest.display(),
        "file moved to dead-letter queue"
    );

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn backoff_increases() {
        let cfg = RetryConfig::default();
        let d1 = backoff_delay(&cfg, 1);
        let d2 = backoff_delay(&cfg, 2);
        let d3 = backoff_delay(&cfg, 3);
        assert_eq!(d1, Duration::from_secs(1));
        assert_eq!(d2, Duration::from_secs(2));
        assert_eq!(d3, Duration::from_secs(4));
    }

    #[test]
    fn backoff_capped() {
        let cfg = RetryConfig::default();
        let d10 = backoff_delay(&cfg, 10);
        assert_eq!(d10, Duration::from_secs(60));
    }
}
