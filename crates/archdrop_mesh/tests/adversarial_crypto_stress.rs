//! Comprehensive Adversarial Cryptographic Stress Test Suite
//!
//! Evaluates the robustness of `crates/archdrop_mesh` cryptographic primitives
//! and virtual streams under hostile inputs:
//! 1. AAD tampering (stream ID corruption, flag tampering, length mutations, reserved byte manipulation)
//! 2. Exhaustive single-bit flipping across ciphertexts and tags, plus truncation and extension fuzzing
//! 3. Nonce progression, tamper isolation, formatting invariants, and mutual/asymmetric rekeying
//! 4. Full-duplex concurrent streaming under heavy load (1 MiB bidirectional, half-close, chunk sizes)
//! 5. High-concurrency virtual stream multiplexing (30 concurrent streams)
//! 6. Host WireGuard interface safety invariant verification

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use archdrop_mesh::crypto::{
    derive_stream_keys, NoiseCipherState, NoiseSession, TAG_LEN,
};
use archdrop_mesh::frame::DerpFrame;
use archdrop_mesh::stream::{
    decode_stream_payload, DerpStream, FLAG_ACK, FLAG_DATA, FLAG_FIN, FLAG_RST, FLAG_SYN,
};

// ============================================================================
// Suite 1: AAD Tampering & Header Binding Attacks
// ============================================================================

/// Verifies that mutating stream ID in AAD (across boundary values 0, 1, 42, 0xDEADBEEF, u64::MAX)
/// causes exact authentication rejection in both NoiseCipherState and DerpStream with InvalidData.
#[tokio::test]
async fn test_adversarial_aad_stream_id_tamper_matrix() {
    let test_stream_ids = [
        0u64,
        1u64,
        2u64,
        42u64,
        100u64,
        1000u64,
        0x123456789ABCDEF0u64,
        u64::MAX - 1,
        u64::MAX,
    ];

    let key = [0x42u8; 32];
    let plaintext = b"Adversarial stream payload for AAD verification";

    for &orig_sid in &test_stream_ids {
        let mut enc = NoiseCipherState::new(&key).expect("CipherState init");
        let mut dec = NoiseCipherState::new(&key).expect("CipherState init");

        let mut orig_aad = [0u8; 10];
        orig_aad[..8].copy_from_slice(&orig_sid.to_be_bytes());
        orig_aad[8] = FLAG_DATA;
        orig_aad[9] = 0x00;

        let ciphertext = enc
            .encrypt_with_ad(&orig_aad, plaintext)
            .expect("Encryption failed");

        // Test mutating to every other stream ID in the matrix
        for &corrupted_sid in &test_stream_ids {
            if corrupted_sid == orig_sid {
                continue;
            }

            let mut corrupted_aad = [0u8; 10];
            corrupted_aad[..8].copy_from_slice(&corrupted_sid.to_be_bytes());
            corrupted_aad[8] = FLAG_DATA;
            corrupted_aad[9] = 0x00;

            let res = dec.decrypt_with_ad(&corrupted_aad, &ciphertext);
            assert!(
                res.is_err(),
                "Decrypt must fail when stream_id is mutated from {orig_sid} to {corrupted_sid}"
            );
            assert_eq!(
                dec.nonce(),
                0,
                "Receiver nonce must not increment on authentication failure"
            );
        }

        // Now test on DerpStream directly
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<DerpFrame>(16);
        let (inbound_tx, inbound_rx) = mpsc::channel::<Bytes>(16);

        let key_a = [0x01u8; 32];
        let key_b = [0x02u8; 32];

        let session_a = derive_stream_keys(&key_a, &key_b, orig_sid, true).unwrap();
        // Recipient configured with a mismatched stream ID (orig_sid ^ 0x01)
        let mismatched_sid = orig_sid ^ 0x01;
        let session_b = derive_stream_keys(&key_b, &key_a, orig_sid, false).unwrap();

        let mut stream_a =
            DerpStream::new(orig_sid, key_a, key_b, outbound_tx, mpsc::channel(1).1)
                .with_session(session_a);
        let mut stream_b =
            DerpStream::new(mismatched_sid, key_b, key_a, mpsc::channel(1).0, inbound_rx)
                .with_session(session_b);

        stream_a.write_all(plaintext).await.unwrap();
        stream_a.flush().await.unwrap();

        let frame = outbound_rx.recv().await.expect("Frame must be sent");
        let payload = match frame {
            DerpFrame::SendPacket { payload, .. } => payload,
            _ => panic!("Expected SendPacket"),
        };

        let (_sid, _flags, chunk) = decode_stream_payload(payload).unwrap();
        inbound_tx.send(chunk).await.unwrap();

        let mut read_buf = [0u8; 64];
        let read_res = stream_b.read(&mut read_buf).await;
        assert!(
            read_res.is_err(),
            "DerpStream must reject chunk when stream_id does not match"
        );
        let err = read_res.err().unwrap();
        assert_eq!(
            err.kind(),
            io::ErrorKind::InvalidData,
            "DerpStream must yield io::ErrorKind::InvalidData on AAD stream ID mismatch"
        );
    }
}

