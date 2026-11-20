//! Core data models and types for BLE discovery, continuity exchange, and Wi-Fi credentials.

use bluer::Address;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

use crate::ble::continuity::hotspot_0x0d::{HotspotStatus0x0E, HotspotWakeup0x0D};
use crate::ble::continuity::proximity_0x05::{ProximityBeacon0x05, APPLE_COMPANY_ID};

/// Local-Only Hotspot (LOHS) Wi-Fi SoftAP credentials returned over GATT.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotspotCredentials {
    /// SoftAP SSID broadcast by companion device.
    pub ssid: String,
    /// WPA2/WPA3 pre-shared key passphrase.
    pub passphrase: String,
    /// Gateway IP address of the SoftAP interface (typically 192.168.49.1 or 192.168.43.1).
    pub gateway_ip: Ipv4Addr,
    /// LocalSend v2 HTTP transfer service port (typically 53317).
    pub port: u16,
}

impl HotspotCredentials {
    /// Creates a new credential set.
    pub fn new(ssid: impl Into<String>, passphrase: impl Into<String>, gateway_ip: Ipv4Addr, port: u16) -> Self {
        Self {
            ssid: ssid.into(),
            passphrase: passphrase.into(),
            gateway_ip,
            port,
        }
    }

    /// Serializes to JSON byte vector for GATT transmission.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserializes from JSON byte slice received over GATT.
    pub fn from_bytes(slice: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(slice)
    }
}

/// BLE controller configuration options.
#[derive(Clone, Debug)]
pub struct BleConfig {
    /// Optional specific BlueZ adapter interface name (e.g., "hci0"). If None, uses default adapter.
    pub adapter_name: Option<String>,
    /// Bluetooth 16-bit Company Identifier (defaults to 0x004C for Apple Continuity framing).
    pub company_id: u16,
}

impl Default for BleConfig {
    fn default() -> Self {
        Self {
            adapter_name: None,
            company_id: APPLE_COMPANY_ID,
        }
    }
}

/// A nearby BLE peer device discovered during background or active scanning.
#[derive(Clone, Debug)]
pub struct DiscoveredDevice {
    /// Bluetooth hardware MAC address of the peer device.
    pub address: Address,
    /// Received Signal Strength Indicator (RSSI) in dBm if available.
    pub rssi: Option<i16>,
    /// Advertised Local Name if available.
    pub name: Option<String>,
    /// Parsed Apple Continuity 0x05 Proximity Presence Beacon if present.
    pub proximity_beacon: Option<ProximityBeacon0x05>,
    /// Parsed Apple Continuity 0x0E Hotspot Status Response if present.
    pub hotspot_status: Option<HotspotStatus0x0E>,
    /// Parsed Apple Continuity 0x0D Hotspot Wakeup Request if present.
    pub hotspot_wakeup: Option<HotspotWakeup0x0D>,
    /// Whether the device advertises the primary Arch Drop Service UUID.
    pub is_archdrop_service: bool,
}

/// Summary representation of a known or connected BLE peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlePeer {
    /// Bluetooth hardware MAC address.
    pub address: Address,
    /// Friendly device name if known.
    pub name: Option<String>,
    /// Whether the device is paired in BlueZ.
    pub paired: bool,
}
