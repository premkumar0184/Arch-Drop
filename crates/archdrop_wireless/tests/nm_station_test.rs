//! NetworkManager Station Controller & Host Safety Invariant Tests
//!
//! Verifies:
//! - Profile dictionary generation for WPA2-Personal, WPA3-Personal, and Open networks
//! - Host safety invariants enforcement (never-default=true, ignore-auto-dns=true, route-metric=2000, ipv6=disabled)
//! - Tamper detection on settings dictionaries
//! - Three-tier dynamic gateway IP resolution (IP4Config gateway, DHCP lease options, Subnet host calculation)
//! - Non-destructive live NetworkManager device discovery
//! - Preservation of host WireGuard interface `tiktok` and routing table 51820

use archdrop_wireless::nm::{
    calculate_subnet_gateway, parse_address_data, parse_dhcp_options, resolve_gateway_from_data,
    ActiveConnectionState, ActiveLohsSession, ConnectionProfileBuilder, LohsConnectionConfig,
    LohsSessionCoordinator, NmStationController, SecurityType, DEFAULT_LOHS_IDLE_TIMEOUT,
    NM_ACTIVE_CONNECTION_STATE_ACTIVATED, NM_ACTIVE_CONNECTION_STATE_ACTIVATING,
    NM_ACTIVE_CONNECTION_STATE_DEACTIVATED, NM_ACTIVE_CONNECTION_STATE_DEACTIVATING,
};
use archdrop_wireless::WirelessError;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::process::Command;
use std::time::Duration;
use zbus::zvariant::{OwnedValue, Value};

#[test]
fn test_wpa2_profile_generation() {
    let config = LohsConnectionConfig::new("AndroidLOHS-WPA2", "securepsk1234", SecurityType::Wpa2Psk)
        .with_bssid("AA:BB:CC:DD:EE:FF");

    let builder = ConnectionProfileBuilder::new(&config)
        .with_uuid("00112233-4455-6677-8899-aabbccddeeff")
        .with_connection_id("Custom-LOHS-ID");

    let settings = builder.build().expect("Profile build failed");

    // 1. Connection setting
    let con = settings.get("connection").expect("missing connection setting");
    assert_eq!(con.get("id"), Some(&Value::from("Custom-LOHS-ID")));
    assert_eq!(con.get("type"), Some(&Value::from("802-11-wireless")));
    assert_eq!(
        con.get("uuid"),
        Some(&Value::from("00112233-4455-6677-8899-aabbccddeeff"))
    );
    assert_eq!(con.get("autoconnect"), Some(&Value::from(false)));

    // 2. 802.11 Wireless setting
    let wifi = settings.get("802-11-wireless").expect("missing 802-11-wireless setting");
    assert_eq!(
        wifi.get("ssid"),
        Some(&Value::from(b"AndroidLOHS-WPA2".to_vec()))
    );
    assert_eq!(wifi.get("mode"), Some(&Value::from("infrastructure")));
    assert_eq!(wifi.get("bssid"), Some(&Value::from("AA:BB:CC:DD:EE:FF")));

    // 3. Wireless security setting
    let sec = settings
        .get("802-11-wireless-security")
        .expect("missing security setting");
    assert_eq!(sec.get("key-mgmt"), Some(&Value::from("wpa-psk")));
    assert_eq!(sec.get("psk"), Some(&Value::from("securepsk1234")));
    assert!(sec.get("pmf").is_none());

    // 4. IPv4 Host Safety Invariants
    let ip4 = settings.get("ipv4").expect("missing ipv4 setting");
    assert_eq!(ip4.get("method"), Some(&Value::from("auto")));
    assert_eq!(ip4.get("never-default"), Some(&Value::from(true)));
    assert_eq!(ip4.get("ignore-auto-dns"), Some(&Value::from(true)));
    assert_eq!(ip4.get("route-metric"), Some(&Value::from(2000i32)));

    // 5. IPv6 Host Safety Invariants
    let ip6 = settings.get("ipv6").expect("missing ipv6 setting");
    assert_eq!(ip6.get("method"), Some(&Value::from("disabled")));
    assert_eq!(ip6.get("never-default"), Some(&Value::from(true)));
}

