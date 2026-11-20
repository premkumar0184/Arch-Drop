//! Deterministic Vector Clock Reconciliation Engine & Wire Protocol.
//!
//! Implements:
//! - 5-step wire synchronization exchange (`VectorExchange`, `SyncBatch`, `SyncAck`)
//! - Monotonic sequence streaming with gap detection and recovery
//! - Transactional idempotent ingestion (`INSERT OR IGNORE`)
//! - Conflict resolution for single-state consumers with zero dropped payloads

pub mod engine;
pub mod messages;

pub use engine::{EventListener, ReconciliationEngine, SyncStats};
pub use messages::{SyncAck, SyncBatch, VectorExchange, WireEvent};
