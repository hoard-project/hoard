//! AWS SigV4 signing for S3 presigned URLs.
//!
//! Pure Rust implementation without external AWS SDK crates.
//! Algorithm matches the standard SigV4 presigned URL spec:
//! https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html

#![deny(unsafe_code)]

use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// Generate a presigned PUT URL using AWS SigV4.
pub async fn presign_put(
    access_key: &str,
    secret_key: &str,
    region: &str,
    endpoint: &str,
    bucket: &str,
    key: &str,
    ttl: Duration,
) -> Result<String> {
    presign_url(
        "PUT", access_key, secret_key, region, endpoint, bucket, key, ttl,
    )
}

/// Generate a presigned GET URL using AWS SigV4.
pub async fn presign_get(
    access_key: &str,
    secret_key: &str,
    region: &str,
    endpoint: &str,
    bucket: &str,
    key: &str,
    ttl: Duration,
) -> Result<String> {
    presign_url(
        "GET", access_key, secret_key, region, endpoint, bucket, key, ttl,
    )
}

/// Generate a presigned DELETE URL using AWS SigV4.
pub async fn presign_delete(
    access_key: &str,
    secret_key: &str,
    region: &str,
    endpoint: &str,
    bucket: &str,
    key: &str,
    ttl: Duration,
) -> Result<String> {
    presign_url(
        "DELETE", access_key, secret_key, region, endpoint, bucket, key, ttl,
    )
}

/// Shared presigned URL builder for any HTTP method.
fn presign_url(
    method: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
    endpoint: &str,
    bucket: &str,
    key: &str,
    ttl: Duration,
) -> Result<String> {
    let endpoint = endpoint.trim_end_matches('/');

    // Extract host[:port] from endpoint URL for canonical headers
    let endpoint_host = endpoint
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    // ── Current UTC time ──────────────────────────────────────────
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let now_secs = now.as_secs();
    // Format: 20260616T170000Z
    let amz_date = format_amz_date(now_secs);
    let date_stamp = &amz_date[..8]; // 20260616

    // ── Build canonical query string (sorted alphabetically) ─────
    let credential = format!("{access_key}/{date_stamp}/{region}/s3/aws4_request");
    let ttl_secs = ttl.as_secs();

    let ttl_str = ttl_secs.to_string();

    let query_params: Vec<(&str, &str)> = vec![
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
        ("X-Amz-Credential", &credential),
        ("X-Amz-Date", &amz_date),
        ("X-Amz-Expires", &ttl_str),
        ("X-Amz-SignedHeaders", "host"),
    ];
    let canonical_querystring = build_canonical_querystring(&query_params);

    // ── Canonical request ─────────────────────────────────────────
    let canonical_uri = format!("/{bucket}/{key}");
    let canonical_headers = format!("host:{endpoint_host}");
    let signed_headers = "host";
    let payload_hash = "UNSIGNED-PAYLOAD";

    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_querystring}\n{canonical_headers}\n\n{signed_headers}\n{payload_hash}"
    );
    let canonical_request_hash = hex::encode(sha2_hash(canonical_request.as_bytes()));

    // ── String to sign ────────────────────────────────────────────
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}");

    // ── Signing key derivation ────────────────────────────────────
    let signing_key = derive_signing_key(secret_key, date_stamp, region, "s3");

    // ── Signature ─────────────────────────────────────────────────
    let signature = hex::encode(hmac_sign(&signing_key, string_to_sign.as_bytes()));

    // ── Assemble final URL ────────────────────────────────────────
    let signed_url =
        format!("{endpoint}{canonical_uri}?{canonical_querystring}&X-Amz-Signature={signature}");

    tracing::debug!(bucket, key, %signed_url, "presigned URL generated");
    Ok(signed_url)
}

// ── Helper functions ──────────────────────────────────────────────

fn format_amz_date(unix_secs: u64) -> String {
    // Convert to UTC date components manually (no chrono dependency)
    let secs_per_day: u64 = 86400;
    let days_since_epoch = unix_secs / secs_per_day;

    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days_since_epoch + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let secs_into_day = unix_secs % secs_per_day;
    let hours = secs_into_day / 3600;
    let minutes = (secs_into_day % 3600) / 60;
    let seconds = secs_into_day % 60;

    format!("{y:04}{m:02}{d:02}T{hours:02}{minutes:02}{seconds:02}Z")
}

fn build_canonical_querystring(params: &[(&str, &str)]) -> String {
    let mut encoded: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect();
    // Already sorted by construction (input is sorted)
    encoded.join("&")
}

/// Percent-encode per AWS SigV4 rules: encode everything except
/// unreserved characters (A-Z, a-z, 0-9, -, _, ., ~).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b'/' => {
                // SigV4 requires / encode as %2F
                out.push_str("%2F");
            }
            _ => {
                out.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    out
}

fn sha2_hash(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

fn hmac_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key derivation failed");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn derive_signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sign(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sign(&k_date, region.as_bytes());
    let k_service = hmac_sign(&k_region, service.as_bytes());
    hmac_sign(&k_service, b"aws4_request")
}

/// Generate a SigV4 Authorization header for a direct (non-presigned) request.
///
/// Returns `(amz_date, authorization_header_value)`.
pub fn sign_request_headers(
    access_key: &str,
    secret_key: &str,
    region: &str,
    method: &str,
    uri_path: &str,
    query_string: &str,
    host: &str,
) -> (String, String) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let now_secs = now.as_secs();
    let amz_date = format_amz_date(now_secs);
    let date_stamp = &amz_date[..8];

    let credential = format!("{access_key}/{date_stamp}/{region}/s3/aws4_request");
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");

    // Canonical request — use UNSIGNED-PAYLOAD for simplicity
    let payload_hash = "UNSIGNED-PAYLOAD";

    let canonical_uri = if uri_path.is_empty() || uri_path.starts_with('/') {
        uri_path.to_string()
    } else {
        format!("/{uri_path}")
    };

    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";

    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{query_string}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let canonical_request_hash = hex::encode(sha2_hash(canonical_request.as_bytes()));

    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}");

    let signing_key = derive_signing_key(secret_key, date_stamp, region, "s3");
    let signature = hex::encode(hmac_sign(&signing_key, string_to_sign.as_bytes()));

    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={credential},SignedHeaders={signed_headers},Signature={signature}"
    );

    (amz_date, auth)
}

#[cfg(test)]
    #[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn presign_generates_url_with_signature() {
        let url = presign_put(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "https://s3.amazonaws.com",
            "test-bucket",
            "test-key.txt",
            Duration::from_secs(300),
        )
        .await
        .unwrap();

        assert!(url.contains("X-Amz-Signature="));
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(url.contains("test-bucket/test-key.txt"));
    }
}
