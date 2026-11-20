use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_mesh::client::{DerpClient, DerpClientConfig};
use archdrop_mesh::crypto::MeshKeyPair;
use archdrop_mesh::dialer::FallbackDialer;
use archdrop_mesh::mock_relay::MockDerpRelay;
use archdrop_transfer::{TransferClient, TransferServer};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::test]
async fn test_e2e_localsend_transfer_over_derp_mesh() {
    // 1. Launch local Mock DERP Relay
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_url = relay.relay_url();

    // 2. Setup Receiver (Server) Identity, Mesh Key, and Temp Dir
    let temp_dir_receiver = tempfile::tempdir().unwrap();
    let download_dir = temp_dir_receiver.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let receiver_identity = Arc::new(CryptoIdentity::generate().unwrap());
    let receiver_mesh_key = Arc::new(MeshKeyPair::generate().unwrap());

    let mut receiver_config = Config::default();
    receiver_config.security.auto_accept_paired = true;
    receiver_config.security.verify_checksums = true;

    // 3. Setup Sender Identity and Mesh Key
    let temp_dir_sender = tempfile::tempdir().unwrap();
    let sender_identity = CryptoIdentity::generate().unwrap();
    let sender_mesh_key = Arc::new(MeshKeyPair::generate().unwrap());

    // Pair sender with receiver
    receiver_config.add_paired_fingerprint(sender_identity.fingerprint.clone());
    let receiver_config = Arc::new(receiver_config);

    // 4. Connect Receiver to DERP Relay and start TransferServer with mesh listener
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
        0,     // random port
        false, // plain HTTP inside the test
    )
    .unwrap();

    let server_listener = receiver_derp.listener();
    tokio::spawn(async move {
        let _ = server.run_with_mesh(Some(server_listener), shutdown_rx).await;
    });

    // 5. Connect Sender to DERP Relay and create FallbackDialer
    let sender_derp = DerpClient::connect(DerpClientConfig::new(
        &relay_url,
        sender_mesh_key.clone(),
    ))
    .await
    .unwrap();

    let dialer = FallbackDialer::new(Some(sender_derp.clone()));

    // 6. Create a synthetic test file (64 KiB)
    let test_file_path = temp_dir_sender.path().join("wan_test_document.txt");
    let mut file_data = vec![0u8; 64 * 1024];
    for (i, b) in file_data.iter_mut().enumerate() {
        *b = (i % 253) as u8;
    }
    tokio::fs::write(&test_file_path, &file_data).await.unwrap();

    let mut expected_hasher = Sha256::new();
    expected_hasher.update(&file_data);
    let expected_sha256 = hex::encode(expected_hasher.finalize());

    // 7. Execute transfer via FallbackDialer pointing to receiver's mesh public key
    let client = TransferClient::new();
    let sender_config = Config::default();

    let summary = client
        .send_files_with_dialer(
            &dialer,
            None, // No direct target, forces DERP fallback
            Some(&receiver_mesh_key.public_key().0),
            false,
            &[test_file_path],
            &sender_config,
            &sender_identity,
        )
        .await
        .expect("Transfer over DERP mesh must succeed");

    assert_eq!(summary.files_sent, 1);
    assert_eq!(summary.total_bytes, file_data.len() as u64);

    // 8. Verify the file arrived in the receiver's download directory with identical SHA-256
    let received_file_path = download_dir.join("wan_test_document.txt");
    assert!(
        received_file_path.exists(),
        "Transferred file must exist in download_dir"
    );

    let received_data = tokio::fs::read(&received_file_path).await.unwrap();
    let mut actual_hasher = Sha256::new();
    actual_hasher.update(&received_data);
    let actual_sha256 = hex::encode(actual_hasher.finalize());

    assert_eq!(
        actual_sha256, expected_sha256,
        "SHA-256 integrity digest must match exactly"
    );

    // 9. Shutdown
    let _ = shutdown_tx.send(true);
    sender_derp.close().await.unwrap();
    receiver_derp.close().await.unwrap();
    relay.stop().await;
}

#[tokio::test]
async fn test_e2e_localsend_transfer_with_fallback_timeout() {
    // Verifies that when a direct IP address is unreachable (e.g. 198.51.100.1:53317),
    // FallbackDialer times out after direct_timeout and transparently completes the transfer over DERP
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

    // Configure 1500ms direct timeout
    let dialer = FallbackDialer::new(Some(sender_derp.clone()))
        .with_direct_timeout(std::time::Duration::from_millis(1500));

    let test_file_path = temp_dir_sender.path().join("fallback_transfer.bin");
    tokio::fs::write(&test_file_path, b"transparent_fallback_content").await.unwrap();

    let client = TransferClient::new();
    let sender_config = Config::default();

    let start = std::time::Instant::now();
    let summary = client
        .send_files_with_dialer(
            &dialer,
            Some(("198.51.100.1", 53317)), // Unroutable blackhole IP
            Some(&receiver_mesh_key.public_key().0),
            false,
            &[test_file_path],
            &sender_config,
            &sender_identity,
        )
        .await
        .expect("Transfer via fallback dialer must succeed");

    let elapsed = start.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(1450),
        "Direct timeout must have elapsed at least ~1500ms (got {elapsed:?})"
    );
    assert_eq!(summary.files_sent, 1);

    let received_content = tokio::fs::read(download_dir.join("fallback_transfer.bin")).await.unwrap();
    assert_eq!(received_content, b"transparent_fallback_content");

    let _ = shutdown_tx.send(true);
    sender_derp.close().await.unwrap();
    receiver_derp.close().await.unwrap();
    relay.stop().await;
}
