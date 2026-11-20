//! Adversarial E2E Integration Stress Test Suite for Milestone 4 (archdrop_transfer)
//!
//! Tests:
//! 1. Multi-megabyte (4 MiB) payload transfer over WAN DERP relay with SHA-256 integrity verification.
//! 2. Fallback dialer under blackhole network conditions (198.51.100.1:53317) conforming strictly to <2.0s SLA.
//! 3. Concurrent file transfers over fallback dialer under simulated network partition.
//! 4. Invariant: Host WireGuard interface `tiktok` (10.2.0.2/32, table 51820) remains active and untouched.

use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_mesh::client::{DerpClient, DerpClientConfig};
use archdrop_mesh::crypto::MeshKeyPair;
use archdrop_mesh::dialer::FallbackDialer;
use archdrop_mesh::mock_relay::MockDerpRelay;
use archdrop_transfer::{TransferClient, TransferServer};
use sha2::{Digest, Sha256};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[tokio::test]
async fn test_adversarial_large_payload_derp_transfer_4mb() {
    // 1. Launch Mock DERP Relay
    let relay = MockDerpRelay::start().await.expect("Failed to start relay");
    let relay_url = relay.relay_url();

    // 2. Setup Receiver
    let temp_dir_receiver = tempfile::tempdir().unwrap();
    let download_dir = temp_dir_receiver.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let receiver_identity = Arc::new(CryptoIdentity::generate().unwrap());
    let receiver_mesh_key = Arc::new(MeshKeyPair::generate().unwrap());

    let mut receiver_config = Config::default();
    receiver_config.security.auto_accept_paired = true;
    receiver_config.security.verify_checksums = true;

    // 3. Setup Sender
    let temp_dir_sender = tempfile::tempdir().unwrap();
    let sender_identity = CryptoIdentity::generate().unwrap();
    let sender_mesh_key = Arc::new(MeshKeyPair::generate().unwrap());

    receiver_config.add_paired_fingerprint(sender_identity.fingerprint.clone());
    let receiver_config = Arc::new(receiver_config);

    // 4. Start Receiver with Mesh Listener
    let receiver_derp = DerpClient::connect(DerpClientConfig::new(
        &relay_url,
        receiver_mesh_key.clone(),
    ))
    .await
    .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = TransferServer::new(
        receiver_config.clone(),
        receiver_identity.clone(),
        download_dir.clone(),
        0,
        false,
    )
    .unwrap();

    let server_listener = receiver_derp.listener();
    tokio::spawn(async move {
        let _ = server.run_with_mesh(Some(server_listener), shutdown_rx).await;
    });

    // 5. Connect Sender
    let sender_derp = DerpClient::connect(DerpClientConfig::new(
        &relay_url,
        sender_mesh_key.clone(),
    ))
    .await
    .unwrap();

    let dialer = FallbackDialer::new(Some(sender_derp.clone()))
        .with_direct_timeout(Duration::from_millis(1500));

    // 6. Generate 4 MiB synthetic payload
    let payload_size = 4 * 1024 * 1024; // 4 MiB
    let mut file_data = vec![0u8; payload_size];
    for (i, b) in file_data.iter_mut().enumerate() {
        *b = ((i * 13) % 251) as u8;
    }
    let test_file_path = temp_dir_sender.path().join("large_stress_payload.bin");
    tokio::fs::write(&test_file_path, &file_data).await.unwrap();

    let mut expected_hasher = Sha256::new();
    expected_hasher.update(&file_data);
    let expected_sha256 = hex::encode(expected_hasher.finalize());

    // 7. Execute transfer over DERP mesh
    let client = TransferClient::new();
    let sender_config = Config::default();

    let summary = client
        .send_files_with_dialer(
            &dialer,
            None, // forces DERP relay
            Some(&receiver_mesh_key.public_key().0),
            false,
            &[test_file_path],
            &sender_config,
            &sender_identity,
        )
        .await
        .expect("Transfer of 4 MiB over DERP relay must succeed");

    assert_eq!(summary.files_sent, 1);
    assert_eq!(summary.total_bytes, payload_size as u64);

    // 8. Verify byte-for-byte SHA-256 match
    let received_path = download_dir.join("large_stress_payload.bin");
    assert!(received_path.exists(), "Received file must exist");

    let received_data = tokio::fs::read(&received_path).await.unwrap();
    assert_eq!(received_data.len(), payload_size);

    let mut actual_hasher = Sha256::new();
    actual_hasher.update(&received_data);
    let actual_sha256 = hex::encode(actual_hasher.finalize());

    assert_eq!(
        actual_sha256, expected_sha256,
        "SHA-256 integrity digest of 4 MiB transfer must match bit-for-bit"
    );

    let _ = shutdown_tx.send(true);
    sender_derp.close().await.unwrap();
    receiver_derp.close().await.unwrap();
    relay.stop().await;
}

