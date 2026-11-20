//! Comprehensive End-to-End (E2E) Integration Test for Arch Drop Phase 2
//!
//! Validates:
//! 1. BLE 0x05 Proximity Presence Beacon and 0x0D Hotspot Wakeup / 0x0E Status frame generation & parsing.
//! 2. NetworkManager transient volatile profile dictionary generation and safety invariant enforcement.
//! 3. Multi-tier dynamic gateway IP resolution logic across IP4Config, DHCP options, and subnet fallback.
//! 4. Full LocalSend v2 HTTP streaming file transfer with SHA-256 validation to simulated peer gateway IP.
//! 5. Host WireGuard interface `tiktok` (`10.2.0.2/32`) and routing table 51820 safety invariants.

use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_transfer::{SessionManager, TransferClient, TransferServer};
use archdrop_wireless::ble::{
    HotspotStatus0x0E, HotspotWakeup0x0D, ProximityBeacon0x05, APPLE_COMPANY_ID, BAND_2_4_GHZ,
    BAND_5_GHZ, CONTINUITY_TYPE_AIRDROP, CONTINUITY_TYPE_HOTSPOT_STATUS,
    CONTINUITY_TYPE_HOTSPOT_WAKEUP, DEFAULT_TRANSFER_PORT, FLAG_WAKEUP_5GHZ_PREFERRED,
    FLAG_WAKEUP_START_LOHS, RAT_TYPE_5G, STATE_FLAG_5GHZ_ACTIVE, STATE_FLAG_SOFTAP_UP,
};
use archdrop_wireless::error::WirelessError;
use archdrop_wireless::nm::{
    calculate_subnet_gateway, parse_address_data, parse_dhcp_options, resolve_gateway_from_data,
    ConnectionProfileBuilder, LohsConnectionConfig, SecurityType,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use zbus::zvariant::{OwnedValue, Value};

fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// =========================================================================
// 1. BLE Continuity Frames (0x05 Proximity, 0x0D Wakeup, 0x0E Status)
// =========================================================================

#[test]
fn test_ble_0x05_proximity_frame_generation_and_parsing() {
    let contact_key = b"TEST_SHARED_CONTACT_KEY_32BYTES!";
    let salt: u16 = 0x4A1F;

    // 1. Generate Proximity Beacon
    let beacon = ProximityBeacon0x05::new(contact_key, salt, 0x01);
    assert_eq!(beacon.salt, salt);
    assert_eq!(beacon.flags, 0x01);

    // Verify rolling HMAC verification
    assert!(
        beacon.verify_match(&[contact_key]),
        "Beacon must verify against the matching contact key"
    );
    assert!(
        !beacon.verify_match(&[b"WRONG_CONTACT_KEY_DIFFERENT_32!"]),
        "Beacon must reject non-matching contact keys"
    );

    // 2. Raw Manufacturer Data Serialization
    let raw_bytes = beacon.to_raw_manufacturer_data();
    assert_eq!(
        raw_bytes.len(),
        22,
        "0x05 proximity manufacturer data must be exactly 22 bytes"
    );

    // Company ID: 0x004C (Apple, Little-Endian: 0x4C, 0x00)
    assert_eq!(raw_bytes[0], 0x4C);
    assert_eq!(raw_bytes[1], 0x00);
    assert_eq!(raw_bytes[2], CONTINUITY_TYPE_AIRDROP); // 0x05
    assert_eq!(raw_bytes[3], 0x12); // Length of subpayload = 18 bytes
    assert_eq!(&raw_bytes[4..12], &[0x00; 8]); // Reserved zero padding
    assert_eq!(raw_bytes[12], 0x01); // Version/flags
    assert_eq!(u16::from_be_bytes([raw_bytes[13], raw_bytes[14]]), salt);
    assert_eq!(raw_bytes[21], 0x00); // Trailing zero suffix

    // 3. Deserialization from raw manufacturer data
    let parsed = ProximityBeacon0x05::parse(&raw_bytes)
        .expect("Valid 22-byte 0x05 frame should parse successfully");
    assert_eq!(parsed.salt, salt);
    assert_eq!(parsed.truncated_hashes, beacon.truncated_hashes);
    assert!(parsed.verify_match(&[contact_key]));

    // 4. BlueZ Bluer Manufacturer Data format (u16, Vec<u8>)
    let (cid, subpayload) = beacon.to_bluer_payload();
    assert_eq!(cid, APPLE_COMPANY_ID);
    assert_eq!(subpayload.len(), 20); // 22 bytes minus 2-byte Company ID
    assert_eq!(subpayload[0], CONTINUITY_TYPE_AIRDROP);

    let parsed_bluer = ProximityBeacon0x05::parse(&subpayload)
        .expect("Valid 20-byte subpayload should parse successfully");
    assert_eq!(parsed_bluer.salt, salt);
    assert_eq!(parsed_bluer.truncated_hashes, beacon.truncated_hashes);

    // 5. Tampered Frame Rejection
    let mut corrupted = raw_bytes;
    corrupted[0] = 0xFF; // Bad company ID
    assert!(
        matches!(
            ProximityBeacon0x05::parse(&corrupted),
            Err(WirelessError::InvalidCompanyId(0xFF))
        ),
        "Must reject non-Apple company IDs"
    );
}

#[test]
fn test_ble_0x0d_wakeup_and_0x0e_status_generation_and_parsing() {
    let auth_secret = b"ARCHDROP_LOHS_WAKEUP_AUTH_SECRET";

    // 1. Generate 0x0D Hotspot Wakeup Frame for 5 GHz
    let wakeup_5g = HotspotWakeup0x0D::new(
        auth_secret,
        BAND_5_GHZ,
        DEFAULT_TRANSFER_PORT,
    );

    assert_eq!(wakeup_5g.control_flags, FLAG_WAKEUP_START_LOHS | FLAG_WAKEUP_5GHZ_PREFERRED);
    assert_eq!(wakeup_5g.target_band, BAND_5_GHZ);
    assert_eq!(wakeup_5g.transfer_port, DEFAULT_TRANSFER_PORT);
    assert!(
        wakeup_5g.verify(auth_secret),
        "Freshly generated 0x0D auth token must verify within current minute window"
    );
    assert!(
        !wakeup_5g.verify(b"INCORRECT_AUTH_SECRET_TAMPERED!!"),
        "Must reject incorrect auth secrets"
    );

    let raw_0x0d = wakeup_5g.to_raw_manufacturer_data();
    assert_eq!(raw_0x0d.len(), 12, "0x0D wakeup frame must be exactly 12 bytes");
    assert_eq!(raw_0x0d[0], 0x4C);
    assert_eq!(raw_0x0d[1], 0x00);
    assert_eq!(raw_0x0d[2], CONTINUITY_TYPE_HOTSPOT_WAKEUP); // 0x0D
    assert_eq!(raw_0x0d[3], 0x08); // Subpayload len = 8 bytes

    let parsed_0x0d = HotspotWakeup0x0D::parse(&raw_0x0d)
        .expect("Valid 12-byte 0x0D frame should parse successfully");
    assert_eq!(parsed_0x0d.control_flags, wakeup_5g.control_flags);
    assert_eq!(parsed_0x0d.target_band, BAND_5_GHZ);
    assert_eq!(parsed_0x0d.transfer_port, DEFAULT_TRANSFER_PORT);
    assert!(parsed_0x0d.verify(auth_secret));

    // Also verify 2.4 GHz target band
    let wakeup_24 = HotspotWakeup0x0D::new(
        auth_secret,
        BAND_2_4_GHZ,
        DEFAULT_TRANSFER_PORT,
    );
    assert_eq!(wakeup_24.control_flags, FLAG_WAKEUP_START_LOHS);
    assert_eq!(wakeup_24.target_band, BAND_2_4_GHZ);

    // 2. Generate 0x0E Hotspot Status Frame
    let status = HotspotStatus0x0E::new(
        85, // 85% battery
        RAT_TYPE_5G,
        4,  // 4 signal bars
        STATE_FLAG_SOFTAP_UP | STATE_FLAG_5GHZ_ACTIVE,
        5180, // Channel 36 (5180 MHz)
    );

    assert_eq!(status.battery_percent, 85);
    assert_eq!(status.network_type, RAT_TYPE_5G);
    assert_eq!(status.signal_bars, 4);
    assert_eq!(status.state_flags, STATE_FLAG_SOFTAP_UP | STATE_FLAG_5GHZ_ACTIVE);
    assert_eq!(status.channel_freq_mhz, 5180);

    let raw_0x0e = status.to_raw_manufacturer_data();
    assert_eq!(raw_0x0e.len(), 10, "0x0E status frame must be exactly 10 bytes");
    assert_eq!(raw_0x0e[0], 0x4C);
    assert_eq!(raw_0x0e[1], 0x00);
    assert_eq!(raw_0x0e[2], CONTINUITY_TYPE_HOTSPOT_STATUS); // 0x0E
    assert_eq!(raw_0x0e[3], 0x06); // Subpayload len = 6 bytes

    let parsed_0x0e = HotspotStatus0x0E::parse(&raw_0x0e)
        .expect("Valid 10-byte 0x0E frame should parse successfully");
    assert_eq!(parsed_0x0e.battery_percent, 85);
    assert_eq!(parsed_0x0e.network_type, RAT_TYPE_5G);
    assert_eq!(parsed_0x0e.signal_bars, 4);
    assert_eq!(parsed_0x0e.state_flags, status.state_flags);
    assert_eq!(parsed_0x0e.channel_freq_mhz, 5180);
}

// =========================================================================
// 2. NetworkManager Volatile Profile & Safety Invariants
// =========================================================================

#[test]
fn test_nm_volatile_profile_safety_invariants() {
    let ssid = "ArchDrop_Android_LOHS_1234";
    let passphrase = "SafePassword987!";

    // 1. WPA2 Profile Generation
    let wpa2_config = LohsConnectionConfig::new(ssid, passphrase, SecurityType::Wpa2Psk);
    let builder = ConnectionProfileBuilder::new(&wpa2_config);
    let profile = builder.build().expect("WPA2 profile build must succeed");

    // Verify connection section
    let conn_section = profile.get("connection").expect("Must contain 'connection' section");
    assert_eq!(conn_section.get("type"), Some(&Value::from("802-11-wireless")));
    assert_eq!(conn_section.get("autoconnect"), Some(&Value::from(false)));

    // Verify 802-11-wireless section
    let wifi_section = profile.get("802-11-wireless").expect("Must contain '802-11-wireless' section");
    assert_eq!(wifi_section.get("mode"), Some(&Value::from("infrastructure")));
    assert_eq!(wifi_section.get("ssid"), Some(&Value::from(ssid.as_bytes().to_vec())));

    // Verify 802-11-wireless-security section
    let sec_section = profile.get("802-11-wireless-security").expect("Must contain security section");
    assert_eq!(sec_section.get("key-mgmt"), Some(&Value::from("wpa-psk")));
    assert_eq!(sec_section.get("psk"), Some(&Value::from(passphrase)));

    // Verify MANDATORY host safety invariants in IPv4 section
    let ip4_section = profile.get("ipv4").expect("Must contain 'ipv4' section");
    assert_eq!(ip4_section.get("method"), Some(&Value::from("auto")));
    assert_eq!(
        ip4_section.get("never-default"),
        Some(&Value::from(true)),
        "CRITICAL: ipv4.never-default must be true to prevent hijacking host default gateway"
    );
    assert_eq!(
        ip4_section.get("route-metric"),
        Some(&Value::from(2000i32)),
        "CRITICAL: ipv4.route-metric must be 2000 (below WireGuard and default interface priority)"
    );
    assert_eq!(
        ip4_section.get("ignore-auto-dns"),
        Some(&Value::from(true)),
        "CRITICAL: ipv4.ignore-auto-dns must be true to avoid overwriting /etc/resolv.conf"
    );

    // Verify IPv6 disabled
    let ip6_section = profile.get("ipv6").expect("Must contain 'ipv6' section");
    assert_eq!(ip6_section.get("method"), Some(&Value::from("disabled")));

    // Verify volatile activation options
    let options = ConnectionProfileBuilder::build_options();
    assert_eq!(options.get("persist"), Some(&Value::from("volatile")));
    assert_eq!(options.get("bind-activation"), Some(&Value::from("dbus-client")));

    // Verify safety validation method
    ConnectionProfileBuilder::verify_safety_invariants(&profile)
        .expect("Built profile must satisfy all safety invariants");

    // 2. WPA3 Profile Generation
    let wpa3_config = LohsConnectionConfig::new(ssid, passphrase, SecurityType::Wpa3Sae);
    let wpa3_builder = ConnectionProfileBuilder::new(&wpa3_config);
    let wpa3_profile = wpa3_builder.build().expect("WPA3 profile build must succeed");
    let wpa3_sec = wpa3_profile.get("802-11-wireless-security").expect("Must contain security section");
    assert_eq!(wpa3_sec.get("key-mgmt"), Some(&Value::from("sae")));
    assert_eq!(wpa3_sec.get("pmf"), Some(&Value::from(3i32))); // PMF Required
    ConnectionProfileBuilder::verify_safety_invariants(&wpa3_profile)
        .expect("WPA3 profile must pass safety verification");

    // 3. Tampering Detection: Mutate never-default
    let mut tampered_profile = profile.clone();
    let mut tampered_ip4 = tampered_profile.get("ipv4").unwrap().clone();
    tampered_ip4.insert("never-default".to_string(), Value::from(false));
    tampered_profile.insert("ipv4".to_string(), tampered_ip4);
    assert!(
        matches!(
            ConnectionProfileBuilder::verify_safety_invariants(&tampered_profile),
            Err(WirelessError::SafetyInvariantViolated(_))
        ),
        "Must detect tampered never-default setting"
    );

    // Tampering Detection: Mutate route-metric
    let mut metric_tampered = profile.clone();
    let mut tampered_ip4_metric = metric_tampered.get("ipv4").unwrap().clone();
    tampered_ip4_metric.insert("route-metric".to_string(), Value::from(100i32));
    metric_tampered.insert("ipv4".to_string(), tampered_ip4_metric);
    assert!(
        matches!(
            ConnectionProfileBuilder::verify_safety_invariants(&metric_tampered),
            Err(WirelessError::SafetyInvariantViolated(_))
        ),
        "Must detect tampered route-metric setting"
    );
}

// =========================================================================
// 3. Dynamic Gateway IP Resolution Logic
// =========================================================================

#[test]
fn test_gateway_ip_resolution_tiers() {
    let local_ip = Ipv4Addr::new(192, 168, 43, 88);
    let prefix = 24;

    // Tier 1: Explicit IP4Config Gateway property
    let empty_dhcp = HashMap::new();
    let tier1_res = resolve_gateway_from_data(Some("192.168.43.1"), &empty_dhcp, local_ip, prefix)
        .expect("Tier 1 resolution must succeed");
    assert_eq!(tier1_res.tier, 1);
    assert_eq!(tier1_res.gateway_ip, Ipv4Addr::new(192, 168, 43, 1));

    // Tier 2: DHCP4Config server identifier
    let mut dhcp_server_id = HashMap::new();
    dhcp_server_id.insert("dhcp_server_identifier".to_string(), "192.168.43.1".to_string());
    let tier2_res = resolve_gateway_from_data(None, &dhcp_server_id, local_ip, prefix)
        .expect("Tier 2 server identifier resolution must succeed");
    assert_eq!(tier2_res.tier, 2);
    assert_eq!(tier2_res.gateway_ip, Ipv4Addr::new(192, 168, 43, 1));

    // Tier 2: DHCP4Config routers option
    let mut dhcp_routers = HashMap::new();
    dhcp_routers.insert("routers".to_string(), "192.168.49.1".to_string());
    let tier2_routers_res = resolve_gateway_from_data(None, &dhcp_routers, local_ip, prefix)
        .expect("Tier 2 routers option resolution must succeed");
    assert_eq!(tier2_routers_res.tier, 2);
    assert_eq!(tier2_routers_res.gateway_ip, Ipv4Addr::new(192, 168, 49, 1));

    // Tier 3: Subnet Fallback Calculation
    let tier3_res = resolve_gateway_from_data(None, &empty_dhcp, local_ip, prefix)
        .expect("Tier 3 subnet fallback resolution must succeed");
    assert_eq!(tier3_res.tier, 3);
    assert_eq!(tier3_res.gateway_ip, Ipv4Addr::new(192, 168, 43, 1));

    // Additional subnet prefix checks
    assert_eq!(
        calculate_subnet_gateway(Ipv4Addr::new(10, 0, 0, 42), 8).unwrap(),
        Ipv4Addr::new(10, 0, 0, 1)
    );
    assert_eq!(
        calculate_subnet_gateway(Ipv4Addr::new(172, 16, 5, 23), 16).unwrap(),
        Ipv4Addr::new(172, 16, 0, 1)
    );
    assert_eq!(
        calculate_subnet_gateway(Ipv4Addr::new(192, 168, 1, 35), 28).unwrap(),
        Ipv4Addr::new(192, 168, 1, 33)
    );

    // Parsing Helpers
    let mut sample_dict = HashMap::new();
    sample_dict.insert(
        "address".to_string(),
        OwnedValue::try_from(Value::from("192.168.43.105")).unwrap(),
    );
    sample_dict.insert(
        "prefix".to_string(),
        OwnedValue::try_from(Value::from(24u32)).unwrap(),
    );
    let parsed_addrs = parse_address_data(&[sample_dict]);
    assert_eq!(parsed_addrs, Some((Ipv4Addr::new(192, 168, 43, 105), 24)));

    let mut zvariant_dhcp = HashMap::new();
    zvariant_dhcp.insert(
        "routers".to_string(),
        OwnedValue::try_from(Value::from("192.168.43.1")).unwrap(),
    );
    let parsed_dhcp = parse_dhcp_options(&zvariant_dhcp);
    assert_eq!(parsed_dhcp.get("routers"), Some(&"192.168.43.1".to_string()));
}

// =========================================================================
// 4. LocalSend v2 HTTP Streaming Transfer to Target IP with SHA-256 Validation
// =========================================================================

#[tokio::test]
async fn test_e2e_p2p_localsend_v2_streaming_transfer() {
    let temp_dir = TempDir::new().unwrap();
    let sender_dir = temp_dir.path().join("sender");
    let receiver_dir = temp_dir.path().join("receiver");
    tokio::fs::create_dir_all(&sender_dir).await.unwrap();
    tokio::fs::create_dir_all(&receiver_dir).await.unwrap();

    // 1. Generate 2 MB synthetic binary test file
    let test_file_path = sender_dir.join("p2p_transfer_payload_2mb.bin");
    let mut file = tokio::fs::File::create(&test_file_path).await.unwrap();
    let chunk = vec![0x5Cu8; 64 * 1024]; // 64 KiB chunk
    let mut total_bytes = 0u64;
    for _ in 0..32 {
        file.write_all(&chunk).await.unwrap();
        total_bytes += chunk.len() as u64;
    }
    file.flush().await.unwrap();
    drop(file);

    let original_bytes = tokio::fs::read(&test_file_path).await.unwrap();
    let expected_sha256 = compute_sha256(&original_bytes);
    assert_eq!(original_bytes.len() as u64, total_bytes);

    // 2. Start Receiver TransferServer simulating target peer on gateway IP
    let config = Arc::new(Config::default());
    let receiver_identity = Arc::new(CryptoIdentity::generate().unwrap());

    // Bind on random loopback port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = TransferServer::new(
        config.clone(),
        receiver_identity.clone(),
        receiver_dir.clone(),
        port,
        false, // Plain HTTP mode for local P2P link
    )
    .expect("Failed to initialize TransferServer");

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    // Allow server socket to bind
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;

    // 3. Initiate LocalSend v2 streaming transfer via TransferClient
    let sender_identity = CryptoIdentity::generate().unwrap();
    let client = TransferClient::new();

    let summary = client
        .send_files(
            "127.0.0.1", // In real P2P, this is the resolved LOHS gateway IP (e.g. 192.168.43.1)
            port,
            false,
            &[test_file_path],
            &config,
            &sender_identity,
        )
        .await
        .expect("TransferClient::send_files must succeed");

    assert_eq!(summary.files_sent, 1);
    assert_eq!(summary.total_bytes, total_bytes);

    // 4. Verify received payload on disk
    let received_file_path = receiver_dir.join("p2p_transfer_payload_2mb.bin");
    assert!(
        received_file_path.exists(),
        "Receiver must have saved the uploaded file to disk"
    );

    let received_bytes = tokio::fs::read(&received_file_path).await.unwrap();
    assert_eq!(
        received_bytes.len() as u64,
        total_bytes,
        "Received file size must match original exactly"
    );

    let received_sha256 = compute_sha256(&received_bytes);
    assert_eq!(
        received_sha256, expected_sha256,
        "SHA-256 digest of received file must match original bit-for-bit"
    );

    // 5. Clean teardown
    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

// =========================================================================
// 5. Host Safety Invariant: WireGuard `tiktok` Interface & Routing Table 51820
// =========================================================================

#[test]
fn test_wireguard_safety_invariant_e2e() {
    let addr_output = Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output();

    if let Ok(out) = addr_output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("10.2.0.2/32"),
                "WireGuard interface 'tiktok' with IP 10.2.0.2/32 must remain active"
            );

            let route_output = Command::new("ip")
                .args(["route", "show", "table", "51820"])
                .output()
                .expect("Failed to execute ip route command");

            let route_stdout = String::from_utf8_lossy(&route_output.stdout);
            assert!(
                route_stdout.contains("default dev tiktok"),
                "Policy routing table 51820 must continue routing through dev tiktok"
            );
        }
    }
}


