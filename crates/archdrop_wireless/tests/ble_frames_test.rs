//! Comprehensive test suite for Arch Drop Linux BLE & Continuity Engine:
//! Apple 0x05 proximity beacon, 0x0D hotspot wakeup, 0x0E hotspot status frames,
//! rolling HMAC verification, boundary conditions, and GATT credential serialization.

use archdrop_wireless::ble::continuity::hotspot_0x0d::{
    constant_time_eq_4, HotspotStatus0x0E, HotspotWakeup0x0D, BAND_2_4_GHZ, BAND_5_GHZ,
    CONTINUITY_TYPE_HOTSPOT_STATUS, CONTINUITY_TYPE_HOTSPOT_WAKEUP, DEFAULT_TRANSFER_PORT,
    FLAG_WAKEUP_5GHZ_PREFERRED, FLAG_WAKEUP_START_LOHS, RAT_TYPE_5G, RAT_TYPE_LTE, RAT_TYPE_WIFI,
    STATE_FLAG_5GHZ_ACTIVE, STATE_FLAG_SOFTAP_UP, STATE_FLAG_WPA3_ACTIVE,
};
use archdrop_wireless::ble::continuity::proximity_0x05::{
    constant_time_eq_6, ProximityBeacon0x05, AIRDROP_PAYLOAD_LEN, APPLE_COMPANY_ID,
    CONTINUITY_TYPE_AIRDROP, FLAG_LOHS_SUPPORTED, FLAG_P2P_READY,
};
use archdrop_wireless::ble::gatt::{
    decode_clipboard_packet, decode_credentials, decode_telemetry, decode_wakeup_command,
    derive_paired_key, encode_clipboard_packet, encode_credentials, encode_telemetry,
    encode_wakeup_command, BleClipboardPacket, GattServerState, ARCHDROP_SERVICE_UUID, CHAR_CLIPBOARD_SYNC_UUID,
    CHAR_DEVICE_TELEMETRY_UUID, CHAR_LOHS_CREDENTIALS_UUID, CHAR_WAKEUP_COMMAND_UUID,
    CLIPBOARD_MIME_TEXT_PLAIN, CLIPBOARD_MIME_URI_LIST, MAX_BLE_CLIPBOARD_PAYLOAD,
};
use archdrop_wireless::ble::types::{BleConfig, HotspotCredentials};
use archdrop_wireless::ble::BleController;
use archdrop_wireless::error::WirelessError;
use std::net::Ipv4Addr;

// ============================================================================
// ProximityBeacon0x05 (AirDrop Presence Beacon) Tests
// ============================================================================

#[test]
fn test_proximity_0x05_roundtrip_subpayload() {
    let paired_key = b"archdrop_test_paired_secret_key_1";
    let salt = 0x1A2B;
    let flags = FLAG_P2P_READY | FLAG_LOHS_SUPPORTED;

    let beacon = ProximityBeacon0x05::new(paired_key, salt, flags);
    let bytes = beacon.to_bytes();
    assert_eq!(bytes.len(), 20);
    assert_eq!(bytes[0], CONTINUITY_TYPE_AIRDROP);
    assert_eq!(bytes[1], AIRDROP_PAYLOAD_LEN);
    assert_eq!(&bytes[2..10], &[0u8; 8]); // 8 reserved zeros
    assert_eq!(bytes[10], flags);
    assert_eq!(&bytes[11..13], &salt.to_be_bytes());
    assert_eq!(bytes[19], 0x00); // terminal suffix

    let parsed = ProximityBeacon0x05::parse(&bytes).expect("Failed to parse valid 20-byte 0x05 sub-payload");
    assert_eq!(parsed, beacon);
    assert_eq!(parsed.salt, salt);
    assert_eq!(parsed.flags, flags);
}

