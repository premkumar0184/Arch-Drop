//! Apple Continuity protocol frames and reverse-engineered BLE framing.
//!
//! Provides zero-touch proximity discovery (0x05 AirDrop beacon) and remote hotspot
//! wakeup / status exchange (0x0D and 0x0E Instant Hotspot frames).

pub mod hotspot_0x0d;
pub mod proximity_0x05;

pub use hotspot_0x0d::{
    HotspotStatus0x0E, HotspotWakeup0x0D, BAND_2_4_GHZ, BAND_5_GHZ, CONTINUITY_TYPE_HOTSPOT_STATUS,
    CONTINUITY_TYPE_HOTSPOT_WAKEUP, DEFAULT_TRANSFER_PORT, FLAG_WAKEUP_5GHZ_PREFERRED,
    FLAG_WAKEUP_START_LOHS, RAT_TYPE_5G, RAT_TYPE_LTE, RAT_TYPE_WIFI, STATE_FLAG_5GHZ_ACTIVE,
    STATE_FLAG_SOFTAP_UP, STATE_FLAG_WPA3_ACTIVE, STATUS_PAYLOAD_LEN, WAKEUP_PAYLOAD_LEN,
};
pub use proximity_0x05::{
    ProximityBeacon0x05, AIRDROP_PAYLOAD_LEN, APPLE_COMPANY_ID, CONTINUITY_TYPE_AIRDROP,
    FLAG_LOHS_SUPPORTED, FLAG_P2P_READY,
};