// =========================================================================
// 6. Adversarial Stress Suite: Volatile Profile Tamper & Invariant Matrix
// =========================================================================

#[test]
fn test_adversarial_volatile_profile_exhaustive_tamper_matrix() {
    let ssid = "ArchDrop_Adversarial_Test";
    let passphrase = "VeryStrongPassword123!";

    // 1. Base Valid Profile
    let base_config = LohsConnectionConfig::new(ssid, passphrase, SecurityType::Wpa2Psk);
    let base_builder = ConnectionProfileBuilder::new(&base_config);
    let valid_profile = base_builder.build().expect("Base profile build must succeed");
    assert!(ConnectionProfileBuilder::verify_safety_invariants(&valid_profile).is_ok());

    // 2. Tampering ipv4.never-default:
    // 2a. Set never-default to false
    let mut tampered = valid_profile.clone();
    tampered.get_mut("ipv4").unwrap().insert("never-default".to_string(), Value::from(false));
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(msg)) if msg.contains("never-default must be true")
    ));

    // 2b. Set never-default to integer 0 (type confusion)
    let mut tampered = valid_profile.clone();
    tampered.get_mut("ipv4").unwrap().insert("never-default".to_string(), Value::from(0i32));
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(_))
    ));

    // 2c. Remove never-default key
    let mut tampered = valid_profile.clone();
    tampered.get_mut("ipv4").unwrap().remove("never-default");
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(_))
    ));

    // 3. Tampering ipv4.route-metric:
    // 3a. Metric < 2000 (e.g. 100, 0, 1999, negative)
    for bad_metric in [0i32, 1i32, 50i32, 100i32, 500i32, 1999i32, -100i32] {
        let mut tampered = valid_profile.clone();
        tampered.get_mut("ipv4").unwrap().insert("route-metric".to_string(), Value::from(bad_metric));
        assert!(matches!(
            ConnectionProfileBuilder::verify_safety_invariants(&tampered),
            Err(WirelessError::SafetyInvariantViolated(msg)) if msg.contains("route-metric must be >=")
        ), "Failed to reject bad route metric: {}", bad_metric);
    }

    // 3b. Route metric as string (type confusion)
    let mut tampered = valid_profile.clone();
    tampered.get_mut("ipv4").unwrap().insert("route-metric".to_string(), Value::from("2000"));
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(_))
    ));

    // 3c. Remove route-metric key
    let mut tampered = valid_profile.clone();
    tampered.get_mut("ipv4").unwrap().remove("route-metric");
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(_))
    ));

    // 3d. Valid boundary metrics: 2000, 2001, 5000 (must succeed)
    for good_metric in [2000i32, 2001i32, 5000i32] {
        let mut profile = valid_profile.clone();
        profile.get_mut("ipv4").unwrap().insert("route-metric".to_string(), Value::from(good_metric));
        assert!(ConnectionProfileBuilder::verify_safety_invariants(&profile).is_ok());
    }

    // 4. Tampering ipv4.ignore-auto-dns:
    // 4a. Set to false
    let mut tampered = valid_profile.clone();
    tampered.get_mut("ipv4").unwrap().insert("ignore-auto-dns".to_string(), Value::from(false));
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(msg)) if msg.contains("ignore-auto-dns must be true")
    ));

    // 4b. Remove ignore-auto-dns key
    let mut tampered = valid_profile.clone();
    tampered.get_mut("ipv4").unwrap().remove("ignore-auto-dns");
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(_))
    ));

    // 5. Tampering ipv6.method:
    for rogue_method in ["auto", "dhcp", "ignore", "manual", "link-local", ""] {
        let mut tampered = valid_profile.clone();
        tampered.get_mut("ipv6").unwrap().insert("method".to_string(), Value::from(rogue_method));
        assert!(matches!(
            ConnectionProfileBuilder::verify_safety_invariants(&tampered),
            Err(WirelessError::SafetyInvariantViolated(msg)) if msg.contains("ipv6.method must be 'disabled'")
        ), "Failed to reject rogue ipv6 method: {}", rogue_method);
    }

    // 6. Tampering connection.autoconnect:
    let mut tampered = valid_profile.clone();
    tampered.get_mut("connection").unwrap().insert("autoconnect".to_string(), Value::from(true));
    assert!(matches!(
        ConnectionProfileBuilder::verify_safety_invariants(&tampered),
        Err(WirelessError::SafetyInvariantViolated(msg)) if msg.contains("connection.autoconnect must be false")
    ));

    // 7. Missing required sections:
    for section in ["ipv4", "ipv6", "connection"] {
        let mut tampered = valid_profile.clone();
        tampered.remove(section);
        assert!(matches!(
            ConnectionProfileBuilder::verify_safety_invariants(&tampered),
            Err(WirelessError::SafetyInvariantViolated(_))
        ), "Failed to detect missing section: {}", section);
    }

    // 8. Builder invariant validation during construction
    let mut bad_config = base_config.clone();
    bad_config.never_default = false;
    assert!(ConnectionProfileBuilder::new(&bad_config).build().is_err());

    let mut bad_config = base_config.clone();
    bad_config.route_metric = 500;
    assert!(ConnectionProfileBuilder::new(&bad_config).build().is_err());

    let mut bad_config = base_config.clone();
    bad_config.ignore_auto_dns = false;
    assert!(ConnectionProfileBuilder::new(&bad_config).build().is_err());

    let mut bad_config = base_config.clone();
    bad_config.disable_ipv6 = false;
    assert!(ConnectionProfileBuilder::new(&bad_config).build().is_err());

    let mut bad_config = base_config.clone();
    bad_config.autoconnect = true;
    assert!(ConnectionProfileBuilder::new(&bad_config).build().is_err());
}

