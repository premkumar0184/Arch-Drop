use crate::ledger::error::LedgerError;
use crate::ledger::models::{AckStatus, Event, OutboxEntry, SyncSummary, VectorClockEntry};
use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Thread-safe SQLite deterministic state ledger.
#[derive(Clone)]
pub struct EventLedger {
    conn: Arc<Mutex<Connection>>,
    local_device_id: String,
}

impl EventLedger {
    /// Opens or creates a persistent SQLite ledger at the specified path.
    pub fn open<P: AsRef<Path>>(path: P, local_device_id: impl Into<String>) -> Result<Self, LedgerError> {
        let conn = Connection::open(path)?;
        let ledger = Self {
            conn: Arc::new(Mutex::new(conn)),
            local_device_id: local_device_id.into(),
        };
        ledger.init_db()?;
        Ok(ledger)
    }

    /// Opens an in-memory SQLite ledger (ideal for isolated testing and fast benchmarks).
    pub fn open_in_memory(local_device_id: impl Into<String>) -> Result<Self, LedgerError> {
        let conn = Connection::open_in_memory()?;
        let ledger = Self {
            conn: Arc::new(Mutex::new(conn)),
            local_device_id: local_device_id.into(),
        };
        ledger.init_db()?;
        Ok(ledger)
    }

    /// Returns the local device ID configured for this ledger.
    pub fn local_device_id(&self) -> &str {
        &self.local_device_id
    }