#[tokio::test]
async fn test_adversarial_blackhole_fallback_sla_timing() {
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_url = relay.relay_url();

    let temp_dir_receiver = tempfile::tempdir().unwrap();
    let download_dir = temp_dir_receiver.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let receiver_identity = Arc::new(CryptoIdentity::generate().unwrap());
    let receiver_mesh_key = Arc::new(MeshKeyPair::generate().unwrap());

    let mut receiver_config = Config::default();
    receiver_config.security.auto_accept_paired = true;
    receiver_config.security.verify_checksums = true;

    let temp_dir_sender = tempfile::tempdir().unwrap();
    let sender_identity = CryptoIdentity::generate().unwrap();
    let sender_mesh_key = Arc::new(MeshKeyPair::generate().unwrap());

    receiver_config.add_paired_fingerprint(sender_identity.fingerprint.clone());
    let receiver_config = Arc::new(receiver_config);

    let receiver_derp = DerpClient::connect(DerpClientConfig::new(
        &relay_url,
        receiver_mesh_key.clone(),
    ))
    .await
    .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = TransferServer::new(
        receiver_config.clone(),
        receiver_identity.clone(),
        download_dir.clone(),
        0,
        false,
    )
    .unwrap();

    let server_listener = receiver_derp.listener();
    tokio::spawn(async move {
        let _ = server.run_with_mesh(Some(server_listener), shutdown_rx).await;
    });

    let sender_derp = DerpClient::connect(DerpClientConfig::new(
        &relay_url,
        sender_mesh_key.clone(),
    ))
    .await
    .unwrap();

    // Direct timeout = 1500ms
    let dialer = FallbackDialer::new(Some(sender_derp.clone()))
        .with_direct_timeout(Duration::from_millis(1500));

    let test_file = temp_dir_sender.path().join("blackhole_test.txt");
    tokio::fs::write(&test_file, b"blackhole_sla_payload").await.unwrap();

    let client = TransferClient::new();
    let sender_config = Config::default();

    let start = Instant::now();
    let summary = client
        .send_files_with_dialer(
            &dialer,
            Some(("198.51.100.1", 53317)), // RFC 5737 TEST-NET-2 non-routable blackhole
            Some(&receiver_mesh_key.public_key().0),
            false,
            &[test_file],
            &sender_config,
            &sender_identity,
        )
        .await
        .expect("Transfer must succeed via DERP fallback upon blackhole timeout");

    let elapsed = start.elapsed();

    // Verify SLA: Direct timeout is 1500ms, total transfer time must be < 2000ms
    assert!(
        elapsed >= Duration::from_millis(1450),
        "Must wait for direct timeout of ~1500ms before falling back (got {elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_millis(2000),
        "Total fallback transfer must complete within 2.0s SLA (got {elapsed:?})"
    );
    assert_eq!(summary.files_sent, 1);

    let received_data = tokio::fs::read(download_dir.join("blackhole_test.txt")).await.unwrap();
    assert_eq!(received_data, b"blackhole_sla_payload");

    let _ = shutdown_tx.send(true);
    sender_derp.close().await.unwrap();
    receiver_derp.close().await.unwrap();
    relay.stop().await;
}

