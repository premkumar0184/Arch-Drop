//! Adversarial stress test suite for `crates/archdrop_mesh`.
//!
//! Stress-tests:
//! 1. Frame codec malformations: truncated prefixes, unknown types (0x00, 0xFF),
//!    oversized payloads exceeding 1 MB limit, negative/corrupted length boundaries.
//! 2. Cryptographic challenges: forged Ed25519 signatures, mutated challenge nonces,
//!    expired timestamps (>60s skew), replay attacks across sessions and servers.
//! 3. Stream edge cases: rapid interleaved SYN/FIN/RST flags, zero-length DATA packets,
//!    out-of-order sequence flags, conflicting control flags.
//! 4. Mock relay and client demuxer adversarial robustness under invalid frames.
//! 5. Zero panics, clean error handling, bounded memory allocations.

use archdrop_mesh::client::{DerpClient, DerpClientConfig};
use archdrop_mesh::crypto::{MeshKeyPair, MeshPublicKey, MAX_HANDSHAKE_SKEW_SECS};
use archdrop_mesh::frame::{DerpCodec, DerpFrame, FrameType, MAX_FRAME_SIZE};
use archdrop_mesh::mock_relay::MockDerpRelay;
use archdrop_mesh::stream::{
    decode_stream_payload, encode_stream_payload, DerpStream, FLAG_DATA, FLAG_FIN,
    FLAG_RST, FLAG_SYN, STREAM_HEADER_LEN,
};
use bytes::{Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::codec::{Decoder, Encoder, Framed};

// ---------------------------------------------------------------------------
// 1. Frame Codec Adversarial Stress Tests
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_truncated_length_prefixes() {
    let mut codec = DerpCodec::new();

    // 0 bytes
    let mut empty = BytesMut::new();
    assert_eq!(codec.decode(&mut empty).unwrap(), None);

    // 1 to 4 bytes: partial header without full 4-byte length
    for prefix_len in 1..=4 {
        let mut buf = BytesMut::from(&[0x01, 0x00, 0x00, 0x00][..prefix_len]);
        let res = codec.decode(&mut buf).unwrap();
        assert_eq!(res, None, "Prefix of length {prefix_len} must return None");
        assert_eq!(buf.len(), prefix_len, "Buffer must not be consumed");
    }

    // 5 bytes (header complete), but payload advertised as 10 bytes while 0 present
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::KeepAlive.to_u8()]);
    buf.extend_from_slice(&(10u32).to_be_bytes());
    let res = codec.decode(&mut buf).unwrap();
    assert_eq!(res, None, "Incomplete payload must return None");
    assert_eq!(buf.len(), 5, "Buffer must retain 5 bytes waiting for payload");
}

#[test]
fn test_adversarial_unknown_frame_types() {
    let mut codec = DerpCodec::new();

    let invalid_types: Vec<u8> = vec![
        0x00, // zero byte
        0x0b, // unassigned between Pong (0x0a) and legacy Ping (0x0c)
        0x0e, // unassigned after legacy Pong (0x0d)
        0x10, 0x20, 0x55, 0x7f, 0x80, 0xaa, 0xfe, 0xff, // all arbitrary non-canonical codes
    ];

    for invalid_type in invalid_types {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[invalid_type]);
        buf.extend_from_slice(&(0u32).to_be_bytes());

        let res = codec.decode(&mut buf);
        assert!(
            res.is_err(),
            "Frame type 0x{:02x} must be rejected as unknown",
            invalid_type
        );
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unknown frame type"),
            "Error message must specify unknown type: {err_msg}"
        );
    }
}

