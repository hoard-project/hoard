//! Zero-copy upload pipeline with compile-time type-state enforcement.
//!
//! The upload pipeline is a type-state machine:
//!
//! ```text
//! Pending → Checkpointed → Presigned → Connected → HeaderWritten → BodyTransmitted → UploadOutcome
//! ```
//!
//! Each state is a zero-size type parameter `S` on `UploadPipeline<S>`.
//! State transitions consume `self` and return a new type. Methods only
//! exist on the correct state — calling them out of order is a **compile error**.

#![deny(unsafe_code)]

pub mod outcome;
pub mod pipeline;
pub mod retry;

// Public state tokens — zero-size types that encode upload progress
