//! Adversarial stress tests for Arch Drop Wireless:
//! 1. HotspotCredentials JSON serialization/deserialization across extreme inputs
//! 2. Rolling epoch token verification across time boundaries (+/- 2 min window, extremes, replay)
//! 3. BleController initialization and graceful error handling
//! 4. High concurrency stress on GattServerState RwLocks and GATT application read handlers
//! 5. Host WireGuard interface safety verification

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use archdrop_wireless::ble::continuity::hotspot_0x0d::{
    HotspotStatus0x0E, HotspotWakeup0x0D, BAND_2_4_GHZ, BAND_5_GHZ, FLAG_WAKEUP_5GHZ_PREFERRED,
    FLAG_WAKEUP_START_LOHS, RAT_TYPE_5G, STATE_FLAG_5GHZ_ACTIVE, STATE_FLAG_SOFTAP_UP,
    STATE_FLAG_WPA3_ACTIVE,
};
use archdrop_wireless::ble::continuity::proximity_0x05::APPLE_COMPANY_ID;
use archdrop_wireless::ble::gatt::{
    build_gatt_application, decode_credentials, decode_telemetry, encode_credentials,
    encode_telemetry, GattServerState, CHAR_LOHS_CREDENTIALS_UUID,
};
use archdrop_wireless::ble::types::{BleConfig, HotspotCredentials};
use archdrop_wireless::ble::BleController;
use archdrop_wireless::error::WirelessError;

// ============================================================================
// 1. HotspotCredentials Serialization / Deserialization Stress Tests
// ============================================================================

#[test]
fn test_adversarial_credentials_boundary_ports() {
    let base_ip = Ipv4Addr::new(192, 168, 49, 1);

    // 1. Port 1 (Minimum valid port)
    let creds_p1 = HotspotCredentials::new("SSID_P1", "Pass_P1", base_ip, 1);
    let bytes_p1 = encode_credentials(&creds_p1).expect("Failed to encode port 1");
    let dec_p1 = decode_credentials(&bytes_p1).expect("Failed to decode port 1");
    assert_eq!(dec_p1.port, 1);
    assert_eq!(dec_p1, creds_p1);

    // 2. Port 65535 (Maximum 16-bit port)
    let creds_pmax = HotspotCredentials::new("SSID_PMAX", "Pass_PMAX", base_ip, 65535);
    let bytes_pmax = encode_credentials(&creds_pmax).expect("Failed to encode port 65535");
    let dec_pmax = decode_credentials(&bytes_pmax).expect("Failed to decode port 65535");
    assert_eq!(dec_pmax.port, 65535);
    assert_eq!(dec_pmax, creds_pmax);

    // 3. Port 0 (Reserved port)
    let creds_p0 = HotspotCredentials::new("SSID_P0", "Pass_P0", base_ip, 0);
    let bytes_p0 = encode_credentials(&creds_p0).expect("Failed to encode port 0");
    let dec_p0 = decode_credentials(&bytes_p0).expect("Failed to decode port 0");
    assert_eq!(dec_p0.port, 0);
    assert_eq!(dec_p0, creds_p0);

    // 4. Port overflow (65536) in raw JSON -> must fail
    let json_overflow =
        br#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.49.1","port":65536}"#;
    assert!(decode_credentials(json_overflow).is_err());

    // 5. Negative port (-1) in raw JSON -> must fail
    let json_negative = br#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.49.1","port":-1}"#;
    assert!(decode_credentials(json_negative).is_err());

    // 6. Port as string ("53317") -> must fail (type confusion)
    let json_string_port =
        br#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.49.1","port":"53317"}"#;
    assert!(decode_credentials(json_string_port).is_err());

    // 7. Float port (53317.5) -> must fail
    let json_float_port =
        br#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.49.1","port":53317.5}"#;
    assert!(decode_credentials(json_float_port).is_err());
}

