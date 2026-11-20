use archdrop_clipboard::{
    compute_sha256, from_kdeconnect_packet, pack_clipboard_data, to_kdeconnect_packet,
    ClipboardPayloadCache, ClipboardTtlManager,
};
use archdrop_core::models::clipboard::{
    ClearReason, ClipboardPacket, KdeConnectClipboard, KdeConnectPacket, CLIPBOARD_INLINE_THRESHOLD,
};
use std::time::Duration;
use tokio::sync::broadcast;

#[test]
fn test_apple_4kb_threshold_exact_boundaries() {
    let device_id = "test-laptop-01";
    let mime = "text/plain;charset=utf-8";

    // 1. Exactly 4096 bytes -> MUST be Inline
    let data_4096 = vec![b'A'; CLIPBOARD_INLINE_THRESHOLD];
    let (packet_4096, cached_4096) = pack_clipboard_data(device_id, mime, &data_4096, None);

    assert!(cached_4096.is_none(), "Payload <= 4 KB must not require cache");
    match packet_4096 {
        ClipboardPacket::Inline(p) => {
            assert_eq!(p.payload.size_bytes, 4096);
            assert_eq!(p.payload.content.len(), 4096);
            assert_eq!(p.payload.sha256, compute_sha256(&data_4096));
        }
        _ => panic!("Expected ClipboardPacket::Inline for 4096 bytes"),
    }

    // 2. Exactly 4097 bytes -> MUST be Descriptor
    let data_4097 = vec![b'B'; CLIPBOARD_INLINE_THRESHOLD + 1];
    let (packet_4097, cached_4097) = pack_clipboard_data(
        device_id,
        mime,
        &data_4097,
        Some("http://127.0.0.1:53317"),
    );

    assert!(cached_4097.is_some(), "Payload > 4 KB must produce CachedPayload");
    match packet_4097 {
        ClipboardPacket::Descriptor(p) => {
            assert_eq!(p.descriptor.size_bytes, 4097);
            assert_eq!(p.descriptor.sha256, compute_sha256(&data_4097));
            assert!(p.descriptor.download_url.unwrap().contains("/api/v2/clipboard/payload/"));
        }
        _ => panic!("Expected ClipboardPacket::Descriptor for 4097 bytes"),
    }
}

#[test]
fn test_multibyte_utf8_threshold_boundary() {
    let device_id = "test-device";
    let mime = "text/plain;charset=utf-8";

    // Emoji 🦀 is 4 bytes in UTF-8
    let crab = "🦀";
    assert_eq!(crab.len(), 4);

    // 1024 crabs = 4096 bytes -> Inline
    let crabs_1024 = crab.repeat(1024);
    assert_eq!(crabs_1024.len(), 4096);
    let (packet, cached) = pack_clipboard_data(device_id, mime, crabs_1024.as_bytes(), None);
    assert!(cached.is_none());
    assert!(matches!(packet, ClipboardPacket::Inline(_)));

    // 1025 crabs = 4100 bytes -> Descriptor
    let crabs_1025 = crab.repeat(1025);
    assert_eq!(crabs_1025.len(), 4100);
    let (packet2, cached2) = pack_clipboard_data(device_id, mime, crabs_1025.as_bytes(), None);
    assert!(cached2.is_some());
    assert!(matches!(packet2, ClipboardPacket::Descriptor(_)));
}

#[tokio::test]
async fn test_payload_cache_and_token_validation() {
    let cache = ClipboardPayloadCache::new(Duration::from_secs(120));
    let device_id = "laptop";
    let large_data = vec![0xFEu8; 10_000];

    let (_packet, cached) = pack_clipboard_data(device_id, "application/octet-stream", &large_data, None);
    let cached = cached.expect("Must produce cached payload");
    let payload_id = cached.payload_id.clone();
    let token = cached.token.clone();

    cache.insert(cached).await;
    assert_eq!(cache.len().await, 1);

    // 1. Valid retrieval
    let retrieved = cache.get(&payload_id, &token).await.unwrap();
    assert_eq!(retrieved.as_ref(), large_data.as_slice());

    // 2. Invalid token
    let bad_token = cache.get(&payload_id, "WRONG_TOKEN").await;
    assert!(bad_token.is_err());

    // 3. Non-existent ID
    let not_found = cache.get("non-existent-id", &token).await;
    assert!(not_found.is_err());

    // 4. Zeroization / clear
    cache.clear().await;
    assert_eq!(cache.len().await, 0);
    assert!(cache.get(&payload_id, &token).await.is_err());
}

