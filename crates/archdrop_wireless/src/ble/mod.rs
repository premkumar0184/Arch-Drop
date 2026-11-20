//! Bluetooth Low Energy (BLE) sub-system for Arch Drop.
//!
//! Encompasses Apple Continuity wire frames (0x05, 0x0D, 0x0E), BlueZ controller integration,
//! and GATT service definitions for peer discovery and zero-touch Local-Only Hotspot activation.

pub mod continuity;
pub mod controller;
pub mod gatt;
pub mod types;

pub use continuity::{
    HotspotStatus0x0E, HotspotWakeup0x0D, ProximityBeacon0x05, AIRDROP_PAYLOAD_LEN,
    APPLE_COMPANY_ID, BAND_2_4_GHZ, BAND_5_GHZ, CONTINUITY_TYPE_AIRDROP,
    CONTINUITY_TYPE_HOTSPOT_STATUS, CONTINUITY_TYPE_HOTSPOT_WAKEUP, DEFAULT_TRANSFER_PORT,
    FLAG_LOHS_SUPPORTED, FLAG_P2P_READY, FLAG_WAKEUP_5GHZ_PREFERRED, FLAG_WAKEUP_START_LOHS,
    RAT_TYPE_5G, RAT_TYPE_LTE, RAT_TYPE_WIFI, STATE_FLAG_5GHZ_ACTIVE, STATE_FLAG_SOFTAP_UP,
    STATE_FLAG_WPA3_ACTIVE, STATUS_PAYLOAD_LEN, WAKEUP_PAYLOAD_LEN,
};
pub use controller::BleController;
pub use gatt::{
    build_gatt_application, decode_clipboard_packet, decode_credentials, decode_telemetry,
    decode_wakeup_command, derive_paired_key, encode_clipboard_packet, encode_credentials,
    encode_telemetry, encode_wakeup_command, BleClipboardPacket, GattServerState,
    ARCHDROP_SERVICE_UUID, BLE_KEY_DERIVATION_SALT, CHAR_CLIPBOARD_SYNC_UUID,
    CHAR_DEVICE_TELEMETRY_UUID, CHAR_LOHS_CREDENTIALS_UUID, CHAR_WAKEUP_COMMAND_UUID,
    CLIPBOARD_MIME_TEXT_PLAIN, CLIPBOARD_MIME_URI_LIST, MAX_BLE_CLIPBOARD_PAYLOAD,
};
pub use types::{BleConfig, BlePeer, DiscoveredDevice, HotspotCredentials};
