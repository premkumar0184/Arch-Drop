use crate::error::MeshError;
use crate::sync::messages::{SyncAck, SyncBatch, VectorExchange, WireEvent};
use archdrop_core::ledger::{Event, EventLedger, LedgerError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub type EventListener = Arc<dyn Fn(&Event) + Send + Sync + 'static>;

/// Statistics summarizing a completed synchronization session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStats {
    pub batches_sent: usize,
    pub events_sent: usize,
    pub batches_received: usize,
    pub events_received: usize,
    pub final_clock: HashMap<String, i64>,
}

/// Deterministic vector clock reconciliation engine.
#[derive(Clone)]
pub struct ReconciliationEngine {
    ledger: Arc<EventLedger>,
    listeners: Vec<EventListener>,
}

impl ReconciliationEngine {
    pub fn new(ledger: Arc<EventLedger>) -> Self {
        Self {
            ledger,
            listeners: Vec::new(),
        }
    }

    /// Attaches an event listener callback invoked whenever a remote event is successfully ingested.
    pub fn with_listener<F>(mut self, listener: F) -> Self
    where
        F: Fn(&Event) + Send + Sync + 'static,
    {
        self.listeners.push(Arc::new(listener));
        self
    }

    /// Access to the underlying local ledger.
    pub fn ledger(&self) -> &Arc<EventLedger> {
        &self.ledger
    }

    /// Generates Step 1 / Step 2 `VectorExchange` advertisement from the local ledger.
    pub fn create_vector_exchange(&self) -> Result<VectorExchange, MeshError> {
        let clock = self
            .ledger
            .get_vector_clock()
            .map_err(|e| MeshError::Protocol(e.to_string()))?;
        Ok(VectorExchange::new(self.ledger.local_device_id(), clock))
    }

    /// Calculates which events the LOCAL node possesses that the REMOTE node is missing.
    /// Returns map of `origin_device_id -> (start_seq, end_seq)` missing ranges on remote.
    pub fn calculate_remote_missing_intervals(
        &self,
        remote_clock: &HashMap<String, i64>,
    ) -> Result<HashMap<String, (i64, i64)>, MeshError> {
        let local_clock = self
            .ledger
            .get_vector_clock()
            .map_err(|e| MeshError::Protocol(e.to_string()))?;
        let mut missing = HashMap::new();

        for (origin, local_hwm) in &local_clock {
            let remote_hwm = remote_clock.get(origin).copied().unwrap_or(0);
            if *local_hwm > remote_hwm {
                missing.insert(origin.clone(), (remote_hwm + 1, *local_hwm));
            }
        }

        Ok(missing)
    }

    /// Calculates which events the LOCAL node is missing from the REMOTE node.
    /// Returns map of `origin_device_id -> (start_seq, end_seq)` missing ranges locally.
    pub fn calculate_local_missing_intervals(
        &self,
        remote_clock: &HashMap<String, i64>,
    ) -> Result<HashMap<String, (i64, i64)>, MeshError> {
        let local_clock = self
            .ledger
            .get_vector_clock()
            .map_err(|e| MeshError::Protocol(e.to_string()))?;
        let mut missing = HashMap::new();

        for (origin, remote_hwm) in remote_clock {
            let local_hwm = local_clock.get(origin).copied().unwrap_or(0);
            if *remote_hwm > local_hwm {
                missing.insert(origin.clone(), (local_hwm + 1, *remote_hwm));
            }
        }

        Ok(missing)
    }

    /// Creates a bounded `SyncBatch` of events for a specific origin strictly after `since_seq`.
    pub fn create_sync_batch(
        &self,
        origin: &str,
        since_seq: i64,
        limit: usize,
    ) -> Result<SyncBatch, MeshError> {
        let events = self
            .ledger
            .get_events_since(origin, since_seq, Some(limit + 1))
            .map_err(|e| MeshError::Protocol(e.to_string()))?;

        let has_more = events.len() > limit;
        let batch_events: Vec<WireEvent> = events
            .into_iter()
            .take(limit)
            .map(WireEvent::from)
            .collect();

        let start_seq = batch_events.first().map(|e| e.sequence_number).unwrap_or(since_seq + 1);
        let end_seq = batch_events.last().map(|e| e.sequence_number).unwrap_or(since_seq);

        Ok(SyncBatch::new(
            origin,
            start_seq,
            end_seq,
            has_more,
            batch_events,
        ))
    }

