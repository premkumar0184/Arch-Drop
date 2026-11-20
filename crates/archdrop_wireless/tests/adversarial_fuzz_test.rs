//! Empirical Adversarial Stress and Fuzzing Suite for `archdrop_wireless`
//!
//! Validates:
//! - Truncated, malformed, bit-flipped, and corrupted payloads
//! - Boundary conditions on all protocol fields
//! - Apple 0x05, 0x0D, and 0x0E continuity parsers
//! - Zero panics across 300,000+ adversarial parsing runs

use archdrop_wireless::ble::continuity::hotspot_0x0d::{
    HotspotStatus0x0E, HotspotWakeup0x0D, BAND_2_4_GHZ, BAND_5_GHZ,
    CONTINUITY_TYPE_HOTSPOT_WAKEUP, DEFAULT_TRANSFER_PORT, RAT_TYPE_5G,
    RAT_TYPE_LTE, RAT_TYPE_WIFI, STATE_FLAG_SOFTAP_UP,
};
use archdrop_wireless::ble::continuity::proximity_0x05::{
    ProximityBeacon0x05, AIRDROP_PAYLOAD_LEN, APPLE_COMPANY_ID, CONTINUITY_TYPE_AIRDROP,
    FLAG_P2P_READY,
};
use archdrop_wireless::ble::gatt::{decode_credentials, decode_telemetry, decode_wakeup_command};
use archdrop_wireless::error::WirelessError;

/// Simple fast deterministic pseudo-random number generator (Xorshift64)
struct FastRng {
    state: u64,
}

impl FastRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xCAFE_BABE_DEAD_BEEF } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    fn fill_bytes(&mut self, buf: &mut [u8]) {
        for byte in buf.iter_mut() {
            *byte = self.next_u8();
        }
    }
}

// ============================================================================
// 1. ProximityBeacon0x05 Stress & Boundary Tests
// ============================================================================

#[test]
fn test_proximity_0x05_truncated_lengths_0_to_19() {
    let mut rng = FastRng::new(0x1234_5678);
    for len in 0..20 {
        // Test with zeros
        let zero_buf = vec![0u8; len];
        match ProximityBeacon0x05::parse(&zero_buf) {
            Err(WirelessError::FrameTruncated { expected, actual }) => {
                assert_eq!(expected, 20);
                assert_eq!(actual, len);
            }
            other => panic!("Len {len}: expected FrameTruncated, got {other:?}"),
        }

        // Test with 0xFF
        let ff_buf = vec![0xFFu8; len];
        match ProximityBeacon0x05::parse(&ff_buf) {
            Err(WirelessError::FrameTruncated { expected, actual }) => {
                assert_eq!(expected, 20);
                assert_eq!(actual, len);
            }
            other => panic!("Len {len}: expected FrameTruncated, got {other:?}"),
        }

        // Test with random bytes
        let mut rand_buf = vec![0u8; len];
        rng.fill_bytes(&mut rand_buf);
        match ProximityBeacon0x05::parse(&rand_buf) {
            Err(WirelessError::FrameTruncated { expected, actual }) => {
                assert_eq!(expected, 20);
                assert_eq!(actual, len);
            }
            other => panic!("Len {len}: expected FrameTruncated, got {other:?}"),
        }
    }
}

