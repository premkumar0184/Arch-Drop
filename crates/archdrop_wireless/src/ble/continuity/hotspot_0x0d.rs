//! Apple Continuity 0x0D Remote Hotspot Wakeup Trigger and 0x0E Hotspot Status Response frames.
//!
//! Encapsulates 12-byte and 10-byte Manufacturer Data frames under Company ID 0x004C (Apple Inc.).
//! 0x0D wakes up the Android companion device from standby and requests Local-Only Hotspot (LOHS) activation.
//! 0x0E broadcasts cellular telemetry, battery state, and active SoftAP channel frequency.

use crate::ble::continuity::proximity_0x05::APPLE_COMPANY_ID;
use crate::error::WirelessError;

/// Apple Continuity sub-type for Remote Hotspot Wakeup Trigger.
pub const CONTINUITY_TYPE_HOTSPOT_WAKEUP: u8 = 0x0D;

/// Sub-payload length for 0x0D frame following Type and Length (8 bytes).
pub const WAKEUP_PAYLOAD_LEN: u8 = 0x08;

/// Default LocalSend HTTP receiver port.
pub const DEFAULT_TRANSFER_PORT: u16 = 53317;

/// Wakeup Control Bit 0: Request Android to start Local-Only Hotspot.
pub const FLAG_WAKEUP_START_LOHS: u8 = 0x01;
/// Wakeup Control Bit 1: Suggest 5 GHz band for high-speed transfer.
pub const FLAG_WAKEUP_5GHZ_PREFERRED: u8 = 0x02;

/// Target RAT / Band: 2.4 GHz channel.
pub const BAND_2_4_GHZ: u8 = 0x01;
/// Target RAT / Band: 5 GHz channel.
pub const BAND_5_GHZ: u8 = 0x02;

/// Apple Continuity 0x0D Remote Hotspot Wakeup Trigger frame.
///
/// Wire format:
/// - Offset 0x00 (1B): Type (0x0D)
/// - Offset 0x01 (1B): Length (0x08)
/// - Offset 0x02 (1B): Control Flags (Bit 0: Start LOHS, Bit 1: 5GHz Preferred)
/// - Offset 0x03 (1B): Target Band (0x01 = 2.4 GHz, 0x02 = 5 GHz)
/// - Offset 0x04 (4B): Rolling Epoch HMAC-SHA256 Token
/// - Offset 0x08 (2B): LocalSend Transfer Port (big-endian u16, default 53317)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotspotWakeup0x0D {
    /// Control flags.
    pub control_flags: u8,
    /// Preferred radio frequency band.
    pub target_band: u8,
    /// 4-byte rolling auth token preventing replays.
    pub auth_token: [u8; 4],
    /// LocalSend v2 HTTP receiver port (53317).
    pub transfer_port: u16,
}