#[test]
fn test_adversarial_credentials_extreme_ssids() {
    let base_ip = Ipv4Addr::new(192, 168, 49, 1);

    // 1. Empty SSID
    let empty_creds = HotspotCredentials::new("", "password123", base_ip, 53317);
    let bytes = encode_credentials(&empty_creds).expect("encode empty SSID failed");
    let dec = decode_credentials(&bytes).expect("decode empty SSID failed");
    assert_eq!(dec.ssid, "");

    // 2. Exactly 32-byte SSID (standard 802.11 maximum)
    let ssid_32 = "A".repeat(32);
    let creds_32 = HotspotCredentials::new(&ssid_32, "password123", base_ip, 53317);
    let bytes = encode_credentials(&creds_32).expect("encode 32B SSID failed");
    let dec = decode_credentials(&bytes).expect("decode 32B SSID failed");
    assert_eq!(dec.ssid, ssid_32);

    // 3. Very long SSID (256 bytes)
    let ssid_256 = "B".repeat(256);
    let creds_256 = HotspotCredentials::new(&ssid_256, "password123", base_ip, 53317);
    let bytes = encode_credentials(&creds_256).expect("encode 256B SSID failed");
    let dec = decode_credentials(&bytes).expect("decode 256B SSID failed");
    assert_eq!(dec.ssid, ssid_256);

    // 4. Giant SSID (10,000 bytes) - memory allocation stress
    let ssid_10k = "Z".repeat(10_000);
    let creds_10k = HotspotCredentials::new(&ssid_10k, "password123", base_ip, 53317);
    let bytes = encode_credentials(&creds_10k).expect("encode 10k SSID failed");
    let dec = decode_credentials(&bytes).expect("decode 10k SSID failed");
    assert_eq!(dec.ssid.len(), 10_000);

    // 5. Embedded null bytes in SSID
    let ssid_null = "ArchDrop\0Hidden\0SoftAP";
    let creds_null = HotspotCredentials::new(ssid_null, "password123", base_ip, 53317);
    let bytes = encode_credentials(&creds_null).expect("encode null-byte SSID failed");
    let dec = decode_credentials(&bytes).expect("decode null-byte SSID failed");
    assert_eq!(dec.ssid, ssid_null);

    // 6. Terminal and ANSI escape sequences
    let ssid_ansi = "\x1b[31;1mArchDrop\x1b[0m\r\n\t";
    let creds_ansi = HotspotCredentials::new(ssid_ansi, "password123", base_ip, 53317);
    let bytes = encode_credentials(&creds_ansi).expect("encode ANSI SSID failed");
    let dec = decode_credentials(&bytes).expect("decode ANSI SSID failed");
    assert_eq!(dec.ssid, ssid_ansi);
}

#[test]
fn test_adversarial_credentials_utf8_passphrases() {
    let base_ip = Ipv4Addr::new(192, 168, 49, 1);

    // 1. High Unicode Emojis & multi-byte UTF-8
    let pass_unicode = "🦀 Rust ⚡ AirDrop 🔥 100% P2P 🚀 🌈";
    let creds_u = HotspotCredentials::new("SSID_UTF8", pass_unicode, base_ip, 53317);
    let bytes = encode_credentials(&creds_u).expect("encode unicode pass failed");
    let dec = decode_credentials(&bytes).expect("decode unicode pass failed");
    assert_eq!(dec.passphrase, pass_unicode);

    // 2. Multilingual scripts (CJK, Arabic RTL, Devanagari, Cyrillic)
    let pass_multi = "日本語パスワード_كلمة_السر_गुप्त_पासवर्ड_пароль_12345";
    let creds_m = HotspotCredentials::new("SSID_MULTI", pass_multi, base_ip, 53317);
    let bytes = encode_credentials(&creds_m).expect("encode multi pass failed");
    let dec = decode_credentials(&bytes).expect("decode multi pass failed");
    assert_eq!(dec.passphrase, pass_multi);

    // 3. Quotes, Backslashes, JSON Delimiters
    let pass_escapes = r#"\"'\\/\b\f\n\r\t{}[]:,"#;
    let creds_e = HotspotCredentials::new("SSID_ESC", pass_escapes, base_ip, 53317);
    let bytes = encode_credentials(&creds_e).expect("encode escapes pass failed");
    let dec = decode_credentials(&bytes).expect("decode escapes pass failed");
    assert_eq!(dec.passphrase, pass_escapes);

    // 4. SQL / Shell Injection Patterns
    let pass_injection = "'; DROP TABLE credentials; -- $(reboot) `rm -rf /` | cat /etc/shadow";
    let creds_inj = HotspotCredentials::new("SSID_INJ", pass_injection, base_ip, 53317);
    let bytes = encode_credentials(&creds_inj).expect("encode injection pass failed");
    let dec = decode_credentials(&bytes).expect("decode injection pass failed");
    assert_eq!(dec.passphrase, pass_injection);

    // 5. Giant passphrase (8,192 bytes)
    let pass_8k = "P".repeat(8192);
    let creds_8k = HotspotCredentials::new("SSID_8K", &pass_8k, base_ip, 53317);
    let bytes = encode_credentials(&creds_8k).expect("encode 8k pass failed");
    let dec = decode_credentials(&bytes).expect("decode 8k pass failed");
    assert_eq!(dec.passphrase.len(), 8192);

    // 6. Empty passphrase
    let creds_empty = HotspotCredentials::new("SSID_OPEN", "", base_ip, 53317);
    let bytes = encode_credentials(&creds_empty).expect("encode empty pass failed");
    let dec = decode_credentials(&bytes).expect("decode empty pass failed");
    assert_eq!(dec.passphrase, "");
}