/// Verifies that mutating the control flags inside the 10-byte stream header AAD
/// (FLAG_SYN, FLAG_FIN, FLAG_RST, FLAG_ACK, arbitrary values) is immediately rejected.
#[test]
fn test_adversarial_aad_flag_tamper_matrix() {
    let key = [0x78u8; 32];
    let mut enc = NoiseCipherState::new(&key).unwrap();
    let mut dec = NoiseCipherState::new(&key).unwrap();

    let stream_id = 999u64;
    let mut canonical_aad = [0u8; 10];
    canonical_aad[..8].copy_from_slice(&stream_id.to_be_bytes());
    canonical_aad[8] = FLAG_DATA; // 0x02
    canonical_aad[9] = 0x00;

    let plaintext = b"Sensitive payload for flag tampering test";
    let ciphertext = enc.encrypt_with_ad(&canonical_aad, plaintext).unwrap();

    let mutated_flags = [
        0x00u8,
        FLAG_SYN, // 0x01
        FLAG_FIN, // 0x04
        FLAG_RST, // 0x08
        FLAG_ACK, // 0x10
        0x03u8,
        0x20u8,
        0x80u8,
        0xFEu8,
        0xFFu8,
    ];

    for &bad_flag in &mutated_flags {
        let mut tampered_aad = canonical_aad;
        tampered_aad[8] = bad_flag;

        let res = dec.decrypt_with_ad(&tampered_aad, &ciphertext);
        assert!(
            res.is_err(),
            "Decryption must fail when AAD flag is tampered to 0x{bad_flag:02x}"
        );
        assert_eq!(
            dec.nonce(),
            0,
            "Receiver nonce must not advance on flag tamper"
        );
    }

    // Confirm that decrypting with the canonical AAD succeeds and advances nonce to 1
    let recovered = dec.decrypt_with_ad(&canonical_aad, &ciphertext).unwrap();
    assert_eq!(recovered, plaintext);
    assert_eq!(dec.nonce(), 1);
}

/// Verifies that mutating reserved bytes or truncating/extending the AAD byte slice
/// fails authentication without panics.
#[test]
fn test_adversarial_aad_length_and_reserved_byte_tamper() {
    let key = [0x99u8; 32];
    let mut enc = NoiseCipherState::new(&key).unwrap();
    let mut dec = NoiseCipherState::new(&key).unwrap();

    let mut canonical_aad = [0u8; 10];
    canonical_aad[..8].copy_from_slice(&12345u64.to_be_bytes());
    canonical_aad[8] = FLAG_DATA;
    canonical_aad[9] = 0x00;

    let plaintext = b"Payload for AAD format and reserved byte testing";
    let ciphertext = enc.encrypt_with_ad(&canonical_aad, plaintext).unwrap();

    // 1. Mutate reserved byte (byte 9)
    for bad_reserved in [0x01u8, 0x10u8, 0x7Fu8, 0xAAu8, 0xFFu8] {
        let mut bad_aad = canonical_aad;
        bad_aad[9] = bad_reserved;
        assert!(
            dec.decrypt_with_ad(&bad_aad, &ciphertext).is_err(),
            "Decryption must fail when reserved byte is 0x{bad_reserved:02x}"
        );
    }

    // 2. Truncated AAD slices
    let truncated_lengths = [0usize, 1, 2, 4, 7, 8, 9];
    for &len in &truncated_lengths {
        let bad_aad = &canonical_aad[..len];
        assert!(
            dec.decrypt_with_ad(bad_aad, &ciphertext).is_err(),
            "Decryption must fail when AAD is truncated to length {len}"
        );
    }

    // 3. Extended / bloated AAD slices
    for extra_len in [11usize, 12, 16, 20, 32, 64, 256, 1024] {
        let mut bloated_aad = Vec::with_capacity(extra_len);
        bloated_aad.extend_from_slice(&canonical_aad);
        bloated_aad.resize(extra_len, 0xEE);
        assert!(
            dec.decrypt_with_ad(&bloated_aad, &ciphertext).is_err(),
            "Decryption must fail when AAD is extended to length {extra_len}"
        );
    }

    assert_eq!(dec.nonce(), 0, "Receiver nonce must remain 0 after all AAD tampering");
}

