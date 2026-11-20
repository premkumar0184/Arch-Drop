use archdrop_core::models::clipboard::{
    ClearReason, ClipboardClearPacket, ClipboardDescriptorPacket, ClipboardDescriptorPayload,
    ClipboardInlinePacket, ClipboardInlinePayload, ClipboardItemDto, ClipboardPacket,
    ClipboardPayload, ClipboardType, KdeConnectClipboard, KdeConnectPacket, SessionLockEvent,
    CLIPBOARD_EPHEMERAL_TTL_SECS, CLIPBOARD_INLINE_THRESHOLD,
};
use archdrop_core::models::device::{
    Device, DeviceType, PairingState, PeerDevice, ProtocolType, TransferDirection, TransferStatus,
};
use archdrop_core::models::dto_v2::{
    FileDto, FileMetadata, InfoResponseDtoV2, PrepareUploadRequestDtoV2,
    PrepareUploadResponseDtoV2, RegisterDtoV2, RegisterResponseDtoV2, DEFAULT_PORT,
    PROTOCOL_VERSION_V2,
};
use std::collections::HashMap;

#[test]
fn test_register_dto_v2_wire_compatibility() {
    let json_payload = r#"{
        "alias": "Test Pixel 8",
        "version": "2.2",
        "deviceModel": "Pixel 8",
        "deviceType": "mobile",
        "fingerprint": "1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF",
        "port": 53317,
        "protocol": "https",
        "download": true
    }"#;

    let dto: RegisterDtoV2 =
        serde_json::from_str(json_payload).expect("Deserialization must match LocalSend v2");
    assert_eq!(dto.alias, "Test Pixel 8");
    assert_eq!(dto.device_type, Some(DeviceType::Mobile));
    assert_eq!(dto.protocol, ProtocolType::Https);
    assert_eq!(dto.port, 53317);
    assert!(dto.download);

    // Roundtrip back to JSON
    let serialized = serde_json::to_string(&dto).unwrap();
    assert!(serialized.contains(r#""deviceType":"mobile""#));
    assert!(serialized.contains(r#""protocol":"https""#));
    assert!(serialized.contains(r#""port":53317"#));
    assert!(serialized.contains(r#""download":true"#));

    let deserialized: RegisterDtoV2 = serde_json::from_str(&serialized).unwrap();
    assert_eq!(dto, deserialized);
}

#[test]
fn test_device_type_fallback_to_desktop() {
    let json_payload = r#"{
        "alias": "Smart Fridge",
        "version": "2.2",
        "deviceType": "refrigerator",
        "fingerprint": "123456",
        "port": 53317,
        "protocol": "http"
    }"#;

    let dto: RegisterDtoV2 = serde_json::from_str(json_payload).unwrap();
    assert_eq!(dto.device_type, Some(DeviceType::Desktop));
    assert_eq!(dto.protocol, ProtocolType::Http);
    assert!(!dto.download);
}

#[test]
fn test_info_response_dto_v2() {
    let info = InfoResponseDtoV2 {
        alias: "Arch Drop Laptop".to_string(),
        version: PROTOCOL_VERSION_V2.to_string(),
        device_model: Some("Arch Linux (Hyprland)".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA".to_string(),
        download: false,
    };

    let serialized = serde_json::to_string(&info).unwrap();
    assert!(serialized.contains(r#""deviceType":"desktop""#));
    assert!(serialized.contains(
        r#""fingerprint":"4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA""#
    ));

    let deserialized: InfoResponseDtoV2 = serde_json::from_str(&serialized).unwrap();
    assert_eq!(info, deserialized);
}

#[test]
fn test_prepare_upload_serde() {
    let mut files = HashMap::new();
    files.insert(
        "file-1".to_string(),
        FileDto {
            id: "file-1".to_string(),
            file_name: "test.iso".to_string(),
            size: 1048576,
            file_type: "application/x-iso9660-image".to_string(),
            sha256: Some(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
            ),
            preview: None,
            metadata: Some(FileMetadata {
                modified: Some("2026-11-08T03:30:00Z".to_string()),
                accessed: None,
            }),
        },
    );

    let req = PrepareUploadRequestDtoV2 {
        info: RegisterDtoV2 {
            alias: "Sender".to_string(),
            version: "2.2".to_string(),
            device_model: None,
            device_type: Some(DeviceType::Mobile),
            fingerprint: "AAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDD"
                .to_string(),
            port: DEFAULT_PORT,
            protocol: ProtocolType::Https,
            download: false,
        },
        files,
    };

    let serialized = serde_json::to_string(&req).unwrap();
    let deserialized: PrepareUploadRequestDtoV2 = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.files.len(), 1);
    assert_eq!(deserialized.files["file-1"].file_name, "test.iso");

    // Response DTO
    let mut resp_files = HashMap::new();
    resp_files.insert("file-1".to_string(), "token-1234".to_string());
    let resp = PrepareUploadResponseDtoV2 {
        session_id: "session-abc".to_string(),
        files: resp_files,
    };
    let resp_json = serde_json::to_string(&resp).unwrap();
    let resp_de: PrepareUploadResponseDtoV2 = serde_json::from_str(&resp_json).unwrap();
    assert_eq!(resp_de.session_id, "session-abc");
    assert_eq!(resp_de.files["file-1"], "token-1234");
}

#[test]
fn test_clipboard_payload_threshold_rule() {
    // 1. Text payload <= 4 KB: Inlined
    let small_text = "Hello from Arch Drop Hyprland";
    assert!(small_text.len() <= CLIPBOARD_INLINE_THRESHOLD);

    let inline_payload = ClipboardInlinePayload {
        mime_type: "text/plain;charset=utf-8".to_string(),
        content: small_text.to_string(),
        sha256: "dummy-hash".to_string(),
        size_bytes: small_text.len(),
    };
    let payload = ClipboardPayload::Inline(inline_payload);
    assert!(payload.is_inline());
    assert_eq!(payload.size_bytes(), small_text.len());
    assert_eq!(payload.mime_type(), "text/plain;charset=utf-8");

    // 2. Large payload > 4 KB: Descriptor
    let large_size = 5000;
    assert!(large_size > CLIPBOARD_INLINE_THRESHOLD);

    let descriptor_payload = ClipboardDescriptorPayload {
        payload_id: "uuid-1234-5678".to_string(),
        primary_mime: "text/plain;charset=utf-8".to_string(),
        available_mimes: vec!["text/plain".to_string()],
        size_bytes: large_size,
        sha256: "large-hash".to_string(),
        ephemeral_token: "token-999".to_string(),
        download_url: Some("http://127.0.0.1:53317/api/v2/clipboard/uuid-1234-5678".to_string()),
    };
    let payload_desc = ClipboardPayload::Descriptor(descriptor_payload);
    assert!(!payload_desc.is_inline());
    assert_eq!(payload_desc.size_bytes(), large_size);
}

#[test]
fn test_clipboard_packets_and_kdeconnect() {
    // Inline packet
    let inline_packet = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "laptop-node".to_string(),
        timestamp: 1794022800000,
        ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
        payload: ClipboardInlinePayload {
            mime_type: "text/plain".to_string(),
            content: "Copied text".to_string(),
            sha256: "abc".to_string(),
            size_bytes: 11,
        },
    });
    let inline_json = serde_json::to_string(&inline_packet).unwrap();
    assert!(inline_json.contains(r#""type":"clipboard.inline""#));

    // Descriptor packet
    let descriptor_packet = ClipboardPacket::Descriptor(ClipboardDescriptorPacket {
        version: 2,
        device_id: "laptop-node".to_string(),
        timestamp: 1794022800000,
        ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
        descriptor: ClipboardDescriptorPayload {
            payload_id: "desc-123".to_string(),
            primary_mime: "text/plain".to_string(),
            available_mimes: vec!["text/plain".to_string()],
            size_bytes: 8192,
            sha256: "hash123".to_string(),
            ephemeral_token: "tok".to_string(),
            download_url: None,
        },
    });
    let desc_json = serde_json::to_string(&descriptor_packet).unwrap();
    assert!(desc_json.contains(r#""type":"clipboard.descriptor""#));

    // Clear packet with reason
    let clear_packet = ClipboardPacket::Clear(ClipboardClearPacket {
        version: 2,
        device_id: "laptop-node".to_string(),
        timestamp: 1794022800000,
        reason: ClearReason::SessionLock,
    });
    let clear_json = serde_json::to_string(&clear_packet).unwrap();
    assert!(clear_json.contains(r#""reason":"session_lock""#));

    // KDE Connect packet
    let mut kde = KdeConnectPacket::new_clipboard("Universal clipboard".to_string());
    assert_eq!(kde.packet_type, "kdeconnect.clipboard");
    assert_eq!(kde.body.content, "Universal clipboard");

    kde.body = KdeConnectClipboard {
        content: "Updated".to_string(),
        timestamp: Some(12345),
    };
    assert_eq!(kde.body.content, "Updated");

    // ClipboardItemDto
    let item = ClipboardItemDto {
        id: "item-1".to_string(),
        mime_type: "text/plain".to_string(),
        size: 5,
        sha256: "abc".to_string(),
        timestamp_epoch_ms: 12345,
        is_sensitive: false,
        content: Some("hello".to_string()),
        download_url: None,
    };
    assert_eq!(item.content.as_deref(), Some("hello"));

    // RegisterResponseDtoV2
    let reg_resp = RegisterResponseDtoV2 {
        alias: "Peer".to_string(),
        version: "2.2".to_string(),
        device_model: None,
        device_type: Some(DeviceType::Mobile),
        fingerprint: "ABC".to_string(),
        download: true,
    };
    assert_eq!(reg_resp.alias, "Peer");
}

#[test]
fn test_device_and_peer_models() {
    let device = Device {
        id: "id-1".to_string(),
        alias: "Arch Drop".to_string(),
        device_type: DeviceType::Desktop,
        device_model: Some("Hyprland".to_string()),
        fingerprint: "FP123".to_string(),
        port: 53317,
        protocol: ProtocolType::Https,
        download: true,
        ip: Some("127.0.0.1".parse().unwrap()),
    };

    let reg = device.to_register_dto();
    assert_eq!(reg.alias, "Arch Drop");
    assert_eq!(reg.device_type, Some(DeviceType::Desktop));

    let info = device.to_info_dto();
    assert_eq!(info.fingerprint, "FP123");

    let peer = PeerDevice::from_register_dto(reg, "10.2.0.2".parse().unwrap(), true);
    assert_eq!(peer.alias, "Arch Drop");
    assert_eq!(peer.pairing_state, PairingState::Paired);
    assert!(peer.is_paired);

    // Verify enums
    let _ = TransferDirection::Outgoing;
    let _ = TransferStatus::InProgress;
    let _ = ClipboardType::Text;
    let _ = SessionLockEvent {
        locked: true,
        timestamp_epoch_ms: 1000,
    };
}