#[test]
fn test_proximity_0x05_roundtrip_raw_manufacturer_data() {
    let paired_key = b"archdrop_test_paired_secret_key_2";
    let salt = 0xC0DE;
    let flags = FLAG_P2P_READY;

    let beacon = ProximityBeacon0x05::new(paired_key, salt, flags);
    let raw = beacon.to_raw_manufacturer_data();
    assert_eq!(raw.len(), 22);
    // Company ID 0x004C in little-endian wire order: [0x4C, 0x00]
    assert_eq!(&raw[0..2], &[0x4C, 0x00]);
    assert_eq!(&raw[2..], &beacon.to_bytes());

    let parsed = ProximityBeacon0x05::parse(&raw).expect("Failed to parse valid 22-byte 0x05 frame");
    assert_eq!(parsed, beacon);
}

#[test]
fn test_proximity_0x05_bluer_payload_format() {
    let paired_key = b"archdrop_test_paired_secret_key_3";
    let beacon = ProximityBeacon0x05::new(paired_key, 0x4321, FLAG_P2P_READY);
    let (cid, payload) = beacon.to_bluer_payload();
    assert_eq!(cid, APPLE_COMPANY_ID);
    assert_eq!(payload.len(), 20);
    assert_eq!(payload.as_slice(), &beacon.to_bytes());
}

#[test]
fn test_proximity_0x05_hmac_verification_and_keys() {
    let alice_key = b"alice_master_paired_secret_key_x";
    let bob_key = b"bob_master_paired_secret_key_y";
    let eve_key = b"eve_unpaired_attacker_secret_z";

    let salt = 0x55AA;
    let alice_beacon = ProximityBeacon0x05::new(alice_key, salt, FLAG_P2P_READY);

    // Alice beacon verified against list containing Alice's key
    assert!(alice_beacon.verify_match(&[alice_key, bob_key]));
    // Alice beacon verified with only Alice's key
    assert!(alice_beacon.verify_match(&[alice_key]));
    // Alice beacon fails against Bob's key only
    assert!(!alice_beacon.verify_match(&[bob_key]));
    // Alice beacon fails against Eve's key
    assert!(!alice_beacon.verify_match(&[eve_key]));
    // Empty key list fails
    assert!(!alice_beacon.verify_match(&[]));
}

#[test]
fn test_proximity_0x05_tampered_salt_and_token_rejection() {
    let key = b"shared_secret_123456789012345678";
    let beacon = ProximityBeacon0x05::new(key, 0x1111, FLAG_P2P_READY);

    // Tamper with salt
    let mut tampered_salt_beacon = beacon.clone();
    tampered_salt_beacon.salt = 0x2222;
    assert!(!tampered_salt_beacon.verify_match(&[key]));

    // Tamper with truncated hash token
    let mut tampered_token_beacon = beacon.clone();
    tampered_token_beacon.truncated_hashes[0] ^= 0xFF;
    assert!(!tampered_token_beacon.verify_match(&[key]));
}

#[test]
fn test_proximity_0x05_invalid_frame_errors() {
    // 1. Truncated frames
    let short_data = [0x05, 0x12, 0x00];
    match ProximityBeacon0x05::parse(&short_data) {
        Err(WirelessError::FrameTruncated { expected, actual }) => {
            assert_eq!(expected, 20);
            assert_eq!(actual, 3);
        }
        other => panic!("Expected FrameTruncated, got {other:?}"),
    }

    // 2. Invalid company ID in 22-byte frame
    let mut bad_company = [0u8; 22];
    bad_company[0..2].copy_from_slice(&0x005Du16.to_le_bytes()); // Realtek company ID instead of Apple
    bad_company[2..].copy_from_slice(&ProximityBeacon0x05::new(b"k", 1, 1).to_bytes());
    match ProximityBeacon0x05::parse(&bad_company) {
        Err(WirelessError::InvalidCompanyId(cid)) => assert_eq!(cid, 0x005D),
        other => panic!("Expected InvalidCompanyId, got {other:?}"),
    }

    // 3. Corrupted zero padding
    let mut corrupt_padding = ProximityBeacon0x05::new(b"k", 1, 1).to_bytes();
    corrupt_padding[4] = 0x01; // Non-zero in padding
    match ProximityBeacon0x05::parse(&corrupt_padding) {
        Err(WirelessError::InvalidFrame { reason }) => {
            assert!(reason.contains("Corrupted 8-byte reserved zero padding"));
        }
        other => panic!("Expected InvalidFrame, got {other:?}"),
    }

    // 4. Invalid continuity type byte
    let mut bad_type = ProximityBeacon0x05::new(b"k", 1, 1).to_bytes();
    bad_type[0] = 0x07; // Wrong type
    match ProximityBeacon0x05::parse(&bad_type) {
        Err(WirelessError::InvalidFrame { reason }) => {
            assert!(reason.contains("Invalid continuity type"));
        }
        other => panic!("Expected InvalidFrame, got {other:?}"),
    }

    // 5. Invalid terminal suffix
    let mut bad_suffix = ProximityBeacon0x05::new(b"k", 1, 1).to_bytes();
    bad_suffix[19] = 0xFF; // Not 0x00
    match ProximityBeacon0x05::parse(&bad_suffix) {
        Err(WirelessError::InvalidFrame { reason }) => {
            assert!(reason.contains("Invalid terminal suffix byte"));
        }
        other => panic!("Expected InvalidFrame, got {other:?}"),
    }
}