#[test]
fn test_adversarial_credentials_unusual_gateway_ips() {
    let test_ips = [
        Ipv4Addr::new(0, 0, 0, 0),         // Unspecified
        Ipv4Addr::new(255, 255, 255, 255), // Broadcast
        Ipv4Addr::new(127, 0, 0, 1),       // Loopback
        Ipv4Addr::new(224, 0, 0, 1),       // Multicast
        Ipv4Addr::new(10, 2, 0, 2),        // Host WireGuard IP
        Ipv4Addr::new(192, 168, 49, 1),    // Android LOHS default
        Ipv4Addr::new(192, 168, 43, 1),    // Android Tethering default
        Ipv4Addr::new(169, 254, 1, 1),     // Link-local
    ];

    for ip in test_ips {
        let creds = HotspotCredentials::new("SSID_IP", "pass", ip, 53317);
        let bytes = encode_credentials(&creds).expect("encode IP failed");
        let dec = decode_credentials(&bytes).expect("decode IP failed");
        assert_eq!(dec.gateway_ip, ip);
    }

    // Malformed IP strings in raw JSON
    let invalid_ips = [
        r#"{"ssid":"s","passphrase":"p","gateway_ip":"256.0.0.1","port":53317}"#,
        r#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.1","port":53317}"#,
        r#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.1.1.1","port":53317}"#,
        r#"{"ssid":"s","passphrase":"p","gateway_ip":"","port":53317}"#,
        r#"{"ssid":"s","passphrase":"p","gateway_ip":"::1","port":53317}"#,
        r#"{"ssid":"s","passphrase":"p","gateway_ip":"fe80::1","port":53317}"#,
        r#"{"ssid":"s","passphrase":"p","gateway_ip":1234567,"port":53317}"#,
        r#"{"ssid":"s","passphrase":"p","gateway_ip":null,"port":53317}"#,
    ];

    for raw in invalid_ips {
        let res = decode_credentials(raw.as_bytes());
        assert!(
            res.is_err(),
            "Expected error for invalid IP raw JSON: {raw}"
        );
    }
}

#[test]
fn test_adversarial_credentials_malformed_json_fuzzing() {
    // 1. Empty slice
    assert!(decode_credentials(&[]).is_err());

    // 2. Incomplete / Truncated JSON
    assert!(decode_credentials(b"{\"ssid\": \"test\"").is_err());
    assert!(decode_credentials(b"{\"ssid\": \"test\", \"passphrase\"").is_err());

    // 3. Missing required fields
    let missing_ssid = br#"{"passphrase":"p","gateway_ip":"192.168.49.1","port":53317}"#;
    assert!(decode_credentials(missing_ssid).is_err());

    let missing_pass = br#"{"ssid":"s","gateway_ip":"192.168.49.1","port":53317}"#;
    assert!(decode_credentials(missing_pass).is_err());

    let missing_ip = br#"{"ssid":"s","passphrase":"p","port":53317}"#;
    assert!(decode_credentials(missing_ip).is_err());

    let missing_port = br#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.49.1"}"#;
    assert!(decode_credentials(missing_port).is_err());

    // 4. Binary garbage
    let garbage = [0xFF, 0xFE, 0x00, 0x12, 0x88, 0x99, 0xAA, 0xBB];
    assert!(decode_credentials(&garbage).is_err());

    // 5. Deeply nested object instead of string
    let nested = br#"{"ssid":{"nested":"object"},"passphrase":"p","gateway_ip":"192.168.49.1","port":53317}"#;
    assert!(decode_credentials(nested).is_err());

    // 6. Array instead of object
    let array_json = br#"[1, 2, 3, 4, 5]"#;
    assert!(decode_credentials(array_json).is_err());

    // 7. Extra forward-compatible fields (serde default ignores extra fields)
    let extra_fields = br#"{"ssid":"s","passphrase":"p","gateway_ip":"192.168.49.1","port":53317,"future_field":true,"vendor_id":123}"#;
    let res = decode_credentials(extra_fields)
        .expect("Serde should gracefully ignore extra forward-compatible fields");
    assert_eq!(res.ssid, "s");
    assert_eq!(res.port, 53317);
}