impl HotspotWakeup0x0D {
    /// Computes the 4-byte authentication token for a given minute epoch.
    pub fn compute_auth_token(paired_key: &[u8], epoch_minute: u64) -> [u8; 4] {
        let s_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, paired_key);
        let msg = epoch_minute.to_be_bytes();
        let tag = ring::hmac::sign(&s_key, &msg);
        let mut token = [0u8; 4];
        token.copy_from_slice(&tag.as_ref()[0..4]);
        token
    }

    /// Returns the current Unix epoch time in minutes.
    pub fn current_epoch_minute() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() / 60
    }

    /// Creates a new wakeup trigger frame signed with the paired secret for the current time window.
    pub fn new(paired_key: &[u8], target_band: u8, port: u16) -> Self {
        let epoch_minute = Self::current_epoch_minute();
        let auth_token = Self::compute_auth_token(paired_key, epoch_minute);
        let mut control_flags = FLAG_WAKEUP_START_LOHS;
        if target_band == BAND_5_GHZ {
            control_flags |= FLAG_WAKEUP_5GHZ_PREFERRED;
        }
        Self {
            control_flags,
            target_band,
            auth_token,
            transfer_port: port,
        }
    }

    /// Directly constructs a wakeup trigger frame from parts (useful for testing forged tokens).
    pub fn from_parts(control_flags: u8, target_band: u8, auth_token: [u8; 4], transfer_port: u16) -> Self {
        Self {
            control_flags,
            target_band,
            auth_token,
            transfer_port,
        }
    }

    /// Serializes the frame into the 10-byte Continuity Manufacturer Data sub-payload.
    pub fn to_bytes(&self) -> [u8; 10] {
        let mut buf = [0u8; 10];
        buf[0] = CONTINUITY_TYPE_HOTSPOT_WAKEUP;
        buf[1] = WAKEUP_PAYLOAD_LEN;
        buf[2] = self.control_flags;
        buf[3] = self.target_band;
        buf[4..8].copy_from_slice(&self.auth_token);
        buf[8..10].copy_from_slice(&self.transfer_port.to_be_bytes());
        buf
    }

    /// Serializes into the complete 12-byte Manufacturer Data frame including Company ID (0x004C).
    pub fn to_raw_manufacturer_data(&self) -> [u8; 12] {
        let mut raw = [0u8; 12];
        raw[0..2].copy_from_slice(&APPLE_COMPANY_ID.to_le_bytes());
        raw[2..12].copy_from_slice(&self.to_bytes());
        raw
    }

    /// Returns the Company ID and sub-payload tuple suitable for `bluer` advertisement.
    pub fn to_bluer_payload(&self) -> (u16, Vec<u8>) {
        (APPLE_COMPANY_ID, self.to_bytes().to_vec())
    }

    /// Parses a 0x0D frame from either a 12-byte raw manufacturer slice or a 10-byte sub-payload.
    pub fn parse(data: &[u8]) -> Result<Self, WirelessError> {
        let payload = if data.len() == 12 {
            let company_id = u16::from_le_bytes([data[0], data[1]]);
            if company_id != APPLE_COMPANY_ID {
                return Err(WirelessError::InvalidCompanyId(company_id));
            }
            &data[2..]
        } else if data.len() == 10 {
            data
        } else if data.len() < 10 {
            return Err(WirelessError::FrameTruncated {
                expected: 10,
                actual: data.len(),
            });
        } else {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Unexpected 0x0D frame length: {}", data.len()),
            });
        };

        if payload[0] != CONTINUITY_TYPE_HOTSPOT_WAKEUP {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Invalid continuity type: expected 0x0D, got 0x{:02X}", payload[0]),
            });
        }

        if payload[1] != WAKEUP_PAYLOAD_LEN {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Invalid payload length: expected 0x08, got 0x{:02X}", payload[1]),
            });
        }

        let control_flags = payload[2];
        let target_band = payload[3];
        let mut auth_token = [0u8; 4];
        auth_token.copy_from_slice(&payload[4..8]);
        let transfer_port = u16::from_be_bytes([payload[8], payload[9]]);

        Ok(Self {
            control_flags,
            target_band,
            auth_token,
            transfer_port,
        })
    }

    /// Verifies the 4-byte auth token with a +/- 2 minute clock skew tolerance in constant time.
    pub fn verify(&self, paired_key: &[u8]) -> bool {
        let current_minute = Self::current_epoch_minute();
        for offset in [0i64, -1, 1, -2, 2] {
            let test_minute = (current_minute as i64 + offset).max(0) as u64;
            if self.verify_at_epoch(paired_key, test_minute) {
                return true;
            }
        }
        false
    }

    /// Verifies the token against an exact epoch minute in constant time.
    pub fn verify_at_epoch(&self, paired_key: &[u8], epoch_minute: u64) -> bool {
        let expected = Self::compute_auth_token(paired_key, epoch_minute);
        constant_time_eq_4(&self.auth_token, &expected)
    }
}

/// Constant-time comparison of two 4-byte slices to prevent timing side channels.
#[inline]
pub fn constant_time_eq_4(a: &[u8; 4], b: &[u8; 4]) -> bool {
    let mut diff = 0u8;
    for i in 0..4 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Apple Continuity sub-type for Hotspot Status Response.
pub const CONTINUITY_TYPE_HOTSPOT_STATUS: u8 = 0x0E;

/// Sub-payload length for 0x0E frame following Type and Length (6 bytes).
pub const STATUS_PAYLOAD_LEN: u8 = 0x06;

/// Radio Access Technology (RAT) type constants.
pub const RAT_TYPE_WIFI: u8 = 0x01;
pub const RAT_TYPE_LTE: u8 = 0x04;
pub const RAT_TYPE_5G: u8 = 0x05;

/// Hotspot state flags.
pub const STATE_FLAG_SOFTAP_UP: u8 = 0x01;
pub const STATE_FLAG_WPA3_ACTIVE: u8 = 0x02;
pub const STATE_FLAG_5GHZ_ACTIVE: u8 = 0x04;

/// Apple Continuity 0x0E Hotspot Status Response frame.
///
/// Wire format:
/// - Offset 0x00 (1B): Type (0x0E)
/// - Offset 0x01 (1B): Length (0x06)
/// - Offset 0x02 (1B): Battery percentage (0..=100)
/// - Offset 0x03 (1B): Cell Network Type (0x01=Wi-Fi, 0x04=LTE, 0x05=5G)
/// - Offset 0x04 (1B): Cellular Signal Bars (0..=5)
/// - Offset 0x05 (1B): Hotspot State Flags (Bit 0: SoftAP Up, Bit 1: WPA3, Bit 2: 5GHz)
/// - Offset 0x06 (2B): SoftAP Channel Frequency in MHz (big-endian u16, e.g. 5180)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotspotStatus0x0E {
    /// Battery level percentage (0-100%).
    pub battery_percent: u8,
    /// Radio network type (Wi-Fi, LTE, 5G).
    pub network_type: u8,
    /// Signal strength bars (0-5).
    pub signal_bars: u8,
    /// Hotspot status bitmask.
    pub state_flags: u8,
    /// SoftAP channel frequency in MHz (e.g., 2412 or 5180).
    pub channel_freq_mhz: u16,
}

