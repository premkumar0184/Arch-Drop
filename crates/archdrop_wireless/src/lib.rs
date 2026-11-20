//! Arch Drop Wireless: Linux BLE & Continuity Engine, Apple 0x05 / 0x0D / 0x0E wire protocols,
//! GATT credential negotiation, and wireless hardware orchestration.

pub mod ble;
pub mod error;
pub mod nm;

// Direct re-exports
pub use ble::{
    build_gatt_application, decode_clipboard_packet, decode_credentials, decode_telemetry,
    decode_wakeup_command, derive_paired_key, encode_clipboard_packet, encode_credentials,
    encode_telemetry, encode_wakeup_command, BleClipboardPacket, BleConfig, BleController,
    BlePeer, DiscoveredDevice, GattServerState, HotspotCredentials, HotspotStatus0x0E,
    HotspotWakeup0x0D, ProximityBeacon0x05, AIRDROP_PAYLOAD_LEN, APPLE_COMPANY_ID,
    ARCHDROP_SERVICE_UUID, BAND_2_4_GHZ, BAND_5_GHZ, BLE_KEY_DERIVATION_SALT,
    CHAR_CLIPBOARD_SYNC_UUID, CHAR_DEVICE_TELEMETRY_UUID, CHAR_LOHS_CREDENTIALS_UUID,
    CHAR_WAKEUP_COMMAND_UUID, CLIPBOARD_MIME_TEXT_PLAIN, CLIPBOARD_MIME_URI_LIST,
    CONTINUITY_TYPE_AIRDROP, CONTINUITY_TYPE_HOTSPOT_STATUS, CONTINUITY_TYPE_HOTSPOT_WAKEUP,
    DEFAULT_TRANSFER_PORT, FLAG_LOHS_SUPPORTED, FLAG_P2P_READY, FLAG_WAKEUP_5GHZ_PREFERRED,
    FLAG_WAKEUP_START_LOHS, MAX_BLE_CLIPBOARD_PAYLOAD, RAT_TYPE_5G, RAT_TYPE_LTE, RAT_TYPE_WIFI,
    STATE_FLAG_5GHZ_ACTIVE, STATE_FLAG_SOFTAP_UP, STATE_FLAG_WPA3_ACTIVE, STATUS_PAYLOAD_LEN,
    WAKEUP_PAYLOAD_LEN,
};
pub use bluer::Address;
pub use error::WirelessError;
pub use nm::{
    calculate_subnet_gateway, parse_address_data, parse_dhcp_options, resolve_gateway_from_data,
    AccessPointInfo, ActiveConnectionState, ActiveLohsSession, ConnectionProfileBuilder,
    LohsConnectionConfig, NmStationController, ResolvedGatewayInfo, SecurityType,
    DEFAULT_ROUTE_METRIC, NM_ACTIVE_CONNECTION_STATE_ACTIVATED,
    NM_ACTIVE_CONNECTION_STATE_ACTIVATING, NM_ACTIVE_CONNECTION_STATE_DEACTIVATED,
    NM_ACTIVE_CONNECTION_STATE_DEACTIVATING, NM_DEVICE_TYPE_WIFI, NM_DEVICE_TYPE_WIFI_P2P,
};

/// Prelude containing frequently used wireless types.
pub mod prelude {
    pub use crate::ble::{
        BleConfig, BleController, BlePeer, DiscoveredDevice, HotspotCredentials, HotspotStatus0x0E,
        HotspotWakeup0x0D, ProximityBeacon0x05, ARCHDROP_SERVICE_UUID, CHAR_LOHS_CREDENTIALS_UUID,
        CHAR_WAKEUP_COMMAND_UUID,
    };
    pub use crate::error::WirelessError;
    pub use crate::nm::{
        AccessPointInfo, ActiveConnectionState, ActiveLohsSession, ConnectionProfileBuilder,
        LohsConnectionConfig, NmStationController, ResolvedGatewayInfo, SecurityType,
        DEFAULT_ROUTE_METRIC,
    };
}