#[tokio::test]
async fn test_ttl_manager_zeroize_and_broadcast() {
    let cache = ClipboardPayloadCache::new(Duration::from_secs(120));
    let (tx, mut rx) = broadcast::channel(16);
    let ttl_manager = ClipboardTtlManager::new("laptop-01".to_string(), cache.clone(), tx);

    let large_data = vec![0xCCu8; 8000];
    let (_, cached) = pack_clipboard_data("laptop-01", "text/plain", &large_data, None);
    cache.insert(cached.unwrap()).await;
    assert_eq!(cache.len().await, 1);

    // Trigger session lock zeroization
    ttl_manager.zeroize(ClearReason::SessionLock).await;

    // Verify volatile cache was zeroized
    assert_eq!(cache.len().await, 0);

    // Verify packet broadcast received Clear packet with SessionLock reason
    let received_packet = rx.recv().await.unwrap();
    match received_packet {
        ClipboardPacket::Clear(p) => {
            assert_eq!(p.reason, ClearReason::SessionLock);
            assert_eq!(p.device_id, "laptop-01");
        }
        _ => panic!("Expected ClipboardPacket::Clear"),
    }
}

#[test]
fn test_kdeconnect_packet_interoperability() {
    let device_id = "laptop-kde";
    let kde_pkt = KdeConnectPacket {
        id: 123456789,
        packet_type: "kdeconnect.clipboard".to_string(),
        body: KdeConnectClipboard {
            content: "Hello from Android KDE Connect".to_string(),
            timestamp: Some(123456789),
        },
    };

    // KDE Connect -> Native ClipboardPacket
    let native_packet = from_kdeconnect_packet(device_id, &kde_pkt);
    match &native_packet {
        ClipboardPacket::Inline(p) => {
            assert_eq!(p.payload.content, "Hello from Android KDE Connect");
            assert_eq!(p.device_id, device_id);
        }
        _ => panic!("Expected inline packet"),
    }

    // Native ClipboardPacket -> KDE Connect
    let converted_back = to_kdeconnect_packet(&native_packet).unwrap();
    assert_eq!(converted_back.body.content, "Hello from Android KDE Connect");
    assert_eq!(converted_back.packet_type, "kdeconnect.clipboard");
}

