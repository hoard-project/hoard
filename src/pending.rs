//! Persistent pending-set backed by SQLite.
//!
//! On crash/restart, pending files are recovered from the database.
//! Each BPF event inserts; each successful upload deletes;
//! drain clears all entries. SQLite's crash-safety guarantees
//! no partial state after power loss.

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// A pending-set that survives process restarts via SQLite.
pub struct PersistentPending {
    set: std::collections::HashSet<PathBuf>,
    db: Connection,
}

impl PersistentPending {
    /// Open (or create) the pending database at `db_path`.
    /// Recovers any previously-persisted entries into the in-memory set.
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = Connection::open(db_path)
            .with_context(|| format!("failed to open pending db at {}", db_path.display()))?;

        // Crash-safe: WAL mode + synchronous NORMAL
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS pending (
                 path TEXT PRIMARY KEY,
                 added_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .context("failed to initialize pending table")?;

        // Recover existing entries
        let mut set = std::collections::HashSet::new();
        let mut stmt = db.prepare("SELECT path FROM pending")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let path = row?;
            set.insert(PathBuf::from(path));
        }

        tracing::info!(count = set.len(), "pending db opened, recovered entries");

        Ok(Self { set, db })
    }

    /// Insert a path into the pending set. Idempotent.
    pub fn insert(&mut self, path: &Path) {
        if self.set.insert(path.to_path_buf()) {
            // Best-effort persistence — log but don't crash on DB error
            if let Err(e) = self.db.execute(
                "INSERT OR IGNORE INTO pending (path) VALUES (?1)",
                rusqlite::params![path.to_string_lossy().as_ref()],
            ) {
                tracing::error!(%e, path = %path.display(), "failed to persist pending insert");
            }
        }
    }

    /// Remove a path from the pending set (after successful upload).
    pub fn remove(&mut self, path: &Path) {
        if self.set.remove(path) {
            if let Err(e) = self.db.execute(
                "DELETE FROM pending WHERE path = ?1",
                rusqlite::params![path.to_string_lossy().as_ref()],
            ) {
                tracing::error!(%e, path = %path.display(), "failed to persist pending remove");
            }
        }
    }

    /// Drain all entries and clear the database.
    pub fn drain(&mut self) -> Vec<PathBuf> {
        let files: Vec<PathBuf> = self.set.drain().collect();
        if let Err(e) = self.db.execute_batch("DELETE FROM pending;") {
            tracing::error!(%e, "failed to clear pending database during drain");
        }
        files
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }
}
