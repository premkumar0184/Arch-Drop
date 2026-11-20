use archdrop_clipboard::{
    compute_sha256, unpack_clipboard_packet, ClipboardPayloadCache,
    UnpackedClipboard, WaylandClipboardAdapter,
};
use archdrop_core::models::clipboard::{
    ClipboardInlinePacket, ClipboardInlinePayload, ClipboardPacket,
};
use base64::prelude::*;
use std::time::Duration;
use tokio::sync::broadcast;

// Standard valid 1x1 PNG bytes with null bytes and chunk headers
const VALID_1X1_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15c4\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82";

// Minimal valid JPEG bytes with multiple null bytes
const VALID_JPEG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x01\x00`\x00`\x00\x00\xff\xdb\x00C\x00\x08\x06\x06\x07\x06\x05\x08\x07\x07\x07\t\t\x08\n\x0c\x14\r\x0c\x0b\x0b\x0c\x19\x12\x13\x0f\x14\x1d\x1a\x1f\x1e\x1d\x1a\x1c\x1c $.\x27 \",#\x1c\x1c(7),01444\x1f\x279=82<.342\xff\xc0\x00\x0b\x08\x00\x01\x00\x01\x01\x01\x11\x00\xff\xc4\x00\x1f\x00\x00\x01\x05\x01\x01\x01\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\xff\xda\x00\x08\x01\x01\x00\x00?\x00\xbf\x00\xff\xd9";

#[test]
fn test_adversarial_png_unpacking_and_null_bytes() {
    let b64 = BASE64_STANDARD.encode(VALID_1X1_PNG);
    let hash = compute_sha256(VALID_1X1_PNG);

    // 1. Raw Base64
    let packet_raw = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-1".to_string(),
        timestamp: 1000,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: b64.clone(),
            sha256: hash.clone(),
            size_bytes: VALID_1X1_PNG.len(),
        },
    });

    let unpacked = unpack_clipboard_packet(&packet_raw).expect("Must unpack raw base64 PNG");
    match unpacked {
        UnpackedClipboard::InlineBinary { mime_type, data, sha256 } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(data, VALID_1X1_PNG);
            assert_eq!(sha256, hash);
            // Verify null bytes preserved
            assert!(data.contains(&0x00));
            assert_eq!(&data[0..8], b"\x89PNG\r\n\x1a\n");
        }
        _ => panic!("Expected InlineBinary"),
    }

    // 2. Data URI prefix
    let packet_uri = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-1".to_string(),
        timestamp: 1001,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: format!("data:image/png;base64,{}", b64),
            sha256: hash.clone(),
            size_bytes: VALID_1X1_PNG.len(),
        },
    });

    let unpacked_uri = unpack_clipboard_packet(&packet_uri).expect("Must unpack data URI PNG");
    match unpacked_uri {
        UnpackedClipboard::InlineBinary { mime_type, data, sha256 } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(data, VALID_1X1_PNG);
            assert_eq!(sha256, hash);
        }
        _ => panic!("Expected InlineBinary"),
    }
}

#[test]
fn test_adversarial_jpeg_unpacking_and_null_bytes() {
    let b64 = BASE64_STANDARD.encode(VALID_JPEG);
    let hash = compute_sha256(VALID_JPEG);

    let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-1".to_string(),
        timestamp: 1000,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/jpeg".to_string(),
            content: b64,
            sha256: hash.clone(),
            size_bytes: VALID_JPEG.len(),
        },
    });

    let unpacked = unpack_clipboard_packet(&packet).expect("Must unpack raw base64 JPEG");
    match unpacked {
        UnpackedClipboard::InlineBinary { mime_type, data, sha256 } => {
            assert_eq!(mime_type, "image/jpeg");
            assert_eq!(data, VALID_JPEG);
            assert_eq!(sha256, hash);
            assert!(data.contains(&0x00));
            assert_eq!(&data[0..2], b"\xff\xd8");
            assert_eq!(&data[data.len() - 2..], b"\xff\xd9");
        }
        _ => panic!("Expected InlineBinary"),
    }
}