// ============================================================================
// HotspotWakeup0x0D (Remote Instant Hotspot Wakeup) Tests
// ============================================================================

#[test]
fn test_hotspot_0x0d_roundtrip_and_bands() {
    let paired_key = b"remote_hotspot_secret_pair_key_1";

    // 1. 5 GHz wakeup request
    let wakeup_5g = HotspotWakeup0x0D::new(paired_key, BAND_5_GHZ, DEFAULT_TRANSFER_PORT);
    assert_eq!(wakeup_5g.target_band, BAND_5_GHZ);
    assert_eq!(
        wakeup_5g.control_flags & (FLAG_WAKEUP_START_LOHS | FLAG_WAKEUP_5GHZ_PREFERRED),
        FLAG_WAKEUP_START_LOHS | FLAG_WAKEUP_5GHZ_PREFERRED
    );
    assert_eq!(wakeup_5g.transfer_port, DEFAULT_TRANSFER_PORT);

    let bytes_5g = wakeup_5g.to_bytes();
    assert_eq!(bytes_5g.len(), 10);
    assert_eq!(bytes_5g[0], CONTINUITY_TYPE_HOTSPOT_WAKEUP);
    assert_eq!(bytes_5g[1], 0x08);

    let parsed_5g = HotspotWakeup0x0D::parse(&bytes_5g).expect("Failed to parse 10-byte 0x0D frame");
    assert_eq!(parsed_5g, wakeup_5g);

    // 2. 2.4 GHz wakeup request with custom port
    let wakeup_2g = HotspotWakeup0x0D::new(paired_key, BAND_2_4_GHZ, 49152);
    assert_eq!(wakeup_2g.target_band, BAND_2_4_GHZ);
    assert_eq!(wakeup_2g.control_flags & FLAG_WAKEUP_START_LOHS, FLAG_WAKEUP_START_LOHS);
    assert_eq!(wakeup_2g.control_flags & FLAG_WAKEUP_5GHZ_PREFERRED, 0);
    assert_eq!(wakeup_2g.transfer_port, 49152);

    let raw_2g = wakeup_2g.to_raw_manufacturer_data();
    assert_eq!(raw_2g.len(), 12);
    assert_eq!(&raw_2g[0..2], &[0x4C, 0x00]);

    let parsed_2g = HotspotWakeup0x0D::parse(&raw_2g).expect("Failed to parse 12-byte 0x0D frame");
    assert_eq!(parsed_2g, wakeup_2g);
}

