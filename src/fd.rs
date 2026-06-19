#![allow(dead_code)]
//! File descriptor newtypes providing type-safe FD management.
//!
//! `FileFd` and `SocketFd` are zero-cost wrappers around `RawFd`
//! that prevent accidental interchange in sendfile calls.
//!
//! ## Safety invariants
//! - `FileFd` wraps a regular file descriptor (seekable, not a socket/pipes)
//! - `SocketFd` wraps a TCP socket descriptor
//! - Both implement `Drop` to close the FD via `ffi::close_fd`, preventing leaks
//! - No `From<RawFd>` or `From<i32>` — must use explicit constructors

#![deny(unsafe_code)]

use std::fs::File;
use std::io;
use std::os::fd::{IntoRawFd, RawFd};

/// A regular file descriptor (seekable, mmap-able).
///
/// Constructed only from `std::fs::File`. Cannot be created from
/// a bare `RawFd` — prevents accidental socket/file confusion.
#[derive(Debug)]
pub struct FileFd(RawFd);

impl FileFd {
    /// Consume a `std::fs::File` and take ownership of its FD.
    ///
    /// The original `File` is consumed — only the raw FD is kept.
    pub fn from_file(f: File) -> Self {
        let fd = f.into_raw_fd();
        Self(fd)
    }

    /// Return the raw FD for use in FFI calls.
    pub fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Drop for FileFd {
    fn drop(&mut self) {
        crate::ffi::close_fd(self.0);
    }
}

/// A TCP socket descriptor.
///
/// Constructed from `tokio::TcpStream::into_std()` or `std::net::TcpStream`.
#[derive(Debug)]
pub struct SocketFd(RawFd);

impl SocketFd {
    /// Take ownership of a raw socket FD.
    pub fn from_raw(fd: RawFd) -> Self {
        Self(fd)
    }

    /// Consume this wrapper and return the raw FD.
    ///
    /// Used when transferring ownership back to tokio or for shutdown.
    pub fn into_raw_fd(self) -> RawFd {
        let fd = self.0;
        std::mem::forget(self); // prevent Drop from closing
        fd
    }

    /// Return the raw FD without consuming the wrapper.
    pub fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl From<std::net::TcpStream> for SocketFd {
    fn from(s: std::net::TcpStream) -> Self {
        let fd = s.into_raw_fd();
        Self(fd)
    }
}

impl Drop for SocketFd {
    fn drop(&mut self) {
        crate::ffi::close_fd(self.0);
    }
}

/// Construct a `FileFd` by opening a path read-only.
pub fn open_file_read(path: &std::path::Path) -> io::Result<FileFd> {
    let file = File::open(path)?;
    Ok(FileFd::from_file(file))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn filefd_from_file_works() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"hello").unwrap();
        let fd = FileFd::from_file(tmp.into_file());
        assert!(fd.as_raw_fd() >= 0);
    }

    #[test]
    fn filefd_drop_closes() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"world").unwrap();
        let fd = FileFd::from_file(tmp.into_file());
        let raw = fd.as_raw_fd();
        drop(fd);
        // After drop, the FD should be closed
        let ret = crate::ffi::fcntl_getfd(raw);
        assert!(ret.is_err());
    }
}