#[tokio::test]
async fn test_adversarial_concurrent_fallback_transfers() {
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_url = relay.relay_url();

    let temp_dir_receiver = tempfile::tempdir().unwrap();
    let download_dir = temp_dir_receiver.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let receiver_identity = Arc::new(CryptoIdentity::generate().unwrap());
    let receiver_mesh_key = Arc::new(MeshKeyPair::generate().unwrap());

    let mut receiver_config = Config::default();
    receiver_config.security.auto_accept_paired = true;
    receiver_config.security.verify_checksums = true;

    // 3 concurrent senders
    let num_senders = 3;
    let mut sender_identities = Vec::new();
    let mut sender_mesh_keys = Vec::new();

    for _ in 0..num_senders {
        let sid = CryptoIdentity::generate().unwrap();
        let smk = Arc::new(MeshKeyPair::generate().unwrap());
        receiver_config.add_paired_fingerprint(sid.fingerprint.clone());
        sender_identities.push(sid);
        sender_mesh_keys.push(smk);
    }

    let receiver_config = Arc::new(receiver_config);

    let receiver_derp = DerpClient::connect(DerpClientConfig::new(
        &relay_url,
        receiver_mesh_key.clone(),
    ))
    .await
    .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = TransferServer::new(
        receiver_config.clone(),
        receiver_identity.clone(),
        download_dir.clone(),
        0,
        false,
    )
    .unwrap();

    let server_listener = receiver_derp.listener();
    tokio::spawn(async move {
        let _ = server.run_with_mesh(Some(server_listener), shutdown_rx).await;
    });

    let mut handles = Vec::new();

    for i in 0..num_senders {
        let r_url = relay_url.clone();
        let smk = sender_mesh_keys[i].clone();
        let sid = sender_identities[i].clone();
        let r_pub = receiver_mesh_key.public_key().0;

        let handle = tokio::spawn(async move {
            let sender_derp = DerpClient::connect(DerpClientConfig::new(&r_url, smk))
                .await
                .unwrap();

            let dialer = FallbackDialer::new(Some(sender_derp.clone()))
                .with_direct_timeout(Duration::from_millis(1500));

            let temp_dir = tempfile::tempdir().unwrap();
            let file_name = format!("concurrent_file_{}.bin", i);
            let test_file = temp_dir.path().join(&file_name);
            let payload = format!("concurrent_content_{}", i).into_bytes();
            tokio::fs::write(&test_file, &payload).await.unwrap();

            let client = TransferClient::new();
            let config = Config::default();

            // Dial blackhole IP with different IP addresses per sender
            let blackhole_ip = format!("198.51.100.{}", i + 1);
            let summary = client
                .send_files_with_dialer(
                    &dialer,
                    Some((&blackhole_ip, 53317)),
                    Some(&r_pub),
                    false,
                    &[test_file],
                    &config,
                    &sid,
                )
                .await
                .expect("Concurrent transfer with fallback must succeed");

            assert_eq!(summary.files_sent, 1);
            sender_derp.close().await.unwrap();
            (file_name, payload)
        });

        handles.push(handle);
    }

    for h in handles {
        let (file_name, expected_payload) = h.await.unwrap();
        let received = tokio::fs::read(download_dir.join(&file_name)).await.unwrap();
        assert_eq!(received, expected_payload);
    }

    let _ = shutdown_tx.send(true);
    receiver_derp.close().await.unwrap();
    relay.stop().await;
}

#[test]
fn test_adversarial_host_wireguard_safety() {
    let out = Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output()
        .expect("Failed to execute ip addr");
    assert!(out.status.success(), "tiktok interface must be active");
    let out_str = String::from_utf8_lossy(&out.stdout);
    assert!(
        out_str.contains("10.2.0.2"),
        "tiktok must have 10.2.0.2: {out_str}"
    );

    let route_out = Command::new("ip")
        .args(["route", "show", "table", "51820"])
        .output()
        .expect("Failed to execute ip route table 51820");
    assert!(route_out.status.success(), "route table 51820 must exist");
    let route_str = String::from_utf8_lossy(&route_out.stdout);
    assert!(
        route_str.contains("default dev tiktok"),
        "table 51820 must have default dev tiktok: {route_str}"
    );
}
