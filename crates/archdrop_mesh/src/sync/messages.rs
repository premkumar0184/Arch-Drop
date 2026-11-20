use archdrop_core::ledger::Event;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_type_vector_exchange() -> String {
    "sync.vector_exchange".to_string()
}

fn default_type_batch() -> String {
    "sync.batch".to_string()
}

fn default_type_ack() -> String {
    "sync.ack".to_string()
}

fn default_status_success() -> String {
    "SUCCESS".to_string()
}

fn default_batch_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Message 1 & 2: Vector Clock High-Water Mark Exchange Envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorExchange {
    #[serde(default = "default_type_vector_exchange")]
    pub r#type: String,

    #[serde(alias = "originDeviceId", alias = "origin", alias = "senderDeviceId")]
    pub origin_device_id: String,

    #[serde(alias = "vector")]
    pub clock: HashMap<String, i64>,

    #[serde(alias = "timestampMs", default)]
    pub timestamp_ms: i64,
}

impl VectorExchange {
    pub fn new(origin_device_id: impl Into<String>, clock: HashMap<String, i64>) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Self {
            r#type: default_type_vector_exchange(),
            origin_device_id: origin_device_id.into(),
            clock,
            timestamp_ms: now_ms,
        }
    }
}

/// Serde wire representation of an individual ledger event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireEvent {
    #[serde(alias = "originDeviceId", alias = "origin")]
    pub origin_device_id: String,

    #[serde(alias = "sequenceNumber", alias = "seq")]
    pub sequence_number: i64,

    #[serde(alias = "eventId", alias = "id")]
    pub event_id: String,

    #[serde(alias = "timestampMs", alias = "timestamp")]
    pub timestamp_ms: i64,

    #[serde(alias = "eventType", alias = "type")]
    pub event_type: String,

    pub payload: String,

    #[serde(alias = "payloadHash")]
    pub payload_hash: String,

    #[serde(alias = "ackStatus", default)]
    pub ack_status: i32,

    #[serde(alias = "createdAt", default)]
    pub created_at: i64,
}

impl From<Event> for WireEvent {
    fn from(e: Event) -> Self {
        let payload_str = String::from_utf8(e.payload.clone())
            .unwrap_or_else(|_| hex::encode(&e.payload));
        Self {
            origin_device_id: e.origin_device_id,
            sequence_number: e.sequence_number,
            event_id: e.event_id,
            timestamp_ms: e.timestamp_ms,
            event_type: e.event_type,
            payload: payload_str,
            payload_hash: e.payload_hash,
            ack_status: e.ack_status,
            created_at: e.created_at,
        }
    }
}

impl From<WireEvent> for Event {
    fn from(w: WireEvent) -> Self {
        Event {
            origin_device_id: w.origin_device_id,
            sequence_number: w.sequence_number,
            event_id: w.event_id,
            timestamp_ms: w.timestamp_ms,
            event_type: w.event_type,
            payload: w.payload.into_bytes(),
            payload_hash: w.payload_hash,
            ack_status: w.ack_status,
            created_at: w.created_at,
        }
    }
}

/// Message 3 & 5: Contiguous, Monotonically Ordered Event Batch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncBatch {
    #[serde(default = "default_type_batch")]
    pub r#type: String,

    #[serde(alias = "originDeviceId", alias = "origin")]
    pub origin_device_id: String,

    #[serde(alias = "batchId", default = "default_batch_id")]
    pub batch_id: String,

    #[serde(alias = "startSequence", default)]
    pub start_sequence: i64,

    #[serde(alias = "endSequence", default)]
    pub end_sequence: i64,

    #[serde(alias = "hasMore", default)]
    pub has_more: bool,

    pub events: Vec<WireEvent>,
}

impl SyncBatch {
    pub fn new(
        origin_device_id: impl Into<String>,
        start_sequence: i64,
        end_sequence: i64,
        has_more: bool,
        events: Vec<WireEvent>,
    ) -> Self {
        Self {
            r#type: default_type_batch(),
            origin_device_id: origin_device_id.into(),
            batch_id: default_batch_id(),
            start_sequence,
            end_sequence,
            has_more,
            events,
        }
    }
}

/// Message 4 & 6: Batch Ingestion Acknowledgment & Vector Clock Advancement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncAck {
    #[serde(default = "default_type_ack")]
    pub r#type: String,

    #[serde(alias = "originDeviceId", alias = "origin")]
    pub origin_device_id: String,

    #[serde(alias = "acknowledgedSequence", default)]
    pub acknowledged_sequence: i64,

    #[serde(alias = "vector", default)]
    pub clock: HashMap<String, i64>,

    #[serde(default = "default_status_success")]
    pub status: String,

    #[serde(alias = "expectedSequence", default, skip_serializing_if = "Option::is_none")]
    pub expected_sequence: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SyncAck {
    pub fn success(
        origin_device_id: impl Into<String>,
        acknowledged_sequence: i64,
        clock: HashMap<String, i64>,
    ) -> Self {
        Self {
            r#type: default_type_ack(),
            origin_device_id: origin_device_id.into(),
            acknowledged_sequence,
            clock,
            status: default_status_success(),
            expected_sequence: None,
            error: None,
        }
    }

    pub fn gap_detected(
        origin_device_id: impl Into<String>,
        current_hwm: i64,
        expected_sequence: i64,
        clock: HashMap<String, i64>,
    ) -> Self {
        Self {
            r#type: default_type_ack(),
            origin_device_id: origin_device_id.into(),
            acknowledged_sequence: current_hwm,
            clock,
            status: "GAP_DETECTED".to_string(),
            expected_sequence: Some(expected_sequence),
            error: Some(format!(
                "Sequence gap detected: high-water mark is {}, expected {}",
                current_hwm, expected_sequence
            )),
        }
    }

    pub fn error(
        origin_device_id: impl Into<String>,
        acknowledged_sequence: i64,
        error_msg: impl Into<String>,
    ) -> Self {
        Self {
            r#type: default_type_ack(),
            origin_device_id: origin_device_id.into(),
            acknowledged_sequence,
            clock: HashMap::new(),
            status: "ERROR".to_string(),
            expected_sequence: None,
            error: Some(error_msg.into()),
        }
    }

    pub fn is_success(&self) -> bool {
        self.status == "SUCCESS"
    }

    pub fn is_gap(&self) -> bool {
        self.status == "GAP_DETECTED"
    }
}
