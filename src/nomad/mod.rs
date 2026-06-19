//! Nomad integration: meta auto-discovery and drain events.
//!
//! When `mode = "nomad"` and `meta_enabled = true`, hoard polls
//! the Nomad API every N seconds, reads job metadata, and creates
//! virtual volumes from `hoard.*` meta keys.
//!
//! Meta takes priority over conf.d volumes — a Nomad job with
//! `hoard.class = "critical"` overrides any file-based volume
//! that matches the same path.

#![deny(unsafe_code)]

pub mod meta;
