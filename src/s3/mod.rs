//! S3 backend abstraction with credential gating.
//!
//! `S3Backend` stores credentials but cannot perform S3 operations.
//! Only `VerifiedS3Backend` (obtained via `.verify()`) can access S3.
//! This prevents unverified credentials from being used accidentally.

#![deny(unsafe_code)]

pub mod backend;
pub mod gc;
pub mod sign;

pub use backend::{S3Backend, VerifiedS3Backend};
