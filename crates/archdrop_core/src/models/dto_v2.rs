//! LocalSend Protocol v2.2 Data Transfer Objects (DTOs).
//!
//! Adapted from upstream `localsend/packages/core` under Apache-2.0.
//! Provides exact wire-level compatibility with LocalSend v2 clients.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The protocol version (major.minor) implemented for the LocalSend v2 protocol.
pub const PROTOCOL_VERSION_V2: &str = "2.2";

/// Default listening port for LocalSend v2 HTTP server.
pub const DEFAULT_PORT: u16 = 53317;

/// Device type category in LocalSend.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeviceType {
    Mobile,
    Desktop,
    Web,
    Headless,
    Server,
}

impl DeviceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceType::Mobile => "mobile",
            DeviceType::Desktop => "desktop",
            DeviceType::Web => "web",
            DeviceType::Headless => "headless",
            DeviceType::Server => "server",
        }
    }
}

/// Protocol type for HTTP or HTTPS connections.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProtocolType {
    Http,
    Https,
}

impl ProtocolType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProtocolType::Http => "http",
            ProtocolType::Https => "https",
        }
    }
}

/// Serde serialization/deserialization helpers for `ProtocolType` in the v2 protocol.
///
/// The v2 wire protocol uses lowercase strings ("http" / "https").
pub mod protocol_type_v2 {
    use super::ProtocolType;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &ProtocolType,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ProtocolType, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.to_lowercase().as_str() {
            "http" => Ok(ProtocolType::Http),
            "https" => Ok(ProtocolType::Https),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["http", "https"],
            )),
        }
    }
}

/// Serde serialization/deserialization helpers for `DeviceType` in the v2 protocol.
///
/// The v2 wire protocol uses lowercase strings (e.g. "desktop", "mobile").
/// Unknown values fall back to `DeviceType::Desktop` as required by LocalSend protocol section 7.1.
pub mod device_type_v2 {
    use super::DeviceType;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<DeviceType>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(dt) => serializer.serialize_str(dt.as_str()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DeviceType>, D::Error> {
        let val = Option::<String>::deserialize(deserializer)?;
        Ok(val.map(|v| match v.to_lowercase().as_str() {
            "mobile" => DeviceType::Mobile,
            "desktop" => DeviceType::Desktop,
            "web" => DeviceType::Web,
            "headless" => DeviceType::Headless,
            "server" => DeviceType::Server,
            _ => DeviceType::Desktop, // Fallback to Desktop per LocalSend protocol section 7.1
        }))
    }
}

/// Registration announcement DTO for LocalSend v2.
///
/// Sent via `POST /api/localsend/v2/register` or embedded in `PrepareUploadRequestDtoV2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterDtoV2 {
    /// Human-readable display name of the device.
    pub alias: String,

    /// Protocol version string (e.g. "2.0", "2.2").
    pub version: String,

    /// Device model string (e.g. "Arch Linux (Hyprland)", "Pixel 8"). Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,

    /// Device category. Serialized lowercase; falls back to Desktop on unknown strings.
    #[serde(
        default,
        with = "device_type_v2",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_type: Option<DeviceType>,

    /// Device certificate fingerprint (uppercase hex SHA-256) or random string.
    pub fingerprint: String,

    /// TCP port number the device is listening on (normally 53317).
    pub port: u16,

    /// Protocol type (http or https). Serialized lowercase.
    #[serde(with = "protocol_type_v2")]
    pub protocol: ProtocolType,

    /// Whether the download API is active.
    #[serde(default)]
    pub download: bool,
}

/// Registration response DTO for LocalSend v2.
///
/// Returned from `POST /api/localsend/v2/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponseDtoV2 {
    pub alias: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(
        default,
        with = "device_type_v2",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_type: Option<DeviceType>,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub download: bool,
}

/// Device info response DTO for LocalSend v2.
///
/// Returned from `GET /api/localsend/v2/info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponseDtoV2 {
    pub alias: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(
        default,
        with = "device_type_v2",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_type: Option<DeviceType>,
    pub fingerprint: String,
    #[serde(default)]
    pub download: bool,
}

/// File metadata containing RFC 3339 timestamp strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessed: Option<String>,
}

/// File description DTO for transfer negotiations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    /// Unique identifier for this file within the session (e.g. UUIDv4 or incrementing string).
    pub id: String,

    /// Base filename including extension.
    pub file_name: String,

    /// Exact file size in bytes.
    pub size: u64,

    /// MIME content type (e.g. "image/png", "application/pdf").
    pub file_type: String,

    /// Optional SHA-256 hex digest of the file for integrity verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// Optional base64 preview string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,

    /// Optional filesystem timestamp metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<FileMetadata>,
}

