//! Integration tests for Milestone 2:
//! Event-Driven Waybar IPC Socket Streaming, Speed Engine, and State Machine.

use archdropd::events::{
    calculate_eta, calculate_percentage, format_eta, format_size, format_speed, DaemonEvent,
    DaemonState, Direction, SpeedEngine, WaybarPayload,
};
use archdropd::ipc::{DaemonStatusEvent, IpcServer};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, watch, RwLock};

// =========================================================================
// 1. WaybarPayload Schema and Serialization Contract
// =========================================================================

#[test]
fn test_waybar_payload_schema_and_serialization() {
    let payload = WaybarPayload {
        text: "󰇚 14.5 MB/s (45%)".to_string(),
        tooltip: "<b>󰇚 Receiving</b>: archive.tar\n<b>From</b>: Pixel 8\n<b>Progress</b>: 45.0 MB / 100.0 MB (45%)\n<b>Speed</b>: 14.5 MB/s | <b>ETA</b>: 4s".to_string(),
        class: vec!["transferring".to_string(), "inbound".to_string()],
        percentage: Some(45),
    };

    let json_str = serde_json::to_string(&payload).expect("Failed to serialize WaybarPayload");
    let json_val: serde_json::Value =
        serde_json::from_str(&json_str).expect("Failed to parse serialized JSON");

    assert_eq!(json_val["text"], "󰇚 14.5 MB/s (45%)");
    assert_eq!(json_val["percentage"], 45);
    assert_eq!(json_val["class"][0], "transferring");
    assert_eq!(json_val["class"][1], "inbound");
    assert!(json_val["tooltip"].as_str().unwrap().contains("archive.tar"));

    // Deserialization roundtrip
    let deserialized: WaybarPayload =
        serde_json::from_str(&json_str).expect("Roundtrip deserialize failed");
    assert_eq!(payload, deserialized);

    // Ensure percentage is omitted when None
    let idle_payload = WaybarPayload {
        text: "󰄡 ArchDrop".to_string(),
        tooltip: "<b>Arch Drop</b>: Ready".to_string(),
        class: vec!["idle".to_string()],
        percentage: None,
    };
    let idle_json = serde_json::to_string(&idle_payload).unwrap();
    assert!(!idle_json.contains("percentage"));
}

// =========================================================================
// 2. Speed Engine (EMA alpha=0.35), ETA & Unit Formatting
// =========================================================================

#[test]
fn test_speed_engine_ema_and_formatting() {
    let mut engine = SpeedEngine::new();
    let start = Instant::now();

    // Initial update at t=0
    let s0 = engine.update(0, start);
    assert_eq!(s0, 0);

    // 10 MB in 1 second => instantaneous 10,000,000 B/s
    let t1 = start + Duration::from_secs(1);
    let s1 = engine.update(10_000_000, t1);
    assert_eq!(s1, 10_000_000);

    // Another 20 MB in next 1 second => inst = 20,000,000 B/s
    // EMA = 0.35 * 20_000_000 + 0.65 * 10_000_000 = 7_000_000 + 6_500_000 = 13_500_000
    let t2 = t1 + Duration::from_secs(1);
    let s2 = engine.update(30_000_000, t2);
    assert_eq!(s2, 13_500_000);

    // Test speed formatters
    assert_eq!(format_speed(500), "500 B/s");
    assert_eq!(format_speed(256 * 1024), "256.0 KB/s");
    assert_eq!(format_speed(14 * 1024 * 1024), "14.0 MB/s");
    assert_eq!(format_speed(2 * 1024 * 1024 * 1024), "2.0 GB/s");

    // Test size formatters
    assert_eq!(format_size(100), "100 B");
    assert_eq!(format_size(2048), "2.0 KB");
    assert_eq!(format_size(10 * 1024 * 1024), "10.0 MB");
    assert_eq!(format_size(4 * 1024 * 1024 * 1024), "4.0 GB");

    // Test percentage calculation
    assert_eq!(calculate_percentage(0, 100), 0);
    assert_eq!(calculate_percentage(45, 100), 45);
    assert_eq!(calculate_percentage(100, 100), 100);
    assert_eq!(calculate_percentage(150, 100), 100); // Clamped at 100
    assert_eq!(calculate_percentage(50, 0), 0);

    // Test ETA calculation
    let eta = calculate_eta(50_000_000, 100_000_000, 10_000_000);
    assert_eq!(eta, Some(Duration::from_secs(5)));
    assert_eq!(format_eta(eta), "5s");

    let eta_min = calculate_eta(0, 120_000_000, 1_000_000);
    assert_eq!(format_eta(eta_min), "2m 0s");

    assert_eq!(calculate_eta(100, 100, 10), None);
    assert_eq!(format_eta(None), "--");
}

// =========================================================================
// 3. Daemon State Machine Transitions
// =========================================================================

