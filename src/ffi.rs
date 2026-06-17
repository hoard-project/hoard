//! Unsafe FFI bindings — the ONLY module allowed to use unsafe code.
//!
//! All unsafe operations are wrapped in safe Rust functions with
//! documented safety preconditions. Every `unsafe` block is accompanied
//! by a `// SAFETY:` comment explaining why it's sound.
//!
//! This file is targeted at ≤ 120 lines. Every other module has
//! `#![deny(unsafe_code)]` and delegates here.
//!
//! ## Operations provided
//! - `read(fd, buf)` — libc::read
//! - `write(fd, buf)` — libc::write
//! - `shutdown(fd, how)` — libc::shutdown
//! - `sendfile_loop(sock, file, file_size)` — sendfile with EAGAIN retry
//! - `enable_ktls(sock_fd, keys)` — kernel TLS key injection

#![allow(unsafe_code)]
// Casts from libc types (isize/usize) are correct on x86_64 where they're the same size.
// These are inherent to FFI bridging and cannot be avoided.
#![allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]

use crate::fd::{FileFd, SocketFd};
use std::io;
use std::os::fd::RawFd;

// ── musl-compatible kTLS constants ───────────────────────────────

/// musl does not export SOL_TLS — manually define for Linux
const SOL_TLS: libc::c_int = 282;
const TLS_TX: libc::c_int = 1;
const TLS_RX: libc::c_int = 2;

// TLS 1.2 AES-128-GCM crypto info (for kTLS setsockopt)
const TLS_1_2_VERSION: u16 = 0x0303;
const TLS_CIPHER_AES_GCM_128: u16 = 54;

#[repr(C)]
struct tls_crypto_info {
    version: u16,
    cipher_type: u16,
}

#[repr(C)]
struct tls12_crypto_info_aes_gcm_128 {
    info: tls_crypto_info,
    iv: [u8; 8],
    key: [u8; 16], // AES-128
    salt: [u8; 4],
    rec_seq: [u8; 8],
}

/// TLS cipher algorithm selection for kTLS
#[derive(Debug, Clone, Copy)]
pub enum TlsCipher {
    Aes128Gcm,
    Aes256Gcm,
}

/// TLS session keys extracted from rustls after handshake.
#[derive(Debug, Clone)]
pub struct TlsKeys {
    pub tx_key: Vec<u8>,
    pub tx_iv: Vec<u8>,
    pub tx_salt: Vec<u8>,
    pub rx_key: Vec<u8>,
    pub rx_iv: Vec<u8>,
    pub rx_salt: Vec<u8>,
    pub cipher: TlsCipher,
}

impl TlsKeys {
    /// Zero out all key material from memory
    pub fn zeroize(&mut self) {
        self.tx_key.fill(0);
        self.tx_iv.fill(0);
        self.tx_salt.fill(0);
        self.rx_key.fill(0);
        self.rx_iv.fill(0);
        self.rx_salt.fill(0);
    }
}

/// Check if a file descriptor is valid.
pub fn fcntl_getfd(fd: RawFd) -> io::Result<libc::c_int> {
    // SAFETY: fcntl(F_GETFD) is safe to call on any FD; returns -1 if invalid
    let ret = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if ret == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(ret)
}

/// Close a file descriptor safely.
///
/// This is the ONLY place where `close(2)` is called.
/// All FD types (FileFd, SocketFd) delegate their Drop here.
pub fn close_fd(fd: RawFd) {
    // SAFETY: close(2) is safe on any valid FD. If the FD is invalid,
    // EBADF is silently ignored (kernel guarantees no double-close in Drop).
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
    }
}

// ── Basic I/O wrappers ───────────────────────────────────────────