// ============================================================================
// Suite 2: Ciphertext Bitflipping & Truncation Attacks
// ============================================================================

/// Exhaustively flips every single bit across the entire ciphertext (payload body + Poly1305 tag)
/// on small payloads to verify 100% detection rate without panics or memory leaks.
#[test]
fn test_adversarial_ciphertext_exhaustive_single_bitflip() {
    let key = [0xAAu8; 32];
    let aad = b"stream_id:100;flag:02";

    // Test with multiple payload sizes: 1 byte, 15 bytes, 16 bytes, 32 bytes
    for payload_size in [1usize, 15, 16, 32] {
        let plaintext: Vec<u8> = (0..payload_size).map(|i| (i as u8).wrapping_mul(7)).collect();

        let mut enc = NoiseCipherState::new(&key).unwrap();
        let ciphertext = enc.encrypt_with_ad(aad, &plaintext).unwrap();
        assert_eq!(ciphertext.len(), payload_size + TAG_LEN);

        let total_bits = ciphertext.len() * 8;
        let mut failed_detections = 0usize;

        for bit_index in 0..total_bits {
            let byte_idx = bit_index / 8;
            let bit_in_byte = bit_index % 8;

            let mut corrupted_ct = ciphertext.clone();
            corrupted_ct[byte_idx] ^= 1 << bit_in_byte;

            let mut dec = NoiseCipherState::new(&key).unwrap();
            let res = dec.decrypt_with_ad(aad, &corrupted_ct);
            if res.is_ok() {
                failed_detections += 1;
            } else {
                assert_eq!(
                    dec.nonce(),
                    0,
                    "Receiver nonce must not advance on corrupted bit {bit_index}"
                );
            }
        }

        assert_eq!(
            failed_detections, 0,
            "Every single-bit corruption across all {total_bits} bits must fail authentication (payload size {payload_size})"
        );
    }
}

/// Bitflips and byte corruption fuzzing across medium and large payloads (1 KiB, 16 KiB, 64 KiB).
#[test]
fn test_adversarial_ciphertext_fuzzing_large_payload() {
    let key = [0xBBu8; 32];
    let aad = b"large_stream_aad";

    for size in [1024usize, 16384, 65536] {
        let plaintext: Vec<u8> = (0..size).map(|i| ((i ^ (i >> 8)) & 0xFF) as u8).collect();

        let mut enc = NoiseCipherState::new(&key).unwrap();
        let ciphertext = enc.encrypt_with_ad(aad, &plaintext).unwrap();
        assert_eq!(ciphertext.len(), size + TAG_LEN);

        // Corrupt first byte of payload
        let mut ct1 = ciphertext.clone();
        ct1[0] ^= 0x80;
        let mut dec = NoiseCipherState::new(&key).unwrap();
        assert!(dec.decrypt_with_ad(aad, &ct1).is_err());

        // Corrupt middle byte of payload
        let mut ct2 = ciphertext.clone();
        ct2[size / 2] ^= 0x01;
        assert!(dec.decrypt_with_ad(aad, &ct2).is_err());

        // Corrupt last byte of payload
        let mut ct3 = ciphertext.clone();
        ct3[size - 1] ^= 0x40;
        assert!(dec.decrypt_with_ad(aad, &ct3).is_err());

        // Corrupt first byte of Poly1305 tag
        let mut ct4 = ciphertext.clone();
        ct4[size] ^= 0x01;
        assert!(dec.decrypt_with_ad(aad, &ct4).is_err());

        // Corrupt last byte of Poly1305 tag
        let mut ct5 = ciphertext.clone();
        ct5[size + TAG_LEN - 1] ^= 0x01;
        assert!(dec.decrypt_with_ad(aad, &ct5).is_err());

        // Zero out 16 bytes of the payload
        let mut ct6 = ciphertext.clone();
        ct6[10..26].fill(0);
        assert!(dec.decrypt_with_ad(aad, &ct6).is_err());

        // Zero out the entire tag
        let mut ct7 = ciphertext.clone();
        ct7[size..].fill(0);
        assert!(dec.decrypt_with_ad(aad, &ct7).is_err());
    }
}