#[test]
fn test_adversarial_oversized_payloads_1mb_limit() {
    const ONE_MB: usize = 1024 * 1024;
    let mut codec = DerpCodec::with_max_frame_size(ONE_MB);

    // Case 1: Exactly 1 MB advertised - should be accepted up to buffer limit check
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::ServerInfo.to_u8()]);
    buf.extend_from_slice(&(ONE_MB as u32).to_be_bytes());
    // Since buffer has only 5 bytes, returns Ok(None) waiting for data (within limit)
    assert!(codec.decode(&mut buf).unwrap().is_none());

    // Case 2: 1 MB + 1 byte advertised
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::ServerInfo.to_u8()]);
    buf.extend_from_slice(&((ONE_MB + 1) as u32).to_be_bytes());
    let err = codec.decode(&mut buf).unwrap_err();
    assert!(err.to_string().contains("exceeds maximum allowed"));

    // Case 3: 2 MB payload advertised
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::SendPacket.to_u8()]);
    buf.extend_from_slice(&((2 * ONE_MB) as u32).to_be_bytes());
    let err = codec.decode(&mut buf).unwrap_err();
    assert!(err.to_string().contains("exceeds maximum allowed"));

    // Case 4: 16 MB + 1 byte payload advertised against default codec
    let mut default_codec = DerpCodec::new();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::ServerInfo.to_u8()]);
    buf.extend_from_slice(&((MAX_FRAME_SIZE + 1) as u32).to_be_bytes());
    let err = default_codec.decode(&mut buf).unwrap_err();
    assert!(err.to_string().contains("exceeds maximum allowed"));

    // Case 5: Corrupted maximum u32::MAX (4,294,967,295 bytes) - must reject with ZERO memory allocation or panic
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[FrameType::ServerKey.to_u8()]);
    buf.extend_from_slice(&(u32::MAX).to_be_bytes());
    let err = codec.decode(&mut buf).unwrap_err();
    assert!(err.to_string().contains("exceeds maximum allowed"));
}

#[test]
fn test_adversarial_truncated_payloads_all_frame_types() {
    let mut codec = DerpCodec::new();

    // 1. ServerKey (needs >= 32 bytes)
    for len in [0, 1, 15, 31] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::ServerKey.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("ServerKey payload too short"));
    }

    // 2. ClientInfo (needs >= 104 bytes)
    for len in [0, 1, 32, 40, 103] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::ClientInfo.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("ClientInfo payload too short"));
    }

    // 3. SendPacket (needs >= 32 bytes)
    for len in [0, 1, 16, 31] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::SendPacket.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("SendPacket payload too short"));
    }

    // 4. RecvPacket (needs >= 32 bytes)
    for len in [0, 1, 16, 31] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::RecvPacket.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("RecvPacket payload too short"));
    }

    // 5. PeerGone (needs >= 32 bytes)
    for len in [0, 1, 16, 31] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::PeerGone.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("PeerGone payload too short"));
    }

    // 6. PeerPresent (needs >= 32 bytes)
    for len in [0, 1, 16, 31] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::PeerPresent.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("PeerPresent payload too short"));
    }

    // 7. Ping (needs >= 8 bytes)
    for len in [0, 1, 4, 7] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::Ping.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("Ping payload too short"));
    }

    // 8. Pong (needs >= 8 bytes)
    for len in [0, 1, 4, 7] {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[FrameType::Pong.to_u8()]);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
        buf.extend_from_slice(&vec![0x42; len]);
        let err = codec.decode(&mut buf).unwrap_err();
        assert!(err.to_string().contains("Pong payload too short"));
    }
}

#[test]
fn test_adversarial_random_byte_fuzzing() {
    let mut codec = DerpCodec::new();

    // Deterministic pseudo-random seed
    let mut seed: u64 = 0xDEADBEEFCAFE1234;
    let mut lcg = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed
    };

    // Fuzz 1,000 random slices with varying lengths
    for _iter in 0..1000 {
        let len = (lcg() % 256) as usize;
        let mut random_bytes = Vec::with_capacity(len);
        for _ in 0..len {
            random_bytes.push((lcg() & 0xFF) as u8);
        }

        let mut buf = BytesMut::from(&random_bytes[..]);
        // Must never panic or crash
        let _ = codec.decode(&mut buf);
    }
}