impl HotspotStatus0x0E {
    /// Creates a new status response frame.
    pub fn new(
        battery_percent: u8,
        network_type: u8,
        signal_bars: u8,
        state_flags: u8,
        channel_freq_mhz: u16,
    ) -> Self {
        Self {
            battery_percent: battery_percent.min(100),
            network_type,
            signal_bars: signal_bars.min(5),
            state_flags,
            channel_freq_mhz,
        }
    }

    /// Serializes into the 8-byte Continuity Manufacturer Data sub-payload.
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut buf = [0u8; 8];
        buf[0] = CONTINUITY_TYPE_HOTSPOT_STATUS;
        buf[1] = STATUS_PAYLOAD_LEN;
        buf[2] = self.battery_percent;
        buf[3] = self.network_type;
        buf[4] = self.signal_bars;
        buf[5] = self.state_flags;
        buf[6..8].copy_from_slice(&self.channel_freq_mhz.to_be_bytes());
        buf
    }

    /// Serializes into the complete 10-byte Manufacturer Data frame including Company ID (0x004C).
    pub fn to_raw_manufacturer_data(&self) -> [u8; 10] {
        let mut raw = [0u8; 10];
        raw[0..2].copy_from_slice(&APPLE_COMPANY_ID.to_le_bytes());
        raw[2..10].copy_from_slice(&self.to_bytes());
        raw
    }

    /// Returns the Company ID and sub-payload tuple suitable for `bluer` advertisement.
    pub fn to_bluer_payload(&self) -> (u16, Vec<u8>) {
        (APPLE_COMPANY_ID, self.to_bytes().to_vec())
    }

    /// Parses a 0x0E frame from either a 10-byte raw manufacturer slice or an 8-byte sub-payload.
    pub fn parse(data: &[u8]) -> Result<Self, WirelessError> {
        let payload = if data.len() == 10 {
            let company_id = u16::from_le_bytes([data[0], data[1]]);
            if company_id != APPLE_COMPANY_ID {
                return Err(WirelessError::InvalidCompanyId(company_id));
            }
            &data[2..]
        } else if data.len() == 8 {
            data
        } else if data.len() < 8 {
            return Err(WirelessError::FrameTruncated {
                expected: 8,
                actual: data.len(),
            });
        } else {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Unexpected 0x0E frame length: {}", data.len()),
            });
        };

        if payload[0] != CONTINUITY_TYPE_HOTSPOT_STATUS {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Invalid continuity type: expected 0x0E, got 0x{:02X}", payload[0]),
            });
        }

        if payload[1] != STATUS_PAYLOAD_LEN {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Invalid payload length: expected 0x06, got 0x{:02X}", payload[1]),
            });
        }

        let battery_percent = payload[2];
        let network_type = payload[3];
        let signal_bars = payload[4];
        let state_flags = payload[5];
        let channel_freq_mhz = u16::from_be_bytes([payload[6], payload[7]]);

        let status = Self {
            battery_percent,
            network_type,
            signal_bars,
            state_flags,
            channel_freq_mhz,
        };

        if !status.verify() {
            return Err(WirelessError::InvalidFrame {
                reason: format!(
                    "Status telemetry fields out of valid range: battery={battery_percent}, signal={signal_bars}, freq={channel_freq_mhz}"
                ),
            });
        }

        Ok(status)
    }

    /// Verifies that telemetry data conforms to expected bounds.
    pub fn verify(&self) -> bool {
        self.battery_percent <= 100
            && self.signal_bars <= 5
            && (self.channel_freq_mhz == 0
                || (2400..=2500).contains(&self.channel_freq_mhz)
                || (4900..=6000).contains(&self.channel_freq_mhz))
    }

    /// Returns true if the SoftAP is reported as active.
    pub fn is_softap_active(&self) -> bool {
        (self.state_flags & STATE_FLAG_SOFTAP_UP) != 0
    }
}
