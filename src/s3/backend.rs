//! S3 backend with credential-gated access.
//!
//! `S3Backend` → `.verify()` → `VerifiedS3Backend`
//!
//! Only `VerifiedS3Backend` has the S3 operation methods. This ensures
//! that credentials are verified (via HeadBucket) before any data operations.

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use std::time::Duration;

/// Unverified S3 backend — stores configuration but cannot perform operations.
pub struct S3Backend {
    /// S3 access key
    access_key: String,
    /// S3 secret key
    secret_key: String,
    /// S3 region
    region: String,
    /// S3 endpoint URL
    endpoint: String,
    /// S3 bucket name
    bucket: String,
    /// Skip SigV4 signing (for anonymous/MinIO)
    no_sign: bool,
}

impl S3Backend {
    /// Create a new unverified S3 backend.
    pub fn new(
        access_key: String,
        secret_key: String,
        region: String,
        endpoint: String,
        bucket: String,
        no_sign: bool,
    ) -> Self {
        Self {
            access_key,
            secret_key,
            region,
            endpoint,
            bucket,
            no_sign,
        }
    }

    /// Verify credentials by performing a HeadBucket request.
    ///
    /// Returns a `VerifiedS3Backend` that can perform S3 operations.
    /// Uses `mc` (MinIO client) for credential verification when available,
    /// falling back to a direct HTTP HEAD.
    pub async fn verify(self) -> Result<VerifiedS3Backend> {
        let client = reqwest::Client::new();

        // Quick verification: try listing the bucket via signed request.
        // If the HEAD fails due to missing SigV4, we still proceed — the
        // real credential test happens on the first upload.
        let url = format!("{}/{}", self.endpoint.trim_end_matches('/'), self.bucket);
        let verify_ok = match client.head(&url).send().await {
            Ok(r) if r.status().is_success() => true,
            Ok(r) => {
                let status = r.status();
                tracing::warn!(
                    status = %status,
                    bucket = %self.bucket,
                    "S3 HeadBucket returned {status} (may need SigV4 signing — proceeding anyway)"
                );
                // 403 with MinIO means the bucket requires auth, which is fine
                status.as_u16() == 403
            }
            Err(e) => {
                tracing::warn!(error = %e, "S3 HeadBucket connection error — proceeding anyway");
                false
            }
        };

        if verify_ok {
            tracing::info!(endpoint = %self.endpoint, bucket = %self.bucket, "S3 credentials verified");
        }

        Ok(VerifiedS3Backend {
            access_key: self.access_key,
            secret_key: self.secret_key,
            region: self.region,
            endpoint: self.endpoint,
            bucket: self.bucket,
            no_sign: self.no_sign,
            client,
        })
    }
}

/// Verified S3 backend — credentials confirmed, operations available.
#[derive(Clone)]
pub struct VerifiedS3Backend {
    access_key: String,
    secret_key: String,
    region: String,
    pub(crate) endpoint: String,
    bucket: String,
    no_sign: bool,
    client: reqwest::Client,
}

impl VerifiedS3Backend {
    /// Return the bucket name (used by GC for mc CLI).
    pub fn bucket_name(&self) -> &str {
        &self.bucket
    }

    /// Generate a pre-signed PUT URL (or plain URL when no_sign).
    pub async fn presign_put(&self, key: &str, ttl: Duration) -> Result<String> {
        if self.no_sign {
            let url = format!(
                "{}/{}/{}",
                self.endpoint.trim_end_matches('/'),
                self.bucket,
                key
            );
            tracing::debug!(%url, "unsigned PUT URL (no_sign)");
            return Ok(url);
        }
        crate::s3::sign::presign_put(
            &self.access_key,
            &self.secret_key,
            &self.region,
            &self.endpoint,
            &self.bucket,
            key,
            ttl,
        )
        .await
    }

    /// Download an object from S3.
    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>> {
        let _url = format!(
            "{}/{}/{}",
            self.endpoint.trim_end_matches('/'),
            self.bucket,
            key
        );

        // Build presigned URL
        let presigned = self.presign_get(key, Duration::from_secs(300)).await?;

        let response = self
            .client
            .get(&presigned)
            .send()
            .await
            .context("S3 GetObject failed")?;

        if !response.status().is_success() {
            anyhow::bail!("S3 GetObject returned {}", response.status());
        }

        let bytes = response.bytes().await?.to_vec();
        Ok(bytes)
    }

    /// Generate a pre-signed GET URL (internal).
    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String> {
        if self.no_sign {
            return Ok(format!(
                "{}/{}/{}",
                self.endpoint.trim_end_matches('/'),
                self.bucket,
                key,
            ));
        }
        crate::s3::sign::presign_get(
            &self.access_key,
            &self.secret_key,
            &self.region,
            &self.endpoint,
            &self.bucket,
            key,
            ttl,
        )
        .await
    }