#[test]
fn test_daemon_state_machine_transitions() {
    let now = Instant::now();
    let mut state = DaemonState::new(0, 53317, false);

    // 1. Initial state is Idle
    let p0 = state.to_waybar_payload(now);
    assert_eq!(p0.class, vec!["idle".to_string()]);
    assert!(p0.percentage.is_none());
    assert!(p0.tooltip.contains("Listening (:53317)"));

    // 2. Peer connected -> transitions to paired
    let p1 = state.apply_event(
        &DaemonEvent::PeerConnected {
            alias: "Pixel 8".to_string(),
            address: "10.2.0.2:53317".to_string(),
            fingerprint: "abc123".to_string(),
        },
        now,
    );
    assert_eq!(p1.class, vec!["paired".to_string()]);
    assert_eq!(p1.text, "󰄡 Pixel 8");

    // 3. Transfer started (Inbound)
    let p2 = state.apply_event(
        &DaemonEvent::TransferStarted {
            session_id: "sess-1".to_string(),
            peer_alias: "Pixel 8".to_string(),
            direction: Direction::Inbound,
            total_files: 1,
            total_bytes: 100_000_000,
        },
        now,
    );
    assert!(p2.class.contains(&"transferring".to_string()));
    assert!(p2.class.contains(&"inbound".to_string()));

    // 4. Transfer progress ticks
    let p3 = state.apply_event(
        &DaemonEvent::TransferProgress {
            session_id: "sess-1".to_string(),
            file_name: "archive.tar.gz".to_string(),
            bytes_transferred: 50_000_000,
            total_bytes: 100_000_000,
            speed_bps: 12_500_000,
            percentage: 50,
        },
        now,
    );
    assert_eq!(p3.percentage, Some(50));
    assert!(p3.text.contains("11.9 MB/s (50%)"));
    assert!(p3.tooltip.contains("archive.tar.gz"));

    // 5. Transfer completed -> transitions to completed with 100%
    let p4 = state.apply_event(
        &DaemonEvent::TransferCompleted {
            session_id: "sess-1".to_string(),
            peer_alias: "Pixel 8".to_string(),
            file_name: "archive.tar.gz".to_string(),
            file_count: 1,
            total_bytes: 100_000_000,
        },
        now,
    );
    assert_eq!(p4.class, vec!["completed".to_string()]);
    assert_eq!(p4.percentage, Some(100));
    assert!(p4.text.contains("Received (95.4 MB)"));

    // 6. After 6 seconds, transient completed state expires back to paired
    let later = now + Duration::from_secs(6);
    let p5 = state.to_waybar_payload(later);
    assert_eq!(p5.class, vec!["paired".to_string()]);

    // 7. Clipboard sync event -> transitions to clipboard
    let p6 = state.apply_event(
        &DaemonEvent::ClipboardSynced {
            mime_type: "text/plain".to_string(),
            byte_size: 4096,
            origin_device: "Pixel 8".to_string(),
        },
        now,
    );
    assert!(p6.class.contains(&"clipboard".to_string()));
    assert!(p6.text.contains("Clip: 4.0 KB"));

    // 8. Error event
    let p7 = state.apply_event(
        &DaemonEvent::TransferFailed {
            session_id: "sess-err".to_string(),
            peer_alias: "Pixel 8".to_string(),
            error: "Checksum mismatch".to_string(),
        },
        now,
    );
    assert_eq!(p7.class, vec!["error".to_string()]);
    assert_eq!(p7.text, "󰅚 Transfer Failed");
}

// =========================================================================
// 4. Socket Permissions and Lifecycle Check
// =========================================================================

#[tokio::test]
async fn test_socket_permissions_and_lifecycle() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_dir = tmp.path().join("archdrop");
    let sock_path = sock_dir.join("archdropd.sock");

    let (event_tx, _) = broadcast::channel(16);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(0)));

    let server = IpcServer::bind(&sock_path, event_tx, shutdown_rx, status)
        .expect("Failed to bind IPC server");

    let sock_meta = std::fs::metadata(&sock_path).expect("Failed to read socket metadata");
    let sock_mode = sock_meta.permissions().mode() & 0o777;
    assert_eq!(
        sock_mode, 0o600,
        "IPC socket file must have 0o600 permissions"
    );

    drop(server);
    assert!(
        !sock_path.exists(),
        "Socket file must be cleaned up on server drop"
    );
}

// =========================================================================
// 5. Waybar Follow Stream (Single Client)
// =========================================================================