/// Prepare upload request DTO for LocalSend v2.
///
/// Sent via `POST /api/localsend/v2/prepare-upload` to negotiate a transfer session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadRequestDtoV2 {
    /// Sender device information.
    pub info: RegisterDtoV2,

    /// Map of file ID to file descriptor.
    pub files: HashMap<String, FileDto>,
}

/// Prepare upload response DTO for LocalSend v2.
///
/// Returned from `POST /api/localsend/v2/prepare-upload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadResponseDtoV2 {
    /// Unique session identifier generated by the receiver.
    pub session_id: String,

    /// Map of accepted file ID to upload authentication token (`fileId` -> `fileToken`).
    /// Files declined or rejected by the receiver are omitted.
    pub files: HashMap<String, String>,
}

/// Query parameters for streaming file upload (`POST /api/localsend/v2/upload`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadQueryParams {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "fileId")]
    pub file_id: String,
    pub token: String,
}

/// Query parameters for session cancellation (`POST /api/localsend/v2/cancel`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelQueryParams {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_dto_roundtrip() {
        let original = RegisterDtoV2 {
            alias: "Arch Drop Laptop".to_string(),
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: Some("Arch Linux (Hyprland)".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: "A1B2C3D4E5F6".to_string(),
            port: DEFAULT_PORT,
            protocol: ProtocolType::Https,
            download: true,
        };

        let json = serde_json::to_string(&original).expect("serialize failed");
        assert!(json.contains("\"alias\":\"Arch Drop Laptop\""));
        assert!(json.contains("\"deviceModel\":\"Arch Linux (Hyprland)\""));
        assert!(json.contains("\"deviceType\":\"desktop\""));
        assert!(json.contains("\"protocol\":\"https\""));
        assert!(json.contains("\"port\":53317"));
        assert!(json.contains("\"download\":true"));

        let deserialized: RegisterDtoV2 = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_device_type_fallback_to_desktop() {
        let json = r#"{
            "alias": "Living Room TV",
            "version": "2.2",
            "deviceType": "smart_tv_box",
            "fingerprint": "123456",
            "port": 53317,
            "protocol": "http"
        }"#;

        let dto: RegisterDtoV2 = serde_json::from_str(json).expect("deserialize unknown type");
        assert_eq!(dto.device_type, Some(DeviceType::Desktop));
        assert_eq!(dto.protocol, ProtocolType::Http);
        assert!(!dto.download);
    }

    #[test]
    fn test_register_response_defaults() {
        let json = r#"{
            "alias": "Peer Device",
            "version": "2.0",
            "fingerprint": "abc123"
        }"#;

        let dto: RegisterResponseDtoV2 = serde_json::from_str(json).expect("deserialize response");
        assert_eq!(dto.alias, "Peer Device");
        assert_eq!(dto.device_model, None);
        assert_eq!(dto.device_type, None);
        assert!(!dto.download);
    }

    #[test]
    fn test_prepare_upload_dto_roundtrip() {
        let mut files = HashMap::new();
        files.insert(
            "file-001".to_string(),
            FileDto {
                id: "file-001".to_string(),
                file_name: "test.png".to_string(),
                size: 2048,
                file_type: "image/png".to_string(),
                sha256: Some(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
                ),
                preview: None,
                metadata: Some(FileMetadata {
                    modified: Some("2026-11-08T03:00:00Z".to_string()),
                    accessed: None,
                }),
            },
        );

        let req = PrepareUploadRequestDtoV2 {
            info: RegisterDtoV2 {
                alias: "Android Phone".to_string(),
                version: "2.2".to_string(),
                device_model: Some("Pixel 8".to_string()),
                device_type: Some(DeviceType::Mobile),
                fingerprint: "FEEDBEEF0102".to_string(),
                port: 53317,
                protocol: ProtocolType::Https,
                download: false,
            },
            files,
        };

        let json = serde_json::to_string(&req).expect("serialize req");
        let parsed: PrepareUploadRequestDtoV2 =
            serde_json::from_str(&json).expect("deserialize req");
        assert_eq!(req, parsed);

        let mut accepted_files = HashMap::new();
        accepted_files.insert("file-001".to_string(), "token-abc-123".to_string());
        let res = PrepareUploadResponseDtoV2 {
            session_id: "session-xyz-789".to_string(),
            files: accepted_files,
        };

        let res_json = serde_json::to_string(&res).expect("serialize res");
        assert!(res_json.contains("\"sessionId\":\"session-xyz-789\""));
        assert!(res_json.contains("\"files\":{\"file-001\":\"token-abc-123\"}"));

        let res_parsed: PrepareUploadResponseDtoV2 =
            serde_json::from_str(&res_json).expect("deserialize res");
        assert_eq!(res, res_parsed);
    }
}