#[test]
fn test_unpack_clipboard_raw_base64_and_data_uri_images() {
    use archdrop_clipboard::{unpack_clipboard_packet, UnpackedClipboard};
    use archdrop_core::models::clipboard::ClipboardInlinePacket;
    use base64::prelude::*;

    let raw_png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01";
    let base64_str = BASE64_STANDARD.encode(raw_png_bytes);
    let sha256_hash = compute_sha256(raw_png_bytes);

    // 1. Raw base64 image (no data: prefix)
    let raw_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-01".to_string(),
        timestamp: 123456,
        ttl_seconds: 120,
        payload: archdrop_core::models::clipboard::ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: base64_str.clone(),
            sha256: sha256_hash.clone(),
            size_bytes: raw_png_bytes.len(),
        },
    });

    let unpacked = unpack_clipboard_packet(&raw_packet).expect("Must unpack raw base64 image");
    match unpacked {
        UnpackedClipboard::InlineBinary { mime_type, data, sha256 } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(data, raw_png_bytes);
            assert_eq!(sha256, sha256_hash);
        }
        _ => panic!("Expected InlineBinary for raw base64 image"),
    }

    // 2. Data URI prefixed image
    let data_uri_content = format!("data:image/png;base64,{}", base64_str);
    let prefixed_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-01".to_string(),
        timestamp: 123456,
        ttl_seconds: 120,
        payload: archdrop_core::models::clipboard::ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: data_uri_content,
            sha256: sha256_hash.clone(),
            size_bytes: raw_png_bytes.len(),
        },
    });

    let unpacked2 = unpack_clipboard_packet(&prefixed_packet).expect("Must unpack data URI image");
    match unpacked2 {
        UnpackedClipboard::InlineBinary { mime_type, data, sha256 } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(data, raw_png_bytes);
            assert_eq!(sha256, sha256_hash);
        }
        _ => panic!("Expected InlineBinary for data URI image"),
    }

    // 3. Raw base64 image/jpeg with whitespace/newlines
    let raw_jpeg_bytes = b"\xFF\xD8\xFF\xE0\x00\x10JFIF";
    let jpeg_b64 = format!("  \r\n{}  \n", BASE64_STANDARD.encode(raw_jpeg_bytes));
    let jpeg_sha256 = compute_sha256(raw_jpeg_bytes);
    let jpeg_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-01".to_string(),
        timestamp: 123456,
        ttl_seconds: 120,
        payload: archdrop_core::models::clipboard::ClipboardInlinePayload {
            mime_type: "image/jpeg".to_string(),
            content: jpeg_b64,
            sha256: jpeg_sha256.clone(),
            size_bytes: raw_jpeg_bytes.len(),
        },
    });

    let unpacked3 = unpack_clipboard_packet(&jpeg_packet).expect("Must unpack JPEG image with whitespace");
    match unpacked3 {
        UnpackedClipboard::InlineBinary { mime_type, data, sha256 } => {
            assert_eq!(mime_type, "image/jpeg");
            assert_eq!(data, raw_jpeg_bytes);
            assert_eq!(sha256, jpeg_sha256);
        }
        _ => panic!("Expected InlineBinary for JPEG image"),
    }

    // 4. text/uri-list should unpack to InlineText
    let uri_list_content = "file:///home/prem/Downloads/image.png\r\n";
    let uri_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-01".to_string(),
        timestamp: 123456,
        ttl_seconds: 120,
        payload: archdrop_core::models::clipboard::ClipboardInlinePayload {
            mime_type: "text/uri-list".to_string(),
            content: uri_list_content.to_string(),
            sha256: compute_sha256(uri_list_content.as_bytes()),
            size_bytes: uri_list_content.len(),
        },
    });

    let unpacked4 = unpack_clipboard_packet(&uri_packet).expect("Must unpack text/uri-list");
    match unpacked4 {
        UnpackedClipboard::InlineText { mime_type, text, .. } => {
            assert_eq!(mime_type, "text/uri-list");
            assert_eq!(text, uri_list_content);
        }
        _ => panic!("Expected InlineText for text/uri-list"),
    }
}

#[test]
fn test_adapter_echo_suppression_and_case_insensitivity_binary_and_uri_list() {
    use archdrop_clipboard::WaylandClipboardAdapter;

    let (tx, _rx) = broadcast::channel(16);
    let cache = ClipboardPayloadCache::new(Duration::from_secs(120));
    let adapter = WaylandClipboardAdapter::new("test-laptop".to_string(), cache, tx);

    let image_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDRtestimagebytes";
    let image_hash = compute_sha256(image_bytes);

    // Paste remote payload records hash into last_pasted_sha256
    let _ = adapter.paste_remote_payload("image/png", image_bytes);
    assert_eq!(adapter.get_last_pasted_hash(), Some(image_hash.clone()));

    // Echo suppression checks: lowercase, uppercase, mixed case
    assert!(adapter.is_recent_paste(&image_hash));
    assert!(adapter.is_recent_paste(&image_hash.to_uppercase()));
    assert!(!adapter.is_recent_paste("different_hash_value_1234567890abcdef"));

    // text/uri-list echo suppression
    let uri_payload = b"file:///home/prem/Downloads/doc.pdf\r\n";
    let uri_hash = compute_sha256(uri_payload);
    let _ = adapter.paste_remote_payload("text/uri-list", uri_payload);
    assert_eq!(adapter.get_last_pasted_hash(), Some(uri_hash.clone()));
    assert!(adapter.is_recent_paste(&uri_hash));
    assert!(adapter.is_recent_paste(&uri_hash.to_uppercase()));
}