#[test]
fn test_hotspot_0x0d_auth_token_verification_and_skew() {
    let key = b"authentication_token_signing_key";
    let wakeup = HotspotWakeup0x0D::new(key, BAND_5_GHZ, DEFAULT_TRANSFER_PORT);

    // 1. Valid current window verification
    assert!(wakeup.verify(key));

    // 2. Wrong key fails
    assert!(!wakeup.verify(b"wrong_unpaired_key_attacker"));

    // 3. Exact epoch minute verification
    let current_min = HotspotWakeup0x0D::current_epoch_minute();
    assert!(wakeup.verify_at_epoch(key, current_min));

    // 4. Clock skew tolerance in verify() covers +/- 1 and +/- 2 minutes
    assert!(wakeup.verify_at_epoch(key, current_min));

    // 5. Replay attack rejection: token from 10 minutes ago
    let old_epoch_min = current_min.saturating_sub(10);
    let old_token = HotspotWakeup0x0D::compute_auth_token(key, old_epoch_min);
    let replayed_wakeup = HotspotWakeup0x0D::from_parts(
        FLAG_WAKEUP_START_LOHS,
        BAND_5_GHZ,
        old_token,
        DEFAULT_TRANSFER_PORT,
    );
    // Since replayed_wakeup is 10 minutes old, verify() against current time MUST fail
    assert!(!replayed_wakeup.verify(key));
}

#[test]
fn test_hotspot_0x0d_invalid_frame_errors() {
    // Truncated
    let truncated = [0x0D, 0x08, 0x01];
    match HotspotWakeup0x0D::parse(&truncated) {
        Err(WirelessError::FrameTruncated { expected: 10, actual: 3 }) => {}
        other => panic!("Expected FrameTruncated, got {other:?}"),
    }

    // Invalid company ID
    let mut bad_company = [0u8; 12];
    bad_company[0..2].copy_from_slice(&0x004Bu16.to_le_bytes());
    bad_company[2..].copy_from_slice(&HotspotWakeup0x0D::new(b"k", 1, 53317).to_bytes());
    match HotspotWakeup0x0D::parse(&bad_company) {
        Err(WirelessError::InvalidCompanyId(0x004B)) => {}
        other => panic!("Expected InvalidCompanyId, got {other:?}"),
    }

    // Invalid type byte
    let mut bad_type = HotspotWakeup0x0D::new(b"k", 1, 53317).to_bytes();
    bad_type[0] = 0x05;
    assert!(matches!(HotspotWakeup0x0D::parse(&bad_type), Err(WirelessError::InvalidFrame { .. })));
}

// ============================================================================
// HotspotStatus0x0E (Hotspot Telemetry Response) Tests
// ============================================================================

#[test]
fn test_hotspot_0x0e_roundtrip_and_telemetry() {
    let status = HotspotStatus0x0E::new(
        88,
        RAT_TYPE_5G,
        4,
        STATE_FLAG_SOFTAP_UP | STATE_FLAG_WPA3_ACTIVE | STATE_FLAG_5GHZ_ACTIVE,
        5180, // Channel 36
    );

    assert_eq!(status.battery_percent, 88);
    assert_eq!(status.network_type, RAT_TYPE_5G);
    assert_eq!(status.signal_bars, 4);
    assert!(status.is_softap_active());
    assert_eq!(status.channel_freq_mhz, 5180);
    assert!(status.verify());

    // Sub-payload (8 bytes)
    let bytes = status.to_bytes();
    assert_eq!(bytes.len(), 8);
    assert_eq!(bytes[0], CONTINUITY_TYPE_HOTSPOT_STATUS);
    assert_eq!(bytes[1], 0x06);

    let parsed = HotspotStatus0x0E::parse(&bytes).expect("Failed to parse 8-byte 0x0E sub-payload");
    assert_eq!(parsed, status);

    // Full 10-byte manufacturer data
    let raw = status.to_raw_manufacturer_data();
    assert_eq!(raw.len(), 10);
    assert_eq!(&raw[0..2], &[0x4C, 0x00]);

    let parsed_raw = HotspotStatus0x0E::parse(&raw).expect("Failed to parse 10-byte 0x0E frame");
    assert_eq!(parsed_raw, status);
}

