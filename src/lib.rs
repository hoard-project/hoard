//! Hoard library crate — shared between `hoard` daemon and `hoardctl`.
//!
//! Only re-exports the CLI modules needed by binaries.

pub mod cli;
pub mod config;
pub mod ebpf;
pub mod fd;
pub mod ffi;
pub mod hoard;
pub mod s3;
pub mod trigger;
pub mod upload;
pub mod metrics;