#[test]
fn test_wpa3_profile_generation() {
    let config = LohsConnectionConfig::new("AndroidLOHS-WPA3", "saepassphrase", SecurityType::Wpa3Sae);

    let builder = ConnectionProfileBuilder::new(&config);
    let settings = builder.build().expect("WPA3 profile build failed");

    let sec = settings
        .get("802-11-wireless-security")
        .expect("missing security setting");
    assert_eq!(sec.get("key-mgmt"), Some(&Value::from("sae")));
    assert_eq!(sec.get("psk"), Some(&Value::from("saepassphrase")));
    // PMF required (3) for WPA3 SAE
    assert_eq!(sec.get("pmf"), Some(&Value::from(3i32)));

    // Safety invariants preserved
    let ip4 = settings.get("ipv4").unwrap();
    assert_eq!(ip4.get("never-default"), Some(&Value::from(true)));
    assert_eq!(ip4.get("ignore-auto-dns"), Some(&Value::from(true)));
    assert_eq!(ip4.get("route-metric"), Some(&Value::from(2000i32)));
}

#[test]
fn test_open_network_profile_generation() {
    let config = LohsConnectionConfig::new("OpenHotspot", "", SecurityType::Open);
    let builder = ConnectionProfileBuilder::new(&config);
    let settings = builder.build().expect("Open profile build failed");

    // Open networks do not have a security setting
    assert!(!settings.contains_key("802-11-wireless-security"));

    // Host safety invariants still enforced
    let ip4 = settings.get("ipv4").unwrap();
    assert_eq!(ip4.get("never-default"), Some(&Value::from(true)));
    assert_eq!(ip4.get("ignore-auto-dns"), Some(&Value::from(true)));
    assert_eq!(ip4.get("route-metric"), Some(&Value::from(2000i32)));
}

#[test]
fn test_profile_options_volatile_and_dbus_bound() {
    let options = ConnectionProfileBuilder::build_options();
    assert_eq!(options.get("persist"), Some(&Value::from("volatile")));
    assert_eq!(
        options.get("bind-activation"),
        Some(&Value::from("dbus-client"))
    );
}

#[test]
fn test_safety_invariant_violation_never_default() {
    let mut config = LohsConnectionConfig::new("Test", "pass", SecurityType::Wpa2Psk);
    config.never_default = false;

    let builder = ConnectionProfileBuilder::new(&config);
    let err = builder.build().unwrap_err();
    match err {
        WirelessError::SafetyInvariantViolated(msg) => {
            assert!(msg.contains("never_default"));
        }
        other => panic!("Expected SafetyInvariantViolated, got {other:?}"),
    }
}

#[test]
fn test_safety_invariant_violation_ignore_auto_dns() {
    let mut config = LohsConnectionConfig::new("Test", "pass", SecurityType::Wpa2Psk);
    config.ignore_auto_dns = false;

    let builder = ConnectionProfileBuilder::new(&config);
    let err = builder.build().unwrap_err();
    match err {
        WirelessError::SafetyInvariantViolated(msg) => {
            assert!(msg.contains("ignore_auto_dns"));
        }
        other => panic!("Expected SafetyInvariantViolated, got {other:?}"),
    }
}

#[test]
fn test_safety_invariant_violation_route_metric() {
    let mut config = LohsConnectionConfig::new("Test", "pass", SecurityType::Wpa2Psk);
    config.route_metric = 500; // Less than 2000

    let builder = ConnectionProfileBuilder::new(&config);
    let err = builder.build().unwrap_err();
    match err {
        WirelessError::SafetyInvariantViolated(msg) => {
            assert!(msg.contains("route_metric"));
        }
        other => panic!("Expected SafetyInvariantViolated, got {other:?}"),
    }
}

#[test]
fn test_safety_invariant_violation_disable_ipv6() {
    let mut config = LohsConnectionConfig::new("Test", "pass", SecurityType::Wpa2Psk);
    config.disable_ipv6 = false;

    let builder = ConnectionProfileBuilder::new(&config);
    let err = builder.build().unwrap_err();
    match err {
        WirelessError::SafetyInvariantViolated(msg) => {
            assert!(msg.contains("disable_ipv6"));
        }
        other => panic!("Expected SafetyInvariantViolated, got {other:?}"),
    }
}

#[test]
fn test_safety_invariant_violation_autoconnect() {
    let mut config = LohsConnectionConfig::new("Test", "pass", SecurityType::Wpa2Psk);
    config.autoconnect = true;

    let builder = ConnectionProfileBuilder::new(&config);
    let err = builder.build().unwrap_err();
    match err {
        WirelessError::SafetyInvariantViolated(msg) => {
            assert!(msg.contains("autoconnect"));
        }
        other => panic!("Expected SafetyInvariantViolated, got {other:?}"),
    }
}