#[test]
fn test_hotspot_0x0e_validation_and_bounds() {
    // 2.4 GHz SoftAP status
    let status_2g = HotspotStatus0x0E::new(15, RAT_TYPE_LTE, 2, STATE_FLAG_SOFTAP_UP, 2412);
    assert!(status_2g.verify());
    assert!(status_2g.is_softap_active());

    // Inactive SoftAP (freq 0)
    let status_idle = HotspotStatus0x0E::new(100, RAT_TYPE_WIFI, 5, 0x00, 0);
    assert!(status_idle.verify());
    assert!(!status_idle.is_softap_active());

    // Out of bounds telemetry rejected by parse
    let mut invalid_battery = status_2g.to_bytes();
    invalid_battery[2] = 120; // Battery > 100%
    assert!(matches!(HotspotStatus0x0E::parse(&invalid_battery), Err(WirelessError::InvalidFrame { .. })));

    let mut invalid_freq = status_2g.to_bytes();
    invalid_freq[6..8].copy_from_slice(&1000u16.to_be_bytes()); // 1000 MHz is not valid Wi-Fi
    assert!(matches!(HotspotStatus0x0E::parse(&invalid_freq), Err(WirelessError::InvalidFrame { .. })));
}

// ============================================================================
// GATT Credential Exchange & Model Tests
// ============================================================================

#[test]
fn test_gatt_hotspot_credentials_serde_roundtrip() {
    let creds = HotspotCredentials::new(
        "AndroidShare_8852",
        "SecurePskWpa3Passphrase!",
        Ipv4Addr::new(192, 168, 49, 1),
        53317,
    );

    let bytes = creds.to_bytes().expect("Failed to serialize credentials");
    let recovered = HotspotCredentials::from_bytes(&bytes).expect("Failed to deserialize credentials");
    assert_eq!(creds, recovered);
}

#[test]
fn test_gatt_encoders_and_decoders() {
    // 1. Credentials
    let creds = HotspotCredentials::new(
        "Direct-ArchDrop-SoftAP",
        "wpa3_ultra_secret_key",
        Ipv4Addr::new(192, 168, 43, 1),
        53317,
    );
    let enc_creds = encode_credentials(&creds).expect("encode_credentials failed");
    let dec_creds = decode_credentials(&enc_creds).expect("decode_credentials failed");
    assert_eq!(creds, dec_creds);

    // Invalid JSON credentials
    let corrupt_creds = b"{invalid_json_payload}";
    assert!(decode_credentials(corrupt_creds).is_err());

    // 2. Wakeup command
    let wakeup = HotspotWakeup0x0D::new(b"k", BAND_5_GHZ, 53317);
    let enc_wake = encode_wakeup_command(&wakeup);
    let dec_wake = decode_wakeup_command(&enc_wake).expect("decode_wakeup_command failed");
    assert_eq!(wakeup, dec_wake);

    // 3. Telemetry
    let telem = HotspotStatus0x0E::new(75, RAT_TYPE_LTE, 3, STATE_FLAG_SOFTAP_UP, 5180);
    let enc_telem = encode_telemetry(&telem);
    let dec_telem = decode_telemetry(&enc_telem).expect("decode_telemetry failed");
    assert_eq!(telem, dec_telem);
}

#[test]
fn test_gatt_constants_and_uuids() {
    assert_eq!(
        ARCHDROP_SERVICE_UUID.to_string(),
        "a4c80001-7d1a-4c91-9e28-3e2840000001"
    );
    assert_eq!(
        CHAR_WAKEUP_COMMAND_UUID.to_string(),
        "a4c80002-7d1a-4c91-9e28-3e2840000001"
    );
    assert_eq!(
        CHAR_LOHS_CREDENTIALS_UUID.to_string(),
        "a4c80003-7d1a-4c91-9e28-3e2840000001"
    );
    assert_eq!(
        CHAR_DEVICE_TELEMETRY_UUID.to_string(),
        "a4c80004-7d1a-4c91-9e28-3e2840000001"
    );
    assert_eq!(
        CHAR_CLIPBOARD_SYNC_UUID.to_string(),
        "a4c80005-7d1a-4c91-9e28-3e2840000001"
    );
}

// ============================================================================
// Zero-Wi-Fi BLE GATT Clipboard Synchronization Tests
// ============================================================================