// ============================================================================
// 2. Rolling Epoch Token Verification Across Time Boundaries Stress Tests
// ============================================================================

#[test]
fn test_adversarial_epoch_token_window_boundaries() {
    let key = b"adversarial_test_epoch_signing_key_secret_2026";
    let current_minute = HotspotWakeup0x0D::current_epoch_minute();

    // The specification allows +/- 2 minutes tolerance (5-minute sliding window).
    // Test minutes from -5 to +5 relative to current_minute.
    for delta in -5i64..=5i64 {
        let test_minute = (current_minute as i64 + delta).max(0) as u64;
        let token = HotspotWakeup0x0D::compute_auth_token(key, test_minute);
        let wakeup = HotspotWakeup0x0D::from_parts(
            FLAG_WAKEUP_START_LOHS | FLAG_WAKEUP_5GHZ_PREFERRED,
            BAND_5_GHZ,
            token,
            53317,
        );

        let verified = wakeup.verify(key);
        if (-2..=2).contains(&delta) {
            assert!(
                verified,
                "Token for delta {delta} (minute {test_minute}) should verify within +/- 2 min window of {current_minute}"
            );
        } else {
            assert!(
                !verified,
                "Token for delta {delta} (minute {test_minute}) MUST NOT verify outside +/- 2 min window of {current_minute}"
            );
        }
    }
}

#[test]
fn test_adversarial_epoch_token_exact_verification() {
    let key = b"adversarial_exact_epoch_key";
    let target_minute = 12_345_678u64;
    let token = HotspotWakeup0x0D::compute_auth_token(key, target_minute);
    let wakeup = HotspotWakeup0x0D::from_parts(FLAG_WAKEUP_START_LOHS, BAND_2_4_GHZ, token, 53317);

    // Exact match must be true
    assert!(wakeup.verify_at_epoch(key, target_minute));

    // Even 1 minute offset must fail verify_at_epoch
    assert!(!wakeup.verify_at_epoch(key, target_minute - 1));
    assert!(!wakeup.verify_at_epoch(key, target_minute + 1));
}

#[test]
fn test_adversarial_epoch_token_historical_and_future_extremes() {
    let key = b"extremes_signing_key_epoch_boundaries";

    // 1. Unix epoch minute 0 (1970-01-01 00:00:00 UTC)
    let token_0 = HotspotWakeup0x0D::compute_auth_token(key, 0);
    let wakeup_0 =
        HotspotWakeup0x0D::from_parts(FLAG_WAKEUP_START_LOHS, BAND_5_GHZ, token_0, 53317);
    assert!(wakeup_0.verify_at_epoch(key, 0));
    assert!(!wakeup_0.verify_at_epoch(key, 1));

    // 2. Year 2038 epoch (35,781,120 minutes)
    let min_2038 = 35_781_120u64;
    let token_2038 = HotspotWakeup0x0D::compute_auth_token(key, min_2038);
    let wakeup_2038 =
        HotspotWakeup0x0D::from_parts(FLAG_WAKEUP_START_LOHS, BAND_5_GHZ, token_2038, 53317);
    assert!(wakeup_2038.verify_at_epoch(key, min_2038));
    assert!(!wakeup_2038.verify_at_epoch(key, min_2038 + 1));

    // 3. Year 2100 epoch (68,377,920 minutes)
    let min_2100 = 68_377_920u64;
    let token_2100 = HotspotWakeup0x0D::compute_auth_token(key, min_2100);
    let wakeup_2100 =
        HotspotWakeup0x0D::from_parts(FLAG_WAKEUP_START_LOHS, BAND_5_GHZ, token_2100, 53317);
    assert!(wakeup_2100.verify_at_epoch(key, min_2100));

    // 4. Stale replay attempts (10 mins, 60 mins, 1 day ago)
    let current_min = HotspotWakeup0x0D::current_epoch_minute();
    for stale_offset in [10u64, 60, 1440, 100_000] {
        let stale_min = current_min.saturating_sub(stale_offset);
        let stale_token = HotspotWakeup0x0D::compute_auth_token(key, stale_min);
        let stale_wakeup =
            HotspotWakeup0x0D::from_parts(FLAG_WAKEUP_START_LOHS, BAND_5_GHZ, stale_token, 53317);
        assert!(
            !stale_wakeup.verify(key),
            "Replayed token from {stale_offset} minutes ago MUST be rejected"
        );
    }
}