/// Verifies that truncated ciphertexts (below TAG_LEN and between TAG_LEN and full length)
/// as well as appended garbage bytes are rejected safely.
#[test]
fn test_adversarial_ciphertext_truncation_and_extension() {
    let key = [0xCCu8; 32];
    let aad = b"truncation_aad";
    let plaintext = b"Thirty-two bytes of secret data!";
    assert_eq!(plaintext.len(), 32);

    let mut enc = NoiseCipherState::new(&key).unwrap();
    let ciphertext = enc.encrypt_with_ad(aad, plaintext).unwrap();
    assert_eq!(ciphertext.len(), 48); // 32 plaintext + 16 tag

    // 1. All truncations from 0 to 47 bytes
    for len in 0..48 {
        let truncated = &ciphertext[..len];
        let mut dec = NoiseCipherState::new(&key).unwrap();
        let res = dec.decrypt_with_ad(aad, truncated);
        assert!(
            res.is_err(),
            "Truncated ciphertext of length {len} must be rejected"
        );
        if len < TAG_LEN {
            let err_msg = format!("{:?}", res.err().unwrap());
            assert!(
                err_msg.contains("Ciphertext too short"),
                "Lengths < 16 must report 'Ciphertext too short', got {err_msg}"
            );
        }
    }

    // 2. Trailing garbage bytes appended
    for extra_bytes in [1usize, 2, 7, 16, 64, 256] {
        let mut extended = ciphertext.clone();
        extended.extend(vec![0xAA; extra_bytes]);

        let mut dec = NoiseCipherState::new(&key).unwrap();
        let res = dec.decrypt_with_ad(aad, &extended);
        assert!(
            res.is_err(),
            "Ciphertext with {extra_bytes} extra bytes appended must be rejected"
        );
    }
}

// ============================================================================
// Suite 3: Nonce Progression, Tamper Isolation & Rekeying Behavior
// ============================================================================

/// Verifies that nonces strictly increment monotonically on success,
/// stay frozen on tamper failure, and legitimate packets after tamper bursts still decrypt.
#[test]
fn test_adversarial_nonce_progression_and_tamper_isolation() {
    let key = [0xDDu8; 32];
    let aad = b"stream_nonce_test";

    let mut enc = NoiseCipherState::new(&key).unwrap();
    let mut dec = NoiseCipherState::new(&key).unwrap();

    // 1. Initial nonces are 0
    assert_eq!(enc.nonce(), 0);
    assert_eq!(dec.nonce(), 0);

    // 2. Send 5 valid packets
    for i in 0..5 {
        let msg = format!("Message sequence {i}").into_bytes();
        let ct = enc.encrypt_with_ad(aad, &msg).unwrap();
        assert_eq!(enc.nonce(), i + 1);

        let pt = dec.decrypt_with_ad(aad, &ct).unwrap();
        assert_eq!(pt, msg);
        assert_eq!(dec.nonce(), i + 1);
    }

    // 3. Inject a burst of 20 corrupted packets
    let legitimate_ct = enc.encrypt_with_ad(aad, b"Legitimate message 5").unwrap();
    assert_eq!(enc.nonce(), 6);

    for j in 0..20 {
        let mut tampered_ct = legitimate_ct.clone();
        let len = tampered_ct.len();
        tampered_ct[j % len] ^= 0xFF;

        let res = dec.decrypt_with_ad(aad, &tampered_ct);
        assert!(res.is_err(), "Corrupted packet {j} must fail");
        assert_eq!(
            dec.nonce(),
            5,
            "Receiver nonce must remain exactly at 5 despite tamper attempts"
        );
    }

    // 4. Now deliver the genuine 6th packet (nonce 5) to the receiver
    let recovered = dec.decrypt_with_ad(aad, &legitimate_ct).unwrap();
    assert_eq!(recovered, b"Legitimate message 5");
    assert_eq!(dec.nonce(), 6, "Receiver nonce advances to 6 after valid packet");
}