#[tokio::test]
async fn test_waybar_follow_stream_single_client() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("waybar_stream.sock");

    let (event_tx, _) = broadcast::channel(32);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(1)));

    let server = IpcServer::bind(&sock_path, event_tx.clone(), shutdown_rx, status.clone())
        .expect("Bind failed");

    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Connect client
    let stream = UnixStream::connect(&sock_path)
        .await
        .expect("Failed to connect to socket");
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Request Waybar stream
    writer
        .write_all(b"status --follow\n")
        .await
        .expect("Failed to write follow command");

    // 1. Initial snapshot line must be valid WaybarPayload in idle/paired state
    let line1 = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected line 1");
    let payload1: WaybarPayload =
        serde_json::from_str(&line1).expect("Failed to deserialize initial WaybarPayload");
    assert!(payload1.class.contains(&"idle".to_string()) || payload1.class.contains(&"paired".to_string()));

    // 2. Broadcast transfer update
    let transfer_event = DaemonStatusEvent::transferring("test.tar", 42.0, 10_000_000);
    event_tx
        .send(transfer_event)
        .expect("Failed to send transfer event");

    let line2 = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected line 2");
    let payload2: WaybarPayload =
        serde_json::from_str(&line2).expect("Failed to deserialize transfer WaybarPayload");
    assert_eq!(payload2.class, vec!["transferring".to_string()]);
    assert_eq!(payload2.percentage, Some(42));
    assert!(payload2.tooltip.contains("test.tar"));

    // 3. Monotonic progress increase
    let transfer_event_85 = DaemonStatusEvent::transferring("test.tar", 85.0, 20_000_000);
    event_tx
        .send(transfer_event_85)
        .expect("Failed to send transfer event");

    let line3 = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected line 3");
    let payload3: WaybarPayload =
        serde_json::from_str(&line3).expect("Failed to deserialize transfer WaybarPayload");
    assert_eq!(payload3.percentage, Some(85));
}

// =========================================================================
// 6. Alternative Follow Command Permutations
// =========================================================================

#[tokio::test]
async fn test_waybar_alternative_follow_commands() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("alt_cmds.sock");

    let (event_tx, _) = broadcast::channel(16);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(0)));

    let server = IpcServer::bind(&sock_path, event_tx.clone(), shutdown_rx, status.clone())
        .expect("Bind failed");
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Test command: 'follow'
    {
        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer.write_all(b"follow\n").await.unwrap();

        let line = lines.next_line().await.unwrap().unwrap();
        let payload: WaybarPayload = serde_json::from_str(&line).unwrap();
        assert_eq!(payload.class, vec!["idle".to_string()]);
    }

    // Test command: '{"cmd": "subscribe", "format": "waybar"}'
    {
        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer
            .write_all(b"{\"cmd\": \"subscribe\", \"format\": \"waybar\"}\n")
            .await
            .unwrap();

        let line = lines.next_line().await.unwrap().unwrap();
        let payload: WaybarPayload = serde_json::from_str(&line).unwrap();
        assert_eq!(payload.class, vec!["idle".to_string()]);
    }

    // Test command: 'ping'
    {
        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer.write_all(b"ping\n").await.unwrap();

        let line = lines.next_line().await.unwrap().unwrap();
        let pong: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(pong["type"], "pong");
    }
}

// =========================================================================
// 7. Concurrent Clients Stress Broadcast (5 clients, zero deadlocks)
// =========================================================================

#[tokio::test]
async fn test_waybar_concurrent_clients_broadcast_stress() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("concurrent_stress.sock");

    let (event_tx, _) = broadcast::channel(128);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(1)));

    let server = IpcServer::bind(&sock_path, event_tx.clone(), shutdown_rx, status.clone())
        .expect("Bind failed");
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    const NUM_CLIENTS: usize = 5;
    const NUM_EVENTS: usize = 10;

    let mut handles = Vec::new();
    for client_id in 0..NUM_CLIENTS {
        let path = sock_path.clone();
        handles.push(tokio::spawn(async move {
            let stream = UnixStream::connect(&path).await.expect("Client connect failed");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();

            writer.write_all(b"status --follow\n").await.expect("Write follow failed");

            // Line 1: initial snapshot
            let _initial = lines.next_line().await.unwrap().unwrap();

            // Receive NUM_EVENTS broadcasted lines
            let mut received_count = 0;
            while received_count < NUM_EVENTS {
                if let Ok(Some(line)) = lines.next_line().await {
                    let payload: Result<WaybarPayload, _> = serde_json::from_str(&line);
                    assert!(payload.is_ok(), "Client {client_id} received invalid JSON: {line}");
                    received_count += 1;
                }
            }
            received_count
        }));
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Broadcast NUM_EVENTS
    for i in 1..=NUM_EVENTS {
        let progress = (i * 10) as f64;
        let event = DaemonStatusEvent::transferring("bigfile.iso", progress, 15_000_000);
        event_tx.send(event).expect("Broadcast failed");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    // Await all client handles
    for handle in handles {
        let count = handle.await.expect("Client task panicked");
        assert_eq!(count, NUM_EVENTS, "Each client must receive all events");
    }
}

// =========================================================================
// 8. Host Safety Invariant: WireGuard tiktok Interface Remains Untouched
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
