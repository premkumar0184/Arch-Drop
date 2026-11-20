use archdrop_mesh::client::{DerpClient, DerpClientConfig};
use archdrop_mesh::crypto::MeshKeyPair;
use archdrop_mesh::mock_relay::MockDerpRelay;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn test_relay_e2e_two_peers_virtual_stream_transfer() {
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .unwrap();

    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .unwrap();

    let listener_b = client_b.listener();
    let key_a_pub = *client_a.local_key();

    // 256 KiB payload from A to B
    let mut payload_a_to_b = vec![0u8; 256 * 1024];
    for (idx, b) in payload_a_to_b.iter_mut().enumerate() {
        *b = (idx % 241) as u8;
    }
    let mut hasher_a = Sha256::new();
    hasher_a.update(&payload_a_to_b);
    let expected_hash_a = hasher_a.finalize();

    // 128 KiB payload from B to A
    let mut payload_b_to_a = vec![0u8; 128 * 1024];
    for (idx, b) in payload_b_to_a.iter_mut().enumerate() {
        *b = (idx % 239) as u8;
    }
    let mut hasher_b = Sha256::new();
    hasher_b.update(&payload_b_to_a);
    let expected_hash_b = hasher_b.finalize();

    let payload_a_to_b_clone = payload_a_to_b.clone();
    let payload_b_to_a_clone = payload_b_to_a.clone();

    // Receiver task on B
    let handle_b = tokio::spawn(async move {
        let (mut stream_b, remote_peer) = listener_b.accept().await.unwrap();
        assert_eq!(&remote_peer, &key_a_pub);

        // Read all bytes sent by A
        let mut received = vec![0u8; payload_a_to_b_clone.len()];
        stream_b.read_exact(&mut received).await.unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&received);
        let actual_hash_a = hasher.finalize();

        // Send response to A
        stream_b.write_all(&payload_b_to_a_clone).await.unwrap();
        stream_b.flush().await.unwrap();
        stream_b.shutdown().await.unwrap();

        actual_hash_a
    });

    // Sender task on A
    let mut stream_a = client_a.open_stream(client_b.local_key()).await.unwrap();
    stream_a.write_all(&payload_a_to_b).await.unwrap();
    stream_a.flush().await.unwrap();

    let mut received_at_a = Vec::new();
    stream_a.read_to_end(&mut received_at_a).await.unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&received_at_a);
    let actual_hash_b = hasher.finalize();

    let actual_hash_a = handle_b.await.unwrap();

    assert_eq!(actual_hash_a, expected_hash_a);
    assert_eq!(actual_hash_b, expected_hash_b);

    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
}

#[tokio::test]
async fn test_relay_e2e_keepalive_heartbeats() {
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_url = relay.relay_url();

    let key = Arc::new(MeshKeyPair::generate().unwrap());
    let mut config = DerpClientConfig::new(&relay_url, key);
    config.keepalive_interval = Duration::from_millis(50); // fast heartbeat for test

    let client = DerpClient::connect(config).await.unwrap();
    assert!(client.is_connected());

    // Allow multiple keepalive heartbeats to tick
    tokio::time::sleep(Duration::from_millis(250)).await;

    assert!(client.is_connected(), "Client must remain connected after heartbeats");
    client.close().await.unwrap();
    relay.stop().await;
}
