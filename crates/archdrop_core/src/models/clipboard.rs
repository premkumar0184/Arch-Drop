//! Universal Clipboard Data Models & Wire Schemas.
//!
//! Enforces Apple Continuity reference patterns:
//! - 4 KB threshold rule: Payloads <= 4096 bytes are inlined; payloads > 4096 bytes use metadata descriptors.
//! - 120-second ephemeral TTL.
//! - Session lock zeroization support.
//! - Legacy KDE Connect packet compatibility (`kdeconnect.clipboard`).

use serde::{Deserialize, Serialize};

/// Maximum payload byte size permitted for immediate inlined synchronization.
///
/// Payloads exceeding this threshold transition to the lazy on-demand metadata descriptor tier.
pub const CLIPBOARD_INLINE_THRESHOLD: usize = 4096;

/// Default expiration time-to-live for synchronized clipboard entries (120 seconds).
pub const CLIPBOARD_EPHEMERAL_TTL_SECS: u64 = 120;

/// Reason for a clipboard zeroization or clear event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearReason {
    SessionLock,
    TtlExpired,
    UserAction,
}

/// Content type classifier for clipboard payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardType {
    Text,
    Image,
    File,
    Binary,
}

/// Event dispatched when system screen/session is locked or unlocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLockEvent {
    pub locked: bool,
    pub timestamp_epoch_ms: u64,
}

/// Unified clipboard item representation used by the clipboard engine and IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardItemDto {
    /// Unique identifier for this item (UUIDv4).
    pub id: String,

    /// MIME content type (e.g. "text/plain;charset=utf-8", "image/png").
    pub mime_type: String,

    /// Size of the payload in bytes.
    pub size: usize,

    /// Hexadecimal lowercase SHA-256 checksum of the raw bytes.
    pub sha256: String,

    /// Unix timestamp in milliseconds when the item was copied.
    pub timestamp_epoch_ms: u64,

    /// Flag indicating whether this item contains sensitive data (e.g. password manager hint).
    pub is_sensitive: bool,

    /// Inlined text content if <= 4 KB and valid UTF-8.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// URL/endpoint for lazy retrieval if > 4 KB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

/// Inlined payload content (<= 4 KB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardInlinePayload {
    pub mime_type: String,
    pub content: String,
    pub sha256: String,
    pub size_bytes: usize,
}

/// Metadata descriptor for large clipboard payloads (> 4 KB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardDescriptorPayload {
    /// Unique payload identifier (UUIDv4) for HTTP retrieval.
    pub payload_id: String,

    /// Primary MIME type of the payload.
    pub primary_mime: String,

    /// All MIME types offered by the source.
    pub available_mimes: Vec<String>,

    /// Exact size of the raw binary payload in bytes.
    pub size_bytes: usize,

    /// SHA-256 checksum for integrity verification upon retrieval.
    pub sha256: String,

    /// Single-use or ephemeral token required to download the payload.
    pub ephemeral_token: String,

    /// Optional direct download URL (e.g. `http://<ip>:53317/api/v2/clipboard/payload/<id>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

/// Domain payload envelope supporting both inline content and metadata descriptors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipboardPayload {
    #[serde(rename = "inline")]
    Inline(ClipboardInlinePayload),
    #[serde(rename = "descriptor")]
    Descriptor(ClipboardDescriptorPayload),
}

impl ClipboardPayload {
    /// Returns true if this payload is inlined.
    pub fn is_inline(&self) -> bool {
        matches!(self, ClipboardPayload::Inline(_))
    }

    /// Returns the byte size of the underlying clipboard data.
    pub fn size_bytes(&self) -> usize {
        match self {
            ClipboardPayload::Inline(p) => p.size_bytes,
            ClipboardPayload::Descriptor(p) => p.size_bytes,
        }
    }

    /// Returns the SHA-256 checksum string.
    pub fn sha256(&self) -> &str {
        match self {
            ClipboardPayload::Inline(p) => &p.sha256,
            ClipboardPayload::Descriptor(p) => &p.sha256,
        }
    }

    /// Returns the primary MIME type.
    pub fn mime_type(&self) -> &str {
        match self {
            ClipboardPayload::Inline(p) => &p.mime_type,
            ClipboardPayload::Descriptor(p) => &p.primary_mime,
        }
    }
}

