use archdrop_clipboard::{
    compute_sha256, CachedPayload, ClipboardPayloadCache, WaylandClipboardAdapter,
};
use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_core::models::clipboard::{
    ClipboardDescriptorPacket, ClipboardDescriptorPayload, ClipboardInlinePacket,
    ClipboardInlinePayload, ClipboardPacket,
};
use archdrop_transfer::{TransferClient, TransferServer};
use archdrop_clipboard::threshold::base64::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

const VALID_1X1_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15c4\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82";

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
        let handle = tokio::spawn(async move { server.run(rx).await });

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        if handle.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            continue;
        }

        return (port, handle);
    }
    panic!("Failed to spawn test server");
}

#[tokio::test]
async fn test_adversarial_binary_image_pipe_to_wl_copy_and_null_bytes() {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = Arc::new(Config::default());
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let cb_clone = received_packets_clone.clone();

    // Server WITHOUT clipboard adapter (forces pipe_to_wl_copy fallback)
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

    // 1. Raw binary PNG with null bytes
    let b64 = BASE64_STANDARD.encode(VALID_1X1_PNG);
    let hash = compute_sha256(VALID_1X1_PNG);

    let png_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-test".to_string(),
        timestamp: 100,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: b64,
            sha256: hash.clone(),
            size_bytes: VALID_1X1_PNG.len(),
        },
    });

    let res = client
        .send_clipboard("127.0.0.1", port, false, None, Some("phone-test"), &png_packet)
        .await;
    assert!(res.is_ok(), "PNG piping to wl-copy must succeed: {:?}", res.err());

    // 2. Corrupted base64 payload to server without adapter
    let corrupted_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-test".to_string(),
        timestamp: 101,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: "Corrupted!Base64@String".to_string(),
            sha256: compute_sha256(b"Corrupted!Base64@String"),
            size_bytes: 23,
        },
    });

    let res2 = client
        .send_clipboard("127.0.0.1", port, false, None, Some("phone-test"), &corrupted_packet)
        .await;
    assert!(res2.is_ok(), "Corrupted base64 must be handled gracefully: {:?}", res2.err());

    // 3. Empty content to server without adapter
    let empty_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-test".to_string(),
        timestamp: 102,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: "".to_string(),
            sha256: compute_sha256(b""),
            size_bytes: 0,
        },
    });

    let res3 = client
        .send_clipboard("127.0.0.1", port, false, None, Some("phone-test"), &empty_packet)
        .await;
    assert!(res3.is_ok(), "Empty content must be handled gracefully: {:?}", res3.err());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 3, "Expected 3 callbacks");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_adversarial_binary_descriptor_download_and_case_insensitive_echo() {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = Config::default();
    config.security.paired_fingerprints = HashSet::from(["TRUSTED_PEER_FP".to_string()]);
    let config = Arc::new(config);
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let cache = ClipboardPayloadCache::new(std::time::Duration::from_secs(60));
    let (packet_tx, _packet_rx) = broadcast::channel(16);
    let adapter = WaylandClipboardAdapter::new("desktop_device".to_string(), cache.clone(), packet_tx);

    let received_packets = Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_packets_clone = received_packets.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let adapter_clone = adapter.clone();
    let cache_clone = cache.clone();
    let cb_clone = received_packets_clone.clone();

    let (port, server_handle) = spawn_test_server(
        config.clone(),
        identity.clone(),
        download_dir,
        move |s| {
            s.set_clipboard_adapter(adapter_clone.clone());
            s.set_clipboard_cache(cache_clone.clone());
            let cb = cb_clone.clone();
            s.set_clipboard_callback(Arc::new(move |pkt| {
                let mut list = cb.lock().unwrap();
                list.push(pkt);
            }));
        },
        shutdown_rx,
    )
    .await;

    // Create a 16 KB binary image with repeated PNG chunks and null bytes
    let mut large_image = Vec::with_capacity(16384);
    while large_image.len() < 16384 {
        large_image.extend_from_slice(VALID_1X1_PNG);
    }
    large_image.truncate(16384);

    let binary_hash = compute_sha256(&large_image);
    let uppercase_hash = binary_hash.to_ascii_uppercase();
    let payload_id = "desc-image-payload-uuid".to_string();
    let token = "token-secret-image-xyz".to_string();

    cache
        .insert(CachedPayload {
            payload_id: payload_id.clone(),
            mime_type: "image/png".to_string(),
            data: Arc::from(large_image.into_boxed_slice()),
            token: token.clone(),
            sha256: binary_hash.clone(),
            created_at: std::time::Instant::now(),
        })
        .await;

    // Send descriptor packet with UPPERCASE SHA-256 and relative download URL
    let relative_url = format!("/api/v2/clipboard/payload/{}?token={}", payload_id, token);
    let client = TransferClient::new();
    let desc_packet = ClipboardPacket::Descriptor(ClipboardDescriptorPacket {
        version: 2,
        device_id: "TRUSTED_PEER_FP".to_string(),
        timestamp: 1234567890,
        ttl_seconds: 120,
        descriptor: ClipboardDescriptorPayload {
            payload_id: payload_id.clone(),
            primary_mime: "image/png".to_string(),
            available_mimes: vec!["image/png".to_string()],
            size_bytes: 16384,
            sha256: uppercase_hash.clone(),
            ephemeral_token: token,
            download_url: Some(relative_url),
        },
    });

    let res = client
        .send_clipboard("127.0.0.1", port, false, None, Some("TRUSTED_PEER_FP"), &desc_packet)
        .await;
    assert!(res.is_ok(), "Descriptor sending must succeed");

    // Wait for async download and paste
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Expected 1 descriptor packet callback");
    }

    // Verify adapter recorded hash and is_recent_paste suppresses lowercase and uppercase
    assert!(adapter.is_recent_paste(&binary_hash));
    assert!(adapter.is_recent_paste(&uppercase_hash));

    // Duplicate descriptor must be echo-suppressed
    let dup_res = client
        .send_clipboard("127.0.0.1", port, false, None, Some("TRUSTED_PEER_FP"), &desc_packet)
        .await;
    assert!(dup_res.is_ok());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    {
        let list = received_packets.lock().unwrap();
        assert_eq!(list.len(), 1, "Duplicate descriptor must be echo-suppressed");
    }

    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}
