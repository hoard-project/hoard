//! Nomad integration: API client, meta auto-discovery, and drain events.
//!
//! - `client` — HTTP API client for Nomad (job listing, alloc query, meta scan)
//! - `meta`  — periodic discovery: polls Nomad for `hoard.*` meta → dynamic volumes
//!
//! When `mode = "nomad"` and `meta_enabled = true`, hoard polls
//! the Nomad API every N seconds, reads job metadata, and creates
//! virtual volumes from `hoard.*` meta keys.

#![deny(unsafe_code)]

pub mod client;
pub mod meta;