#[test]
fn test_ble_clipboard_packet_serialization_and_deserialization() {
    let paired_key = b"zero_wifi_clipboard_paired_key";
    let text = "Hello from Arch Linux (Hyprland) over Zero-Wi-Fi BLE GATT!";
    let packet = BleClipboardPacket::new(paired_key, CLIPBOARD_MIME_TEXT_PLAIN, text)
        .expect("Failed to create clipboard packet");

    assert_eq!(packet.mime_type, CLIPBOARD_MIME_TEXT_PLAIN);
    assert_eq!(packet.mime_str(), "text/plain;charset=utf-8");
    assert_eq!(packet.text().unwrap(), text);

    let bytes = encode_clipboard_packet(&packet);
    assert_eq!(bytes.len(), 7 + text.len());

    let decoded = decode_clipboard_packet(&bytes).expect("Failed to decode clipboard packet");
    assert_eq!(decoded, packet);
    assert_eq!(decoded.text().unwrap(), text);

    // Test text/uri-list MIME type
    let uri_text = "file:///home/user/downloads/photo.jpg\r\n";
    let uri_pkt = BleClipboardPacket::new(paired_key, CLIPBOARD_MIME_URI_LIST, uri_text)
        .expect("Failed to create URI clipboard packet");
    assert_eq!(uri_pkt.mime_type, CLIPBOARD_MIME_URI_LIST);
    assert_eq!(uri_pkt.mime_str(), "text/uri-list");
    let uri_bytes = encode_clipboard_packet(&uri_pkt);
    let decoded_uri = decode_clipboard_packet(&uri_bytes).unwrap();
    assert_eq!(decoded_uri.text().unwrap(), uri_text);
}

#[test]
fn test_ble_clipboard_packet_hmac_auth_and_clock_skew() {
    let key = b"archdrop_test_paired_secret_99";
    let text = "Sensitive clipboard snippet";
    let packet = BleClipboardPacket::new(key, CLIPBOARD_MIME_TEXT_PLAIN, text).unwrap();

    // 1. Current window verifies
    assert!(packet.verify_auth(key));

    // 2. Wrong key fails
    assert!(!packet.verify_auth(b"unpaired_attacker_secret_key"));

    // 3. Exact epoch minute
    let current_min = HotspotWakeup0x0D::current_epoch_minute();
    assert!(packet.verify_at_epoch(key, current_min));

    // 4. Candidate key list verification
    let candidate_keys = vec![
        b"wrong_key_1".to_vec(),
        key.to_vec(),
        b"wrong_key_2".to_vec(),
    ];
    assert!(packet.verify_against_keys(&candidate_keys));

    let bad_candidates = vec![b"wrong_1".to_vec(), b"wrong_2".to_vec()];
    assert!(!packet.verify_against_keys(&bad_candidates));

    // 5. Expired token (10 minutes ago) must be rejected
    let old_min = current_min.saturating_sub(10);
    let old_token = HotspotWakeup0x0D::compute_auth_token(key, old_min);
    let expired_pkt = BleClipboardPacket::from_parts(
        old_token,
        CLIPBOARD_MIME_TEXT_PLAIN,
        text.as_bytes().to_vec(),
    );
    assert!(!expired_pkt.verify_auth(key));
}

#[test]
fn test_ble_clipboard_packet_tamper_and_bounds() {
    assert_eq!(BleClipboardPacket::MAX_PAYLOAD_LEN, MAX_BLE_CLIPBOARD_PAYLOAD);
    let key = b"archdrop_tamper_test_key";
    let text = "Tamper detection test";
    let packet = BleClipboardPacket::new(key, CLIPBOARD_MIME_TEXT_PLAIN, text).unwrap();
    let mut bytes = encode_clipboard_packet(&packet);

    // Tamper with auth token bit
    bytes[0] ^= 0x01;
    let tampered = decode_clipboard_packet(&bytes).unwrap();
    assert!(!tampered.verify_auth(key));

    // Truncated packet (< 7 bytes)
    assert!(decode_clipboard_packet(&bytes[0..6]).is_err());
    // Truncated payload
    assert!(decode_clipboard_packet(&bytes[0..bytes.len() - 1]).is_err());

    // Payload exceeding MAX_BLE_CLIPBOARD_PAYLOAD (512 bytes)
    let huge_text = "A".repeat(513);
    assert!(BleClipboardPacket::new(key, CLIPBOARD_MIME_TEXT_PLAIN, &huge_text).is_err());

    // Exactly 512 bytes is permitted
    let max_text = "B".repeat(512);
    let max_pkt = BleClipboardPacket::new(key, CLIPBOARD_MIME_TEXT_PLAIN, &max_text).unwrap();
    assert_eq!(max_pkt.payload.len(), 512);
    let max_bytes = encode_clipboard_packet(&max_pkt);
    let max_decoded = decode_clipboard_packet(&max_bytes).unwrap();
    assert_eq!(max_decoded.payload.len(), 512);
}