    /// Ingests a received `SyncBatch` with gap detection, transactional idempotence (`INSERT OR IGNORE`),
    /// and vector clock advancement.
    pub fn ingest_batch(&self, batch: &SyncBatch) -> Result<SyncAck, MeshError> {
        let clock = self
            .ledger
            .get_vector_clock()
            .map_err(|e| MeshError::Protocol(e.to_string()))?;


        let current_hwm = clock.get(&batch.origin_device_id).copied().unwrap_or(0);

        if batch.events.is_empty() {
            return Ok(SyncAck::success(
                &batch.origin_device_id,
                current_hwm,
                clock,
            ));
        }

        // Gap Detection Invariant:
        // If incoming batch starts at sequence > current_hwm + 1, sequence gap exists.
        let first_seq = batch.events[0].sequence_number;
        if first_seq > current_hwm + 1 {
            return Ok(SyncAck::gap_detected(
                &batch.origin_device_id,
                current_hwm,
                current_hwm + 1,
                clock,
            ));
        }

        // Convert wire events to domain events
        let domain_events: Vec<Event> = batch.events.iter().cloned().map(Event::from).collect();

        // Atomically apply batch in SQLite transaction
        match self.ledger.apply_batch(&domain_events) {
            Ok(_summary) => {
                // Notify listeners of newly ingested events
                for event in &domain_events {
                    for listener in &self.listeners {
                        listener(event);
                    }
                }

                let updated_clock = self
                    .ledger
                    .get_vector_clock()
                    .map_err(|e| MeshError::Protocol(e.to_string()))?;
                let max_applied = updated_clock
                    .get(&batch.origin_device_id)
                    .copied()
                    .unwrap_or(current_hwm);

                Ok(SyncAck::success(
                    &batch.origin_device_id,
                    max_applied,
                    updated_clock,
                ))
            }
            Err(LedgerError::PayloadHashMismatch { sequence, expected, actual }) => {
                Ok(SyncAck::error(
                    &batch.origin_device_id,
                    current_hwm,
                    format!("Payload hash mismatch at seq {}: expected {}, calculated {}", sequence, expected, actual),
                ))
            }
            Err(e) => Err(MeshError::Protocol(e.to_string())),
        }
    }

    /// Handles a received `SyncAck` by acknowledging outbox queue entries and updating event ack_status.
    pub fn handle_ack(&self, target_peer_id: &str, ack: &SyncAck) -> Result<(), MeshError> {
        if ack.is_success() {
            self.ledger
                .mark_outbox_acknowledged(target_peer_id, &ack.origin_device_id, ack.acknowledged_sequence)
                .map_err(|e| MeshError::Protocol(e.to_string()))?;
            self.ledger
                .mark_events_acknowledged(&ack.origin_device_id, ack.acknowledged_sequence)
                .map_err(|e| MeshError::Protocol(e.to_string()))?;
        }
        Ok(())
    }

    /// Performs a non-streaming pairwise reconciliation step:
    /// Given a remote vector exchange, produces the local vector exchange and all pending
    /// batches that the remote peer needs.
    pub fn reconcile_pairwise(
        &self,
        remote_exchange: &VectorExchange,
        limit_per_batch: usize,
    ) -> Result<(VectorExchange, Vec<SyncBatch>), MeshError> {
        let local_exchange = self.create_vector_exchange()?;
        let missing_remote = self.calculate_remote_missing_intervals(&remote_exchange.clock)?;
        let mut outgoing_batches = Vec::new();

        for (origin, (start_seq, _end_seq)) in missing_remote {
            let batch = self.create_sync_batch(&origin, start_seq - 1, limit_per_batch)?;
            if !batch.events.is_empty() {
                outgoing_batches.push(batch);
            }
        }

        Ok((local_exchange, outgoing_batches))
    }

    // --- Length-prefixed framing helpers for async streaming transport ---

    pub async fn write_envelope<W, T>(writer: &mut W, envelope: &T) -> Result<(), MeshError>
    where
        W: AsyncWrite + Unpin,
        T: Serialize,
    {
        let json_bytes = serde_json::to_vec(envelope).map_err(|e| MeshError::Frame(e.to_string()))?;
        let len = json_bytes.len() as u32;
        writer.write_all(&len.to_be_bytes()).await?;
        writer.write_all(&json_bytes).await?;
        writer.flush().await?;
        Ok(())
    }

    pub async fn read_envelope<R, T>(reader: &mut R) -> Result<T, MeshError>
    where
        R: AsyncRead + Unpin,
        T: for<'de> Deserialize<'de>,
    {
        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len > 10 * 1024 * 1024 {
            return Err(MeshError::Frame("Sync envelope exceeds 10 MB limit".into()));
        }

        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        let envelope = serde_json::from_slice(&buf).map_err(|e| MeshError::Frame(e.to_string()))?;
        Ok(envelope)
    }