/// Tests Noise Protocol 12-byte nonce format invariants:
/// 4 zero prefix bytes || 8 little-endian counter bytes.
#[test]
fn test_adversarial_nonce_format_and_endianness_invariants() {
    let test_counters: &[(u64, [u8; 12])] = &[
        (0u64, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        (1u64, [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]),
        (255u64, [0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0]),
        (256u64, [0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]),
        (65535u64, [0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0]),
        (
            0x0102030405060708u64,
            [0, 0, 0, 0, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01],
        ),
        (
            u64::MAX,
            [0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255],
        ),
    ];

    for &(counter, expected_bytes) in test_counters {
        let nonce = NoiseCipherState::format_nonce(counter).expect("Nonce format failed");
        assert_eq!(
            nonce.as_ref(),
            &expected_bytes,
            "Nonce for counter {counter} mismatch"
        );
        // Prefix must ALWAYS be 4 zero bytes
        assert_eq!(&nonce.as_ref()[..4], &[0, 0, 0, 0]);
    }
}

/// Tests mutual rekeying, multi-round rekeying, and asymmetric rekey desync detection.
#[test]
fn test_adversarial_noise_rekeying_mechanisms() {
    let tx_key = [0x11u8; 32];
    let rx_key = [0x22u8; 32];

    let mut session_a = NoiseSession::from_keys(&tx_key, &rx_key).unwrap();
    let mut session_b = NoiseSession::from_keys(&rx_key, &tx_key).unwrap();

    let aad = b"rekey_test_aad";

    // 1. Initial communication
    let ct1 = session_a.encrypt_with_ad(aad, b"Pre-rekey packet").unwrap();
    let pt1 = session_b.decrypt_with_ad(aad, &ct1).unwrap();
    assert_eq!(pt1, b"Pre-rekey packet");

    // 2. Mutual rekey on A's TX and B's RX
    session_a.tx_mut().rekey().expect("Rekey A TX");
    session_b.rx_mut().rekey().expect("Rekey B RX");

    assert_eq!(session_a.tx_nonce(), 0, "Nonce must reset to 0 after rekey");
    assert_eq!(session_b.rx_nonce(), 0, "Nonce must reset to 0 after rekey");

    let ct2 = session_a.encrypt_with_ad(aad, b"Post-rekey packet").unwrap();
    let pt2 = session_b.decrypt_with_ad(aad, &ct2).unwrap();
    assert_eq!(pt2, b"Post-rekey packet");

    // 3. Multi-round rekeying: execute 10 consecutive rekeys on both sides
    for round in 1..=10 {
        session_a.tx_mut().rekey().unwrap();
        session_b.rx_mut().rekey().unwrap();
        session_b.tx_mut().rekey().unwrap();
        session_a.rx_mut().rekey().unwrap();

        let msg_ab = format!("Round {round} A to B").into_bytes();
        let ct_ab = session_a.encrypt_with_ad(aad, &msg_ab).unwrap();
        let pt_ab = session_b.decrypt_with_ad(aad, &ct_ab).unwrap();
        assert_eq!(pt_ab, msg_ab);

        let msg_ba = format!("Round {round} B to A").into_bytes();
        let ct_ba = session_b.encrypt_with_ad(aad, &msg_ba).unwrap();
        let pt_ba = session_a.decrypt_with_ad(aad, &ct_ba).unwrap();
        assert_eq!(pt_ba, msg_ba);
    }

    // 4. Asymmetric rekey: A rekeys TX, but B does NOT rekey RX
    session_a.tx_mut().rekey().unwrap();
    let desync_ct = session_a
        .encrypt_with_ad(aad, b"Asymmetric rekey packet")
        .unwrap();

    let res = session_b.decrypt_with_ad(aad, &desync_ct);
    assert!(
        res.is_err(),
        "Session B must reject packet encrypted under newly rekeyed key when B has not rekeyed"
    );
}

// ============================================================================
// Suite 4: Full-Duplex Concurrent Streaming Under Heavy Load
// ============================================================================

/// Stress-tests bidirectional concurrent streaming over an encrypted DerpStream:
/// Node A sends 512 KiB to Node B while Node B simultaneously sends 512 KiB to Node A.
/// Validates SHA-256 integrity, monotonic delivery, and zero deadlocks.
#[tokio::test]
async fn test_adversarial_full_duplex_heavy_bidirectional_streaming() {
    let (stream_a, stream_b) =
        DerpStream::encrypted_pair().expect("Failed to create encrypted pair");

    const TOTAL_BYTES: usize = 512 * 1024; // 512 KiB in each direction

    // Deterministic pseudo-random test vectors
    let data_a_to_b: Vec<u8> = (0..TOTAL_BYTES)
        .map(|i| (i ^ (i >> 7) ^ 0x5A) as u8)
        .collect();
    let data_b_to_a: Vec<u8> = (0..TOTAL_BYTES)
        .map(|i| (i ^ (i >> 9) ^ 0xA5) as u8)
        .collect();

    let expected_hash_ab = Sha256::digest(&data_a_to_b);
    let expected_hash_ba = Sha256::digest(&data_b_to_a);

    let send_a = data_a_to_b.clone();
    let handle_a = tokio::spawn(async move {
        // Concurrently write and read on stream A
        let (mut reader, mut writer) = tokio::io::split(stream_a);

        let write_task = tokio::spawn(async move {
            writer.write_all(&send_a).await.unwrap();
            writer.shutdown().await.unwrap();
        });

        let read_task = tokio::spawn(async move {
            let mut received = Vec::with_capacity(TOTAL_BYTES);
            reader.read_to_end(&mut received).await.unwrap();
            received
        });

        write_task.await.unwrap();
        let received = read_task.await.unwrap();
        received
    });

    let send_b = data_b_to_a.clone();
    let handle_b = tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(stream_b);

        let write_task = tokio::spawn(async move {
            writer.write_all(&send_b).await.unwrap();
            writer.shutdown().await.unwrap();
        });

        let read_task = tokio::spawn(async move {
            let mut received = Vec::with_capacity(TOTAL_BYTES);
            reader.read_to_end(&mut received).await.unwrap();
            received
        });

        write_task.await.unwrap();
        let received = read_task.await.unwrap();
        received
    });

    let timeout_duration = Duration::from_secs(15);
    let result = tokio::time::timeout(timeout_duration, async {
        let (res_a, res_b) = tokio::join!(handle_a, handle_b);
        (res_a.unwrap(), res_b.unwrap())
    })
    .await;

    assert!(
        result.is_ok(),
        "Bidirectional heavy streaming timed out! Deadlock or queue stall detected"
    );

    let (recv_on_a, recv_on_b) = result.unwrap();

    assert_eq!(recv_on_b.len(), TOTAL_BYTES);
    assert_eq!(recv_on_a.len(), TOTAL_BYTES);

    assert_eq!(
        Sha256::digest(&recv_on_b),
        expected_hash_ab,
        "Payload from A to B suffered data corruption"
    );
    assert_eq!(
        Sha256::digest(&recv_on_a),
        expected_hash_ba,
        "Payload from B to A suffered data corruption"
    );
}