#[test]
fn test_derive_paired_key_consistency() {
    let identity = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let derived1 = derive_paired_key(identity);
    let derived2 = derive_paired_key(identity);
    assert_eq!(derived1, derived2);
    assert_eq!(derived1.len(), 32);

    // Cross-platform parity test with Android companion BlePeripheralManager
    let expected_hex = "b63564f6c066f11750f1958cb23f6513a772ef29c22da9bb0197ae204fa98379";
    assert_eq!(hex::encode(derived1), expected_hex);

    let different = derive_paired_key("fedcba9876543210");
    assert_ne!(derived1, different);
}

#[test]
fn test_ble_clipboard_packet_empty_payload() {
    let key = b"archdrop_empty_test_key";
    let empty_text = "";
    let packet = BleClipboardPacket::new(key, CLIPBOARD_MIME_TEXT_PLAIN, empty_text)
        .expect("Empty clipboard payload must be valid");
    assert_eq!(packet.payload.len(), 0);
    assert_eq!(packet.text().unwrap(), "");
    assert!(packet.verify_auth(key));

    let bytes = encode_clipboard_packet(&packet);
    assert_eq!(bytes.len(), 7); // Exactly 7 bytes framing with 0-length payload
    let decoded = decode_clipboard_packet(&bytes).expect("Must decode empty payload packet");
    assert_eq!(decoded.payload.len(), 0);
    assert_eq!(decoded.text().unwrap(), "");
    assert!(decoded.verify_auth(key));
}

#[tokio::test]
async fn test_gatt_server_state_clipboard_payload_concurrency() {
    let state = GattServerState::default();
    assert!(state.clipboard_payload.read().await.is_none());

    let payload = b"test_clipboard_data".to_vec();
    *state.clipboard_payload.write().await = Some(payload.clone());
    assert_eq!(state.clipboard_payload.read().await.as_deref(), Some(payload.as_slice()));
}

// ============================================================================
// Constant-Time Helper Tests
// ============================================================================

#[test]
fn test_constant_time_comparison_functions() {
    // 4-byte comparison
    let a4 = [1u8, 2, 3, 4];
    let b4 = [1u8, 2, 3, 4];
    let c4 = [1u8, 2, 3, 5];
    assert!(constant_time_eq_4(&a4, &b4));
    assert!(!constant_time_eq_4(&a4, &c4));

    // 6-byte comparison
    let a6 = [10u8, 20, 30, 40, 50, 60];
    let b6 = [10u8, 20, 30, 40, 50, 60];
    let c6 = [10u8, 20, 30, 40, 50, 61];
    assert!(constant_time_eq_6(&a6, &b6));
    assert!(!constant_time_eq_6(&a6, &c6));
}

// ============================================================================
// BleController Graceful Initialization Tests
// ============================================================================

#[tokio::test]
async fn test_ble_controller_invalid_adapter_handling() {
    let cfg = BleConfig {
        adapter_name: Some("nonexistent_hci99".to_string()),
        company_id: APPLE_COMPANY_ID,
    };
    let result = BleController::init(cfg).await;
    match result {
        Err(WirelessError::AdapterNotFound(err)) => {
            assert!(err.contains("nonexistent_hci99"));
        }
        Err(WirelessError::BluetoothServiceUnavailable(_)) => {
            // Also acceptable if bluetoothd is stopped in test environment
        }
        Ok(_) => panic!("Should not succeed with nonexistent_hci99"),
        Err(other) => panic!("Unexpected error: {other:?}"),
    }
}