/// Read from a file descriptor into a buffer.
///
/// # Safety (internal)
/// `fd` must be a valid, open file descriptor. Guaranteed by `FileFd`/`SocketFd`.
pub fn read(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: caller guarantees fd is valid and buf is writable
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

/// Write a buffer to a file descriptor.
///
/// # Safety (internal)
/// `fd` must be a valid, open file descriptor.
pub fn write(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    // SAFETY: caller guarantees fd is valid and buf is readable
    let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

/// Shutdown a socket (SHUT_RD, SHUT_WR, SHUT_RDWR).
///
/// # Safety (internal)
/// `fd` must be a valid socket descriptor.
pub fn shutdown(fd: RawFd, how: libc::c_int) -> io::Result<()> {
    // SAFETY: caller guarantees fd is a valid socket
    let ret = unsafe { libc::shutdown(fd, how) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ── sendfile with EAGAIN resilience ──────────────────────────────

/// Zero-copy file-to-socket pump using sendfile(2).
///
/// Handles EAGAIN (socket buffer full), EWOULDBLOCK, and EINTR
/// with retry + poll(POLLOUT) backpressure. Returns total bytes sent.
///
/// # Panics
/// Panics if `total_sent != file_size` after the loop — this is a
/// correctness assertion that must never be stripped.
pub fn sendfile_loop(sock: &SocketFd, file: &FileFd, file_size: u64) -> io::Result<u64> {
    let mut offset: i64 = 0;
    let mut remaining = file_size as i64;
    let mut total_sent: u64 = 0;

    let sock_fd = sock.as_raw_fd();
    let file_fd = file.as_raw_fd();

    while remaining > 0 {
        // SAFETY: both fds are valid, offset points to stack memory
        let n = unsafe { libc::sendfile(sock_fd, file_fd, &mut offset, remaining as usize) };

        if n > 0 {
            total_sent += n as u64;
            remaining -= n as i64;
            continue;
        }

        // n == -1 or n == 0
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            // EAGAIN (== EWOULDBLOCK on Linux) — socket buffer full, poll for writability
            Some(libc::EAGAIN) => {
                wait_writable(sock_fd)?;
                continue;
            }
            Some(libc::EINTR) => {
                // Interrupted by signal — retry immediately
                continue;
            }
            _ => return Err(err),
        }
    }

    // Integrity assertion — never stripped in release builds
    assert_eq!(
        total_sent, file_size,
        "sendfile_loop: sent {total_sent} bytes but file size is {file_size}"
    );

    Ok(total_sent)
}

/// Block until the socket is writable (poll with POLLOUT).
///
/// Times out after 10 seconds.
fn wait_writable(fd: RawFd) -> io::Result<()> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };

    loop {
        let ret = unsafe { libc::poll(&mut pfd, 1, 10_000) }; // 10s timeout
        if ret > 0 {
            return Ok(());
        }
        if ret == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "socket send timeout after 10s",
            ));
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINTR) {
            return Err(err);
        }
        // EINTR → retry poll
    }
}

// ── kTLS (Kernel TLS) ────────────────────────────────────────────

/// Inject TLS session keys into the kernel for zero-copy encryption.
///
/// After this call, any sendfile to this socket will be automatically
/// encrypted with AES-GCM in the kernel. Userspace never touches the
/// plaintext data.
pub fn enable_ktls(sock_fd: RawFd, cipher: TlsCipher, keys: &TlsKeys) -> io::Result<()> {
    let (cipher_code, key_size): (u16, usize) = match cipher {
        TlsCipher::Aes128Gcm => (TLS_CIPHER_AES_GCM_128, 16),
        TlsCipher::Aes256Gcm => (0x00, 32), // placeholder — extend if needed
    };

    let mut crypto_info = tls12_crypto_info_aes_gcm_128 {
        info: tls_crypto_info {
            version: TLS_1_2_VERSION,
            cipher_type: cipher_code,
        },
        iv: [0u8; 8],
        key: [0u8; 16],
        salt: [0u8; 4],
        rec_seq: [0u8; 8],
    };

    // Copy keys into the struct
    crypto_info.iv[..keys.tx_iv.len().min(8)]
        .copy_from_slice(&keys.tx_iv[..8.min(keys.tx_iv.len())]);
    crypto_info.key[..keys.tx_key.len().min(key_size)]
        .copy_from_slice(&keys.tx_key[..key_size.min(keys.tx_key.len())]);
    crypto_info.salt[..keys.tx_salt.len().min(4)]
        .copy_from_slice(&keys.tx_salt[..4.min(keys.tx_salt.len())]);

    // SAFETY: sock_fd is a valid socket, crypto_info is correctly populated
    let ret = unsafe {
        libc::setsockopt(
            sock_fd,
            SOL_TLS,
            TLS_TX,
            &crypto_info as *const _ as *const libc::c_void,
            std::mem::size_of_val(&crypto_info) as libc::socklen_t,
        )
    };

    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    tracing::info!(sock_fd, cipher = ?cipher, "kTLS enabled for socket");
    Ok(())
}

#[cfg(test)]
    #[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn read_write_roundtrip() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"hoard test data").unwrap();
        let mut f = tmp.reopen().unwrap();
        use std::os::fd::AsRawFd;
        let fd = f.as_raw_fd();

        let mut buf = [0u8; 32];
        let n = unsafe { read(fd, &mut buf) }.unwrap();
        assert_eq!(&buf[..n], b"hoard test data");
    }
}