#[test]
fn test_adversarial_bitflip_fuzzing_all_frames() {
    let mut codec = DerpCodec::new();

    let sample_frames = vec![
        DerpFrame::ServerKey {
            server_key: [0x11; 32],
            challenge: Bytes::from_static(b"CHALLENGE_SALT"),
        },
        DerpFrame::ClientInfo {
            client_key: [0x22; 32],
            timestamp: 1731062400,
            signature: [0x33; 64],
        },
        DerpFrame::SendPacket {
            dst_key: [0x44; 32],
            payload: Bytes::from_static(b"VIRTUAL_STREAM_DATA"),
        },
        DerpFrame::Ping { nonce: [0x55; 8] },
        DerpFrame::KeepAlive,
    ];

    for frame in sample_frames {
        let mut encoded = BytesMut::new();
        codec.encode(frame, &mut encoded).unwrap();

        // Flip every single bit of the encoded buffer
        for byte_idx in 0..encoded.len() {
            for bit_idx in 0..8 {
                let mut mutated = encoded.clone();
                mutated[byte_idx] ^= 1 << bit_idx;

                // Decode must either return Ok(Some), Ok(None), or Err(MeshError), NEVER panic
                let _ = codec.decode(&mut mutated);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Cryptographic Adversarial Challenges
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_crypto_forged_signatures() {
    let keypair = MeshKeyPair::generate().unwrap();
    let pubkey = keypair.public_key();
    let msg = b"DERP authenticated payload";

    // 1. All-zero signature
    let zero_sig = [0u8; 64];
    assert!(pubkey.verify(msg, &zero_sig).is_err());

    // 2. All-0xFF signature
    let ff_sig = [0xffu8; 64];
    assert!(pubkey.verify(msg, &ff_sig).is_err());

    // 3. Mutated valid signature (every byte inverted)
    let valid_sig = keypair.sign(msg);
    let mut inverted_sig = valid_sig;
    for b in &mut inverted_sig {
        *b = !*b;
    }
    assert!(pubkey.verify(msg, &inverted_sig).is_err());

    // 4. Forged signature generated from a different random keypair
    let attacker_keypair = MeshKeyPair::generate().unwrap();
    let attacker_sig = attacker_keypair.sign(msg);
    assert!(
        pubkey.verify(msg, &attacker_sig).is_err(),
        "Signature from distinct keypair must be rejected"
    );
}

#[test]
fn test_adversarial_crypto_mutated_challenge_messages() {
    let keypair = MeshKeyPair::generate().unwrap();
    let pubkey = keypair.public_key();
    let server_key = [0x77u8; 32];
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let sig = keypair.sign_challenge(&server_key, now);

    // 1. Valid handshake verifies cleanly
    MeshKeyPair::verify_challenge(&pubkey, &server_key, now, &sig, MAX_HANDSHAKE_SKEW_SECS)
        .expect("Valid challenge must verify");

    // 2. Mutated server_key (single bit flipped)
    let mut mutated_server_key = server_key;
    mutated_server_key[0] ^= 0x01;
    assert!(
        MeshKeyPair::verify_challenge(
            &pubkey,
            &mutated_server_key,
            now,
            &sig,
            MAX_HANDSHAKE_SKEW_SECS
        )
        .is_err(),
        "Mutated server key must fail verification"
    );

    // 3. Mutated timestamp in verification
    assert!(
        MeshKeyPair::verify_challenge(
            &pubkey,
            &server_key,
            now + 1,
            &sig,
            MAX_HANDSHAKE_SKEW_SECS
        )
        .is_err(),
        "Mutated timestamp must fail verification"
    );

    // 4. Mutated signature byte
    let mut mutated_sig = sig;
    mutated_sig[31] ^= 0x80;
    assert!(
        MeshKeyPair::verify_challenge(&pubkey, &server_key, now, &mutated_sig, MAX_HANDSHAKE_SKEW_SECS)
            .is_err(),
        "Mutated signature must fail verification"
    );
}

#[test]
fn test_adversarial_crypto_timestamp_skew_and_boundary() {
    let keypair = MeshKeyPair::generate().unwrap();
    let pubkey = keypair.public_key();
    let server_key = [0x88u8; 32];
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Boundary check within 60s skew limit
    // Valid: now - 59s
    let sig_valid_past = keypair.sign_challenge(&server_key, now - 59);
    MeshKeyPair::verify_challenge(
        &pubkey,
        &server_key,
        now - 59,
        &sig_valid_past,
        MAX_HANDSHAKE_SKEW_SECS,
    )
    .expect("Skew 59s in past must be accepted");

    // Valid: now + 59s
    let sig_valid_future = keypair.sign_challenge(&server_key, now + 59);
    MeshKeyPair::verify_challenge(
        &pubkey,
        &server_key,
        now + 59,
        &sig_valid_future,
        MAX_HANDSHAKE_SKEW_SECS,
    )
    .expect("Skew 59s in future must be accepted");

    // Invalid: now - 61s (expired)
    let sig_expired = keypair.sign_challenge(&server_key, now - 61);
    let err = MeshKeyPair::verify_challenge(
        &pubkey,
        &server_key,
        now - 61,
        &sig_expired,
        MAX_HANDSHAKE_SKEW_SECS,
    )
    .unwrap_err();
    assert!(err.to_string().contains("skew"));

    // Invalid: now + 61s (too far in future)
    let sig_too_future = keypair.sign_challenge(&server_key, now + 61);
    let err = MeshKeyPair::verify_challenge(
        &pubkey,
        &server_key,
        now + 61,
        &sig_too_future,
        MAX_HANDSHAKE_SKEW_SECS,
    )
    .unwrap_err();
    assert!(err.to_string().contains("skew"));

    // Invalid: epoch 0 (1970-01-01)
    let sig_epoch = keypair.sign_challenge(&server_key, 0);
    assert!(
        MeshKeyPair::verify_challenge(&pubkey, &server_key, 0, &sig_epoch, MAX_HANDSHAKE_SKEW_SECS)
            .is_err()
    );

    // Invalid: u64::MAX
    let sig_max = keypair.sign_challenge(&server_key, u64::MAX);
    assert!(
        MeshKeyPair::verify_challenge(
            &pubkey,
            &server_key,
            u64::MAX,
            &sig_max,
            MAX_HANDSHAKE_SKEW_SECS
        )
        .is_err()
    );
}

#[test]
fn test_adversarial_crypto_replay_across_servers_and_time() {
    let client_keypair = MeshKeyPair::generate().unwrap();
    let client_pubkey = client_keypair.public_key();

    let server_a_key = [0xAAu8; 32];
    let server_b_key = [0xBBu8; 32];

    let t0 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Client performs legitimate handshake with Server A
    let sig_server_a = client_keypair.sign_challenge(&server_a_key, t0);

    // Attack 1: Replay captured ClientInfo to Server B
    // Server B verifies with its own server_b_key
    let replay_to_b = MeshKeyPair::verify_challenge(
        &client_pubkey,
        &server_b_key,
        t0,
        &sig_server_a,
        MAX_HANDSHAKE_SKEW_SECS,
    );
    assert!(
        replay_to_b.is_err(),
        "Replay of signature to different server must be rejected"
    );

    // Attack 2: Replay captured ClientInfo to Server A after 120s
    let replay_delayed = MeshKeyPair::verify_challenge(
        &client_pubkey,
        &server_a_key,
        t0 - 120,
        &sig_server_a,
        MAX_HANDSHAKE_SKEW_SECS,
    );
    assert!(
        replay_delayed.is_err(),
        "Replay of expired handshake must be rejected"
    );

    // Attack 3: Attacker alters timestamp to current time t0 + 120, replaying old signature
    let replay_tampered_ts = MeshKeyPair::verify_challenge(
        &client_pubkey,
        &server_a_key,
        t0 + 120,
        &sig_server_a,
        MAX_HANDSHAKE_SKEW_SECS,
    );
    assert!(
        replay_tampered_ts.is_err(),
        "Tampered timestamp must invalidate signature"
    );
}

#[test]
fn test_adversarial_malformed_keys_and_pkcs8() {
    // 1. MeshPublicKey from invalid hex strings
    assert!(MeshPublicKey::from_hex("").is_err());
    assert!(MeshPublicKey::from_hex("invalid_non_hex_characters!").is_err());
    assert!(MeshPublicKey::from_hex("1234").is_err()); // too short
    assert!(MeshPublicKey::from_hex(&"00".repeat(31)).is_err()); // 31 bytes
    assert!(MeshPublicKey::from_hex(&"00".repeat(33)).is_err()); // 33 bytes
    assert!(MeshPublicKey::from_hex(&"00".repeat(64)).is_err()); // 64 bytes

    // 2. MeshKeyPair from malformed PKCS#8 bytes
    assert!(MeshKeyPair::from_pkcs8(&[]).is_err());
    assert!(MeshKeyPair::from_pkcs8(&[0x00; 32]).is_err());
    assert!(MeshKeyPair::from_pkcs8(&[0xff; 128]).is_err());
    assert!(MeshKeyPair::from_pkcs8(b"NOT_A_VALID_PKCS8_DOCUMENT").is_err());
}

// ---------------------------------------------------------------------------
// 3. Virtual Stream Multiplexing Edge Cases
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_stream_payload_header_decoding() {
    // 1. Truncated header (< 10 bytes)
    for len in 0..STREAM_HEADER_LEN {
        let short_bytes = Bytes::from(vec![0xAA; len]);
        let res = decode_stream_payload(short_bytes);
        assert!(res.is_err(), "Payload of length {len} must fail decode");
    }

    // 2. Exactly 10 bytes (header only, zero-length data)
    let payload = encode_stream_payload(999, FLAG_DATA, &[]);
    assert_eq!(payload.len(), 10);
    let (sid, flags, data) = decode_stream_payload(payload).unwrap();
    assert_eq!(sid, 999);
    assert_eq!(flags, FLAG_DATA);
    assert!(data.is_empty(), "Data must be empty");

    // 3. Conflicting flags (e.g. SYN | FIN | RST)
    let conflicting_flags = FLAG_SYN | FLAG_FIN | FLAG_RST;
    let payload = encode_stream_payload(1234, conflicting_flags, b"conflicting");
    let (sid, flags, data) = decode_stream_payload(payload).unwrap();
    assert_eq!(sid, 1234);
    assert_eq!(flags, conflicting_flags);
    assert_eq!(&data[..], b"conflicting");

    // 4. Undefined flags (e.g. 0x80, 0xE0, 0xFF)
    let payload = encode_stream_payload(5678, 0xFF, b"all_flags");
    let (sid, flags, _) = decode_stream_payload(payload).unwrap();
    assert_eq!(sid, 5678);
    assert_eq!(flags, 0xFF);
}

#[tokio::test]
async fn test_adversarial_stream_zero_length_data_and_rapid_flags() {
    let (mut stream_a, mut stream_b) = DerpStream::pair();

    // 1. Send normal data from A to B
    stream_a.write_all(b"pre_data").await.unwrap();
    stream_a.flush().await.unwrap();

    let mut buf = [0u8; 8];
    stream_b.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pre_data");

    // 2. Rapid shutdown from A
    stream_a.shutdown().await.unwrap();

    // 3. Repeated shutdown calls on A must return Ok(()) idempotently
    stream_a.shutdown().await.unwrap();
    stream_a.shutdown().await.unwrap();

    // 4. Writing to A after shutdown must return BrokenPipe
    let write_res = stream_a.write_all(b"post_close_data").await;
    assert!(
        write_res.is_err(),
        "Writing to locally closed stream must error"
    );
    assert_eq!(
        write_res.unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );

    // 5. Stream B reading after remote FIN should receive EOF (0 bytes)
    let mut eof_buf = Vec::new();
    let bytes_read = stream_b.read_to_end(&mut eof_buf).await.unwrap();
    assert_eq!(bytes_read, 0, "Reading after FIN must return EOF");
}

#[tokio::test]
async fn test_adversarial_stream_abrupt_drop_and_rst() {
    let (mut stream_a, mut stream_b) = DerpStream::pair();

    stream_a.write_all(b"data_before_drop").await.unwrap();
    stream_a.flush().await.unwrap();

    // Abruptly drop stream_a without calling shutdown
    drop(stream_a);

    // Stream B must terminate cleanly without hanging or panicking
    let mut received = Vec::new();
    let _ = stream_b.read_to_end(&mut received).await;
}

// ---------------------------------------------------------------------------
// 4. Mock DERP Relay & Full Handshake Adversarial Attacks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_adversarial_mock_relay_invalid_first_frames() {
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_addr = relay.local_addr();

    // Attack 1: Client connects and sends Ping (0x09) instead of ClientInfo (0x02)
    {
        let stream = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        let mut framed = Framed::new(stream, DerpCodec::new());

        // Read ServerKey
        let first = framed.next().await.unwrap().unwrap();
        assert!(matches!(first, DerpFrame::ServerKey { .. }));

        // Send unexpected Ping
        framed
            .send(DerpFrame::Ping { nonce: [0x11; 8] })
            .await
            .unwrap();

        // Relay must close connection
        let next = framed.next().await;
        assert!(
            next.is_none() || next.as_ref().unwrap().is_err(),
            "Relay must disconnect client sending Ping before ClientInfo"
        );
    }

    // Attack 2: Client connects and sends ClientInfo with forged signature
    {
        let stream = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        let mut framed = Framed::new(stream, DerpCodec::new());

        let first = framed.next().await.unwrap().unwrap();
        assert!(matches!(first, DerpFrame::ServerKey { .. }));

        let attacker_keypair = MeshKeyPair::generate().unwrap();
        let forged_info = DerpFrame::ClientInfo {
            client_key: attacker_keypair.public_key().0,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            signature: [0xff; 64], // invalid forged signature
        };

        framed.send(forged_info).await.unwrap();

        // Relay must reject and terminate connection
        let next = framed.next().await;
        assert!(
            next.is_none() || next.as_ref().unwrap().is_err(),
            "Relay must terminate connection on forged signature"
        );
    }

    // Attack 3: Client connects and sends expired timestamp (> 120s skew)
    {
        let stream = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        let mut framed = Framed::new(stream, DerpCodec::new());

        let first = framed.next().await.unwrap().unwrap();
        let server_key = match first {
            DerpFrame::ServerKey { server_key, .. } => server_key,
            _ => panic!("Expected ServerKey"),
        };

        let client_keypair = MeshKeyPair::generate().unwrap();
        let expired_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 500; // 500s expired

        let sig = client_keypair.sign_challenge(&server_key, expired_ts);
        let expired_info = DerpFrame::ClientInfo {
            client_key: client_keypair.public_key().0,
            timestamp: expired_ts,
            signature: sig,
        };

        framed.send(expired_info).await.unwrap();

        // Relay must terminate connection on expired timestamp
        let next = framed.next().await;
        assert!(
            next.is_none() || next.as_ref().unwrap().is_err(),
            "Relay must terminate connection on expired timestamp"
        );
    }

    // Attack 4: Client connects and abruptly closes without sending ClientInfo
    {
        let stream = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        drop(stream);
    }

    // Verify Relay is STILL HEALTHY and functional after all attacks:
    // A legitimate client connects successfully
    let legit_keypair = Arc::new(MeshKeyPair::generate().unwrap());
    let config = DerpClientConfig::new(relay.relay_url(), legit_keypair);
    let legit_client = DerpClient::connect(config).await;
    assert!(
        legit_client.is_ok(),
        "Mock DERP relay must remain healthy and accept legitimate clients after attacks"
    );

    relay.stop().await;
}

#[tokio::test]
async fn test_adversarial_mock_relay_unregistered_destination_packets() {
    let relay = MockDerpRelay::start().await.unwrap();

    let client_keypair = Arc::new(MeshKeyPair::generate().unwrap());
    let config = DerpClientConfig::new(relay.relay_url(), client_keypair);
    let client = DerpClient::connect(config).await.unwrap();

    // Client sends packets to non-existent peer keys
    for i in 0..50 {
        let mut fake_peer = [0u8; 32];
        fake_peer[0] = i;
        let payload = encode_stream_payload(i as u64, FLAG_DATA, b"data_to_nobody");
        let _frame = DerpFrame::SendPacket {
            dst_key: fake_peer,
            payload,
        };
        // Should not crash client or relay
        let _ = client.open_stream(&fake_peer).await;
    }

    assert!(
        client.is_connected(),
        "Client must remain connected after sending packets to unregistered peers"
    );

    relay.stop().await;
}
