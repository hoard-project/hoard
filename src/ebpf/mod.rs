//! eBPF subsystem: program loading, RingBuffer polling, and stat-based debounce.
//!
//! See §四 Layer 1 of the architecture document.

#![deny(unsafe_code)]

pub mod debounce;
pub mod filter;
pub mod loader;
pub mod resolve;

pub use filter::FileFilter;
pub use loader::BpfProgram;