#[test]
fn test_adversarial_epoch_token_forgery_and_key_isolation() {
    let key_alice = b"alice_private_master_key_32_bytes!!";
    let key_bob = b"bob_private_master_key_32_bytes!!!";

    let wakeup_alice = HotspotWakeup0x0D::new(key_alice, BAND_5_GHZ, 53317);

    // Alice token verifies with Alice key
    assert!(wakeup_alice.verify(key_alice));

    // Alice token MUST NOT verify with Bob key
    assert!(!wakeup_alice.verify(key_bob));

    // Key with single bit flipped MUST NOT verify
    let mut key_tampered = key_alice.to_vec();
    key_tampered[0] ^= 0x01;
    assert!(!wakeup_alice.verify(&key_tampered));

    // Forgery stress: 10,000 synthetic random 4-byte tokens
    // Mathematical chance of false acceptance is 5 / 2^32 ≈ 1.16e-9
    let mut forged_count = 0;
    for i in 0..10_000u32 {
        let fake_token = i.to_be_bytes();
        let fake_wakeup =
            HotspotWakeup0x0D::from_parts(FLAG_WAKEUP_START_LOHS, BAND_5_GHZ, fake_token, 53317);
        if fake_wakeup.verify(key_alice) {
            forged_count += 1;
        }
    }
    assert_eq!(
        forged_count, 0,
        "10,000 forged tokens produced {forged_count} false acceptances!"
    );

    // Key length variations
    let empty_key = b"";
    let t_empty = HotspotWakeup0x0D::new(empty_key, BAND_5_GHZ, 53317);
    assert!(t_empty.verify(empty_key));

    let single_byte_key = b"x";
    let t_single = HotspotWakeup0x0D::new(single_byte_key, BAND_5_GHZ, 53317);
    assert!(t_single.verify(single_byte_key));

    let long_key = vec![0x42u8; 1024];
    let t_long = HotspotWakeup0x0D::new(&long_key, BAND_5_GHZ, 53317);
    assert!(t_long.verify(&long_key));
}

// ============================================================================
// 3. BleController Initialization and Graceful Error Handling
// ============================================================================

#[tokio::test]
async fn test_adversarial_ble_controller_adapter_error_handling() {
    // 1. Non-existent adapter "hci99"
    let cfg_bad = BleConfig {
        adapter_name: Some("hci99".to_string()),
        company_id: APPLE_COMPANY_ID,
    };
    match BleController::init(cfg_bad).await {
        Err(WirelessError::AdapterNotFound(err)) => {
            assert!(err.contains("hci99"));
        }
        Err(WirelessError::BluetoothServiceUnavailable(_)) => {
            // Also acceptable if bluetoothd is stopped
        }
        Ok(_) => panic!("Should not succeed with nonexistent adapter hci99"),
        Err(e) => panic!("Unexpected error: {e:?}"),
    }

    // 2. Malformed adapter name
    let cfg_path = BleConfig {
        adapter_name: Some("/dev/null".to_string()),
        company_id: APPLE_COMPANY_ID,
    };
    match BleController::init(cfg_path).await {
        Err(WirelessError::AdapterNotFound(_))
        | Err(WirelessError::BluetoothServiceUnavailable(_)) => {}
        Ok(_) => panic!("Should not succeed with invalid path adapter name"),
        Err(e) => panic!("Unexpected error: {e:?}"),
    }

    // 3. Empty string adapter name
    let cfg_empty = BleConfig {
        adapter_name: Some("".to_string()),
        company_id: APPLE_COMPANY_ID,
    };
    match BleController::init(cfg_empty).await {
        Err(WirelessError::AdapterNotFound(_))
        | Err(WirelessError::BluetoothServiceUnavailable(_)) => {}
        Ok(_) => panic!("Should not succeed with empty adapter name"),
        Err(e) => panic!("Unexpected error: {e:?}"),
    }
}