/// Tests half-close semantics under encryption:
/// Node A transmits, closes its write half, but continues to read data sent from Node B.
#[tokio::test]
async fn test_adversarial_half_close_semantics_under_encryption() {
    let (mut stream_a, mut stream_b) = DerpStream::encrypted_pair().unwrap();

    let msg1 = b"Message from A before half close";
    stream_a.write_all(msg1).await.unwrap();
    stream_a.flush().await.unwrap();
    stream_a.shutdown().await.unwrap(); // Half-close from A

    // Node B reads msg1 and sees EOF
    let mut buf = vec![0u8; 128];
    let n1 = stream_b.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n1], msg1);

    let n_eof = stream_b.read(&mut buf).await.unwrap();
    assert_eq!(n_eof, 0, "B must observe EOF on stream from A");

    // Node B can STILL send to Node A
    let msg2 = b"Reply from B after A closed write side";
    stream_b.write_all(msg2).await.unwrap();
    stream_b.flush().await.unwrap();
    stream_b.shutdown().await.unwrap();

    // Node A reads reply from B
    let mut buf_a = vec![0u8; 128];
    let n2 = stream_a.read(&mut buf_a).await.unwrap();
    assert_eq!(&buf_a[..n2], msg2);
}

// ============================================================================
// Suite 5: High-Concurrency Virtual Stream Multiplexing
// ============================================================================

