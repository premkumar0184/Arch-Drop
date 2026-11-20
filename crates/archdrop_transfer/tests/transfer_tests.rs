use archdrop_clipboard::{ClipboardPayloadCache, WaylandClipboardAdapter};
use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_core::models::clipboard::{ClipboardInlinePacket, ClipboardInlinePayload, ClipboardPacket};
use archdrop_transfer::{TransferClient, TransferServer};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[tokio::test]
async fn test_server_info_and_register_endpoints() {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = Arc::new(Config::default());
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    // Bind on random available port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = TransferServer::new(
        config.clone(),
        identity.clone(),
        download_dir,
        port,
        false, // plain HTTP
    )
    .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    // Allow server to bind
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = TransferClient::new();
    let info = client.get_info("127.0.0.1", port, false, None).await.unwrap();
    assert_eq!(info.alias, "Arch Drop");
    assert_eq!(info.device_model, Some("Arch Linux (Hyprland)".to_string()));
    assert_eq!(info.version, "2.2");
    assert_eq!(info.fingerprint, identity.fingerprint);

    let sender_reg = archdrop_core::models::dto_v2::RegisterDtoV2 {
        alias: "Test Peer".to_string(),
        version: "2.2".to_string(),
        device_model: Some("Android 14".to_string()),
        device_type: Some(archdrop_core::models::dto_v2::DeviceType::Mobile),
        fingerprint: "PEER_FP_1234567890".to_string(),
        port: 53317,
        protocol: archdrop_core::models::dto_v2::ProtocolType::Http,
        download: true,
    };

    let reg_res = client
        .register("127.0.0.1", port, false, None, &sender_reg)
        .await
        .unwrap();
    assert_eq!(reg_res.fingerprint, identity.fingerprint);

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_direct_point_to_point_transfer_loopback() {
    let temp_dir = TempDir::new().unwrap();
    let sender_dir = temp_dir.path().join("sender");
    let receiver_dir = temp_dir.path().join("receiver");
    tokio::fs::create_dir_all(&sender_dir).await.unwrap();
    tokio::fs::create_dir_all(&receiver_dir).await.unwrap();

    // 1. Generate 2 MB synthetic payload
    let test_file_path = sender_dir.join("payload_2mb.bin");
    let mut file = tokio::fs::File::create(&test_file_path).await.unwrap();
    let chunk = vec![0xABu8; 64 * 1024]; // 64 KiB
    let mut total_written = 0;
    for _ in 0..32 {
        file.write_all(&chunk).await.unwrap();
        total_written += chunk.len() as u64;
    }
    file.flush().await.unwrap();
    drop(file);

    let original_bytes = tokio::fs::read(&test_file_path).await.unwrap();
    let expected_sha256 = compute_sha256(&original_bytes);

    // 2. Start Receiver Transfer Server
    let config = Arc::new(Config::default());
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = TransferServer::new(
        config.clone(),
        identity.clone(),
        receiver_dir.clone(),
        port,
        false,
    )
    .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 3. Send file via TransferClient
    let client = TransferClient::new();
    let sender_identity = CryptoIdentity::generate().unwrap();
    let summary = client
        .send_files(
            "127.0.0.1",
            port,
            false,
            &[test_file_path],
            &config,
            &sender_identity,
        )
        .await
        .unwrap();

    assert_eq!(summary.files_sent, 1);
    assert_eq!(summary.total_bytes, total_written);

    // 4. Verify received file on disk
    let received_file_path = receiver_dir.join("payload_2mb.bin");
    assert!(received_file_path.exists(), "Received file must exist");

    let received_bytes = tokio::fs::read(&received_file_path).await.unwrap();
    assert_eq!(received_bytes.len() as u64, total_written);
    let received_sha256 = compute_sha256(&received_bytes);
    assert_eq!(
        received_sha256, expected_sha256,
        "SHA-256 checksum of received file must match original exactly"
    );

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_transfer_rejection_for_unpaired_peer() {
    let temp_dir = TempDir::new().unwrap();
    let receiver_dir = temp_dir.path().join("receiver");
    let sender_dir = temp_dir.path().join("sender");
    tokio::fs::create_dir_all(&receiver_dir).await.unwrap();
    tokio::fs::create_dir_all(&sender_dir).await.unwrap();

    let test_file = sender_dir.join("secret.txt");
    tokio::fs::write(&test_file, b"confidential payload").await.unwrap();

    // Receiver has paired fingerprints, but NOT the sender's
    let mut config = Config::default();
    let mut paired = HashSet::new();
    paired.insert("TRUSTED_PEER_FINGERPRINT_112233445566".to_string());
    config.security.paired_fingerprints = paired;
    let config = Arc::new(config);
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = TransferServer::new(
        config.clone(),
        identity.clone(),
        receiver_dir,
        port,
        false,
    )
    .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Sender with random/unpaired identity
    let client = TransferClient::new();
    let sender_identity = CryptoIdentity::generate().unwrap();

    let result = client
        .send_files(
            "127.0.0.1",
            port,
            false,
            &[test_file],
            &config,
            &sender_identity,
        )
        .await;

    assert!(result.is_err(), "Transfer from unpaired peer must be rejected");

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_tls_encrypted_transfer_loopback() {
    let temp_dir = TempDir::new().unwrap();
    let sender_dir = temp_dir.path().join("sender");
    let receiver_dir = temp_dir.path().join("receiver");
    tokio::fs::create_dir_all(&sender_dir).await.unwrap();
    tokio::fs::create_dir_all(&receiver_dir).await.unwrap();

    let test_file_path = sender_dir.join("secure_doc.txt");
    let content = b"End-to-end encrypted transfer via mTLS and SHA-256 verification.";
    tokio::fs::write(&test_file_path, content).await.unwrap();
    let expected_sha256 = compute_sha256(content);

    let config = Arc::new(Config::default());
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = TransferServer::new(
        config.clone(),
        identity.clone(),
        receiver_dir.clone(),
        port,
        true, // TLS enabled
    )
    .unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = TransferClient::new();
    let sender_identity = CryptoIdentity::generate().unwrap();
    let summary = client
        .send_files(
            "127.0.0.1",
            port,
            true, // use TLS
            &[test_file_path],
            &config,
            &sender_identity,
        )
        .await
        .unwrap();

    assert_eq!(summary.files_sent, 1);
    assert_eq!(summary.total_bytes, content.len() as u64);

    let received_file_path = receiver_dir.join("secure_doc.txt");
    assert!(received_file_path.exists());
    let received_bytes = tokio::fs::read(&received_file_path).await.unwrap();
    assert_eq!(compute_sha256(&received_bytes), expected_sha256);

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_clipboard_endpoint_and_echo_suppression() {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = Config::default();
    config.security.paired_fingerprints = HashSet::from(["TRUSTED_PEER_FP".to_string()]);
    let config = Arc::new(config);
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut server = TransferServer::new(
        config.clone(),
        identity.clone(),
        download_dir,
        port,
        false,
    )
    .unwrap();

    let cache = ClipboardPayloadCache::new(std::time::Duration::from_secs(60));
    let (packet_tx, _packet_rx) = broadcast::channel(16);
    let adapter = WaylandClipboardAdapter::new("desktop_device".to_string(), cache.clone(), packet_tx);

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();
    let callback = Arc::new(move |pkt: ClipboardPacket| {
        let mut list = received_packets_clone.lock().unwrap();
        list.push(pkt);
    });

    server.set_clipboard_adapter(adapter.clone());
    server.set_clipboard_cache(cache);
    server.set_clipboard_callback(callback);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = TransferClient::new();

    let text_content = "Hello from companion device clipboard sync!";
    let text_sha256 = compute_sha256(text_content.as_bytes());

    let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "TRUSTED_PEER_FP".to_string(),
        timestamp: 1234567890,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "text/plain".to_string(),
            content: text_content.to_string(),
            sha256: text_sha256.clone(),
            size_bytes: text_content.len(),
        },
    });

    // 1. Send with untrusted fingerprint -> must be rejected (403 Forbidden)
    let err_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false,
            None,
            Some("UNTRUSTED_FINGERPRINT"),
            &packet,
        )
        .await;
    assert!(err_res.is_err(), "Untrusted peer should be rejected");

    // 2. Send with trusted fingerprint -> must succeed and invoke callback
    let ok_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false,
            None,
            Some("TRUSTED_PEER_FP"),
            &packet,
        )
        .await;
    assert!(ok_res.is_ok(), "Trusted peer packet should succeed");

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Expected exactly 1 packet callback");
        match &list[0] {
            ClipboardPacket::Inline(p) => {
                assert_eq!(p.payload.content, text_content);
                assert_eq!(p.payload.sha256, text_sha256);
            }
            _ => panic!("Expected inline packet"),
        }
    }

    // Verify adapter recorded the paste
    assert_eq!(adapter.get_last_pasted_hash(), Some(text_sha256.clone()));
    assert!(adapter.is_recent_paste(&text_sha256));

    // 3. Send duplicate packet -> must succeed over HTTP, but echo is suppressed (callback count stays 1)
    let dup_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false,
            None,
            Some("TRUSTED_PEER_FP"),
            &packet,
        )
        .await;
    assert!(dup_res.is_ok(), "Duplicate packet HTTP request should succeed");

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Duplicate packet should be suppressed from re-invoking callback");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_clipboard_descriptor_fetch_and_echo_suppression() {
    use archdrop_core::models::clipboard::{ClipboardDescriptorPacket, ClipboardDescriptorPayload};

    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = Config::default();
    config.security.paired_fingerprints = HashSet::from(["TRUSTED_PEER_FP".to_string()]);
    let config = Arc::new(config);
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut server = TransferServer::new(
        config.clone(),
        identity.clone(),
        download_dir,
        port,
        false,
    )
    .unwrap();

    let cache = ClipboardPayloadCache::new(std::time::Duration::from_secs(60));
    let (packet_tx, _packet_rx) = broadcast::channel(16);
    let adapter = WaylandClipboardAdapter::new("desktop_device".to_string(), cache.clone(), packet_tx);

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();
    let callback = Arc::new(move |pkt: ClipboardPacket| {
        let mut list = received_packets_clone.lock().unwrap();
        list.push(pkt);
    });

    server.set_clipboard_adapter(adapter.clone());
    server.set_clipboard_cache(cache.clone());
    server.set_clipboard_callback(callback);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 1. Create a > 4 KB payload in cache
    let large_data = vec![0x42u8; 8192];
    let data_hash = compute_sha256(&large_data);
    let payload_id = "test-payload-uuid-9999".to_string();
    let token = "test-secret-token-1111".to_string();

    cache
        .insert(archdrop_clipboard::CachedPayload {
            payload_id: payload_id.clone(),
            mime_type: "application/octet-stream".to_string(),
            data: Arc::from(large_data.into_boxed_slice()),
            token: token.clone(),
            sha256: data_hash.clone(),
            created_at: std::time::Instant::now(),
        })
        .await;

    // 2. Verify GET /api/v2/clipboard/payload/:id?token=...
    let client = TransferClient::new();
    let reqwest_client = reqwest::Client::new();
    let payload_url = format!(
        "http://127.0.0.1:{}/api/v2/clipboard/payload/{}?token={}",
        port, payload_id, token
    );
    let resp = reqwest_client.get(&payload_url).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let fetched_bytes = resp.bytes().await.unwrap();
    assert_eq!(compute_sha256(&fetched_bytes), data_hash);

    // 3. Send descriptor packet to /api/v2/clipboard
    let desc_packet = ClipboardPacket::Descriptor(ClipboardDescriptorPacket {
        version: 2,
        device_id: "TRUSTED_PEER_FP".to_string(),
        timestamp: 1234567890,
        ttl_seconds: 120,
        descriptor: ClipboardDescriptorPayload {
            payload_id: payload_id.clone(),
            primary_mime: "application/octet-stream".to_string(),
            available_mimes: vec!["application/octet-stream".to_string()],
            size_bytes: 8192,
            sha256: data_hash.clone(),
            ephemeral_token: token,
            download_url: Some(payload_url),
        },
    });

    let ok_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false,
            None,
            Some("TRUSTED_PEER_FP"),
            &desc_packet,
        )
        .await;
    assert!(ok_res.is_ok(), "Descriptor packet should be accepted");

    // Wait for async download
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Expected 1 packet callback for descriptor");
    }

    // Verify adapter recorded hash
    assert_eq!(adapter.get_last_pasted_hash(), Some(data_hash.clone()));
    assert!(adapter.is_recent_paste(&data_hash));

    // 4. Duplicate descriptor packet should be suppressed
    let dup_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false,
            None,
            Some("TRUSTED_PEER_FP"),
            &desc_packet,
        )
        .await;
    assert!(dup_res.is_ok());

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Duplicate descriptor must be echo-suppressed");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_clipboard_descriptor_relative_url_and_uppercase_hash() {
    use archdrop_core::models::clipboard::{ClipboardDescriptorPacket, ClipboardDescriptorPayload};

    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = Config::default();
    config.security.paired_fingerprints = HashSet::from(["TRUSTED_PEER_FP".to_string()]);
    let config = Arc::new(config);
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut server = TransferServer::new(
        config.clone(),
        identity.clone(),
        download_dir,
        port,
        false,
    )
    .unwrap();

    let cache = ClipboardPayloadCache::new(std::time::Duration::from_secs(60));
    let (packet_tx, _packet_rx) = broadcast::channel(16);
    let adapter = WaylandClipboardAdapter::new("desktop_device".to_string(), cache.clone(), packet_tx);

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();
    let callback = Arc::new(move |pkt: ClipboardPacket| {
        let mut list = received_packets_clone.lock().unwrap();
        list.push(pkt);
    });

    server.set_clipboard_adapter(adapter.clone());
    server.set_clipboard_cache(cache.clone());
    server.set_clipboard_callback(callback);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 1. Create a > 4 KB payload in cache
    let large_data = vec![0x99u8; 8192];
    let data_hash = compute_sha256(&large_data);
    let payload_id = "test-payload-uuid-relative".to_string();
    let token = "test-secret-token-relative".to_string();

    cache
        .insert(archdrop_clipboard::CachedPayload {
            payload_id: payload_id.clone(),
            mime_type: "application/octet-stream".to_string(),
            data: Arc::from(large_data.into_boxed_slice()),
            token: token.clone(),
            sha256: data_hash.clone(),
            created_at: std::time::Instant::now(),
        })
        .await;

    // 2. Relative URL path and uppercase SHA-256 hash
    let relative_url = format!("/api/v2/clipboard/payload/{}?token={}", payload_id, token);
    let uppercase_hash = data_hash.to_ascii_uppercase();

    let client = TransferClient::new();
    let desc_packet = ClipboardPacket::Descriptor(ClipboardDescriptorPacket {
        version: 2,
        device_id: "TRUSTED_PEER_FP".to_string(),
        timestamp: 1234567890,
        ttl_seconds: 120,
        descriptor: ClipboardDescriptorPayload {
            payload_id: payload_id.clone(),
            primary_mime: "application/octet-stream".to_string(),
            available_mimes: vec!["application/octet-stream".to_string()],
            size_bytes: 8192,
            sha256: uppercase_hash.clone(),
            ephemeral_token: token,
            download_url: Some(relative_url),
        },
    });

    let ok_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false,
            None,
            Some("TRUSTED_PEER_FP"),
            &desc_packet,
        )
        .await;
    assert!(ok_res.is_ok(), "Descriptor with relative URL and uppercase hash should be accepted");

    // Wait for async download and paste
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Expected 1 packet callback for relative URL descriptor");
    }

    // Verify adapter recorded hash and is_recent_paste matches regardless of casing
    assert!(adapter.is_recent_paste(&data_hash));
    assert!(adapter.is_recent_paste(&uppercase_hash));

    // Duplicate packet with lowercase hash must be suppressed
    let dup_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false,
            None,
            Some("TRUSTED_PEER_FP"),
            &desc_packet,
        )
        .await;
    assert!(dup_res.is_ok());

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Duplicate descriptor must be echo-suppressed");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_tls_and_plain_http_dual_mode_multiplexing() {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = Config::default();
    config.security.paired_fingerprints = HashSet::from(["TRUSTED_PEER_FP".to_string()]);
    let config = Arc::new(config);
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    // Initialize server with use_tls = true
    let mut server = TransferServer::new(
        config.clone(),
        identity.clone(),
        download_dir,
        port,
        true, // TLS enabled
    )
    .unwrap();

    let cache = ClipboardPayloadCache::new(std::time::Duration::from_secs(60));
    let (packet_tx, _packet_rx) = broadcast::channel(16);
    let adapter = WaylandClipboardAdapter::new("desktop_device".to_string(), cache.clone(), packet_tx);

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();
    let callback = Arc::new(move |pkt: ClipboardPacket| {
        let mut list = received_packets_clone.lock().unwrap();
        list.push(pkt);
    });

    server.set_clipboard_adapter(adapter.clone());
    server.set_clipboard_cache(cache.clone());
    server.set_clipboard_callback(callback);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = TransferClient::new();

    // 1. Send clipboard packet over HTTPS (TLS)
    let tls_text = "Clipboard text over TLS connection";
    let tls_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "TRUSTED_PEER_FP".to_string(),
        timestamp: 1234567890,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "text/plain".to_string(),
            content: tls_text.to_string(),
            sha256: compute_sha256(tls_text.as_bytes()),
            size_bytes: tls_text.len(),
        },
    });

    let tls_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            true, // use_https = true
            Some(identity.fingerprint.clone()),
            Some("TRUSTED_PEER_FP"),
            &tls_packet,
        )
        .await;
    assert!(tls_res.is_ok(), "TLS clipboard POST should succeed: {:?}", tls_res.err());

    // 2. Send clipboard packet over Plain HTTP to the SAME TLS-enabled server port
    let plain_text = "Clipboard text over Plain HTTP connection";
    let plain_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "TRUSTED_PEER_FP".to_string(),
        timestamp: 1234567891,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "text/plain".to_string(),
            content: plain_text.to_string(),
            sha256: compute_sha256(plain_text.as_bytes()),
            size_bytes: plain_text.len(),
        },
    });

    let plain_res = client
        .send_clipboard(
            "127.0.0.1",
            port,
            false, // use_https = false (plain HTTP)
            None,
            Some("TRUSTED_PEER_FP"),
            &plain_packet,
        )
        .await;
    assert!(plain_res.is_ok(), "Plain HTTP clipboard POST to TLS server should succeed: {:?}", plain_res.err());

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 2, "Expected both TLS and Plain HTTP packets to be processed");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

