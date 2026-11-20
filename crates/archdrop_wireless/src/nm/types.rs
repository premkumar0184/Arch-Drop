//! Data types and definitions for NetworkManager station controller.

use serde::{Deserialize, Serialize};
use zbus::zvariant::OwnedObjectPath;

/// NetworkManager 802.11 device types.
pub const NM_DEVICE_TYPE_WIFI: u32 = 2;
pub const NM_DEVICE_TYPE_WIFI_P2P: u32 = 30;

/// NetworkManager ActiveConnection states.
pub const NM_ACTIVE_CONNECTION_STATE_UNKNOWN: u32 = 0;
pub const NM_ACTIVE_CONNECTION_STATE_ACTIVATING: u32 = 1;
pub const NM_ACTIVE_CONNECTION_STATE_ACTIVATED: u32 = 2;
pub const NM_ACTIVE_CONNECTION_STATE_DEACTIVATING: u32 = 3;
pub const NM_ACTIVE_CONNECTION_STATE_DEACTIVATED: u32 = 4;

/// Default route metric assigned to LOHS station connections to prevent overtaking primary routes.
pub const DEFAULT_ROUTE_METRIC: u32 = 2000;

/// 802.11 Security types supported for station association.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityType {
    /// Open network (no encryption)
    Open,
    /// WPA2 Personal (Pre-Shared Key)
    Wpa2Psk,
    /// WPA3 Personal (Simultaneous Authentication of Equals)
    Wpa3Sae,
    /// WPA2/WPA3 Mixed / Transition mode
    Wpa2Wpa3Transition,
}

impl SecurityType {
    /// Parse SecurityType from NetworkManager AccessPoint flags.
    ///
    /// Bit flags based on NetworkManager NM80211ApSecurityFlags:
    /// - NM_802_11_AP_SEC_KEY_MGMT_PSK = 0x00000100
    /// - NM_802_11_AP_SEC_KEY_MGMT_SAE = 0x00000400
    pub fn from_ap_flags(_flags: u32, wpa_flags: u32, rsn_flags: u32) -> Self {
        const SEC_KEY_MGMT_PSK: u32 = 0x0000_0100;
        const SEC_KEY_MGMT_SAE: u32 = 0x0000_0400;

        let has_sae = (rsn_flags & SEC_KEY_MGMT_SAE) != 0;
        let has_psk = (rsn_flags & SEC_KEY_MGMT_PSK) != 0 || (wpa_flags & SEC_KEY_MGMT_PSK) != 0;

        match (has_sae, has_psk) {
            (true, true) => SecurityType::Wpa2Wpa3Transition,
            (true, false) => SecurityType::Wpa3Sae,
            (false, true) => SecurityType::Wpa2Psk,
            (false, false) => SecurityType::Open,
        }
    }

    /// Return the NetworkManager `key-mgmt` string for connection settings.
    pub fn key_mgmt_str(&self) -> &'static str {
        match self {
            SecurityType::Open => "none",
            SecurityType::Wpa2Psk | SecurityType::Wpa2Wpa3Transition => "wpa-psk",
            SecurityType::Wpa3Sae => "sae",
        }
    }

    /// Returns true if this security type uses or supports WPA3 SAE.
    pub fn is_wpa3(&self) -> bool {
        matches!(self, SecurityType::Wpa3Sae | SecurityType::Wpa2Wpa3Transition)
    }
}

/// Information about a detected 802.11 Access Point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessPointInfo {
    /// Service Set Identifier (human-readable network name)
    pub ssid: String,
    /// Basic Service Set Identifier (MAC address of AP)
    pub bssid: String,
    /// Carrier frequency in MHz (e.g. 2412 for ch 1, 5180 for ch 36)
    pub frequency_mhz: u32,
    /// Received Signal Strength Indicator normalized to 0..100
    pub strength: u8,
    /// Detected security type (WPA2, WPA3, Open)
    pub security: SecurityType,
    /// D-Bus object path of the AccessPoint in NetworkManager
    pub dbus_path: OwnedObjectPath,
}

/// Configuration parameters for establishing a station connection to an Android LOHS SoftAP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LohsConnectionConfig {
    /// Target SSID
    pub ssid: String,
    /// Pre-Shared Key or SAE passphrase
    pub passphrase: String,
    /// Security type negotiated over BLE
    pub security: SecurityType,
    /// Optional BSSID constraint
    pub bssid: Option<String>,
    /// Subnet route metric (strictly enforced to 2000 to protect default routes)
    pub route_metric: u32,
    /// Strictly forbid NM from installing a default route (0.0.0.0/0) via this link
    pub never_default: bool,
    /// Strictly forbid NM from altering host DNS / /etc/resolv.conf
    pub ignore_auto_dns: bool,
    /// Strictly disable IPv6 on the LOHS link
    pub disable_ipv6: bool,
    /// Prevent automatic reconnection
    pub autoconnect: bool,
}

impl LohsConnectionConfig {
    /// Create a new LOHS connection config with all host safety invariants enforced by default.
    pub fn new(ssid: impl Into<String>, passphrase: impl Into<String>, security: SecurityType) -> Self {
        Self {
            ssid: ssid.into(),
            passphrase: passphrase.into(),
            security,
            bssid: None,
            route_metric: DEFAULT_ROUTE_METRIC,
            never_default: true,
            ignore_auto_dns: true,
            disable_ipv6: true,
            autoconnect: false,
        }
    }

    /// Optional builder setter for BSSID.
    pub fn with_bssid(mut self, bssid: impl Into<String>) -> Self {
        self.bssid = Some(bssid.into());
        self
    }
}

/// NetworkManager ActiveConnection state representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActiveConnectionState {
    Unknown(u32),
    Activating,
    Activated,
    Deactivating,
    Deactivated,
}

impl From<u32> for ActiveConnectionState {
    fn from(val: u32) -> Self {
        match val {
            NM_ACTIVE_CONNECTION_STATE_ACTIVATING => ActiveConnectionState::Activating,
            NM_ACTIVE_CONNECTION_STATE_ACTIVATED => ActiveConnectionState::Activated,
            NM_ACTIVE_CONNECTION_STATE_DEACTIVATING => ActiveConnectionState::Deactivating,
            NM_ACTIVE_CONNECTION_STATE_DEACTIVATED => ActiveConnectionState::Deactivated,
            other => ActiveConnectionState::Unknown(other),
        }
    }
}

impl ActiveConnectionState {
    /// Returns true if the connection state is Activated (2).
    pub fn is_activated(&self) -> bool {
        matches!(self, ActiveConnectionState::Activated)
    }

    /// Returns true if the connection state has entered a terminal or deactivating state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, ActiveConnectionState::Deactivating | ActiveConnectionState::Deactivated)
    }
}
