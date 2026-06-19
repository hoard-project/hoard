//! Integrity verification — post-upload MD5 comparison with S3 ETag.
//!
//! MinIO (and most S3-compatible servers) set the ``ETag`` header to
//! the MD5 digest of the object body on PUT. We compute the same digest
//! locally and compare. A mismatch indicates silent corruption — either
//! in transit (sendfile), in the kernel, or at the S3 layer.

#![deny(unsafe_code)]

use std::io::Read;
use std::path::Path;

/// Read a local file and compute its MD5 hex digest.
pub fn file_md5(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("verify open {p}: {e}", p = path.display()))?;
    let mut ctx = md5::Context::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("verify read {p}: {e}", p = path.display()))?;
        if n == 0 {
            break;
        }
        ctx.consume(&buf[..n]);
    }
    let digest = ctx.finalize();
    Ok(format!("{digest:x}"))
}

/// Compare a local file's MD5 with the S3 ETag.
///
/// ETags from MinIO are the hex MD5 digest without quotes.
/// Returns `Ok(())` on match, `Err(msg)` on mismatch.
pub fn verify_etag(path: &Path, etag: &str) -> Result<(), String> {
    // MinIO sometimes wraps ETags in double quotes. Strip them.
    let etag = etag.trim_matches('"');
    let local = file_md5(path)?;
    if local.eq_ignore_ascii_case(etag) {
        Ok(())
    } else {
        Err(format!("ETag mismatch: local={local} s3={etag}",))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_md5_known() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.bin");
        std::fs::write(&path, b"hello world\n").expect("write test file");
        let digest = file_md5(&path).expect("compute md5");
        // echo -n "hello world\n" | md5sum → 6f5902ac237024bdd0c176cb93063dc4
        assert_eq!(digest, "6f5902ac237024bdd0c176cb93063dc4");
    }

    #[test]
    fn test_verify_etag_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.bin");
        std::fs::write(&path, b"hello world\n").expect("write test file");
        assert!(verify_etag(&path, "6f5902ac237024bdd0c176cb93063dc4").is_ok());
    }

    #[test]
    fn test_verify_etag_quoted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.bin");
        std::fs::write(&path, b"hello world\n").expect("write test file");
        assert!(verify_etag(&path, "\"6f5902ac237024bdd0c176cb93063dc4\"").is_ok());
    }

    #[test]
    fn test_verify_etag_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.bin");
        std::fs::write(&path, b"hello world\n").expect("write test file");
        assert!(verify_etag(&path, "deadbeef").is_err());
    }
}
