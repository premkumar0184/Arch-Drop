use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;

/// Monotonic compound primary key `(origin_device_id, sequence_number)`
pub type EventKey = (String, i64);

/// Delivery and acknowledgment lifecycle status for an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum AckStatus {
    Pending = 0,
    LocalCommitted = 1,
    InFlight = 2,
    Acknowledged = 3,
}

impl AckStatus {
    pub fn from_i32(val: i32) -> Self {
        match val {
            1 => Self::LocalCommitted,
            2 => Self::InFlight,
            3 => Self::Acknowledged,
            _ => Self::Pending,
        }
    }

    pub fn to_i32(self) -> i32 {
        self as i32
    }
}

/// Immutable event entry in the deterministic state ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
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

    #[serde(serialize_with = "serialize_payload", deserialize_with = "deserialize_payload")]
    pub payload: Vec<u8>,

    #[serde(alias = "payloadHash")]
    pub payload_hash: String,

    #[serde(alias = "ackStatus", default)]
    pub ack_status: i32,

    #[serde(alias = "createdAt", default)]
    pub created_at: i64,
}

/// Structural equality for `Event` comparing immutable domain content.
/// Node-local transient lifecycle status (`ack_status`) is ignored so origin and replica instances
/// evaluate as identical.
impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.origin_device_id == other.origin_device_id
            && self.sequence_number == other.sequence_number
            && self.event_id == other.event_id
            && self.timestamp_ms == other.timestamp_ms
            && self.event_type == other.event_type
            && self.payload == other.payload
            && self.payload_hash == other.payload_hash
    }
}

impl Eq for Event {}

impl Event {
    /// Creates a new local event, computing its SHA-256 payload hash and setting local committed status.
    pub fn new_local(
        origin_device_id: impl Into<String>,
        sequence_number: i64,
        event_type: impl Into<String>,
        payload: Vec<u8>,
        timestamp_ms: i64,
    ) -> Self {
        let hash = hex::encode(Sha256::digest(&payload));
        let event_id = uuid::Uuid::new_v4().to_string();
        Self {
            origin_device_id: origin_device_id.into(),
            sequence_number,
            event_id,
            timestamp_ms,
            event_type: event_type.into(),
            payload_hash: hash,
            payload,
            ack_status: AckStatus::LocalCommitted.to_i32(),
            created_at: timestamp_ms,
        }
    }

    /// Convenience accessor for valid UTF-8 text payloads.
    pub fn payload_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.payload)
    }

    /// Deserializes JSON payloads into structured types.
    pub fn payload_json<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.payload)
    }

    /// Re-calculates and returns the SHA-256 hash of the payload bytes.
    pub fn compute_payload_hash(&self) -> String {
        hex::encode(Sha256::digest(&self.payload))
    }

    /// Verifies whether the recorded payload_hash matches the actual payload bytes.
    pub fn verify_integrity(&self) -> bool {
        self.compute_payload_hash().eq_ignore_ascii_case(&self.payload_hash)
    }
}

fn serialize_payload<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if let Ok(s) = std::str::from_utf8(bytes) {
        serializer.serialize_str(s)
    } else {
        serializer.serialize_bytes(bytes)
    }
}

fn deserialize_payload<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PayloadVisitor;

    impl<'de> Visitor<'de> for PayloadVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string, raw byte array, or JSON value")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v.as_bytes().to_vec())
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v.into_bytes())
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v.to_vec())
        }

        fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut bytes = Vec::new();
            while let Some(byte) = seq.next_element::<u8>()? {
                bytes.push(byte);
            }
            Ok(bytes)
        }

        fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            let val = serde_json::Value::deserialize(de::value::MapAccessDeserializer::new(map))
                .map_err(de::Error::custom)?;
            serde_json::to_vec(&val).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(PayloadVisitor)
}

/// High-water mark cursor entry in `vector_clock` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorClockEntry {
    #[serde(alias = "peerDeviceId", alias = "peer")]
    pub peer_device_id: String,

    #[serde(alias = "maxSequenceNumber", alias = "max_seq")]
    pub max_sequence_number: i64,

    #[serde(alias = "lastSyncedAt", alias = "last_synced")]
    pub last_synced_at: i64,

    #[serde(alias = "syncState", alias = "state")]
    pub sync_state: String,
}

/// Pending transmission entry in the `outbox_queue` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboxEntry {
    #[serde(alias = "outboxId", alias = "queue_id")]
    pub outbox_id: i64,

    #[serde(alias = "originDeviceId", alias = "origin")]
    pub origin_device_id: String,

    #[serde(alias = "sequenceNumber", alias = "seq")]
    pub sequence_number: i64,

    #[serde(alias = "targetPeerId", alias = "target_peer")]
    pub target_peer_id: String,

    #[serde(alias = "retryCount", default)]
    pub retry_count: u32,

    #[serde(alias = "nextRetryAt", alias = "next_retry_ms", default)]
    pub next_retry_at: i64,

    #[serde(default = "default_status_pending")]
    pub status: String,
}

fn default_status_pending() -> String {
    "PENDING".to_string()
}

/// Summary of an atomic batch ingestion transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSummary {
    pub applied_count: usize,
    pub duplicate_count: usize,
    pub new_high_water_marks: HashMap<String, i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_partial_eq_ignores_ack_status() {
        let mut e1 = Event::new_local("DEV_A", 1, "clipboard.text", b"hello".to_vec(), 1000);
        let mut e2 = e1.clone();

        e1.ack_status = AckStatus::LocalCommitted.to_i32();
        e2.ack_status = AckStatus::Acknowledged.to_i32();

        assert_eq!(e1, e2);
        assert_eq!(e2, e1);

        // Different sequence number should not be equal
        e2.sequence_number = 2;
        assert_ne!(e1, e2);
    }
}