/// Full network wire packet envelope for Arch Drop native clipboard sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClipboardPacket {
    #[serde(rename = "clipboard.inline")]
    Inline(ClipboardInlinePacket),

    #[serde(rename = "clipboard.descriptor")]
    Descriptor(ClipboardDescriptorPacket),

    #[serde(rename = "clipboard.clear")]
    Clear(ClipboardClearPacket),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardInlinePacket {
    pub version: u32,
    pub device_id: String,
    pub timestamp: u64,
    pub ttl_seconds: u64,
    pub payload: ClipboardInlinePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardDescriptorPacket {
    pub version: u32,
    pub device_id: String,
    pub timestamp: u64,
    pub ttl_seconds: u64,
    pub descriptor: ClipboardDescriptorPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardClearPacket {
    pub version: u32,
    pub device_id: String,
    pub timestamp: u64,
    pub reason: ClearReason,
}

/// KDE Connect legacy wire format envelope (`kdeconnect.clipboard`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdeConnectPacket {
    pub id: i64,
    #[serde(rename = "type")]
    pub packet_type: String,
    pub body: KdeConnectClipboard,
}

/// Body payload for KDE Connect clipboard packet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KdeConnectClipboard {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

impl KdeConnectPacket {
    pub fn new_clipboard(content: String) -> Self {
        Self {
            id: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            packet_type: "kdeconnect.clipboard".to_string(),
            body: KdeConnectClipboard {
                content,
                timestamp: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inline_packet_serde() {
        let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
            version: 2,
            device_id: "laptop-01".to_string(),
            timestamp: 1794022800000,
            ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
            payload: ClipboardInlinePayload {
                mime_type: "text/plain;charset=utf-8".to_string(),
                content: "https://archlinux.org".to_string(),
                sha256: "abcdef1234567890".to_string(),
                size_bytes: 21,
            },
        });

        let json = serde_json::to_string(&packet).expect("serialize inline");
        assert!(json.contains("\"type\":\"clipboard.inline\""));
        assert!(json.contains("\"deviceId\":\"laptop-01\""));
        assert!(json.contains("\"ttlSeconds\":120"));
        assert!(json.contains("\"content\":\"https://archlinux.org\""));

        let deserialized: ClipboardPacket =
            serde_json::from_str(&json).expect("deserialize inline");
        assert_eq!(packet, deserialized);
    }

    #[test]
    fn test_descriptor_packet_serde() {
        let packet = ClipboardPacket::Descriptor(ClipboardDescriptorPacket {
            version: 2,
            device_id: "laptop-01".to_string(),
            timestamp: 1794022800000,
            ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
            descriptor: ClipboardDescriptorPayload {
                payload_id: "b3e3e0f4-f5dd-e193-88b1-ccaf3d4ef544".to_string(),
                primary_mime: "image/png".to_string(),
                available_mimes: vec!["image/png".to_string(), "image/bmp".to_string()],
                size_bytes: 5_242_880,
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_string(),
                ephemeral_token: "tok_123456".to_string(),
                download_url: Some(
                    "https://10.2.0.2:53317/api/v2/clipboard/payload/b3e3e0f4".to_string(),
                ),
            },
        });

        let json = serde_json::to_string(&packet).expect("serialize descriptor");
        assert!(json.contains("\"type\":\"clipboard.descriptor\""));
        assert!(json.contains("\"payloadId\":\"b3e3e0f4-f5dd-e193-88b1-ccaf3d4ef544\""));
        assert!(json.contains("\"sizeBytes\":5242880"));

        let deserialized: ClipboardPacket =
            serde_json::from_str(&json).expect("deserialize descriptor");
        assert_eq!(packet, deserialized);
    }

    #[test]
    fn test_clear_packet_serde() {
        let packet = ClipboardPacket::Clear(ClipboardClearPacket {
            version: 2,
            device_id: "laptop-01".to_string(),
            timestamp: 1794022920000,
            reason: ClearReason::SessionLock,
        });

        let json = serde_json::to_string(&packet).expect("serialize clear");
        assert!(json.contains("\"type\":\"clipboard.clear\""));
        assert!(json.contains("\"reason\":\"session_lock\""));

        let deserialized: ClipboardPacket = serde_json::from_str(&json).expect("deserialize clear");
        assert_eq!(packet, deserialized);
    }

    #[test]
    fn test_kdeconnect_packet_serde() {
        let packet = KdeConnectPacket {
            id: 1794022800000,
            packet_type: "kdeconnect.clipboard".to_string(),
            body: KdeConnectClipboard {
                content: "Synced text from Android".to_string(),
                timestamp: Some(1794022800000),
            },
        };

        let json = serde_json::to_string(&packet).expect("serialize kdeconnect");
        assert!(json.contains("\"type\":\"kdeconnect.clipboard\""));
        assert!(json.contains("\"content\":\"Synced text from Android\""));

        let deserialized: KdeConnectPacket =
            serde_json::from_str(&json).expect("deserialize kdeconnect");
        assert_eq!(packet, deserialized);
    }
}