#[test]
fn test_adversarial_various_image_mimes() {
    let test_mimes = [
        ("image/webp", b"RIFF\x14\x00\x00\x00WEBPVP8 \x08\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00".as_slice()),
        ("image/gif", b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;".as_slice()),
        ("image/bmp", b"BM\x1e\x00\x00\x00\x00\x00\x00\x00\x1a\x00\x00\x00\x0c\x00\x00\x00\x01\x00\x01\x00\x01\x00\x18\x00\x00\x00\xff\x00".as_slice()),
        ("image/svg+xml", b"<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"5\"/></svg>".as_slice()),
    ];

    for (mime, bytes) in test_mimes {
        let b64 = BASE64_STANDARD.encode(bytes);
        let hash = compute_sha256(bytes);

        let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
            version: 2,
            device_id: "phone-test".to_string(),
            timestamp: 2000,
            ttl_seconds: 120,
            payload: ClipboardInlinePayload {
                mime_type: mime.to_string(),
                content: b64,
                sha256: hash.clone(),
                size_bytes: bytes.len(),
            },
        });

        let unpacked = unpack_clipboard_packet(&packet).unwrap_or_else(|_| panic!("Failed unpacking {}", mime));
        match unpacked {
            UnpackedClipboard::InlineBinary { mime_type, data, sha256 } => {
                assert_eq!(mime_type, mime);
                assert_eq!(data, bytes);
                assert_eq!(sha256, hash);
            }
            _ => panic!("Expected InlineBinary for {}", mime),
        }
    }
}

#[test]
fn test_adversarial_empty_and_whitespace_image_content() {
    // 1. Completely empty content with image/png
    let empty_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-1".to_string(),
        timestamp: 1000,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: "".to_string(),
            sha256: compute_sha256(b""),
            size_bytes: 0,
        },
    });

    let res = unpack_clipboard_packet(&empty_packet);
    assert!(res.is_ok(), "Empty content must not panic");
    match res.unwrap() {
        UnpackedClipboard::InlineText { mime_type, text, .. } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(text, "");
        }
        UnpackedClipboard::InlineBinary { data, .. } => {
            assert_eq!(data.len(), 0);
        }
        _ => panic!("Expected InlineText or InlineBinary"),
    }

    // 2. Whitespace-only content with image/png
    let ws_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-1".to_string(),
        timestamp: 1001,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: "   \r\n\t  \n".to_string(),
            sha256: compute_sha256(b""),
            size_bytes: 0,
        },
    });

    let res_ws = unpack_clipboard_packet(&ws_packet);
    assert!(res_ws.is_ok(), "Whitespace content must not panic");

    // 3. Empty data URI prefix
    let empty_uri_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "phone-1".to_string(),
        timestamp: 1002,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: "data:image/png;base64,".to_string(),
            sha256: compute_sha256(b""),
            size_bytes: 0,
        },
    });

    let res_empty_uri = unpack_clipboard_packet(&empty_uri_packet);
    assert!(res_empty_uri.is_ok(), "Empty data URI must not panic");
}

#[test]
fn test_adversarial_corrupted_base64_strings() {
    let corrupted_samples = [
        "ThisIsNotValidBase64!!@@##$$",
        "A",
        "AAAAA",
        "====",
        "data:image/png;base64,Invalid!Payload?",
        "data:image/png;base64,A",
        "data:image/png;base64,====",
        "data:image/png;base64,!!!!",
    ];

    for (idx, sample) in corrupted_samples.iter().enumerate() {
        let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
            version: 2,
            device_id: format!("device-{}", idx),
            timestamp: 1000 + idx as u64,
            ttl_seconds: 120,
            payload: ClipboardInlinePayload {
                mime_type: "image/png".to_string(),
                content: sample.to_string(),
                sha256: compute_sha256(sample.as_bytes()),
                size_bytes: sample.len(),
            },
        });

        let res = unpack_clipboard_packet(&packet);
        assert!(res.is_ok(), "Corrupted sample '{}' must not panic", sample);
    }
}