// =========================================================================
// 7. Adversarial Stress Suite: High-Concurrency Multi-Client File Transfers
// =========================================================================

#[tokio::test]
async fn test_adversarial_concurrent_p2p_localsend_transfers_stress() {
    let temp_dir = TempDir::new().unwrap();
    let receiver_dir = temp_dir.path().join("receiver");
    tokio::fs::create_dir_all(&receiver_dir).await.unwrap();

    let config = Arc::new(Config::default());
    let receiver_identity = Arc::new(CryptoIdentity::generate().unwrap());

    // Bind on random loopback port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = TransferServer::new(
        config.clone(),
        receiver_identity.clone(),
        receiver_dir.clone(),
        port,
        false, // Plain HTTP mode for local P2P link
    )
    .expect("Failed to initialize TransferServer");

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Prepare 8 concurrent clients with payloads of varying sizes
    const NUM_CLIENTS: usize = 8;
    let mut expected_hashes = Vec::new();
    let mut client_tasks = tokio::task::JoinSet::new();

    for i in 0..NUM_CLIENTS {
        let client_dir = temp_dir.path().join(format!("sender_{}", i));
        tokio::fs::create_dir_all(&client_dir).await.unwrap();

        let filename = format!("payload_client_{}_{}kb.bin", i, (i + 1) * 128);
        let file_path = client_dir.join(&filename);

        // Generate synthetic payload: (i + 1) * 128 KiB
        let payload_size = (i + 1) * 128 * 1024;
        let mut data = vec![(i as u8).wrapping_mul(17).wrapping_add(0x41); payload_size];
        data[0] = i as u8;
        data[payload_size - 1] = 0xAA;

        tokio::fs::write(&file_path, &data).await.unwrap();
        let expected_hash = compute_sha256(&data);
        expected_hashes.push((filename, expected_hash, payload_size as u64));

        let client_config = config.clone();
        let target_port = port;

        client_tasks.spawn(async move {
            let client_identity = CryptoIdentity::generate().unwrap();
            let client = TransferClient::new();
            let summary = client
                .send_files(
                    "127.0.0.1",
                    target_port,
                    false,
                    &[file_path],
                    &client_config,
                    &client_identity,
                )
                .await;
            (i, summary)
        });
    }

    // Await all concurrent client transfers
    let mut completed_count = 0;
    while let Some(res) = client_tasks.join_next().await {
        let (client_idx, transfer_res) = res.expect("Client task panicked");
        let summary = transfer_res.unwrap_or_else(|e| {
            panic!("Client {} transfer failed: {:?}", client_idx, e);
        });
        assert_eq!(summary.files_sent, 1);
        completed_count += 1;
    }
    assert_eq!(completed_count, NUM_CLIENTS, "All concurrent clients must succeed");

    // Verify all received files on disk match expected sizes and SHA-256 hashes
    for (filename, expected_hash, expected_size) in expected_hashes {
        let received_path = receiver_dir.join(&filename);
        assert!(
            received_path.exists(),
            "File {} must exist in receiver directory",
            filename
        );

        let received_bytes = tokio::fs::read(&received_path).await.unwrap();
        assert_eq!(
            received_bytes.len() as u64,
            expected_size,
            "Received size mismatch for {}",
            filename
        );

        let actual_hash = compute_sha256(&received_bytes);
        assert_eq!(
            actual_hash, expected_hash,
            "SHA-256 mismatch for concurrent transfer of {}",
            filename
        );
    }

    // Clean teardown
    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

