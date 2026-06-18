//! Trigger sources for upload initiation.
//!
//! `TriggerSource` is an exhaustive enum — adding a new deployment mode
//! forces every match statement in the project to handle it, making
//! partial implementations a compile error.

#![deny(unsafe_code)]

pub mod nomad;
pub mod standalone;

use tokio::sync::mpsc;

/// The source of upload triggers, determined by deployment mode.
///
/// **Exhaustive enum:** adding a variant causes compile errors in
/// every `match` across the project, enforcing full coverage.
pub enum TriggerSource {
    /// Standalone mode: Unix socket IPC + SIGTERM handler
    Standalone {
        /// Receiver for flush commands from hoardctl
        flush_rx: mpsc::Receiver<()>,
        /// SIGTERM signal stream
        term: tokio::signal::unix::Signal,
    },
    /// Nomad cluster mode: SSE event stream from Nomad agent
    Nomad {
        /// SSE-based allocation event stream
        sse: nomad::NomadEventStream,
    },
}

impl TriggerSource {
    /// Convert the trigger source into a channel of string events.
    ///
    /// Spawns internal tasks that translate each mode's event type
    /// into a unified `mpsc::UnboundedReceiver<String>` for the main loop.
    pub fn into_channel(self) -> mpsc::UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded_channel();

        match self {
            Self::Standalone {
                mut flush_rx,
                mut term,
            } => {
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            Some(()) = flush_rx.recv() => {
                                let _ = tx.send("flush".to_string());
                            }
                            _ = term.recv() => {
                                let _ = tx.send("SIGTERM".to_string());
                                break;
                            }
                            else => break,
                        }
                    }
                });
            }
            Self::Nomad { mut sse } => {
                tokio::spawn(async move {
                    loop {
                        match sse.next().await {
                            Some(event) if event.is_drain() => {
                                let msg = format!("drain:{}:{}", event.job, event.alloc_id);
                                if tx.send(msg).is_err() {
                                    break;
                                }
                            }
                            None => break,
                            _ => continue,
                        }
                    }
                });
            }
        }

        rx
    }
}
