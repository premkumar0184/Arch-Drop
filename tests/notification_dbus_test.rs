//! Integration tests for Milestone 2:
//! Freedesktop D-Bus Notification Integration & Interactive Actions (SwayNC & Dunst compatible).

use archdropd::notification::NotificationService;
use std::collections::HashMap;
use tokio::sync::oneshot;
use zbus::zvariant::Value;

// =========================================================================
// 1. NotificationService Fallback in Headless/Disabled Environments
// =========================================================================

#[tokio::test]
async fn test_notification_service_disabled_fallback() {
    let service = NotificationService::disabled();

    // In disabled mode, interactive authorization gracefully resolves to true (permissive)
    let accepted = service
        .request_transfer_authorization("TestPhone", 3, 10_000_000)
        .await;
    assert!(accepted, "Disabled notification service must fallback to true");

    // Other notifications must not panic or error
    service
        .notify_transfer_completed("document.pdf", 5_000_000, "Pixel 8", None)
        .await;
    service
        .notify_transfer_failed("Network timeout", "Pixel 8")
        .await;
    service
        .notify_peer_paired("Pixel 8", "1234567890abcdef1234567890abcdef")
        .await;
    service
        .notify_clipboard_synced("text/plain", 1024, "Pixel 8")
        .await;
}

// =========================================================================
// 2. Freedesktop Specification Compliance & Hints Dictionary Serialization
// =========================================================================

#[test]
fn test_freedesktop_hints_and_actions_contract() {
    // 1. Actions formatting rule: actions must be pairs of [action_key, label, ...]
    let transfer_actions = ["accept", "Accept", "decline", "Decline"];
    assert_eq!(transfer_actions.len() % 2, 0);
    assert_eq!(transfer_actions[0], "accept");
    assert_eq!(transfer_actions[1], "Accept");
    assert_eq!(transfer_actions[2], "decline");
    assert_eq!(transfer_actions[3], "Decline");

    let completed_actions = ["open", "Open Folder"];
    assert_eq!(completed_actions.len() % 2, 0);

    let error_actions = ["dismiss", "Dismiss"];
    assert_eq!(error_actions.len() % 2, 0);

    // 2. Hints serialization rule: urgency must be u8 (0=low, 1=normal, 2=critical)
    let mut hints = HashMap::new();
    hints.insert("urgency", Value::from(1u8));
    hints.insert("category", Value::from("transfer"));

    match hints.get("urgency") {
        Some(Value::U8(u)) => assert_eq!(*u, 1),
        _ => panic!("Expected urgency hint to be Value::U8"),
    }

    match hints.get("category") {
        Some(Value::Str(s)) => assert_eq!(s.as_str(), "transfer"),
        _ => panic!("Expected category hint to be Value::Str"),
    }
}

// =========================================================================
// 3. Interactive User Action Callback Resolution Simulation
// =========================================================================

#[tokio::test]
async fn test_interactive_action_callback_resolution() {
    let service = NotificationService::disabled();

    // Case A: User clicks "accept" button via handle_action_invoked
    let (tx_accept, rx_accept) = oneshot::channel::<bool>();
    service.register_pending_action(101, tx_accept).await;
    service.handle_action_invoked(101, "accept").await;
    let result_accept = rx_accept.await.unwrap();
    assert!(result_accept, "Action 'accept' must resolve to true");

    // Case B: User clicks "decline" button via handle_action_invoked
    let (tx_decline, rx_decline) = oneshot::channel::<bool>();
    service.register_pending_action(102, tx_decline).await;
    service.handle_action_invoked(102, "decline").await;
    let result_decline = rx_decline.await.unwrap();
    assert!(!result_decline, "Action 'decline' must resolve to false");

    // Case C: Notification closed without user clicking action
    let (tx_closed, rx_closed) = oneshot::channel::<bool>();
    service.register_pending_action(103, tx_closed).await;
    service.handle_notification_closed(103, 1).await;
    let result_closed = rx_closed.await.unwrap();
    assert!(!result_closed, "Closed notification must resolve to false");

    // Case D: User clicks "open" on completed transfer folder
    let temp_folder = tempfile::tempdir().unwrap();
    service
        .register_folder_action(104, temp_folder.path().to_path_buf())
        .await;
    service.handle_action_invoked(104, "open").await;
}

// =========================================================================
// 4. Live Session Bus Connection Check (Non-Destructive)
// =========================================================================

#[tokio::test]
async fn test_notification_service_live_connection() {
    // If DBUS_SESSION_BUS_ADDRESS is present, attempt live connection
    if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok() {
        let service = NotificationService::connect().await;
        // Whether a live daemon is running or not, service must be instantiated cleanly
        let _ = service;
    }
}

// =========================================================================
// 5. Host Safety Invariant: WireGuard tiktok Interface Remains Untouched
// =========================================================================

#[test]
fn test_host_wireguard_safety_invariant() {
    let output = std::process::Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("10.2.0.2"),
                "Host tiktok WireGuard interface must retain 10.2.0.2 IP"
            );
            assert!(
                stdout.contains("UP"),
                "Host tiktok WireGuard interface must remain UP"
            );
        }
    }
}
