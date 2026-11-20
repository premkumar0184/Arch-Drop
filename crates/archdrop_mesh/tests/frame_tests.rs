use archdrop_mesh::frame::{DerpCodec, DerpFrame, FrameType};
use bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

#[test]
fn test_all_10_canonical_frame_types_roundtrip() {
    let mut codec = DerpCodec::new();

    let server_key = [0x11u8; 32];
    let client_key = [0x22u8; 32];
    let peer_key = [0x33u8; 32];
    let signature = [0x44u8; 64];
    let nonce = [0x55u8; 8];

    let frames = vec![
        DerpFrame::ServerKey {
            server_key,
            challenge: Bytes::from_static(b"CHALLENGE_SALT_1234"),
        },
        DerpFrame::ClientInfo {
            client_key,
            timestamp: 1731062400,
            signature,
        },
        DerpFrame::ServerInfo {
            data: Bytes::from_static(b"{\"version\":1,\"region\":\"mesh-1\"}"),
        },
        DerpFrame::SendPacket {
            dst_key: peer_key,
            payload: Bytes::from_static(b"ENCRYPTED_PACKET_PAYLOAD_FOR_PEER"),
        },
        DerpFrame::RecvPacket {
            src_key: peer_key,
            payload: Bytes::from_static(b"ENCRYPTED_PACKET_PAYLOAD_FROM_PEER"),
        },
        DerpFrame::KeepAlive,
        DerpFrame::PeerGone { peer_key },
        DerpFrame::PeerPresent { peer_key },
        DerpFrame::Ping { nonce },
        DerpFrame::Pong { nonce },
    ];

    for original in frames {
        let mut buf = BytesMut::new();
        codec
            .encode(original.clone(), &mut buf)
            .expect("Encoding frame must succeed");

        assert!(buf.len() >= 5, "Frame buffer must contain header");

        let decoded = codec
            .decode(&mut buf)
            .expect("Decoding frame must succeed")
            .expect("Frame must be fully decoded");

        assert_eq!(decoded, original, "Decoded frame must match original");
        assert!(buf.is_empty(), "Buffer must be empty after full frame decode");
    }
}

#[test]
fn test_partial_buffer_feed_and_streaming() {
    let mut codec = DerpCodec::new();
    let frame = DerpFrame::SendPacket {
        dst_key: [0xaa; 32],
        payload: Bytes::from_static(b"chunk_payload_data"),
    };

    let mut full_buf = BytesMut::new();
    codec.encode(frame.clone(), &mut full_buf).unwrap();

    let mut stream_buf = BytesMut::new();

    // Feed bytes one-by-one into stream_buf
    for (i, byte) in full_buf.iter().enumerate() {
        stream_buf.extend_from_slice(&[*byte]);

        let res = codec.decode(&mut stream_buf).unwrap();
        if i < full_buf.len() - 1 {
            assert!(
                res.is_none(),
                "Decoder should return Ok(None) on partial buffer at index {}",
                i
            );
        } else {
            let decoded = res.expect("Decoder should return complete frame on last byte");
            assert_eq!(decoded, frame);
        }
    }
}

#[test]
fn test_frame_size_limit_rejection() {
    let mut codec = DerpCodec::with_max_frame_size(1024);

    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::ServerInfo.to_u8()]);
    // Advertise 2048 bytes (exceeding 1024 limit)
    buf.extend_from_slice(&(2048u32).to_be_bytes());
    buf.extend_from_slice(&vec![0u8; 2048]);

    let err = codec.decode(&mut buf).unwrap_err();
    assert!(
        err.to_string().contains("exceeds maximum allowed"),
        "Error should reflect size limit: {err}"
    );
}

#[test]
fn test_unknown_frame_type_rejection() {
    let mut codec = DerpCodec::new();

    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[0xff]); // Unknown type 0xff
    buf.extend_from_slice(&(0u32).to_be_bytes());

    let err = codec.decode(&mut buf).unwrap_err();
    assert!(
        err.to_string().contains("Unknown frame type"),
        "Error should reflect unknown frame type: {err}"
    );
}

#[test]
fn test_truncated_payload_rejection() {
    let mut codec = DerpCodec::new();

    // ServerKey requires at least 32 bytes
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::ServerKey.to_u8()]);
    buf.extend_from_slice(&(16u32).to_be_bytes()); // Advertises only 16 bytes
    buf.extend_from_slice(&[0u8; 16]);

    let err = codec.decode(&mut buf).unwrap_err();
    assert!(
        err.to_string().contains("too short"),
        "Expected payload too short error: {err}"
    );

    // ClientInfo requires at least 104 bytes
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::ClientInfo.to_u8()]);
    buf.extend_from_slice(&(50u32).to_be_bytes());
    buf.extend_from_slice(&[0u8; 50]);

    let err = codec.decode(&mut buf).unwrap_err();
    assert!(
        err.to_string().contains("too short"),
        "Expected payload too short error: {err}"
    );

    // Ping requires at least 8 bytes
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::Ping.to_u8()]);
    buf.extend_from_slice(&(4u32).to_be_bytes());
    buf.extend_from_slice(&[0u8; 4]);

    let err = codec.decode(&mut buf).unwrap_err();
    assert!(
        err.to_string().contains("too short"),
        "Expected payload too short error: {err}"
    );
}

#[test]
fn test_alternate_type_code_compatibility() {
    let mut codec = DerpCodec::new();

    // 0x0c should decode as Ping
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[0x0c]);
    buf.extend_from_slice(&(8u32).to_be_bytes());
    buf.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

    let decoded = codec.decode(&mut buf).unwrap().expect("Should decode Ping");
    assert_eq!(
        decoded,
        DerpFrame::Ping {
            nonce: [1, 2, 3, 4, 5, 6, 7, 8]
        }
    );

    // 0x0d should decode as Pong
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[0x0d]);
    buf.extend_from_slice(&(8u32).to_be_bytes());
    buf.extend_from_slice(&[8, 7, 6, 5, 4, 3, 2, 1]);

    let decoded = codec.decode(&mut buf).unwrap().expect("Should decode Pong");
    assert_eq!(
        decoded,
        DerpFrame::Pong {
            nonce: [8, 7, 6, 5, 4, 3, 2, 1]
        }
    );
}
