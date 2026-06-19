//! CLI entry points for `hoard` daemon and `hoardctl` control tool.

#![deny(unsafe_code)]

pub mod ctl;
pub mod daemon;
pub mod restore;

use crate::config::{Config, ValidatedConfig};
use anyhow::Result;
use clap::Parser;

/// Parse CLI arguments (and optional TOML config) → validated configuration.
pub fn parse_config() -> Result<ValidatedConfig> {
    Config::try_parse()?.load()
}