#[test]
fn test_tamper_detection_in_verify_safety_invariants() {
    let config = LohsConnectionConfig::new("Test", "pass", SecurityType::Wpa2Psk);
    let settings = ConnectionProfileBuilder::new(&config).build().unwrap();

    // 1. Valid settings pass
    assert!(ConnectionProfileBuilder::verify_safety_invariants(&settings).is_ok());

    // 2. Tampered never-default fails
    let mut tampered1 = settings.clone();
    tampered1
        .get_mut("ipv4")
        .unwrap()
        .insert("never-default".to_string(), Value::from(false));
    assert!(ConnectionProfileBuilder::verify_safety_invariants(&tampered1).is_err());

    // 3. Tampered route-metric fails
    let mut tampered2 = settings.clone();
    tampered2
        .get_mut("ipv4")
        .unwrap()
        .insert("route-metric".to_string(), Value::from(100i32));
    assert!(ConnectionProfileBuilder::verify_safety_invariants(&tampered2).is_err());

    // 4. Missing ipv6 section fails
    let mut tampered3 = settings.clone();
    tampered3.remove("ipv6");
    assert!(ConnectionProfileBuilder::verify_safety_invariants(&tampered3).is_err());

    // 5. Tampered autoconnect fails
    let mut tampered4 = settings.clone();
    tampered4
        .get_mut("connection")
        .unwrap()
        .insert("autoconnect".to_string(), Value::from(true));
    assert!(ConnectionProfileBuilder::verify_safety_invariants(&tampered4).is_err());
}

#[test]
fn test_gateway_resolution_tier_1_ip4config() {
    let dhcp_opts = HashMap::new();
    let local_ip = Ipv4Addr::new(192, 168, 43, 88);

    let res = resolve_gateway_from_data(Some("192.168.43.1"), &dhcp_opts, local_ip, 24)
        .expect("Tier 1 resolution should succeed");

    assert_eq!(res.tier, 1);
    assert_eq!(res.gateway_ip, Ipv4Addr::new(192, 168, 43, 1));
    assert_eq!(res.local_ip, local_ip);
    assert_eq!(res.prefix, 24);
}

#[test]
fn test_gateway_resolution_tier_2_dhcp_server_id() {
    let mut dhcp_opts = HashMap::new();
    dhcp_opts.insert(
        "dhcp_server_identifier".to_string(),
        "192.168.49.1".to_string(),
    );
    let local_ip = Ipv4Addr::new(192, 168, 49, 15);

    // Empty or 0.0.0.0 gateway triggers tier 2 fallback
    let res = resolve_gateway_from_data(Some("0.0.0.0"), &dhcp_opts, local_ip, 24)
        .expect("Tier 2 resolution should succeed");

    assert_eq!(res.tier, 2);
    assert_eq!(res.gateway_ip, Ipv4Addr::new(192, 168, 49, 1));
}

#[test]
fn test_gateway_resolution_tier_2_routers_option() {
    let mut dhcp_opts = HashMap::new();
    dhcp_opts.insert("routers".to_string(), "10.42.0.1".to_string());
    let local_ip = Ipv4Addr::new(10, 42, 0, 99);

    let res = resolve_gateway_from_data(None, &dhcp_opts, local_ip, 24)
        .expect("Tier 2 router option resolution should succeed");

    assert_eq!(res.tier, 2);
    assert_eq!(res.gateway_ip, Ipv4Addr::new(10, 42, 0, 1));
}

#[test]
fn test_gateway_resolution_tier_3_subnet_fallback() {
    let dhcp_opts = HashMap::new();
    let local_ip = Ipv4Addr::new(192, 168, 51, 142);

    let res = resolve_gateway_from_data(None, &dhcp_opts, local_ip, 24)
        .expect("Tier 3 fallback calculation should succeed");

    assert_eq!(res.tier, 3);
    assert_eq!(res.gateway_ip, Ipv4Addr::new(192, 168, 51, 1));
}

#[test]
fn test_calculate_subnet_gateway_various_prefixes() {
    // Standard /24
    assert_eq!(
        calculate_subnet_gateway(Ipv4Addr::new(192, 168, 43, 200), 24).unwrap(),
        Ipv4Addr::new(192, 168, 43, 1)
    );

    // /16 subnet
    assert_eq!(
        calculate_subnet_gateway(Ipv4Addr::new(172, 16, 5, 20), 16).unwrap(),
        Ipv4Addr::new(172, 16, 0, 1)
    );

    // /28 subnet (block 192.168.1.16..31)
    assert_eq!(
        calculate_subnet_gateway(Ipv4Addr::new(192, 168, 1, 25), 28).unwrap(),
        Ipv4Addr::new(192, 168, 1, 17)
    );

    // Invalid prefix
    assert!(calculate_subnet_gateway(Ipv4Addr::new(10, 0, 0, 1), 7).is_err());
    assert!(calculate_subnet_gateway(Ipv4Addr::new(10, 0, 0, 1), 31).is_err());
}