#[tokio::test]
async fn test_ble_controller_live_adapter_initialization_and_methods() {
    // Attempt initialization on host default adapter ("hci0")
    let cfg = BleConfig::default();
    let init_res = BleController::init(cfg).await;

    match init_res {
        Ok(mut controller) => {
            // Check that adapter is powered
            let is_powered = controller.adapter().is_powered().await;
            assert!(
                is_powered.is_ok(),
                "is_powered query failed on live adapter"
            );
            assert!(is_powered.unwrap(), "Live adapter should be powered on");

            // Check config
            assert_eq!(controller.config().company_id, APPLE_COMPANY_ID);

            // Test stop_advertising idempotence when nothing was advertising
            controller.stop_advertising();
            controller.stop_advertising();
        }
        Err(WirelessError::BluetoothServiceUnavailable(_)) => {
            println!("Note: bluetoothd service not available in current container environment");
        }
        Err(WirelessError::AdapterNotFound(_)) => {
            println!("Note: No default Bluetooth adapter found");
        }
        Err(other) => panic!("Unexpected error during live adapter init: {other:?}"),
    }
}

// ============================================================================
// 4. Concurrency & Race Condition Stress Tests
// ============================================================================

#[tokio::test]
async fn test_concurrency_gatt_server_state_rwlock_stress() {
    let state = GattServerState::default();
    let num_writers = 25;
    let num_readers = 50;
    let iterations_per_task = 200;

    let total_successful_reads = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    // Spawn writers
    for writer_id in 0..num_writers {
        let state_clone = state.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..iterations_per_task {
                let ssid = format!("AndroidShare_{writer_id}_{i}");
                let pass = format!("SecurePsk_{writer_id}_{i}!");
                let ip = Ipv4Addr::new(192, 168, 49, (writer_id % 250 + 1) as u8);
                let port = 50000 + (i % 1000) as u16;

                let creds = HotspotCredentials::new(ssid, pass, ip, port);
                {
                    let mut guard = state_clone.credentials.write().await;
                    *guard = Some(creds);
                }
                tokio::task::yield_now().await;
            }
        }));
    }

    // Spawn readers
    for _reader_id in 0..num_readers {
        let state_clone = state.clone();
        let success_counter = total_successful_reads.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..iterations_per_task {
                {
                    let guard = state_clone.credentials.read().await;
                    if let Some(ref c) = *guard {
                        // Validate internal consistency
                        assert!(c.ssid.starts_with("AndroidShare_"));
                        assert!(c.passphrase.starts_with("SecurePsk_"));
                        assert!(c.port >= 50000);
                        assert_eq!(c.gateway_ip.octets()[0..3], [192, 168, 49]);

                        // Verify encode/decode roundtrip while holding read lock
                        let bytes = encode_credentials(c).expect("encode failed");
                        let dec = decode_credentials(&bytes).expect("decode failed");
                        assert_eq!(dec, *c);

                        success_counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
                tokio::task::yield_now().await;
            }
        }));
    }

    // Wait for all 75 concurrent tasks to complete
    for handle in handles {
        handle
            .await
            .expect("Task panicked during concurrent stress");
    }

    let successes = total_successful_reads.load(Ordering::SeqCst);
    println!("Concurrency stress completed: {successes} consistent reads verified");
    assert!(
        successes > 0,
        "Expected at least some successful reads during concurrency test"
    );
}

#[tokio::test]
async fn test_concurrency_gatt_telemetry_rwlock_stress() {
    let state = GattServerState::default();
    let num_writers = 20;
    let num_readers = 30;
    let iterations = 100;

    let mut handles = Vec::new();

    for _writer_id in 0..num_writers {
        let state_clone = state.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..iterations {
                let battery = (i % 101) as u8;
                let bars = (i % 6) as u8;
                let freq = if i % 2 == 0 { 2412 } else { 5180 };
                let telem = HotspotStatus0x0E::new(
                    battery,
                    RAT_TYPE_5G,
                    bars,
                    STATE_FLAG_SOFTAP_UP | STATE_FLAG_WPA3_ACTIVE | STATE_FLAG_5GHZ_ACTIVE,
                    freq,
                );
                {
                    let mut guard = state_clone.telemetry.write().await;
                    *guard = Some(telem);
                }
                tokio::task::yield_now().await;
            }
        }));
    }

    for _ in 0..num_readers {
        let state_clone = state.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..iterations {
                {
                    let guard = state_clone.telemetry.read().await;
                    if let Some(ref t) = *guard {
                        assert!(t.verify());
                        assert!(t.is_softap_active());
                        let bytes = encode_telemetry(t);
                        let dec = decode_telemetry(&bytes).expect("decode telemetry failed");
                        assert_eq!(dec, *t);
                    }
                }
                tokio::task::yield_now().await;
            }
        }));
    }

    for handle in handles {
        handle.await.expect("Telemetry concurrent task panicked");
    }
}