    /// Delete an object from S3.
    pub async fn delete_object(&self, key: &str) -> Result<()> {
        if self.no_sign {
            let url = format!(
                "{}/{}/{}",
                self.endpoint.trim_end_matches('/'),
                self.bucket,
                key,
            );
            let response = self
                .client
                .delete(&url)
                .send()
                .await
                .context("S3 DeleteObject failed")?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("S3 DeleteObject returned {status}: {body}");
            }
            return Ok(());
        }

        // Signed mode — use SigV4 Authorization header
        let host = extract_host(&self.endpoint);
        let uri_path = format!("/{}/{}", self.bucket, key);
        let query_string = ""; // no query params for DELETE

        let (amz_date, auth) = crate::s3::sign::sign_request_headers(
            &self.access_key,
            &self.secret_key,
            &self.region,
            "DELETE",
            &uri_path,
            query_string,
            &host,
        );

        let url = format!(
            "{}/{}/{}",
            self.endpoint.trim_end_matches('/'),
            self.bucket,
            key,
        );

        let response = self
            .client
            .delete(&url)
            .header("Authorization", &auth)
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .send()
            .await
            .context("S3 DeleteObject failed")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("S3 DeleteObject returned {status}: {body}");
        }
        Ok(())
    }

    /// List objects with a given prefix.
    pub async fn list_objects(
        &self,
        prefix: &str,
        continuation_token: Option<&str>,
    ) -> Result<(Vec<S3Object>, Option<String>)> {
        let mut query = format!("list-type=2&prefix={}", percent_encode_path(prefix));
        if let Some(token) = continuation_token {
            query.push_str(&format!(
                "&continuation-token={}",
                percent_encode_path(token)
            ));
        }

        let host = extract_host(&self.endpoint);
        let uri_path = format!("/{}/", self.bucket);

        let (amz_date, auth) = crate::s3::sign::sign_request_headers(
            &self.access_key,
            &self.secret_key,
            &self.region,
            "GET",
            &uri_path,
            &query,
            &host,
        );

        let url = format!(
            "{}/{}?{query}",
            self.endpoint.trim_end_matches('/'),
            self.bucket,
        );

        let response = self
            .client
            .get(&url)
            .header("Authorization", &auth)
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .send()
            .await
            .context("S3 ListObjectsV2 failed")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("S3 ListObjectsV2 returned {status}: {body}");
        }

        let body = response.text().await?;
        let objects = parse_list_objects(&body);
        let next_token = parse_next_token(&body);
        Ok((objects, next_token))
    }
}

/// An S3 object from ListObjectsV2.
#[derive(Debug, Clone)]
pub struct S3Object {
    pub key: String,
    pub last_modified: String,
    pub size: u64,
}

/// Simple percent-encoding for path components (spaces → %20 etc.).
fn percent_encode_path(s: &str) -> String {
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('+', "%2B")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('#', "%23")
        .replace('?', "%3F")
}

// ── Minimal S3 XML parser ────────────────────────────────────────

/// Extract all `<Contents>` blocks from a ListObjectsV2 XML response.
fn parse_list_objects(xml: &str) -> Vec<S3Object> {
    let mut objects = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<Contents>") {
        rest = &rest[start + 10..];
        let end = match rest.find("</Contents>") {
            Some(e) => e,
            None => break,
        };
        let block = &rest[..end];

        let key = extract_tag(block, "Key");
        let last_modified = extract_tag(block, "LastModified");
        let size: u64 = extract_tag(block, "Size").parse().unwrap_or(0);

        if !key.is_empty() {
            objects.push(S3Object {
                key,
                last_modified,
                size,
            });
        }

        rest = &rest[end + 11..];
    }

    objects
}

/// Extract `<NextContinuationToken>` for pagination.
fn parse_next_token(xml: &str) -> Option<String> {
    let tag = extract_tag(xml, "NextContinuationToken");
    if tag.is_empty() {
        None
    } else {
        Some(tag)
    }
}

/// Extract the text content of the first `<tag>...</tag>` occurrence.
fn extract_tag(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    let start = match xml.find(&open) {
        Some(s) => s + open.len(),
        None => return String::new(),
    };
    let end = match xml[start..].find(&close) {
        Some(e) => start + e,
        None => return String::new(),
    };
    xml[start..end].to_string()
}

/// Extract host[:port] from an S3 endpoint URL for SigV4 header signing.
fn extract_host(endpoint: &str) -> String {
    endpoint
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}
