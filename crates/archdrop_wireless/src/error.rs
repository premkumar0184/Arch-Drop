//! Error types for Arch Drop wireless networking, BLE discovery, and GATT operations.

use thiserror::Error;

/// Comprehensive error hierarchy for BLE continuity, GATT transactions, and wireless operations.
#[derive(Error, Debug)]
pub enum WirelessError {
    #[error("System Bluetooth service (bluetoothd) is unavailable: {0}")]
    BluetoothServiceUnavailable(String),

    #[error("Bluetooth adapter not found: {0}")]
    AdapterNotFound(String),

    #[error("Failed to start or manage BLE advertisement: {0}")]
    AdvertisingFailed(String),

    #[error("Failed to perform BLE discovery or device scan: {0}")]
    DiscoveryFailed(String),

    #[error("BLE GATT operation failed: {0}")]
    GattError(String),

    #[error("Invalid Apple Continuity frame: {reason}")]
    InvalidFrame { reason: String },

    #[error("Invalid Bluetooth Company Identifier: expected 0x004C (Apple Inc.), got 0x{0:04X}")]
    InvalidCompanyId(u16),

    #[error("Frame truncated: expected at least {expected} bytes, received {actual} bytes")]
    FrameTruncated { expected: usize, actual: usize },

    #[error("Continuity authentication token verification failed")]
    TokenVerificationFailed,

    #[error("Remote Bluetooth peer device not found: {0}")]
    DeviceNotFound(String),

    #[error("Failed to connect to Bluetooth peer {0}: {1}")]
    ConnectionFailed(String, String),

    #[error("Serialization / Deserialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Archdrop core error: {0}")]
    Core(#[from] archdrop_core::error::ArchdropError),

    #[error("NetworkManager service is unavailable on D-Bus: {0}")]
    NetworkManagerUnavailable(String),

    #[error("Wireless network device not found or unmanaged: {0}")]
    WirelessDeviceNotFound(String),

    #[error("Target Access Point SSID '{0}' not found within scan timeout")]
    AccessPointNotFound(String),

    #[error("Connection activation failed: {0}")]
    ConnectionActivationFailed(String),

    #[error("Failed to dynamically resolve Android gateway IP: {0}")]
    GatewayResolutionFailed(String),

    #[error("DHCP configuration timeout or lease acquisition failed: {0}")]
    DhcpTimeout(String),

    #[error("Host safety invariant violated: {0}")]
    SafetyInvariantViolated(String),

    #[error("D-Bus transport error: {0}")]
    Dbus(#[from] zbus::Error),

    #[error("D-Bus zvariant serialization error: {0}")]
    Zvariant(#[from] zbus::zvariant::Error),

    #[error("Other wireless error: {0}")]
    Other(String),
}