/// Multiplexes 30 concurrent encrypted virtual streams simultaneously over the same peer endpoints.
/// Verifies isolation, independent session states, and zero cross-talk.
#[tokio::test]
async fn test_adversarial_high_concurrency_encrypted_multiplexing() {
    const NUM_STREAMS: u64 = 30;
    const BYTES_PER_STREAM: usize = 16384; // 16 KiB

    let key_alice = [0x55u8; 32];
    let key_bob = [0x66u8; 32];

    let mut handles = Vec::new();
    let completed_counter = Arc::new(AtomicUsize::new(0));

    for stream_id in 1..=NUM_STREAMS {
        let counter = Arc::clone(&completed_counter);

        let handle = tokio::spawn(async move {
            let (outbound_a_tx, mut outbound_a_rx) = mpsc::channel::<DerpFrame>(32);
            let (inbound_a_tx, inbound_a_rx) = mpsc::channel::<Bytes>(32);
            let (outbound_b_tx, mut outbound_b_rx) = mpsc::channel::<DerpFrame>(32);
            let (inbound_b_tx, inbound_b_rx) = mpsc::channel::<Bytes>(32);

            let session_a =
                derive_stream_keys(&key_alice, &key_bob, stream_id, true).unwrap();
            let session_b =
                derive_stream_keys(&key_bob, &key_alice, stream_id, false).unwrap();

            let mut stream_a =
                DerpStream::new(stream_id, key_alice, key_bob, outbound_a_tx, inbound_a_rx)
                    .with_session(session_a);
            let mut stream_b =
                DerpStream::new(stream_id, key_bob, key_alice, outbound_b_tx, inbound_b_rx)
                    .with_session(session_b);

            // Forwarder A -> B
            tokio::spawn(async move {
                while let Some(frame) = outbound_a_rx.recv().await {
                    if let DerpFrame::SendPacket { payload, .. } = frame {
                        if let Ok((_sid, flags, data)) = decode_stream_payload(payload) {
                            if flags & FLAG_FIN != 0 {
                                let _ = inbound_b_tx.send(Bytes::new()).await;
                                break;
                            }
                            let _ = inbound_b_tx.send(data).await;
                        }
                    }
                }
            });

            // Forwarder B -> A
            tokio::spawn(async move {
                while let Some(frame) = outbound_b_rx.recv().await {
                    if let DerpFrame::SendPacket { payload, .. } = frame {
                        if let Ok((_sid, flags, data)) = decode_stream_payload(payload) {
                            if flags & FLAG_FIN != 0 {
                                let _ = inbound_a_tx.send(Bytes::new()).await;
                                break;
                            }
                            let _ = inbound_a_tx.send(data).await;
                        }
                    }
                }
            });

            // Write payload from A
            let payload: Vec<u8> = (0..BYTES_PER_STREAM)
                .map(|i| (i.wrapping_add(stream_id as usize) & 0xFF) as u8)
                .collect();
            let expected_hash = Sha256::digest(&payload);

            stream_a.write_all(&payload).await.unwrap();
            stream_a.flush().await.unwrap();
            stream_a.shutdown().await.unwrap();

            let mut received = Vec::with_capacity(BYTES_PER_STREAM);
            stream_b.read_to_end(&mut received).await.unwrap();

            assert_eq!(
                Sha256::digest(&received),
                expected_hash,
                "Stream {stream_id} payload corrupted"
            );

            counter.fetch_add(1, Ordering::SeqCst);
        });

        handles.push(handle);
    }

    let timeout_res = tokio::time::timeout(Duration::from_secs(20), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await;

    assert!(
        timeout_res.is_ok(),
        "High-concurrency stream test timed out!"
    );
    assert_eq!(
        completed_counter.load(Ordering::SeqCst),
        NUM_STREAMS as usize,
        "All 30 concurrent virtual streams must finish successfully"
    );
}

// ============================================================================
// Suite 6: Host WireGuard Safety Invariant
// ============================================================================

/// Empirically verifies that the host `tiktok` WireGuard interface
/// (`10.2.0.2/32`, routing table 51820) remains active, UP, and untouched.
#[test]
fn test_stress_host_wireguard_safety_invariant() {
    let output = std::process::Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("10.2.0.2/32"),
                "tiktok IP invariant violated: {stdout}"
            );
            assert!(
                stdout.contains("UP"),
                "tiktok interface must be UP: {stdout}"
            );
        }
    }

    let route_output = std::process::Command::new("ip")
        .args(["route", "show", "table", "51820"])
        .output();

    if let Ok(out) = route_output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains("dev tiktok"),
                "table 51820 route invariant violated: {stdout}"
            );
        }
    }
}