#[test]
fn test_adversarial_base64_newlines_and_padding() {
    let raw = VALID_1X1_PNG;
    let b64 = BASE64_STANDARD.encode(raw);
    let hash = compute_sha256(raw);

    // Split base64 with newlines every 16 chars (standard MIME wrapping)
    let wrapped_b64 = b64
        .as_bytes()
        .chunks(16)
        .map(|chunk| std::str::from_utf8(chunk).unwrap())
        .collect::<Vec<_>>()
        .join("\r\n");

    let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "device-wrap".to_string(),
        timestamp: 1000,
        ttl_seconds: 120,
        payload: ClipboardInlinePayload {
            mime_type: "image/png".to_string(),
            content: wrapped_b64,
            sha256: hash.clone(),
            size_bytes: raw.len(),
        },
    });

    let unpacked = unpack_clipboard_packet(&packet).expect("MIME wrapped base64 must unpack");
    match unpacked {
        UnpackedClipboard::InlineBinary { data, sha256, .. } => {
            assert_eq!(data, raw);
            assert_eq!(sha256, hash);
        }
        _ => panic!("Expected InlineBinary"),
    }
}

#[test]
fn test_adversarial_case_insensitive_sha256_echo_suppression() {
    let (tx, _rx) = broadcast::channel(16);
    let cache = ClipboardPayloadCache::new(Duration::from_secs(120));
    let adapter = WaylandClipboardAdapter::new("desktop_test".to_string(), cache, tx);

    let raw_payload = VALID_1X1_PNG;
    let lowercase_hash = compute_sha256(raw_payload);
    let uppercase_hash = lowercase_hash.to_ascii_uppercase();

    // Mixed case hash
    let mixed_hash: String = lowercase_hash
        .chars()
        .enumerate()
        .map(|(i, c)| if i % 2 == 0 { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })
        .collect();

    // Paste remote payload records hash into last_pasted_sha256
    let _ = adapter.paste_remote_payload("image/png", raw_payload);
    assert_eq!(adapter.get_last_pasted_hash(), Some(lowercase_hash.clone()));

    // Echo suppression checks across all casing variants
    assert!(adapter.is_recent_paste(&lowercase_hash), "Lowercase hash must match");
    assert!(adapter.is_recent_paste(&uppercase_hash), "Uppercase hash must match");
    assert!(adapter.is_recent_paste(&mixed_hash), "Mixed-case hash must match");

    // Negative check
    let other_hash = compute_sha256(b"completely different payload");
    assert!(!adapter.is_recent_paste(&other_hash), "Different hash must not match");
    assert!(!adapter.is_recent_paste(&other_hash.to_ascii_uppercase()));
}

#[test]
fn test_adversarial_paste_remote_payload_binary_execution() {
    let (tx, _rx) = broadcast::channel(16);
    let cache = ClipboardPayloadCache::new(Duration::from_secs(120));
    let adapter = WaylandClipboardAdapter::new("desktop_test".to_string(), cache, tx);

    // Test pasting PNG with null bytes into Wayland
    let res_png = adapter.paste_remote_payload("image/png", VALID_1X1_PNG);
    // In Wayland environment, this should succeed via copy_data or wl-copy fallback
    if WaylandClipboardAdapter::is_wayland_available() {
        assert!(res_png.is_ok(), "paste_remote_payload for image/png should succeed in Wayland: {:?}", res_png.err());
    }

    // Test pasting JPEG with null bytes into Wayland
    let res_jpeg = adapter.paste_remote_payload("image/jpeg", VALID_JPEG);
    if WaylandClipboardAdapter::is_wayland_available() {
        assert!(res_jpeg.is_ok(), "paste_remote_payload for image/jpeg should succeed in Wayland: {:?}", res_jpeg.err());
    }

    // Test pasting text/uri-list with null-free payload
    let uri_payload = b"file:///home/prem/Downloads/test_staged.png\r\n";
    let res_uri = adapter.paste_remote_payload("text/uri-list", uri_payload);
    if WaylandClipboardAdapter::is_wayland_available() {
        assert!(res_uri.is_ok(), "paste_remote_payload for text/uri-list should succeed in Wayland: {:?}", res_uri.err());
    }
}