async fn spawn_test_server<F>(
    config: Arc<Config>,
    identity: Arc<CryptoIdentity>,
    download_dir: std::path::PathBuf,
    configure: F,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> (u16, tokio::task::JoinHandle<Result<(), archdrop_transfer::TransferError>>)
where
    F: Fn(&mut TransferServer),
{
    for _ in 0..10 {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(l) => l,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                continue;
            }
        };
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut server = match TransferServer::new(
            config.clone(),
            identity.clone(),
            download_dir.clone(),
            port,
            false,
        ) {
            Ok(s) => s,
            Err(_) => continue,
        };

        configure(&mut server);

        let rx = shutdown_rx.clone();
        let handle = tokio::spawn(async move {
            server.run(rx).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        if handle.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            continue;
        }

        return (port, handle);
    }
    panic!("Failed to bind and spawn test server after 10 attempts");
}

#[tokio::test]
async fn test_clipboard_binary_image_raw_base64_and_echo_suppression() {
    use archdrop_clipboard::threshold::base64::prelude::*;

    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = Arc::new(Config::default());
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let cache = ClipboardPayloadCache::new(std::time::Duration::from_secs(60));
    let (packet_tx, _packet_rx) = broadcast::channel(16);
    let adapter = WaylandClipboardAdapter::new("desktop_device".to_string(), cache.clone(), packet_tx);

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();
    let callback = Arc::new(move |pkt: ClipboardPacket| {
        let mut list = received_packets_clone.lock().unwrap();
        list.push(pkt);
    });

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let adapter_clone = adapter.clone();
    let cache_clone = cache.clone();
    let callback_clone = callback.clone();

    let (port, server_handle) = spawn_test_server(
        config.clone(),
        identity.clone(),
        download_dir,
        move |s| {
            s.set_clipboard_adapter(adapter_clone.clone());
            s.set_clipboard_cache(cache_clone.clone());
            s.set_clipboard_callback(callback_clone.clone());
        },
        shutdown_rx,
    )
    .await;

    let client = TransferClient::new();
    let raw_image_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDRtestbinaryrawimage";
    let base64_image = BASE64_STANDARD.encode(raw_image_bytes);
    let binary_hash = compute_sha256(raw_image_bytes);

    let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-peer".to_string(),
        timestamp: 1234567,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: base64_image,
            sha256: binary_hash.clone(),
            size_bytes: raw_image_bytes.len(),
        },
    });

    let send_res = client
        .send_clipboard("127.0.0.1", port, false, None, Some("phone-peer"), &packet)
        .await;
    assert!(send_res.is_ok(), "Sending binary image packet must succeed: {:?}", send_res.err());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify adapter recorded the binary SHA-256 and echo suppression works case-insensitively
    assert_eq!(adapter.get_last_pasted_hash(), Some(binary_hash.clone()));
    assert!(adapter.is_recent_paste(&binary_hash));
    assert!(adapter.is_recent_paste(&binary_hash.to_uppercase()));

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Expected exactly 1 packet received");
    }

    // Sending identical packet again must be echo-suppressed
    let send_res2 = client
        .send_clipboard("127.0.0.1", port, false, None, Some("phone-peer"), &packet)
        .await;
    assert!(send_res2.is_ok());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Duplicate binary image packet must be echo-suppressed");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_clipboard_staged_file_transfer_and_uri_list_injection() {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = Arc::new(Config::default());
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let cache = ClipboardPayloadCache::new(std::time::Duration::from_secs(60));
    let (packet_tx, _packet_rx) = broadcast::channel(16);
    let adapter = WaylandClipboardAdapter::new("desktop_device".to_string(), cache, packet_tx);

    let completed_events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let completed_clone = completed_events.clone();

    let staged_notifications = Arc::new(std::sync::Mutex::new(Vec::new()));
    let staged_notif_clone = staged_notifications.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let adapter_clone = adapter.clone();
    let comp_clone = completed_clone.clone();
    let st_clone = staged_notif_clone.clone();

    let (port, server_handle) = spawn_test_server(
        config.clone(),
        identity.clone(),
        download_dir.clone(),
        move |s| {
            s.set_clipboard_adapter(adapter_clone.clone());
            let comp = comp_clone.clone();
            s.set_completion_callback(Arc::new(move |c| {
                let mut list = comp.lock().unwrap();
                list.push(c);
            }));
            let st = st_clone.clone();
            s.set_staged_clipboard_callback(Arc::new(move |name, path, bytes, peer| {
                let list = st.clone();
                Box::pin(async move {
                    let mut l = list.lock().unwrap();
                    l.push((name, path, bytes, peer));
                })
            }));
        },
        shutdown_rx,
    )
    .await;

    // 1. Send prepare-upload with header X-Clipboard-Staged: true
    let test_file_bytes = b"Sample document payload for clipboard staging test";
    let test_file_hash = compute_sha256(test_file_bytes);

    let http_client = reqwest::Client::new();
    let prepare_body = serde_json::json!({
        "info": {
            "alias": "Phone Sender",
            "version": "2.2",
            "deviceModel": "Android Phone",
            "deviceType": "mobile",
            "fingerprint": "PHONE_FP_STAGED_TEST",
            "port": 53317,
            "protocol": "http",
            "download": true
        },
        "files": {
            "doc1": {
                "id": "doc1",
                "fileName": "document.pdf",
                "size": test_file_bytes.len(),
                "fileType": "application/pdf",
                "sha256": test_file_hash
            }
        }
    });

    let prep_resp = http_client
        .post(format!("http://127.0.0.1:{}/api/localsend/v2/prepare-upload", port))
        .header("X-Clipboard-Staged", "true")
        .json(&prepare_body)
        .send()
        .await
        .unwrap();

    assert_eq!(prep_resp.status(), 200);
    let prep_json: serde_json::Value = prep_resp.json().await.unwrap();
    let session_id = prep_json["sessionId"].as_str().unwrap();
    let token = prep_json["files"]["doc1"].as_str().unwrap();

    // 2. Upload the file
    let upload_url = format!(
        "http://127.0.0.1:{}/api/localsend/v2/upload?sessionId={}&fileId=doc1&token={}",
        port, session_id, token
    );

    let up_resp = http_client
        .post(upload_url)
        .body(test_file_bytes.to_vec())
        .send()
        .await
        .unwrap();

    assert_eq!(up_resp.status(), 200);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 3. Verify file exists in download_dir
    let final_file = download_dir.join("document.pdf");
    assert!(final_file.exists(), "Target file must be saved in download_dir");
    let saved_contents = tokio::fs::read(&final_file).await.unwrap();
    assert_eq!(saved_contents, test_file_bytes);

    // 4. Verify completion event has is_clipboard_staged = true
    {
        let completed = completed_events.lock().unwrap();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].is_clipboard_staged, "is_clipboard_staged must be true");
        assert_eq!(completed[0].file_name, "document.pdf");
    }

    // 5. Verify staged notification callback was invoked
    {
        let notifs = staged_notifications.lock().unwrap();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].0, "document.pdf");
        assert_eq!(notifs[0].1, final_file);
        assert_eq!(notifs[0].2, test_file_bytes.len() as u64);
        assert_eq!(notifs[0].3, "Phone Sender");
    }

    // 6. Verify text/uri-list was injected into clipboard adapter and echo-suppressed
    let expected_uri_list = format!("file://{}\r\n", final_file.display());
    let expected_uri_hash = compute_sha256(expected_uri_list.as_bytes());
    assert_eq!(adapter.get_last_pasted_hash(), Some(expected_uri_hash.clone()));
    assert!(adapter.is_recent_paste(&expected_uri_hash));
    assert!(adapter.is_recent_paste(&expected_uri_hash.to_uppercase()));

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_clipboard_adapter_none_fallback_handles_image_and_text() {
    use archdrop_clipboard::threshold::base64::prelude::*;

    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = Arc::new(Config::default());
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let cb_clone = received_packets_clone.clone();

    // Create server WITHOUT clipboard adapter
    let (port, server_handle) = spawn_test_server(
        config.clone(),
        identity.clone(),
        download_dir,
        move |s| {
            let cb = cb_clone.clone();
            s.set_clipboard_callback(Arc::new(move |pkt| {
                let mut list = cb.lock().unwrap();
                list.push(pkt);
            }));
        },
        shutdown_rx,
    )
    .await;

    let client = TransferClient::new();

    // 1. Binary image packet with adapter = None
    let raw_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rfallbackimage";
    let b64 = BASE64_STANDARD.encode(raw_bytes);
    let img_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-test".to_string(),
        timestamp: 100,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: b64,
            sha256: compute_sha256(raw_bytes),
            size_bytes: raw_bytes.len(),
        },
    });

    let res1 = client
        .send_clipboard("127.0.0.1", port, false, None, Some("phone-test"), &img_packet)
        .await;
    assert!(res1.is_ok(), "Image packet with adapter=None must succeed: {:?}", res1.err());

    // 2. Text packet with adapter = None
    let text = "Hello without adapter";
    let text_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-test".to_string(),
        timestamp: 101,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "text/plain".to_string(),
            content: text.to_string(),
            sha256: compute_sha256(text.as_bytes()),
            size_bytes: text.len(),
        },
    });

    let res2 = client
        .send_clipboard("127.0.0.1", port, false, None, Some("phone-test"), &text_packet)
        .await;
    assert!(res2.is_ok(), "Text packet with adapter=None must succeed: {:?}", res2.err());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 2, "Both packets must be processed without error");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}





