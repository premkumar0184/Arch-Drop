//! Apple Continuity 0x05 Proximity Presence Beacon (AirDrop wire compatibility).
//!
//! Encapsulates a 22-byte Manufacturer Data frame under Bluetooth SIG Company Identifier 0x004C (Apple Inc.).
//! Uses rolling 16-bit salts and truncated HMAC-SHA256 tokens to prevent offline rainbow-table
//! correlation attacks while maintaining standard legacy BLE PDU (27-byte total packet <= 31 bytes) compatibility.

use crate::error::WirelessError;

/// Bluetooth SIG 16-bit Company Identifier for Apple Inc.
pub const APPLE_COMPANY_ID: u16 = 0x004C;

/// Apple Continuity sub-type for AirDrop Proximity Beacon.
pub const CONTINUITY_TYPE_AIRDROP: u8 = 0x05;

/// Sub-payload length following the Continuity Type and Length bytes (18 bytes).
pub const AIRDROP_PAYLOAD_LEN: u8 = 0x12;

/// Standard version flag indicating P2P readiness (Bit 0) and LOHS support (Bit 1).
pub const FLAG_P2P_READY: u8 = 0x01;
pub const FLAG_LOHS_SUPPORTED: u8 = 0x02;

/// Apple Continuity 0x05 Proximity Presence Beacon.
///
/// Wire format:
/// - Offset 0x00 (1B): Type (0x05)
/// - Offset 0x01 (1B): Length (0x12 = 18 bytes)
/// - Offset 0x02 (8B): Reserved Zeros [0x00; 8]
/// - Offset 0x0A (1B): Version & Capability Flags (0x01 = P2P Ready)
/// - Offset 0x0B (2B): Rolling 16-bit Salt (big-endian)
/// - Offset 0x0D (6B): Truncated HMAC-SHA256 Token (3 x 2-byte contact hash slots)
/// - Offset 0x13 (1B): Terminal Suffix Delimiter (0x00)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProximityBeacon0x05 {
    /// Version and capability flags (default 0x01).
    pub flags: u8,
    /// Rolling 16-bit salt (rotates periodically with MAC address).
    pub salt: u16,
    /// 6-byte truncated HMAC-SHA256 token derived from paired secret and salt.
    pub truncated_hashes: [u8; 6],
}

impl ProximityBeacon0x05 {
    /// Computes the 6-byte truncated HMAC token from a paired shared secret and a 16-bit salt.
    pub fn compute_truncated_hashes(paired_key: &[u8], salt: u16) -> [u8; 6] {
        let s_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, paired_key);
        let tag = ring::hmac::sign(&s_key, &salt.to_be_bytes());
        let mut hashes = [0u8; 6];
        hashes.copy_from_slice(&tag.as_ref()[0..6]);
        hashes
    }

    /// Creates a new rolling salted presence beacon using a paired key, salt, and flags.
    pub fn new(paired_key: &[u8], salt: u16, flags: u8) -> Self {
        let truncated_hashes = Self::compute_truncated_hashes(paired_key, salt);
        Self {
            flags,
            salt,
            truncated_hashes,
        }
    }

    /// Directly constructs a beacon from raw fields (useful for testing and deserialization).
    pub fn from_parts(flags: u8, salt: u16, truncated_hashes: [u8; 6]) -> Self {
        Self {
            flags,
            salt,
            truncated_hashes,
        }
    }

    /// Serializes the beacon into the 20-byte Apple Continuity Manufacturer Data sub-payload.
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut buf = [0u8; 20];
        buf[0] = CONTINUITY_TYPE_AIRDROP;
        buf[1] = AIRDROP_PAYLOAD_LEN;
        // buf[2..10] are 8 zeros: [0x00; 8]
        buf[10] = self.flags;
        buf[11..13].copy_from_slice(&self.salt.to_be_bytes());
        buf[13..19].copy_from_slice(&self.truncated_hashes);
        buf[19] = 0x00;
        buf
    }

    /// Serializes into the complete 22-byte Manufacturer Data frame including Company ID (0x004C).
    pub fn to_raw_manufacturer_data(&self) -> [u8; 22] {
        let mut raw = [0u8; 22];
        raw[0..2].copy_from_slice(&APPLE_COMPANY_ID.to_le_bytes());
        raw[2..22].copy_from_slice(&self.to_bytes());
        raw
    }

    /// Returns the Company ID and sub-payload tuple suitable for `bluer` advertisement registration.
    pub fn to_bluer_payload(&self) -> (u16, Vec<u8>) {
        (APPLE_COMPANY_ID, self.to_bytes().to_vec())
    }

    /// Parses a 0x05 beacon from either a 22-byte raw manufacturer slice or a 20-byte sub-payload.
    pub fn parse(data: &[u8]) -> Result<Self, WirelessError> {
        let payload = if data.len() == 22 {
            let company_id = u16::from_le_bytes([data[0], data[1]]);
            if company_id != APPLE_COMPANY_ID {
                return Err(WirelessError::InvalidCompanyId(company_id));
            }
            &data[2..]
        } else if data.len() == 20 {
            data
        } else if data.len() < 20 {
            return Err(WirelessError::FrameTruncated {
                expected: 20,
                actual: data.len(),
            });
        } else {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Unexpected 0x05 frame length: {}", data.len()),
            });
        };

        if payload[0] != CONTINUITY_TYPE_AIRDROP {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Invalid continuity type: expected 0x05, got 0x{:02X}", payload[0]),
            });
        }

        if payload[1] != AIRDROP_PAYLOAD_LEN {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Invalid payload length: expected 0x12, got 0x{:02X}", payload[1]),
            });
        }

        // Validate 8-byte zero prefix
        if payload[2..10] != [0u8; 8] {
            return Err(WirelessError::InvalidFrame {
                reason: "Corrupted 8-byte reserved zero padding in 0x05 frame".to_string(),
            });
        }

        let flags = payload[10];
        let salt = u16::from_be_bytes([payload[11], payload[12]]);
        let mut truncated_hashes = [0u8; 6];
        truncated_hashes.copy_from_slice(&payload[13..19]);

        if payload[19] != 0x00 {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Invalid terminal suffix byte: expected 0x00, got 0x{:02X}", payload[19]),
            });
        }

        Ok(Self {
            flags,
            salt,
            truncated_hashes,
        })
    }

    /// Verifies if this beacon matches any of the known paired peer keys in constant time.
    pub fn verify_match(&self, paired_keys: &[&[u8]]) -> bool {
        for key in paired_keys {
            let expected = Self::compute_truncated_hashes(key, self.salt);
            if constant_time_eq_6(&self.truncated_hashes, &expected) {
                return true;
            }
        }
        false
    }
}

/// Constant-time comparison of two 6-byte slices to prevent timing side channels.
#[inline]
pub fn constant_time_eq_6(a: &[u8; 6], b: &[u8; 6]) -> bool {
    let mut diff = 0u8;
    for i in 0..6 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}