#[test]
fn test_parse_address_data_and_dhcp_options() {
    let mut addr_entry = HashMap::new();
    addr_entry.insert(
        "address".to_string(),
        OwnedValue::try_from(Value::from("192.168.43.15")).unwrap(),
    );
    addr_entry.insert(
        "prefix".to_string(),
        OwnedValue::try_from(Value::from(24u32)).unwrap(),
    );

    let list = vec![addr_entry];
    let parsed = parse_address_data(&list);
    assert_eq!(parsed, Some((Ipv4Addr::new(192, 168, 43, 15), 24)));

    let mut dhcp_raw = HashMap::new();
    dhcp_raw.insert(
        "dhcp_server_identifier".to_string(),
        OwnedValue::try_from(Value::from("192.168.43.1")).unwrap(),
    );
    dhcp_raw.insert(
        "expiry".to_string(),
        OwnedValue::try_from(Value::from(3600u32)).unwrap(),
    );

    let dhcp_map = parse_dhcp_options(&dhcp_raw);
    assert_eq!(
        dhcp_map.get("dhcp_server_identifier"),
        Some(&"192.168.43.1".to_string())
    );
    assert_eq!(dhcp_map.get("expiry"), Some(&"3600".to_string()));
}

#[test]
fn test_security_type_from_ap_flags() {
    const PSK_FLAG: u32 = 0x0000_0100;
    const SAE_FLAG: u32 = 0x0000_0400;

    assert_eq!(
        SecurityType::from_ap_flags(0, 0, SAE_FLAG),
        SecurityType::Wpa3Sae
    );
    assert_eq!(
        SecurityType::from_ap_flags(0, 0, PSK_FLAG),
        SecurityType::Wpa2Psk
    );
    assert_eq!(
        SecurityType::from_ap_flags(0, PSK_FLAG, 0),
        SecurityType::Wpa2Psk
    );
    assert_eq!(
        SecurityType::from_ap_flags(0, 0, SAE_FLAG | PSK_FLAG),
        SecurityType::Wpa2Wpa3Transition
    );
    assert_eq!(SecurityType::from_ap_flags(0, 0, 0), SecurityType::Open);

    assert_eq!(SecurityType::Wpa2Psk.key_mgmt_str(), "wpa-psk");
    assert_eq!(SecurityType::Wpa3Sae.key_mgmt_str(), "sae");
    assert_eq!(SecurityType::Open.key_mgmt_str(), "none");

    assert!(SecurityType::Wpa3Sae.is_wpa3());
    assert!(SecurityType::Wpa2Wpa3Transition.is_wpa3());
    assert!(!SecurityType::Wpa2Psk.is_wpa3());
    assert!(!SecurityType::Open.is_wpa3());
}

#[test]
fn test_active_connection_state_conversions() {
    assert_eq!(
        ActiveConnectionState::from(NM_ACTIVE_CONNECTION_STATE_ACTIVATING),
        ActiveConnectionState::Activating
    );
    assert_eq!(
        ActiveConnectionState::from(NM_ACTIVE_CONNECTION_STATE_ACTIVATED),
        ActiveConnectionState::Activated
    );
    assert_eq!(
        ActiveConnectionState::from(NM_ACTIVE_CONNECTION_STATE_DEACTIVATING),
        ActiveConnectionState::Deactivating
    );
    assert_eq!(
        ActiveConnectionState::from(NM_ACTIVE_CONNECTION_STATE_DEACTIVATED),
        ActiveConnectionState::Deactivated
    );
    assert_eq!(
        ActiveConnectionState::from(999),
        ActiveConnectionState::Unknown(999)
    );

    assert!(ActiveConnectionState::Activated.is_activated());
    assert!(!ActiveConnectionState::Activating.is_activated());
    assert!(ActiveConnectionState::Deactivated.is_terminal());
    assert!(ActiveConnectionState::Deactivating.is_terminal());
    assert!(!ActiveConnectionState::Activated.is_terminal());
}

#[tokio::test]
async fn test_live_nm_device_enumeration_non_destructive() {
    // If NetworkManager is running on the host system bus, test read-only Wi-Fi device lookup
    if let Ok(controller) = NmStationController::new().await {
        let dev_result = controller.find_wifi_device(None).await;
        match dev_result {
            Ok(path) => {
                println!("Live NM managed Wi-Fi device found: {}", path.as_str());
                assert!(path.as_str().starts_with("/org/freedesktop/NetworkManager/Devices/"));
                // Verify by listing APs non-destructively
                let aps = controller.list_access_points(&path).await;
                assert!(aps.is_ok(), "list_access_points should succeed on live device");
            }
            Err(e) => {
                println!("No managed Wi-Fi device found on host: {e}");
            }
        }
    } else {
        println!("NetworkManager D-Bus not accessible in this environment; skipping live test.");
    }
}