    fn init_db(&self) -> Result<(), LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;",
        )?;
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(())
    }

    fn current_timestamp_ms() -> Result<i64, LedgerError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        Ok(now.as_millis() as i64)
    }

    /// Records a new local event, allocating the next monotonic sequence number for `local_device_id`.
    /// Returns the monotonic compound key `(origin_device_id, sequence_number)`.
    pub fn record_event(&self, event_type: &str, payload: &[u8]) -> Result<(String, i64), LedgerError> {
        self.record_event_for_origin(&self.local_device_id.clone(), event_type, payload)
    }

    /// Records a new event for an explicit origin device ID.
    pub fn record_event_for_origin(
        &self,
        origin_device_id: &str,
        event_type: &str,
        payload: &[u8],
    ) -> Result<(String, i64), LedgerError> {
        let now_ms = Self::current_timestamp_ms()?;
        let payload_hash = hex::encode(Sha256::digest(payload));
        let event_id = Uuid::new_v4().to_string();

        let mut conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Strictly monotonic sequence allocation per origin: max(sequence_number) + 1
        let next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sequence_number), 0) + 1 FROM events WHERE origin_device_id = ?1",
            params![origin_device_id],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO events (
                origin_device_id, sequence_number, event_id, timestamp_ms,
                event_type, payload, payload_hash, ack_status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                origin_device_id,
                next_seq,
                event_id,
                now_ms,
                event_type,
                payload,
                payload_hash,
                AckStatus::LocalCommitted.to_i32(),
                now_ms,
            ],
        )?;

        // Advance vector clock for this origin
        tx.execute(
            "INSERT INTO vector_clock (peer_device_id, max_sequence_number, last_synced_at, sync_state)
             VALUES (?1, ?2, ?3, 'COMMITTED')
             ON CONFLICT(peer_device_id) DO UPDATE SET
                max_sequence_number = MAX(max_sequence_number, excluded.max_sequence_number),
                last_synced_at = excluded.last_synced_at,
                sync_state = excluded.sync_state",
            params![origin_device_id, next_seq, now_ms],
        )?;

        tx.commit()?;
        Ok((origin_device_id.to_string(), next_seq))
    }

    /// Records a new event locally and enqueues outbox tasks for all specified paired peers.
    pub fn record_event_with_peers(
        &self,
        event_type: &str,
        payload: &[u8],
        paired_peers: &[String],
    ) -> Result<Event, LedgerError> {
        let now_ms = Self::current_timestamp_ms()?;
        let payload_hash = hex::encode(Sha256::digest(payload));
        let event_id = Uuid::new_v4().to_string();

        let mut conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sequence_number), 0) + 1 FROM events WHERE origin_device_id = ?1",
            params![self.local_device_id],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO events (
                origin_device_id, sequence_number, event_id, timestamp_ms,
                event_type, payload, payload_hash, ack_status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.local_device_id,
                next_seq,
                event_id,
                now_ms,
                event_type,
                payload,
                payload_hash,
                AckStatus::LocalCommitted.to_i32(),
                now_ms,
            ],
        )?;

        for peer_id in paired_peers {
            tx.execute(
                "INSERT INTO outbox_queue (
                    origin_device_id, sequence_number, target_peer_id,
                    retry_count, next_retry_at, status
                 ) VALUES (?1, ?2, ?3, 0, ?4, 'PENDING')",
                params![self.local_device_id, next_seq, peer_id, now_ms],
            )?;
        }

        tx.execute(
            "INSERT INTO vector_clock (peer_device_id, max_sequence_number, last_synced_at, sync_state)
             VALUES (?1, ?2, ?3, 'COMMITTED')
             ON CONFLICT(peer_device_id) DO UPDATE SET
                max_sequence_number = MAX(max_sequence_number, excluded.max_sequence_number),
                last_synced_at = excluded.last_synced_at,
                sync_state = excluded.sync_state",
            params![self.local_device_id, next_seq, now_ms],
        )?;

        tx.commit()?;

        Ok(Event {
            origin_device_id: self.local_device_id.clone(),
            sequence_number: next_seq,
            event_id,
            timestamp_ms: now_ms,
            event_type: event_type.to_string(),
            payload: payload.to_vec(),
            payload_hash,
            ack_status: AckStatus::LocalCommitted.to_i32(),
            created_at: now_ms,
        })
    }

    /// Retrieves the current vector clock mapping each origin/peer to its highest processed sequence number.
    pub fn get_vector_clock(&self) -> Result<HashMap<String, i64>, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let mut clock = HashMap::new();

        // 1. Gather recorded high water marks from vector_clock table
        let mut stmt = conn.prepare("SELECT peer_device_id, max_sequence_number FROM vector_clock")?;
        let rows = stmt.query_map([], |row| {
            let peer: String = row.get(0)?;
            let max_seq: i64 = row.get(1)?;
            Ok((peer, max_seq))
        })?;

        for row in rows {
            let (peer, max_seq) = row?;
            clock.insert(peer, max_seq);
        }

        // 2. Ensure all origins present in `events` table are accounted for
        let mut events_stmt = conn.prepare(
            "SELECT origin_device_id, MAX(sequence_number) FROM events GROUP BY origin_device_id",
        )?;
        let event_rows = events_stmt.query_map([], |row| {
            let origin: String = row.get(0)?;
            let max_seq: i64 = row.get(1)?;
            Ok((origin, max_seq))
        })?;

        for row in event_rows {
            let (origin, max_seq) = row?;
            clock
                .entry(origin)
                .and_modify(|current| *current = (*current).max(max_seq))
                .or_insert(max_seq);
        }

        Ok(clock)
    }

    /// Retrieves structured vector clock entries from the `vector_clock` table.
    pub fn get_vector_clock_entries(&self) -> Result<Vec<VectorClockEntry>, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT peer_device_id, max_sequence_number, last_synced_at, sync_state FROM vector_clock ORDER BY peer_device_id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(VectorClockEntry {
                peer_device_id: row.get(0)?,
                max_sequence_number: row.get(1)?,
                last_synced_at: row.get(2)?,
                sync_state: row.get(3)?,
            })
        })?;

        let mut entries = Vec::new();
        for entry in rows {
            entries.push(entry?);
        }
        Ok(entries)
    }


    /// Queries contiguous events for a specific origin strictly above `since_seq`, ordered by `sequence_number ASC`.
    pub fn get_events_since(
        &self,
        origin: &str,
        since_seq: i64,
        limit: Option<usize>,
    ) -> Result<Vec<Event>, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let limit_val = limit.unwrap_or(usize::MAX) as i64;

        let mut stmt = conn.prepare(
            "SELECT origin_device_id, sequence_number, event_id, timestamp_ms,
                    event_type, payload, payload_hash, ack_status, created_at
             FROM events
             WHERE origin_device_id = ?1 AND sequence_number > ?2
             ORDER BY sequence_number ASC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![origin, since_seq, limit_val], |row| {
            Ok(Event {
                origin_device_id: row.get(0)?,
                sequence_number: row.get(1)?,
                event_id: row.get(2)?,
                timestamp_ms: row.get(3)?,
                event_type: row.get(4)?,
                payload: row.get(5)?,
                payload_hash: row.get(6)?,
                ack_status: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;

        let mut events = Vec::new();
        for event in rows {
            events.push(event?);
        }
        Ok(events)
    }

    /// Returns all events currently stored in the ledger ordered chronologically.
    pub fn get_all_events(&self) -> Result<Vec<Event>, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT origin_device_id, sequence_number, event_id, timestamp_ms,
                    event_type, payload, payload_hash, ack_status, created_at
             FROM events
             ORDER BY timestamp_ms ASC, origin_device_id ASC, sequence_number ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(Event {
                origin_device_id: row.get(0)?,
                sequence_number: row.get(1)?,
                event_id: row.get(2)?,
                timestamp_ms: row.get(3)?,
                event_type: row.get(4)?,
                payload: row.get(5)?,
                payload_hash: row.get(6)?,
                ack_status: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;

        let mut events = Vec::new();
        for event in rows {
            events.push(event?);
        }
        Ok(events)
    }

    /// Returns events matching a specific event type ordered by timestamp descending.
    pub fn get_events_by_type(&self, event_type: &str, limit: Option<usize>) -> Result<Vec<Event>, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let limit_val = limit.unwrap_or(usize::MAX) as i64;

        let mut stmt = conn.prepare(
            "SELECT origin_device_id, sequence_number, event_id, timestamp_ms,
                    event_type, payload, payload_hash, ack_status, created_at
             FROM events
             WHERE event_type = ?1
             ORDER BY timestamp_ms DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![event_type, limit_val], |row| {
            Ok(Event {
                origin_device_id: row.get(0)?,
                sequence_number: row.get(1)?,
                event_id: row.get(2)?,
                timestamp_ms: row.get(3)?,
                event_type: row.get(4)?,
                payload: row.get(5)?,
                payload_hash: row.get(6)?,
                ack_status: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;

        let mut events = Vec::new();
        for event in rows {
            events.push(event?);
        }
        Ok(events)
    }

    /// Atomically ingests an incoming batch of events inside an immediate SQLite transaction.
    ///
    /// Invariant Guarantees:
    /// - Payload SHA-256 verification: rejects batch with `PayloadHashMismatch` if corrupt.
    /// - Idempotent ingestion: Uses `INSERT OR IGNORE` on `(origin_device_id, sequence_number)`.
    /// - Vector advancement: Advances `vector_clock` high-water mark to `max(sequence_number)` for each origin.
    /// - Zero dropped payloads: All valid events are committed.
    pub fn apply_batch(&self, events: &[Event]) -> Result<SyncSummary, LedgerError> {
        if events.is_empty() {
            return Ok(SyncSummary::default());
        }

        let now_ms = Self::current_timestamp_ms()?;
        let mut conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let mut applied_count = 0;
        let mut duplicate_count = 0;
        let mut origin_max_seqs: HashMap<String, i64> = HashMap::new();

        for event in events {
            // 1. Bit-for-bit SHA-256 verification
            let computed_hash = hex::encode(Sha256::digest(&event.payload));
            if !computed_hash.eq_ignore_ascii_case(&event.payload_hash) {
                return Err(LedgerError::PayloadHashMismatch {
                    expected: event.payload_hash.clone(),
                    actual: computed_hash,
                    sequence: event.sequence_number,
                });
            }

            let event_created_at = if event.created_at > 0 {
                event.created_at
            } else {
                now_ms
            };

            // 2. Idempotent insertion
            let changes = tx.execute(
                "INSERT OR IGNORE INTO events (
                    origin_device_id, sequence_number, event_id, timestamp_ms,
                    event_type, payload, payload_hash, ack_status, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    event.origin_device_id,
                    event.sequence_number,
                    event.event_id,
                    event.timestamp_ms,
                    event.event_type,
                    event.payload,
                    event.payload_hash,
                    AckStatus::Acknowledged.to_i32(),
                    event_created_at,
                ],
            )?;


            if changes > 0 {
                applied_count += 1;
            } else {
                duplicate_count += 1;
            }

            // Track max sequence per origin
            origin_max_seqs
                .entry(event.origin_device_id.clone())
                .and_modify(|current| *current = (*current).max(event.sequence_number))
                .or_insert(event.sequence_number);
        }

        // 3. Advance vector clock for each origin
        for (origin, max_seq) in &origin_max_seqs {
            tx.execute(
                "INSERT INTO vector_clock (peer_device_id, max_sequence_number, last_synced_at, sync_state)
                 VALUES (?1, ?2, ?3, 'SYNCED')
                 ON CONFLICT(peer_device_id) DO UPDATE SET
                    max_sequence_number = MAX(max_sequence_number, excluded.max_sequence_number),
                    last_synced_at = excluded.last_synced_at,
                    sync_state = excluded.sync_state",
                params![origin, max_seq, now_ms],
            )?;
        }

        tx.commit()?;

        Ok(SyncSummary {
            applied_count,
            duplicate_count,
            new_high_water_marks: origin_max_seqs,
        })
    }

    /// Enqueues outbox records for a given event across target peer devices.
    pub fn enqueue_outbox(
        &self,
        origin: &str,
        sequence_number: i64,
        target_peers: &[String],
    ) -> Result<(), LedgerError> {
        let now_ms = Self::current_timestamp_ms()?;
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;

        for peer_id in target_peers {
            conn.execute(
                "INSERT INTO outbox_queue (
                    origin_device_id, sequence_number, target_peer_id,
                    retry_count, next_retry_at, status
                 ) VALUES (?1, ?2, ?3, 0, ?4, 'PENDING')",
                params![origin, sequence_number, peer_id, now_ms],
            )?;
        }
        Ok(())
    }

    /// Retrieves pending outbox tasks targeted for a specific peer.
    pub fn get_pending_outbox(&self, target_peer_id: &str, limit: usize) -> Result<Vec<OutboxEntry>, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let now_ms = Self::current_timestamp_ms()?;

        let mut stmt = conn.prepare(
            "SELECT outbox_id, origin_device_id, sequence_number, target_peer_id,
                    retry_count, next_retry_at, status
             FROM outbox_queue
             WHERE target_peer_id = ?1 AND status = 'PENDING' AND next_retry_at <= ?2
             ORDER BY outbox_id ASC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![target_peer_id, now_ms, limit as i64], |row| {
            let retry_raw: i64 = row.get(4)?;
            Ok(OutboxEntry {
                outbox_id: row.get(0)?,
                origin_device_id: row.get(1)?,
                sequence_number: row.get(2)?,
                target_peer_id: row.get(3)?,
                retry_count: retry_raw as u32,
                next_retry_at: row.get(5)?,
                status: row.get(6)?,
            })
        })?;

        let mut entries = Vec::new();
        for entry in rows {
            entries.push(entry?);
        }
        Ok(entries)
    }

    /// Marks outbox items as acknowledged by deleting all outbox tasks for origin <= ack_seq.
    pub fn mark_outbox_acknowledged(
        &self,
        target_peer_id: &str,
        origin: &str,
        ack_seq: i64,
    ) -> Result<usize, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let rows = conn.execute(
            "DELETE FROM outbox_queue
             WHERE target_peer_id = ?1 AND origin_device_id = ?2 AND sequence_number <= ?3",
            params![target_peer_id, origin, ack_seq],
        )?;
        Ok(rows)
    }

    /// Marks events for a given origin up to ack_seq as acknowledged (AckStatus::Acknowledged).
    pub fn mark_events_acknowledged(
        &self,
        origin: &str,
        ack_seq: i64,
    ) -> Result<usize, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE events
             SET ack_status = ?1
             WHERE origin_device_id = ?2 AND sequence_number <= ?3",
            params![AckStatus::Acknowledged.to_i32(), origin, ack_seq],
        )?;
        Ok(rows)
    }

    /// Records an outbox retry attempt with exponential backoff timestamp.
    pub fn record_outbox_retry(&self, outbox_id: i64, next_retry_delay_ms: i64) -> Result<(), LedgerError> {
        let now_ms = Self::current_timestamp_ms()?;
        let next_retry_at = now_ms + next_retry_delay_ms;
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;

        conn.execute(
            "UPDATE outbox_queue
             SET retry_count = retry_count + 1,
                 next_retry_at = ?1
             WHERE outbox_id = ?2",
            params![next_retry_at, outbox_id],
        )?;
        Ok(())
    }

    /// Deletes an event from the ledger. Cascades to `outbox_queue` due to FOREIGN KEY CASCADE.
    pub fn delete_event(&self, origin: &str, sequence_number: i64) -> Result<bool, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let rows = conn.execute(
            "DELETE FROM events WHERE origin_device_id = ?1 AND sequence_number = ?2",
            params![origin, sequence_number],
        )?;
        Ok(rows > 0)
    }

    /// Counts total stored events in the ledger.
    pub fn count_events(&self) -> Result<usize, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Counts total pending tasks in the outbox queue.
    pub fn count_outbox(&self) -> Result<usize, LedgerError> {
        let conn = self.conn.lock().map_err(|e| LedgerError::Other(e.to_string()))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM outbox_queue", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;


    #[test]
    fn test_ledger_monotonic_allocation_concurrent_10_threads() {
        let ledger = EventLedger::open_in_memory("NODE_DESKTOP_A").unwrap();
        let total_threads = 10;
        let events_per_thread = 10;
        let mut handles = Vec::new();

        for _ in 0..total_threads {
            let l_clone = ledger.clone();
            let handle = thread::spawn(move || {
                let mut seqs = Vec::new();
                for i in 0..events_per_thread {
                    let payload = format!("concurrent event payload {}", i).into_bytes();
                    let (origin, seq) = l_clone.record_event("clipboard.text", &payload).unwrap();
                    assert_eq!(origin, "NODE_DESKTOP_A");
                    seqs.push(seq);
                }
                seqs
            });
            handles.push(handle);
        }

        let mut all_seqs = Vec::new();
        for h in handles {
            let thread_seqs = h.join().unwrap();
            all_seqs.extend(thread_seqs);
        }

        all_seqs.sort();
        assert_eq!(all_seqs.len(), total_threads * events_per_thread);

        // Verify strictly contiguous sequence 1..100 with zero gaps
        for (idx, seq) in all_seqs.iter().enumerate() {
            assert_eq!(*seq, (idx + 1) as i64);
        }

        // Verify ledger count matches exactly
        assert_eq!(ledger.count_events().unwrap(), 100);

        // Vector clock should reflect max_seq = 100 for NODE_DESKTOP_A
        let clock = ledger.get_vector_clock().unwrap();
        assert_eq!(*clock.get("NODE_DESKTOP_A").unwrap(), 100);
    }

    #[test]
    fn test_ledger_idempotent_ingestion() {
        let ledger = EventLedger::open_in_memory("LOCAL_NODE").unwrap();

        let event1 = Event::new_local("REMOTE_NODE_B", 1, "clipboard.text", b"hello".to_vec(), 1000);
        let event2 = Event::new_local("REMOTE_NODE_B", 2, "transfer.status", b"progress".to_vec(), 2000);
        let batch = vec![event1, event2];

        // First ingestion: applies 2 events
        let summary1 = ledger.apply_batch(&batch).unwrap();
        assert_eq!(summary1.applied_count, 2);
        assert_eq!(summary1.duplicate_count, 0);
        assert_eq!(ledger.count_events().unwrap(), 2);

        // Second ingestion with identical batch: idempotent 0 applied, 2 duplicates
        let summary2 = ledger.apply_batch(&batch).unwrap();
        assert_eq!(summary2.applied_count, 0);
        assert_eq!(summary2.duplicate_count, 2);
        assert_eq!(ledger.count_events().unwrap(), 2);
    }

    #[test]
    fn test_ledger_payload_hash_verification_rejection() {
        let ledger = EventLedger::open_in_memory("LOCAL_NODE").unwrap();

        let mut corrupt_event = Event::new_local("REMOTE_NODE_B", 1, "clipboard.text", b"authentic".to_vec(), 1000);
        corrupt_event.payload = b"tampered payload".to_vec();

        let err = ledger.apply_batch(&[corrupt_event]).unwrap_err();
        match err {
            LedgerError::PayloadHashMismatch { sequence, .. } => {
                assert_eq!(sequence, 1);
            }
            other => panic!("Expected PayloadHashMismatch, got {:?}", other),
        }

        assert_eq!(ledger.count_events().unwrap(), 0);
    }

    #[test]
    fn test_ledger_outbox_cascade_prune() {
        let ledger = EventLedger::open_in_memory("LOCAL_NODE").unwrap();
        let peers = vec!["PEER_1".to_string(), "PEER_2".to_string()];

        let event = ledger
            .record_event_with_peers("clipboard.text", b"secret token", &peers)
            .unwrap();

        assert_eq!(event.sequence_number, 1);
        assert_eq!(ledger.count_events().unwrap(), 1);
        assert_eq!(ledger.count_outbox().unwrap(), 2);

        // Outbox tasks should be present for both peers
        let pending_peer1 = ledger.get_pending_outbox("PEER_1", 10).unwrap();
        assert_eq!(pending_peer1.len(), 1);
        assert_eq!(pending_peer1[0].sequence_number, 1);

        // Deleting the event must cascade delete all associated outbox records
        let deleted = ledger.delete_event(&event.origin_device_id, event.sequence_number).unwrap();
        assert!(deleted);

        assert_eq!(ledger.count_events().unwrap(), 0);
        assert_eq!(ledger.count_outbox().unwrap(), 0);
    }

    #[test]
    fn test_get_events_since_ordering() {
        let ledger = EventLedger::open_in_memory("LOCAL_NODE").unwrap();

        for i in 1..=5 {
            ledger
                .record_event_for_origin("REMOTE_ORIGIN", "clipboard.text", format!("item {}", i).as_bytes())
                .unwrap();
        }

        let events_after_2 = ledger.get_events_since("REMOTE_ORIGIN", 2, None).unwrap();
        assert_eq!(events_after_2.len(), 3);
        assert_eq!(events_after_2[0].sequence_number, 3);
        assert_eq!(events_after_2[1].sequence_number, 4);
        assert_eq!(events_after_2[2].sequence_number, 5);

        // With limit
        let limited = ledger.get_events_since("REMOTE_ORIGIN", 2, Some(2)).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].sequence_number, 3);
        assert_eq!(limited[1].sequence_number, 4);
    }

    #[test]
    fn test_mark_events_acknowledged() {
        let ledger = EventLedger::open_in_memory("LOCAL_NODE").unwrap();

        for i in 1..=3 {
            ledger
                .record_event("clipboard.text", format!("msg {}", i).as_bytes())
                .unwrap();
        }

        let events_before = ledger.get_all_events().unwrap();
        assert_eq!(events_before.len(), 3);
        for ev in &events_before {
            assert_eq!(ev.ack_status, AckStatus::LocalCommitted.to_i32());
        }

        // Acknowledge events up to sequence 2
        let updated = ledger.mark_events_acknowledged("LOCAL_NODE", 2).unwrap();
        assert_eq!(updated, 2);

        let events_after = ledger.get_all_events().unwrap();
        assert_eq!(events_after[0].ack_status, AckStatus::Acknowledged.to_i32());
        assert_eq!(events_after[1].ack_status, AckStatus::Acknowledged.to_i32());
        assert_eq!(events_after[2].ack_status, AckStatus::LocalCommitted.to_i32());
    }
}