#[test]
fn test_proximity_0x05_invalid_lengths_above_20() {
    // Length 21 (between valid 20-byte subpayload and 22-byte raw manufacturer frame)
    let buf21 = vec![0x05; 21];
    match ProximityBeacon0x05::parse(&buf21) {
        Err(WirelessError::InvalidFrame { reason }) => {
            assert!(reason.contains("Unexpected 0x05 frame length: 21"));
        }
        other => panic!("Expected InvalidFrame for length 21, got {other:?}"),
    }

    // Lengths 23 through 100
    for len in 23..=100 {
        let buf = vec![0x05; len];
        match ProximityBeacon0x05::parse(&buf) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains(&format!("Unexpected 0x05 frame length: {len}")));
            }
            other => panic!("Len {len}: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_proximity_0x05_invalid_types_20b_and_22b() {
    let valid_beacon = ProximityBeacon0x05::new(b"secret_key_1", 0x1234, FLAG_P2P_READY);
    let valid_20 = valid_beacon.to_bytes();
    let valid_22 = valid_beacon.to_raw_manufacturer_data();

    for t in 0u8..=255 {
        if t == CONTINUITY_TYPE_AIRDROP {
            continue;
        }

        // Test 20-byte frame with invalid type
        let mut bad_20 = valid_20;
        bad_20[0] = t;
        match ProximityBeacon0x05::parse(&bad_20) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Invalid continuity type"));
            }
            other => panic!("Type 0x{t:02X} (20B): expected InvalidFrame, got {other:?}"),
        }

        // Test 22-byte frame with invalid type
        let mut bad_22 = valid_22;
        bad_22[2] = t;
        match ProximityBeacon0x05::parse(&bad_22) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Invalid continuity type"));
            }
            other => panic!("Type 0x{t:02X} (22B): expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_proximity_0x05_wrong_company_ids_22b() {
    let valid_beacon = ProximityBeacon0x05::new(b"secret_key_2", 0x4321, FLAG_P2P_READY);
    let valid_22 = valid_beacon.to_raw_manufacturer_data();

    let test_cids = [0x0000u16, 0x0001, 0x004B, 0x004D, 0x005D, 0x00E0, 0x05AC, 0xFFFF];
    for cid in test_cids {
        assert_ne!(cid, APPLE_COMPANY_ID);
        let mut bad_cid_frame = valid_22;
        bad_cid_frame[0..2].copy_from_slice(&cid.to_le_bytes());
        match ProximityBeacon0x05::parse(&bad_cid_frame) {
            Err(WirelessError::InvalidCompanyId(received)) => {
                assert_eq!(received, cid);
            }
            other => panic!("CID 0x{cid:04X}: expected InvalidCompanyId, got {other:?}"),
        }
    }
}

#[test]
fn test_proximity_0x05_invalid_payload_length_field() {
    let valid_beacon = ProximityBeacon0x05::new(b"secret_key_3", 0x5678, FLAG_P2P_READY);
    let valid_20 = valid_beacon.to_bytes();

    for l in 0u8..=255 {
        if l == AIRDROP_PAYLOAD_LEN {
            continue;
        }
        let mut bad_len_frame = valid_20;
        bad_len_frame[1] = l;
        match ProximityBeacon0x05::parse(&bad_len_frame) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Invalid payload length"));
            }
            other => panic!("Len 0x{l:02X}: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_proximity_0x05_corrupt_padding_and_suffix() {
    let valid_beacon = ProximityBeacon0x05::new(b"secret_key_4", 0x9ABC, FLAG_P2P_READY);
    let valid_20 = valid_beacon.to_bytes();

    // Check each of the 8 reserved padding bytes (indices 2..10)
    for pad_idx in 2..10 {
        let mut corrupt_pad = valid_20;
        corrupt_pad[pad_idx] = 0x01;
        match ProximityBeacon0x05::parse(&corrupt_pad) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Corrupted 8-byte reserved zero padding"));
            }
            other => panic!("Padding idx {pad_idx}: expected InvalidFrame, got {other:?}"),
        }
    }

    // Corrupt terminal suffix (index 19)
    for s in 1u8..=255 {
        let mut corrupt_suffix = valid_20;
        corrupt_suffix[19] = s;
        match ProximityBeacon0x05::parse(&corrupt_suffix) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Invalid terminal suffix byte"));
            }
            other => panic!("Suffix 0x{s:02X}: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_proximity_0x05_crypto_edge_cases() {
    // Empty key HMAC derivation
    let empty_key: &[u8] = b"";
    let hashes = ProximityBeacon0x05::compute_truncated_hashes(empty_key, 0x1234);
    assert_ne!(hashes, [0u8; 6]);

    // Huge key (10 KB)
    let huge_key = vec![0x42u8; 10240];
    let hashes_huge = ProximityBeacon0x05::compute_truncated_hashes(&huge_key, 0x1234);
    assert_ne!(hashes_huge, [0u8; 6]);

    // Verify empty list
    let beacon = ProximityBeacon0x05::new(empty_key, 0x1234, FLAG_P2P_READY);
    assert!(!beacon.verify_match(&[]));
}

