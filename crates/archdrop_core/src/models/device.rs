//! Device Identity & Peer Representation Models.
//!
//! Provides data structures representing local host identity, paired peer status,
//! and conversion utilities between internal domain models and wire-level DTOs.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

pub use crate::models::dto_v2::{
    DeviceType, InfoResponseDtoV2, ProtocolType, RegisterDtoV2, PROTOCOL_VERSION_V2,
};

/// Pairing and trust state for a known peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingState {
    /// Device is verified and paired; auto-accept transfers without prompt.
    Paired,
    /// Pairing request is pending user confirmation.
    Pending,
    /// Discovered or observed device not yet paired.
    Untrusted,
    /// Device explicitly blocked from transfers.
    Blocked,
}

/// Transfer direction indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Outgoing,
    Incoming,
}

/// Lifecycle status for an ongoing or completed transfer session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

/// Local device identity and networking configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Unique identity string (e.g. certificate SHA-256 fingerprint or UUID).
    pub id: String,

    /// Human-readable alias (e.g. "Arch Drop Laptop").
    pub alias: String,

    /// Device category (Desktop, Mobile, etc.).
    pub device_type: DeviceType,

    /// Optional hardware / OS model string.
    pub device_model: Option<String>,

    /// Uppercase hex DER SHA-256 fingerprint of the local X.509 certificate.
    pub fingerprint: String,

    /// Listening port (default: 53317).
    pub port: u16,

    /// Transport protocol (normally Https).
    pub protocol: ProtocolType,

    /// Whether download API is supported.
    pub download: bool,

    /// Local network IP address, if resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<IpAddr>,
}

impl Device {
    /// Converts local device state into a LocalSend v2 `RegisterDtoV2` announcement.
    pub fn to_register_dto(&self) -> RegisterDtoV2 {
        RegisterDtoV2 {
            alias: self.alias.clone(),
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: self.device_model.clone(),
            device_type: Some(self.device_type),
            fingerprint: self.fingerprint.clone(),
            port: self.port,
            protocol: self.protocol,
            download: self.download,
        }
    }

    /// Converts local device state into a LocalSend v2 `InfoResponseDtoV2` response.
    pub fn to_info_dto(&self) -> InfoResponseDtoV2 {
        InfoResponseDtoV2 {
            alias: self.alias.clone(),
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: self.device_model.clone(),
            device_type: Some(self.device_type),
            fingerprint: self.fingerprint.clone(),
            download: self.download,
        }
    }
}

/// Representation of a discovered or paired remote peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerDevice {
    /// Remote device display name.
    pub alias: String,

    /// Remote device category, if advertised.
    pub device_type: Option<DeviceType>,

    /// Remote device hardware or OS model string.
    pub device_model: Option<String>,

    /// Remote IP address (LAN or WireGuard).
    pub ip: IpAddr,

    /// Remote listening port.
    pub port: u16,

    /// Remote protocol type (Http or Https).
    pub protocol: ProtocolType,

    /// Remote certificate fingerprint (uppercase hex SHA-256).
    pub fingerprint: String,

    /// Pairing flag (true if fingerprint is in paired trust store).
    pub is_paired: bool,

    /// Trust lifecycle state.
    pub pairing_state: PairingState,

    /// Millisecond Unix timestamp of last successful communication or discovery.
    pub last_seen_epoch_ms: u64,

    /// Protocol version string (e.g. "2.2").
    pub version: String,

    /// Whether remote device supports download API.
    pub download: bool,
}

impl PeerDevice {
    /// Constructs a `PeerDevice` from a received `RegisterDtoV2` and peer IP.
    pub fn from_register_dto(dto: RegisterDtoV2, ip: IpAddr, is_paired: bool) -> Self {
        let pairing_state = if is_paired {
            PairingState::Paired
        } else {
            PairingState::Untrusted
        };
        Self {
            alias: dto.alias,
            device_type: dto.device_type,
            device_model: dto.device_model,
            ip,
            port: dto.port,
            protocol: dto.protocol,
            fingerprint: dto.fingerprint,
            is_paired,
            pairing_state,
            last_seen_epoch_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            version: dto.version,
            download: dto.download,
        }
    }

    /// Constructs a `PeerDevice` from an `InfoResponseDtoV2` (e.g. obtained via GET /info).
    pub fn from_info_dto(
        dto: InfoResponseDtoV2,
        ip: IpAddr,
        port: u16,
        protocol: ProtocolType,
        is_paired: bool,
    ) -> Self {
        let pairing_state = if is_paired {
            PairingState::Paired
        } else {
            PairingState::Untrusted
        };
        Self {
            alias: dto.alias,
            device_type: dto.device_type,
            device_model: dto.device_model,
            ip,
            port,
            protocol,
            fingerprint: dto.fingerprint,
            is_paired,
            pairing_state,
            last_seen_epoch_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            version: dto.version,
            download: dto.download,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_to_dto_conversions() {
        let device = Device {
            id: "local-cert-hash-123".to_string(),
            alias: "Arch Drop Laptop".to_string(),
            device_type: DeviceType::Desktop,
            device_model: Some("Arch Linux (Hyprland)".to_string()),
            fingerprint: "AA:BB:CC:DD".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: false,
            ip: Some("10.2.0.2".parse().unwrap()),
        };

        let reg_dto = device.to_register_dto();
        assert_eq!(reg_dto.alias, "Arch Drop Laptop");
        assert_eq!(reg_dto.device_type, Some(DeviceType::Desktop));
        assert_eq!(reg_dto.protocol, ProtocolType::Https);
        assert_eq!(reg_dto.port, 53317);

        let info_dto = device.to_info_dto();
        assert_eq!(info_dto.alias, "Arch Drop Laptop");
        assert_eq!(info_dto.fingerprint, "AA:BB:CC:DD");
    }

    #[test]
    fn test_peer_device_from_register_dto() {
        let reg_dto = RegisterDtoV2 {
            alias: "Galaxy S24".to_string(),
            version: "2.2".to_string(),
            device_model: Some("Samsung".to_string()),
            device_type: Some(DeviceType::Mobile),
            fingerprint: "112233445566".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: true,
        };

        let ip = "192.168.1.50".parse().unwrap();
        let peer = PeerDevice::from_register_dto(reg_dto, ip, true);

        assert_eq!(peer.alias, "Galaxy S24");
        assert_eq!(peer.device_type, Some(DeviceType::Mobile));
        assert_eq!(peer.ip, ip);
        assert!(peer.is_paired);
        assert_eq!(peer.pairing_state, PairingState::Paired);
        assert!(peer.download);
        assert!(peer.last_seen_epoch_ms > 0);
    }
}