// =========================================================================
// 8. Adversarial Stress Suite: CLI Permutations and Error Boundaries
// =========================================================================

#[test]
fn test_adversarial_cli_permutations_and_failures() {
    let bin = env!("CARGO_BIN_EXE_archdropd");

    // 1. Send with missing file argument -> Exit Code 2 (Clap parsing error)
    let output = Command::new(bin)
        .args(["send", "--to", "127.0.0.1"])
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("the following required arguments were not provided"));

    // 2. Send without destination (--to or --p2p) -> Exit Code 1
    let output = Command::new(bin)
        .args(["send", "/tmp/nonexistent.txt"])
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Destination required: specify either --to <IP> for direct transfer or --p2p"));

    // 3. Send to invalid IP format (e.g. 999.999.999.999) -> Exit Code 1
    let output = Command::new(bin)
        .args(["send", "--to", "999.999.999.999", "/tmp/nonexistent.txt"])
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Network error") || stderr.contains("builder error"));

    // 4. Send to empty IP string -> Exit Code 1
    let output = Command::new(bin)
        .args(["send", "--to", "", "/tmp/nonexistent.txt"])
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(1));

    // 5. Send with out-of-range port (e.g. 99999) -> Exit Code 2 (Clap error)
    let output = Command::new(bin)
        .args(["send", "--to", "127.0.0.1", "-p", "99999", "/tmp/nonexistent.txt"])
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("is not in 0..=65535"));

    // 6. Wireless status diagnostic subcommand -> Exit Code 0
    let output = Command::new(bin)
        .args(["wireless", "status"])
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("NetworkManager: CONNECTED"));

    // 7. Root status subcommand -> Exit Code 0
    let output = Command::new(bin)
        .arg("status")
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ARCH DROP DAEMON STATUS"));

    // 8. Pair subcommand invalid fingerprint -> Exit Code 1
    let output = Command::new(bin)
        .args(["pair", "add", "INVALID_NOT_HEX"])
        .output()
        .expect("Failed to execute CLI");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Invalid SHA-256 fingerprint format"));
}