// ============================================================================
// 2. HotspotWakeup0x0D Stress & Boundary Tests
// ============================================================================

#[test]
fn test_hotspot_0x0d_truncated_lengths_0_to_9() {
    let mut rng = FastRng::new(0x9999_8888);
    for len in 0..10 {
        let zero_buf = vec![0u8; len];
        match HotspotWakeup0x0D::parse(&zero_buf) {
            Err(WirelessError::FrameTruncated { expected, actual }) => {
                assert_eq!(expected, 10);
                assert_eq!(actual, len);
            }
            other => panic!("Len {len}: expected FrameTruncated, got {other:?}"),
        }

        let mut rand_buf = vec![0u8; len];
        rng.fill_bytes(&mut rand_buf);
        match HotspotWakeup0x0D::parse(&rand_buf) {
            Err(WirelessError::FrameTruncated { expected, actual }) => {
                assert_eq!(expected, 10);
                assert_eq!(actual, len);
            }
            other => panic!("Len {len}: expected FrameTruncated, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0d_invalid_lengths_above_10() {
    // Length 11
    let buf11 = vec![0x0D; 11];
    match HotspotWakeup0x0D::parse(&buf11) {
        Err(WirelessError::InvalidFrame { reason }) => {
            assert!(reason.contains("Unexpected 0x0D frame length: 11"));
        }
        other => panic!("Expected InvalidFrame for length 11, got {other:?}"),
    }

    // Lengths 13 through 64
    for len in 13..=64 {
        let buf = vec![0x0D; len];
        match HotspotWakeup0x0D::parse(&buf) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains(&format!("Unexpected 0x0D frame length: {len}")));
            }
            other => panic!("Len {len}: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0d_invalid_types_10b_and_12b() {
    let wakeup = HotspotWakeup0x0D::new(b"secret_key_w", BAND_5_GHZ, DEFAULT_TRANSFER_PORT);
    let bytes_10 = wakeup.to_bytes();
    let bytes_12 = wakeup.to_raw_manufacturer_data();

    for t in 0u8..=255 {
        if t == CONTINUITY_TYPE_HOTSPOT_WAKEUP {
            continue;
        }

        // 10-byte subpayload
        let mut bad_10 = bytes_10;
        bad_10[0] = t;
        match HotspotWakeup0x0D::parse(&bad_10) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Invalid continuity type"));
            }
            other => panic!("Type 0x{t:02X} (10B): expected InvalidFrame, got {other:?}"),
        }

        // 12-byte raw frame
        let mut bad_12 = bytes_12;
        bad_12[2] = t;
        match HotspotWakeup0x0D::parse(&bad_12) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Invalid continuity type"));
            }
            other => panic!("Type 0x{t:02X} (12B): expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0d_wrong_company_ids_12b() {
    let wakeup = HotspotWakeup0x0D::new(b"secret_key_w2", BAND_2_4_GHZ, DEFAULT_TRANSFER_PORT);
    let bytes_12 = wakeup.to_raw_manufacturer_data();

    let test_cids = [0x0000u16, 0x004B, 0x004D, 0x005D, 0x1234, 0xFFFF];
    for cid in test_cids {
        let mut bad_cid = bytes_12;
        bad_cid[0..2].copy_from_slice(&cid.to_le_bytes());
        match HotspotWakeup0x0D::parse(&bad_cid) {
            Err(WirelessError::InvalidCompanyId(received)) => {
                assert_eq!(received, cid);
            }
            other => panic!("CID 0x{cid:04X}: expected InvalidCompanyId, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0d_invalid_payload_len_field() {
    let wakeup = HotspotWakeup0x0D::new(b"secret_key_w3", BAND_5_GHZ, DEFAULT_TRANSFER_PORT);
    let bytes_10 = wakeup.to_bytes();

    for l in 0u8..=255 {
        if l == 0x08 {
            continue;
        }
        let mut bad_len = bytes_10;
        bad_len[1] = l;
        match HotspotWakeup0x0D::parse(&bad_len) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Invalid payload length"));
            }
            other => panic!("Len 0x{l:02X}: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0d_control_flags_and_port_variations() {
    let wakeup = HotspotWakeup0x0D::new(b"secret_key_w4", BAND_5_GHZ, DEFAULT_TRANSFER_PORT);
    let bytes_10 = wakeup.to_bytes();

    // Test ports: 0, 1, 80, 53317, 65535
    for port in [0u16, 1, 80, 443, 53317, 65535] {
        let mut frame = bytes_10;
        frame[8..10].copy_from_slice(&port.to_be_bytes());
        let res = HotspotWakeup0x0D::parse(&frame);
        assert!(res.is_ok(), "Port {port} failed to parse: {res:?}");
        assert_eq!(res.unwrap().transfer_port, port);
    }

    // Test control flags: all combinations 0x00..=0xFF
    for flags in [0x00u8, 0x01, 0x02, 0x03, 0x80, 0xFF] {
        let mut frame = bytes_10;
        frame[2] = flags;
        let res = HotspotWakeup0x0D::parse(&frame);
        assert!(res.is_ok());
        assert_eq!(res.unwrap().control_flags, flags);
    }

    // Test target band variations: 0, 1 (2.4G), 2 (5G), 3, 255
    for band in [0u8, 1, 2, 3, 255] {
        let mut frame = bytes_10;
        frame[3] = band;
        let res = HotspotWakeup0x0D::parse(&frame);
        assert!(res.is_ok());
        assert_eq!(res.unwrap().target_band, band);
    }
}

// ============================================================================
// 3. HotspotStatus0x0E Stress & Boundary Tests
// ============================================================================

#[test]
fn test_hotspot_0x0e_truncated_lengths_0_to_7() {
    let mut rng = FastRng::new(0x4444_5555);
    for len in 0..8 {
        let zero_buf = vec![0u8; len];
        match HotspotStatus0x0E::parse(&zero_buf) {
            Err(WirelessError::FrameTruncated { expected, actual }) => {
                assert_eq!(expected, 8);
                assert_eq!(actual, len);
            }
            other => panic!("Len {len}: expected FrameTruncated, got {other:?}"),
        }

        let mut rand_buf = vec![0u8; len];
        rng.fill_bytes(&mut rand_buf);
        match HotspotStatus0x0E::parse(&rand_buf) {
            Err(WirelessError::FrameTruncated { expected, actual }) => {
                assert_eq!(expected, 8);
                assert_eq!(actual, len);
            }
            other => panic!("Len {len}: expected FrameTruncated, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0e_invalid_lengths_above_8() {
    // Length 9
    let buf9 = vec![0x0E; 9];
    match HotspotStatus0x0E::parse(&buf9) {
        Err(WirelessError::InvalidFrame { reason }) => {
            assert!(reason.contains("Unexpected 0x0E frame length: 9"));
        }
        other => panic!("Expected InvalidFrame for length 9, got {other:?}"),
    }

    // Lengths 11 through 64
    for len in 11..=64 {
        let buf = vec![0x0E; len];
        match HotspotStatus0x0E::parse(&buf) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains(&format!("Unexpected 0x0E frame length: {len}")));
            }
            other => panic!("Len {len}: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0e_battery_boundary_and_overflow() {
    let valid_status = HotspotStatus0x0E::new(80, RAT_TYPE_5G, 4, STATE_FLAG_SOFTAP_UP, 5180);
    let valid_8 = valid_status.to_bytes();

    // Valid boundary percentages: 0, 50, 100
    for batt in [0u8, 1, 50, 99, 100] {
        let mut frame = valid_8;
        frame[2] = batt;
        let parsed = HotspotStatus0x0E::parse(&frame).expect("Should parse valid battery %");
        assert_eq!(parsed.battery_percent, batt);
    }

    // Out-of-bounds battery percentages: 101 through 255
    for batt in [101u8, 102, 120, 150, 200, 255] {
        let mut frame = valid_8;
        frame[2] = batt;
        match HotspotStatus0x0E::parse(&frame) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Status telemetry fields out of valid range"));
                assert!(reason.contains(&format!("battery={batt}")));
            }
            other => panic!("Battery {batt}%: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0e_signal_bars_boundary() {
    let valid_status = HotspotStatus0x0E::new(80, RAT_TYPE_LTE, 3, STATE_FLAG_SOFTAP_UP, 2412);
    let valid_8 = valid_status.to_bytes();

    // Valid bars according to verify(): 0..=5
    for bars in 0u8..=5 {
        let mut frame = valid_8;
        frame[4] = bars;
        let parsed = HotspotStatus0x0E::parse(&frame).expect("Should parse valid signal bars 0..=5");
        assert_eq!(parsed.signal_bars, bars);
    }

    // Invalid bars: 6 through 255
    for bars in [6u8, 7, 8, 10, 50, 100, 255] {
        let mut frame = valid_8;
        frame[4] = bars;
        match HotspotStatus0x0E::parse(&frame) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Status telemetry fields out of valid range"));
                assert!(reason.contains(&format!("signal={bars}")));
            }
            other => panic!("Signal bars {bars}: expected InvalidFrame, got {other:?}"),
        }
    }
}

#[test]
fn test_hotspot_0x0e_channel_freq_boundaries() {
    let valid_status = HotspotStatus0x0E::new(50, RAT_TYPE_WIFI, 2, 0, 0);
    let valid_8 = valid_status.to_bytes();

    // Valid frequency boundaries:
    // 1. 0 (inactive/idle)
    // 2. 2400..=2500 (2.4 GHz band)
    // 3. 4900..=6000 (5 GHz band)
    let valid_freqs = [0u16, 2400, 2412, 2437, 2462, 2484, 2500, 4900, 5180, 5500, 5825, 6000];
    for freq in valid_freqs {
        let mut frame = valid_8;
        frame[6..8].copy_from_slice(&freq.to_be_bytes());
        let parsed = HotspotStatus0x0E::parse(&frame).expect("Should parse valid freq");
        assert_eq!(parsed.channel_freq_mhz, freq);
    }

    // Invalid frequencies:
    let invalid_freqs = [1u16, 100, 1000, 2399, 2501, 3000, 4899, 6001, 6500, 10000, 65535];
    for freq in invalid_freqs {
        let mut frame = valid_8;
        frame[6..8].copy_from_slice(&freq.to_be_bytes());
        match HotspotStatus0x0E::parse(&frame) {
            Err(WirelessError::InvalidFrame { reason }) => {
                assert!(reason.contains("Status telemetry fields out of valid range"));
                assert!(reason.contains(&format!("freq={freq}")));
            }
            other => panic!("Freq {freq}: expected InvalidFrame, got {other:?}"),
        }
    }
}

// ============================================================================
// 4. Bit-Flip Fuzzing Across All Protocols
// ============================================================================

#[test]
fn test_bitflip_fuzzing_all_frames() {
    let beacon_20 = ProximityBeacon0x05::new(b"bf_key", 0x1122, FLAG_P2P_READY).to_bytes();
    let beacon_22 = ProximityBeacon0x05::new(b"bf_key", 0x1122, FLAG_P2P_READY).to_raw_manufacturer_data();

    let wakeup_10 = HotspotWakeup0x0D::new(b"bf_key", BAND_5_GHZ, DEFAULT_TRANSFER_PORT).to_bytes();
    let wakeup_12 = HotspotWakeup0x0D::new(b"bf_key", BAND_5_GHZ, DEFAULT_TRANSFER_PORT).to_raw_manufacturer_data();

    let status_8 = HotspotStatus0x0E::new(95, RAT_TYPE_5G, 4, STATE_FLAG_SOFTAP_UP, 5180).to_bytes();
    let status_10 = HotspotStatus0x0E::new(95, RAT_TYPE_5G, 4, STATE_FLAG_SOFTAP_UP, 5180).to_raw_manufacturer_data();

    // 1. Bit flip 0x05 (20B & 22B)
    for byte_idx in 0..beacon_20.len() {
        for bit in 0..8 {
            let mut flipped = beacon_20;
            flipped[byte_idx] ^= 1 << bit;
            let _ = ProximityBeacon0x05::parse(&flipped); // MUST NOT PANIC
        }
    }
    for byte_idx in 0..beacon_22.len() {
        for bit in 0..8 {
            let mut flipped = beacon_22;
            flipped[byte_idx] ^= 1 << bit;
            let _ = ProximityBeacon0x05::parse(&flipped); // MUST NOT PANIC
        }
    }

    // 2. Bit flip 0x0D (10B & 12B)
    for byte_idx in 0..wakeup_10.len() {
        for bit in 0..8 {
            let mut flipped = wakeup_10;
            flipped[byte_idx] ^= 1 << bit;
            let _ = HotspotWakeup0x0D::parse(&flipped); // MUST NOT PANIC
        }
    }
    for byte_idx in 0..wakeup_12.len() {
        for bit in 0..8 {
            let mut flipped = wakeup_12;
            flipped[byte_idx] ^= 1 << bit;
            let _ = HotspotWakeup0x0D::parse(&flipped); // MUST NOT PANIC
        }
    }

    // 3. Bit flip 0x0E (8B & 10B)
    for byte_idx in 0..status_8.len() {
        for bit in 0..8 {
            let mut flipped = status_8;
            flipped[byte_idx] ^= 1 << bit;
            let _ = HotspotStatus0x0E::parse(&flipped); // MUST NOT PANIC
        }
    }
    for byte_idx in 0..status_10.len() {
        for bit in 0..8 {
            let mut flipped = status_10;
            flipped[byte_idx] ^= 1 << bit;
            let _ = HotspotStatus0x0E::parse(&flipped); // MUST NOT PANIC
        }
    }
}

// ============================================================================
// 5. Massive Random Payload Fuzzing (100,000 Iterations x 3 Parsers = 300,000 Runs)
// ============================================================================

#[test]
fn test_massive_random_payload_fuzzing() {
    let mut rng = FastRng::new(0xDEAD_BEEF_FEED_F00D);
    let mut buf = [0u8; 256];

    let mut beacon_ok = 0usize;
    let mut wakeup_ok = 0usize;
    let mut status_ok = 0usize;

    for _i in 0..100_000 {
        // Pseudo-random slice length 0..=256
        let len = (rng.next_u64() % 257) as usize;
        rng.fill_bytes(&mut buf[0..len]);
        let slice = &buf[0..len];

        // All parsers MUST execute without panic
        if ProximityBeacon0x05::parse(slice).is_ok() {
            beacon_ok += 1;
        }
        if HotspotWakeup0x0D::parse(slice).is_ok() {
            wakeup_ok += 1;
        }
        if HotspotStatus0x0E::parse(slice).is_ok() {
            status_ok += 1;
        }
    }

    // Almost all random frames should be rejected
    println!(
        "Fuzzing completed: 100,000 runs each. Valid hits: 0x05={beacon_ok}, 0x0D={wakeup_ok}, 0x0E={status_ok}"
    );
}

// ============================================================================
// 6. GATT Decoders Adversarial Stress
// ============================================================================

#[test]
fn test_gatt_decoders_adversarial_stress() {
    let mut rng = FastRng::new(0xFACE_B00C);
    let mut buf = [0u8; 128];

    // Corrupted inputs to GATT decoders
    for _ in 0..10_000 {
        let len = (rng.next_u64() % 129) as usize;
        rng.fill_bytes(&mut buf[0..len]);
        let slice = &buf[0..len];

        let _ = decode_credentials(slice); // serde_json parse, must not panic
        let _ = decode_wakeup_command(slice); // 0x0D parse, must not panic
        let _ = decode_telemetry(slice); // 0x0E parse, must not panic
    }
}