#[test]
fn test_host_wireguard_safety_invariants_post_nm_operations() {
    // Verify host WireGuard interface `tiktok` is up and untouched
    let link_out = Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output();

    if let Ok(out) = link_out {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("10.2.0.2/32"),
                "WireGuard interface 'tiktok' IP 10.2.0.2/32 must remain active"
            );

            // Verify routing table 51820
            let route_out = Command::new("ip")
                .args(["route", "show", "table", "51820"])
                .output()
                .expect("Failed to execute ip route");
            let route_stdout = String::from_utf8_lossy(&route_out.stdout);
            assert!(
                route_stdout.contains("default dev tiktok"),
                "Routing table 51820 default route via tiktok must remain active"
            );
        }
    }
}

#[tokio::test]
async fn test_linux_lohs_session_default_idle_timeout_is_60s() {
    assert_eq!(
        DEFAULT_LOHS_IDLE_TIMEOUT,
        Duration::from_secs(60),
        "DEFAULT_LOHS_IDLE_TIMEOUT must be 60 seconds for RTL8852BE hardware safety"
    );
}

#[tokio::test]
async fn test_linux_lohs_session_inactivity_teardown_watchdog() {
    let session = ActiveLohsSession::new_mock(
        Ipv4Addr::new(192, 168, 43, 1),
        Ipv4Addr::new(192, 168, 43, 100),
    );
    let timeout = Duration::from_millis(60);
    let (coordinator, _handle) = LohsSessionCoordinator::spawn(session, timeout);

    assert!(coordinator.is_active(), "Session must be active initially");
    assert_eq!(coordinator.gateway_ip(), Ipv4Addr::new(192, 168, 43, 1));
    assert_eq!(coordinator.local_ip(), Ipv4Addr::new(192, 168, 43, 100));

    // Wait 30ms (half of timeout): must still be active
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        coordinator.is_active(),
        "Watchdog fired prematurely before timeout"
    );

    // Wait until deactivation notification fires
    let deactivation_notify = coordinator.deactivation_notify();
    tokio::select! {
        _ = deactivation_notify.notified() => {}
        _ = tokio::time::sleep(Duration::from_millis(200)) => {
            panic!("Watchdog failed to fire within timeout window");
        }
    }

    assert!(
        !coordinator.is_active(),
        "Session must be inactive after watchdog triggers"
    );
}

#[tokio::test]
async fn test_linux_lohs_session_activity_resets_watchdog() {
    let session = ActiveLohsSession::new_mock(
        Ipv4Addr::new(192, 168, 43, 1),
        Ipv4Addr::new(192, 168, 43, 100),
    );
    let timeout = Duration::from_millis(80);
    let (coordinator, _handle) = LohsSessionCoordinator::spawn(session, timeout);

    assert!(coordinator.is_active());

    // Sleep 50ms, then record activity to reset the watchdog
    tokio::time::sleep(Duration::from_millis(50)).await;
    coordinator.record_activity();

    // Sleep another 50ms (total 100ms > timeout, but only 50ms since activity): MUST remain active
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        coordinator.is_active(),
        "Watchdog fired despite activity recording"
    );

    // Now wait for silence beyond timeout (80ms + margin)
    let deactivation_notify = coordinator.deactivation_notify();
    tokio::select! {
        _ = deactivation_notify.notified() => {}
        _ = tokio::time::sleep(Duration::from_millis(200)) => {
            panic!("Watchdog failed to fire after silence");
        }
    }

    assert!(
        !coordinator.is_active(),
        "Watchdog failed to fire after timeout from last activity"
    );
}

#[tokio::test]
async fn test_linux_lohs_session_explicit_deactivation() {
    let session = ActiveLohsSession::new_mock(
        Ipv4Addr::new(192, 168, 43, 1),
        Ipv4Addr::new(192, 168, 43, 100),
    );
    let (coordinator, _handle) =
        LohsSessionCoordinator::spawn(session, DEFAULT_LOHS_IDLE_TIMEOUT);

    assert!(coordinator.is_active());
    let res = coordinator.deactivate().await;
    assert!(res.is_ok(), "Explicit deactivation must succeed");
    assert!(!coordinator.is_active(), "Session must be inactive after explicit deactivation");
}

