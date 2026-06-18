//! Upload outcome — the terminal state of the upload pipeline.
//!
//! `#[must_use]` ensures the outcome is always checked. Ignoring it
//! triggers a compile error via `#[deny(clippy::must_use_candidate)]`.

#![deny(unsafe_code)]

/// Result of an S3 upload operation.
///
/// **Must be consumed.** The compiler will reject code that drops this
/// without calling a method on it.
#[must_use]
#[derive(Debug, Clone)]
pub struct UploadOutcome {
    /// HTTP status code from the S3 response
    pub status_code: u16,
    /// ETag from the S3 response (confirms object integrity)
    pub etag: Option<String>,
    /// Error body if the upload failed
    pub error_body: Option<String>,
}

impl UploadOutcome {
    /// Create a successful outcome.
    pub fn success(status_code: u16, etag: Option<String>) -> Self {
        Self {
            status_code,
            etag,
            error_body: None,
        }
    }

    /// Create a failure outcome.
    #[allow(dead_code)]
    pub fn failure(status_code: u16, error_body: String) -> Self {
        Self {
            status_code,
            etag: None,
            error_body: Some(error_body),
        }
    }

    /// Was the upload successful? (2xx status)
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }

    /// Is this error retryable? (5xx or 429 Too Many Requests)
    #[allow(dead_code)]
    pub fn is_retryable(&self) -> bool {
        matches!(self.status_code, 500..=599 | 429)
    }

    /// Get the ETag if upload was successful.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// Get the error message if upload failed.
    #[allow(dead_code)]
    pub fn error(&self) -> Option<&str> {
        self.error_body.as_deref()
    }
}