#[tokio::test]
async fn test_concurrency_gatt_application_read_handlers() {
    let state = GattServerState::default();
    let app = build_gatt_application(state.clone());

    // 1. Find service and characteristics
    assert_eq!(app.services.len(), 1);
    let service = &app.services[0];
    assert_eq!(service.uuid, archdrop_wireless::ble::ARCHDROP_SERVICE_UUID);
    assert!(service.primary);

    // Locate characteristics
    let wakeup_char = service
        .characteristics
        .iter()
        .find(|c| c.uuid == archdrop_wireless::ble::CHAR_WAKEUP_COMMAND_UUID)
        .expect("Wakeup characteristic not found");
    assert!(wakeup_char.write.as_ref().unwrap().write);
    assert!(wakeup_char.write.as_ref().unwrap().write_without_response);

    let creds_char = service
        .characteristics
        .iter()
        .find(|c| c.uuid == CHAR_LOHS_CREDENTIALS_UUID)
        .expect("Credentials characteristic not found");
    assert!(creds_char.read.as_ref().unwrap().read);

    let telem_char = service
        .characteristics
        .iter()
        .find(|c| c.uuid == archdrop_wireless::ble::CHAR_DEVICE_TELEMETRY_UUID)
        .expect("Telemetry characteristic not found");
    assert!(telem_char.read.as_ref().unwrap().read);

    // 2. Concurrency test with 20 concurrent simulated GATT reads while writers update state
    let mut tasks = Vec::new();

    // 10 concurrent readers calling through state
    for _ in 0..10 {
        let state_clone = state.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..50 {
                let guard = state_clone.credentials.read().await;
                if let Some(ref c) = *guard {
                    let bytes = encode_credentials(c).unwrap();
                    let dec = decode_credentials(&bytes).unwrap();
                    assert_eq!(dec, *c);
                }
                tokio::time::sleep(Duration::from_micros(100)).await;
            }
        }));
    }

    // 2 writers updating
    for w in 0..2 {
        let state_clone = state.clone();
        tasks.push(tokio::spawn(async move {
            for i in 0..50 {
                let c = HotspotCredentials::new(
                    format!("Rotated_{w}_{i}"),
                    "pass",
                    Ipv4Addr::new(192, 168, 49, 1),
                    53317,
                );
                let mut guard = state_clone.credentials.write().await;
                *guard = Some(c);
                tokio::time::sleep(Duration::from_micros(150)).await;
            }
        }));
    }

    for task in tasks {
        task.await.expect("Application read task panicked");
    }
}

// ============================================================================
// 5. Host WireGuard Invariant Safety Verification
// ============================================================================

#[test]
fn test_host_wireguard_safety_invariant() {
    use std::process::Command;

    // Verify tiktok interface exists and has IP 10.2.0.2/32
    let ip_out = Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output()
        .expect("Failed to execute ip addr");

    assert!(ip_out.status.success(), "Interface tiktok must be active");
    let ip_str = String::from_utf8_lossy(&ip_out.stdout);
    assert!(
        ip_str.contains("10.2.0.2"),
        "Interface tiktok must retain IP 10.2.0.2: {ip_str}"
    );

    // Verify routing table 51820 has default dev tiktok
    let route_out = Command::new("ip")
        .args(["route", "show", "table", "51820"])
        .output()
        .expect("Failed to execute ip route");

    assert!(
        route_out.status.success(),
        "Routing table 51820 query failed"
    );
    let route_str = String::from_utf8_lossy(&route_out.stdout);
    assert!(
        route_str.contains("default dev tiktok"),
        "Routing table 51820 must contain default dev tiktok: {route_str}"
    );
}
