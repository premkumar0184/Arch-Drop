//! Deterministic State Ledger & Monotonic Sequence Store backed by SQLite (WAL mode).
//!
//! Provides:
//! - Monotonic compound primary key `(origin_device_id, sequence_number)`
//! - WAL-mode SQLite storage engine (`EventLedger`)
//! - Deduplication via idempotent `INSERT OR IGNORE` transactions
//! - High-water mark vector clock cursor maintenance
//! - Outbox task queue with foreign-key cascade pruning
//! - Last-Write-Wins (LWW) conflict arbiter with device ID tie-breaker

pub mod arbiter;
pub mod error;
pub mod models;
pub mod store;

pub use arbiter::{arbitrate_clipboard, arbitrate_lww, compare_events_lww};
pub use error::LedgerError;
pub use models::{AckStatus, Event, EventKey, OutboxEntry, SyncSummary, VectorClockEntry};
pub use store::EventLedger;