    /// Full asynchronous 5-step vector reconciliation protocol runner over any bidirectional stream.
    ///
    /// Protocol Sequence:
    /// 1. Exchange Vector Clock advertisements
    /// 2. Compute missing sequence intervals
    /// 3. Stream missing batches ordered monotonically
    /// 4. Ingest and acknowledge batches
    /// 5. Reach verified causal convergence
    async fn send_missing_batches<S>(
        &self,
        stream: &mut S,
        peer_id: &str,
        remote_needs: &HashMap<String, (i64, i64)>,
        batch_limit: usize,
        stats: &mut SyncStats,
    ) -> Result<(), MeshError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut origins_to_send: Vec<String> = remote_needs.keys().cloned().collect();
        origins_to_send.sort();

        let total_origins = origins_to_send.len() as u32;
        stream.write_all(&total_origins.to_be_bytes()).await?;
        stream.flush().await?;

        for origin in origins_to_send {
            let (start_seq, _) = remote_needs[&origin];
            let mut cursor = start_seq - 1;
            loop {
                let batch = self.create_sync_batch(&origin, cursor, batch_limit)?;
                let event_count = batch.events.len();
                let has_more = batch.has_more;

                Self::write_envelope(stream, &batch).await?;
                stats.batches_sent += 1;
                stats.events_sent += event_count;

                let ack: SyncAck = Self::read_envelope(stream).await?;
                if ack.is_success() {
                    self.handle_ack(peer_id, &ack)?;
                    cursor = ack.acknowledged_sequence;
                } else if ack.is_gap() {
                    if let Some(exp) = ack.expected_sequence {
                        cursor = exp - 1;
                    }
                } else {
                    return Err(MeshError::Protocol(format!("Peer rejected batch: {:?}", ack.error)));
                }

                if !has_more || event_count == 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    async fn receive_missing_batches<S>(
        &self,
        stream: &mut S,
        stats: &mut SyncStats,
    ) -> Result<(), MeshError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut remote_origins_buf = [0u8; 4];
        stream.read_exact(&mut remote_origins_buf).await?;
        let remote_origin_count = u32::from_be_bytes(remote_origins_buf);

        for _ in 0..remote_origin_count {
            loop {
                let incoming_batch: SyncBatch = Self::read_envelope(stream).await?;
                let event_count = incoming_batch.events.len();
                let has_more = incoming_batch.has_more;

                let ack = self.ingest_batch(&incoming_batch)?;
                Self::write_envelope(stream, &ack).await?;

                stats.batches_received += 1;
                stats.events_received += event_count;

                if !has_more || event_count == 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Full asynchronous 5-step vector reconciliation protocol runner over any bidirectional stream.
    ///
    /// Protocol Sequence:
    /// 1. Exchange Vector Clock advertisements
    /// 2. Compute missing sequence intervals
    /// 3. Stream missing batches ordered monotonically
    /// 4. Ingest and acknowledge batches
    /// 5. Reach verified causal convergence
    pub async fn run_sync_session<S>(
        &self,
        stream: &mut S,
        peer_id: &str,
        batch_limit: usize,
    ) -> Result<SyncStats, MeshError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut stats = SyncStats::default();

        // Step 1: Send local VectorExchange
        let local_exchange = self.create_vector_exchange()?;
        Self::write_envelope(stream, &local_exchange).await?;

        // Step 2: Receive remote VectorExchange
        let remote_exchange: VectorExchange = Self::read_envelope(stream).await?;

        // Calculate delta intervals
        let remote_needs = self.calculate_remote_missing_intervals(&remote_exchange.clock)?;

        // Deterministic role assignment for half-duplex batch exchange
        let is_primary_sender = self.ledger.local_device_id() < peer_id;

        if is_primary_sender {
            self.send_missing_batches(stream, peer_id, &remote_needs, batch_limit, &mut stats).await?;
            self.receive_missing_batches(stream, &mut stats).await?;
        } else {
            self.receive_missing_batches(stream, &mut stats).await?;
            self.send_missing_batches(stream, peer_id, &remote_needs, batch_limit, &mut stats).await?;
        }

        stats.final_clock = self
            .ledger
            .get_vector_clock()
            .map_err(|e| MeshError::Protocol(e.to_string()))?;

        Ok(stats)
    }
}