// =========================================================================
// 9. Adversarial Stress Suite: Concurrent CLI Invocations
// =========================================================================

#[test]
fn test_adversarial_concurrent_cli_invocations() {
    let bin = env!("CARGO_BIN_EXE_archdropd");
    use std::thread;

    // Spawn 10 concurrent threads invoking CLI status and wireless status simultaneously
    let mut handles = Vec::new();
    for i in 0..10 {
        let bin_path = bin.to_string();
        let handle = thread::spawn(move || {
            let args = if i % 2 == 0 {
                vec!["status"]
            } else {
                vec!["wireless", "status"]
            };
            let output = Command::new(bin_path)
                .args(&args)
                .output()
                .expect("CLI execution failed");
            assert_eq!(output.status.code(), Some(0), "CLI invocation {} failed", i);
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("CLI thread panicked");
    }
}


// =========================================================================
// 10. Adversarial Stress Suite: Path Traversal & Dest Path Sanitization
// =========================================================================

#[test]
fn test_adversarial_destination_path_sanitization_and_traversal() {
    let temp_dir = TempDir::new().unwrap();
    let dest_dir = temp_dir.path().join("downloads");
    std::fs::create_dir_all(&dest_dir).unwrap();

    let traversal_attempts = [
        "../../../../etc/passwd",
        "../../../etc/shadow",
        "../../../../../../root/.ssh/id_rsa",
        "/absolute/path/override.txt",
        "/etc/hosts",
        "C:\\Windows\\System32\\cmd.exe",
        "..\\..\\sensitive.dat",
        "sub/dir/nested.txt",
        "file\0with\0nulls.txt",
        "file/with/slashes.txt",
        "file\\with\\backslashes.txt",
        ".",
        "..",
        "",
        "CON.txt",
        "normal_file.txt",
    ];

    for attempt in traversal_attempts {
        let resolved = SessionManager::resolve_destination_path(&dest_dir, attempt);

        // Invariant: The resolved path MUST reside strictly inside dest_dir
        assert!(
            resolved.starts_with(&dest_dir),
            "Path traversal escape detected for input: {:?} -> resolved to {:?}",
            attempt,
            resolved
        );

        // Invariant: Slashes, backslashes, and null bytes must NOT appear in the file name
        let file_name = resolved.file_name().unwrap().to_str().unwrap();
        assert!(!file_name.contains('/'), "Filename contains forward slash: {}", file_name);
        assert!(!file_name.contains('\\'), "Filename contains backslash: {}", file_name);
        assert!(!file_name.contains('\0'), "Filename contains null byte: {}", file_name);
    }

    // Verify collision resolution loop
    let first = SessionManager::resolve_destination_path(&dest_dir, "report.pdf");
    assert_eq!(first, dest_dir.join("report.pdf"));
    std::fs::write(&first, b"version 1").unwrap();

    let second = SessionManager::resolve_destination_path(&dest_dir, "report.pdf");
    assert_eq!(second, dest_dir.join("report (1).pdf"));
    std::fs::write(&second, b"version 2").unwrap();

    let third = SessionManager::resolve_destination_path(&dest_dir, "report.pdf");
    assert_eq!(third, dest_dir.join("report (2).pdf"));
}

// =========================================================================
// 11. Adversarial Stress Suite: High-Scale Concurrency Transfer Stress
// =========================================================================

#[tokio::test]
async fn test_adversarial_high_scale_transfer_concurrency() {
    let temp_dir = TempDir::new().unwrap();
    let receiver_dir = temp_dir.path().join("receiver_scale");
    tokio::fs::create_dir_all(&receiver_dir).await.unwrap();

    let config = Arc::new(Config::default());
    let receiver_identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = TransferServer::new(
        config.clone(),
        receiver_identity.clone(),
        receiver_dir.clone(),
        port,
        false,
    )
    .expect("Failed to initialize TransferServer");

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 16 concurrent clients streaming varied file sizes simultaneously
    const NUM_CLIENTS: usize = 16;
    let mut expected_hashes = Vec::new();
    let mut client_tasks = tokio::task::JoinSet::new();

    for i in 0..NUM_CLIENTS {
        let client_dir = temp_dir.path().join(format!("scale_sender_{}", i));
        tokio::fs::create_dir_all(&client_dir).await.unwrap();

        let filename = format!("scale_test_payload_{}.dat", i);
        let file_path = client_dir.join(&filename);

        // Varied payload sizes: 32 KiB to 256 KiB
        let payload_size = (i % 8 + 1) * 32 * 1024;
        let mut data = vec![((i * 37) & 0xFF) as u8; payload_size];
        data[0] = 0xBE;
        data[payload_size - 1] = 0xEF;

        tokio::fs::write(&file_path, &data).await.unwrap();
        let expected_hash = compute_sha256(&data);
        expected_hashes.push((filename, expected_hash, payload_size as u64));

        let client_config = config.clone();
        let target_port = port;

        client_tasks.spawn(async move {
            let client_identity = CryptoIdentity::generate().unwrap();
            let client = TransferClient::new();
            let summary = client
                .send_files(
                    "127.0.0.1",
                    target_port,
                    false,
                    &[file_path],
                    &client_config,
                    &client_identity,
                )
                .await;
            (i, summary)
        });
    }

    let mut success_count = 0;
    while let Some(res) = client_tasks.join_next().await {
        let (client_idx, transfer_res) = res.expect("Scale task panicked");
        let summary = transfer_res.unwrap_or_else(|e| {
            panic!("Scale client {} transfer failed: {:?}", client_idx, e);
        });
        assert_eq!(summary.files_sent, 1);
        success_count += 1;
    }
    assert_eq!(success_count, NUM_CLIENTS);

    for (filename, expected_hash, expected_size) in expected_hashes {
        let received_path = receiver_dir.join(&filename);
        assert!(received_path.exists());
        let received_bytes = tokio::fs::read(&received_path).await.unwrap();
        assert_eq!(received_bytes.len() as u64, expected_size);
        assert_eq!(compute_sha256(&received_bytes), expected_hash);
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}
