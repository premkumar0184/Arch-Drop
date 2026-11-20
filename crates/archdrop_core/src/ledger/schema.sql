-- Arch Drop Deterministic Event Ledger Schema
-- SQLite WAL mode tables

CREATE TABLE IF NOT EXISTS events (
    origin_device_id TEXT NOT NULL,
    sequence_number INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload BLOB NOT NULL,
    payload_hash TEXT NOT NULL,
    ack_status INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (origin_device_id, sequence_number)
);

CREATE INDEX IF NOT EXISTS idx_events_origin_seq 
ON events (origin_device_id, sequence_number ASC);

CREATE INDEX IF NOT EXISTS idx_events_timestamp 
ON events (timestamp_ms ASC);

CREATE INDEX IF NOT EXISTS idx_events_type_time 
ON events (event_type, timestamp_ms DESC);

CREATE TABLE IF NOT EXISTS vector_clock (
    peer_device_id TEXT PRIMARY KEY NOT NULL,
    max_sequence_number INTEGER NOT NULL DEFAULT 0,
    last_synced_at INTEGER NOT NULL DEFAULT 0,
    sync_state TEXT NOT NULL DEFAULT 'IDLE'
);

CREATE TABLE IF NOT EXISTS outbox_queue (
    outbox_id INTEGER PRIMARY KEY AUTOINCREMENT,
    origin_device_id TEXT NOT NULL,
    sequence_number INTEGER NOT NULL,
    target_peer_id TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'PENDING',
    FOREIGN KEY (origin_device_id, sequence_number) 
        REFERENCES events (origin_device_id, sequence_number) 
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_outbox_peer_status 
ON outbox_queue (target_peer_id, status, next_retry_at ASC);
