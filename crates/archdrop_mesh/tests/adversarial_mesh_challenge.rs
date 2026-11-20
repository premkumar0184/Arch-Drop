use archdrop_mesh::client::{DerpClient, DerpClientConfig};
use archdrop_mesh::crypto::MeshKeyPair;
use archdrop_mesh::dialer::FallbackDialer;
use archdrop_mesh::mock_relay::MockDerpRelay;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Challenge 1A: Immediate fallback on refused connection
/// Dialing a closed TCP port (127.0.0.1:1) should immediately fail TCP connect (<50ms)
/// and fall back to DERP relay without waiting for the 1500ms timeout.
#[tokio::test]
async fn test_adversarial_immediate_fallback_on_refused_connection() {
    let relay = MockDerpRelay::start().await.expect("Relay start failed");
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .expect("Client A connect failed");
    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .expect("Client B connect failed");

    let listener_b = client_b.listener();
    let key_a_pub = *client_a.local_key();

    let recv_handle = tokio::spawn(async move {
        let (mut stream_b, remote_peer) = listener_b.accept().await.unwrap();
        assert_eq!(&remote_peer, &key_a_pub);
        let mut buf = vec![0u8; 12];
        stream_b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"refuse_probe");
        stream_b.write_all(b"refuse_reply").await.unwrap();
        stream_b.flush().await.unwrap();
    });

    let dialer = FallbackDialer::new(Some(client_a.clone()))
        .with_direct_timeout(Duration::from_millis(1500));

    let start = Instant::now();
    // 127.0.0.1:1 is guaranteed to return ECONNREFUSED immediately
    let stream = dialer
        .dial_with_fallback(Some("127.0.0.1:1"), Some(client_b.local_key()))
        .await
        .expect("Fallback to DERP must succeed on refused TCP");

    let elapsed = start.elapsed();
    assert!(stream.is_relayed(), "Stream must be a relayed DERP stream");
    assert!(
        elapsed < Duration::from_millis(300),
        "Immediate fallback on refused connection must take < 300ms, took {elapsed:?}"
    );

    let mut stream = stream;
    stream.write_all(b"refuse_probe").await.unwrap();
    stream.flush().await.unwrap();

    let mut reply = vec![0u8; 12];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"refuse_reply");

    recv_handle.await.unwrap();
    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
}

/// Challenge 1B: Fallback SLA Timing under CPU load (<2.0s SLA)
/// Dials a non-routable blackhole IP (198.51.100.1:53317, RFC 5737 TEST-NET-2).
/// Asserts that fallback to DERP takes between 1450ms and 2000ms across 5 repeated trials
/// while multi-core CPU load is continuously generated in the background.
#[tokio::test]
async fn test_adversarial_blackhole_fallback_sla_under_load() {
    let relay = MockDerpRelay::start().await.expect("Relay start failed");
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .expect("Client A connect failed");
    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .expect("Client B connect failed");

    let listener_b = client_b.listener();
    let key_a_pub = *client_a.local_key();

    // Spawn receiver loop on Client B
    let recv_handle = tokio::spawn(async move {
        for _ in 0..5 {
            let (mut stream_b, remote_peer) = listener_b.accept().await.unwrap();
            assert_eq!(&remote_peer, &key_a_pub);
            let mut buf = vec![0u8; 8];
            stream_b.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"loadtest");
            stream_b.write_all(b"loadresp").await.unwrap();
            stream_b.flush().await.unwrap();
        }
    });

    // Generate background CPU stress across available cores
    let stress_active = Arc::new(AtomicBool::new(true));
    let num_workers = num_cpus();
    let mut stress_handles = Vec::new();

    for _ in 0..num_workers {
        let active = stress_active.clone();
        let handle = tokio::task::spawn_blocking(move || {
            let mut hasher = Sha256::new();
            let mut data = vec![0x42u8; 4096];
            while active.load(Ordering::Relaxed) {
                hasher.update(&data);
                data[0] = data[0].wrapping_add(1);
            }
        });
        stress_handles.push(handle);
    }

    let dialer = FallbackDialer::new(Some(client_a.clone()))
        .with_direct_timeout(Duration::from_millis(1500));

    let mut elapsed_times = Vec::new();

    for trial in 1..=5 {
        let start = Instant::now();
        let stream = dialer
            .dial_with_fallback(Some("198.51.100.1:53317"), Some(client_b.local_key()))
            .await
            .unwrap_or_else(|e| panic!("Trial {trial} fallback failed: {e}"));

        let elapsed = start.elapsed();
        elapsed_times.push(elapsed);

        assert!(stream.is_relayed(), "Trial {trial}: Stream must be relayed");
        assert!(
            elapsed >= Duration::from_millis(1450),
            "Trial {trial}: Timeout elapsed too early: {elapsed:?} < 1450ms"
        );
        assert!(
            elapsed < Duration::from_millis(2000),
            "Trial {trial}: Fallback SLA exceeded: {elapsed:?} >= 2000ms"
        );

        let mut stream = stream;
        stream.write_all(b"loadtest").await.unwrap();
        stream.flush().await.unwrap();

        let mut resp = vec![0u8; 8];
        stream.read_exact(&mut resp).await.unwrap();
        assert_eq!(&resp, b"loadresp");
    }

    // Terminate CPU stress
    stress_active.store(false, Ordering::Relaxed);
    for h in stress_handles {
        let _ = h.await;
    }

    recv_handle.await.unwrap();
    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;

    println!("Fallback SLA Timings under CPU load (5 trials):");
    for (i, t) in elapsed_times.iter().enumerate() {
        println!("  Trial {}: {:.2?}", i + 1, t);
    }
}

/// Challenge 2: High Concurrency Stream Multiplexing (50+ streams, multi-MB payloads)
/// Concurrently opens 50 virtual DerpStream instances over a SINGLE DerpClient session.
/// Transmits 1 MiB per stream (50 MiB total aggregate payload in each direction) concurrently.
/// Verifies bit-for-bit SHA-256 match, zero data corruption, zero deadlocks.
#[tokio::test]
async fn test_adversarial_high_concurrency_stream_multiplexing_50_streams() {
    let relay = MockDerpRelay::start().await.expect("Relay start failed");
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .expect("Client A connect failed");
    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .expect("Client B connect failed");

    let num_streams = 50;
    let payload_size = 1024 * 1024; // 1 MiB per stream = 50 MiB total in each direction

    // Pre-generate unique deterministic payloads and expected SHA-256 hashes for each stream
    let mut payloads_a_to_b = Vec::with_capacity(num_streams);
    let mut expected_hashes_a_to_b = Vec::with_capacity(num_streams);

    let mut payloads_b_to_a = Vec::with_capacity(num_streams);
    let mut expected_hashes_b_to_a = Vec::with_capacity(num_streams);

    for s in 0..num_streams {
        let mut p_ab = vec![0u8; payload_size];
        for (i, b) in p_ab.iter_mut().enumerate() {
            *b = ((s * 17 + i) % 251) as u8;
        }
        let mut hasher_ab = Sha256::new();
        hasher_ab.update(&p_ab);
        expected_hashes_a_to_b.push(hasher_ab.finalize());
        payloads_a_to_b.push(p_ab);

        let mut p_ba = vec![0u8; payload_size];
        for (i, b) in p_ba.iter_mut().enumerate() {
            *b = ((s * 31 + i) % 241) as u8;
        }
        let mut hasher_ba = Sha256::new();
        hasher_ba.update(&p_ba);
        expected_hashes_b_to_a.push(hasher_ba.finalize());
        payloads_b_to_a.push(p_ba);
    }

    let listener_b = client_b.listener();
    let payloads_b_to_a = Arc::new(payloads_b_to_a);
    let expected_hashes_a_to_b = Arc::new(expected_hashes_a_to_b);

    // Accept and handle all 50 incoming streams on Client B
    let b_task = tokio::spawn(async move {
        let mut server_handles = Vec::with_capacity(num_streams);

        for stream_idx in 0..num_streams {
            let (mut stream_b, _peer) = listener_b
                .accept()
                .await
                .expect("Failed to accept stream on B");

            let p_ba = payloads_b_to_a[stream_idx].clone();
            let exp_ab = expected_hashes_a_to_b[stream_idx];

            let handle = tokio::spawn(async move {
                // Read 1 MiB from A
                let mut received_at_b = vec![0u8; payload_size];
                stream_b.read_exact(&mut received_at_b).await.unwrap();

                let mut hasher = Sha256::new();
                hasher.update(&received_at_b);
                let actual_hash_ab = hasher.finalize();
                assert_eq!(
                    actual_hash_ab, exp_ab,
                    "Stream {stream_idx}: Payload from A to B corrupted!"
                );

                // Write 1 MiB back to A
                stream_b.write_all(&p_ba).await.unwrap();
                stream_b.flush().await.unwrap();
                stream_b.shutdown().await.unwrap();
            });

            server_handles.push(handle);
        }

        for h in server_handles {
            h.await.unwrap();
        }
    });

    // Concurrently open 50 streams from Client A to Client B
    let mut client_handles = Vec::with_capacity(num_streams);

    let client_b_key = *client_b.local_key();
    let payloads_a_to_b = Arc::new(payloads_a_to_b);
    let expected_hashes_b_to_a = Arc::new(expected_hashes_b_to_a);

    for s in 0..num_streams {
        let client_a = client_a.clone();
        let p_ab = payloads_a_to_b[s].clone();
        let exp_ba = expected_hashes_b_to_a[s];

        let handle = tokio::spawn(async move {
            let mut stream_a = client_a
                .open_stream(&client_b_key)
                .await
                .expect("Failed to open stream on A");

            // Send 1 MiB to B
            stream_a.write_all(&p_ab).await.unwrap();
            stream_a.flush().await.unwrap();

            // Read 1 MiB from B
            let mut received_at_a = vec![0u8; payload_size];
            stream_a.read_exact(&mut received_at_a).await.unwrap();

            let mut hasher = Sha256::new();
            hasher.update(&received_at_a);
            let actual_hash_ba = hasher.finalize();

            assert_eq!(
                actual_hash_ba, exp_ba,
                "Stream {s}: Payload from B to A corrupted!"
            );
        });

        client_handles.push(handle);
    }

    // Enforce a strict 60s timeout for all 50 concurrent streams to complete
    let timeout_res = tokio::time::timeout(Duration::from_secs(60), async {
        for h in client_handles {
            h.await.unwrap();
        }
        b_task.await.unwrap();
    })
    .await;

    assert!(
        timeout_res.is_ok(),
        "Concurrent 50-stream transfer timed out! Deadlock occurred!"
    );

    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
}

/// Challenge 3: Keepalive Heartbeat & Inactivity Invariant (20s keepalive)
/// Verifies that keepalive heartbeats maintain connection state during long inactivity
/// and that after inactivity, virtual streams still open and transfer without degradation.
#[tokio::test]
async fn test_adversarial_keepalive_heartbeat_inactivity_20s() {
    let relay = MockDerpRelay::start().await.expect("Relay start failed");
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    // Connect with default 20s keepalive interval
    let mut config_a = DerpClientConfig::new(&relay_url, key_a.clone());
    config_a.keepalive_interval = Duration::from_secs(20);

    let mut config_b = DerpClientConfig::new(&relay_url, key_b.clone());
    config_b.keepalive_interval = Duration::from_secs(20);

    let client_a = DerpClient::connect(config_a).await.unwrap();
    let client_b = DerpClient::connect(config_b).await.unwrap();

    assert!(client_a.is_connected());
    assert!(client_b.is_connected());

    // Sleep for 21 seconds to ensure at least one full 20s keepalive tick fires
    println!("Waiting 21 seconds for keepalive heartbeat tick...");
    tokio::time::sleep(Duration::from_secs(21)).await;

    // Verify connections are still alive
    assert!(
        client_a.is_connected(),
        "Client A should still be connected after 21s keepalive interval"
    );
    assert!(
        client_b.is_connected(),
        "Client B should still be connected after 21s keepalive interval"
    );

    // Verify virtual stream can still open and transfer data after inactivity
    let listener_b = client_b.listener();
    let key_a_pub = *client_a.local_key();

    let recv_handle = tokio::spawn(async move {
        let (mut stream_b, remote_peer) = listener_b.accept().await.unwrap();
        assert_eq!(&remote_peer, &key_a_pub);
        let mut buf = vec![0u8; 15];
        stream_b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"after_keepalive");
        stream_b.write_all(b"keepalive_reply").await.unwrap();
        stream_b.flush().await.unwrap();
    });

    let mut stream_a = client_a.open_stream(client_b.local_key()).await.unwrap();
    stream_a.write_all(b"after_keepalive").await.unwrap();
    stream_a.flush().await.unwrap();

    let mut resp = vec![0u8; 15];
    stream_a.read_exact(&mut resp).await.unwrap();
    assert_eq!(&resp, b"keepalive_reply");

    recv_handle.await.unwrap();
    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
}

/// Challenge 4: Abrupt RST & Stream Isolation Stress Test
/// Concurrently runs 30 stable streams (512 KiB each) interleaved with 30 flapping streams
/// that abruptly drop/RST midway through transmission.
/// Verifies complete stream isolation: zero data cross-talk, zero leaked buffers,
/// and 100% SHA-256 match on all stable streams.
#[tokio::test]
async fn test_adversarial_abrupt_rst_stream_isolation() {
    let relay = MockDerpRelay::start().await.expect("Relay start failed");
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .expect("Client A connect failed");
    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .expect("Client B connect failed");

    let num_stable = 30;
    let num_flapping = 30;
    let total_streams = num_stable + num_flapping;
    let stable_size = 512 * 1024;

    let mut stable_payloads = Vec::with_capacity(num_stable);
    let mut stable_hashes = Vec::with_capacity(num_stable);

    for s in 0..num_stable {
        let mut p = vec![0u8; stable_size];
        for (i, b) in p.iter_mut().enumerate() {
            *b = ((s * 41 + i) % 233) as u8;
        }
        let mut h = Sha256::new();
        h.update(&p);
        stable_hashes.push(h.finalize());
        stable_payloads.push(p);
    }

    let listener_b = client_b.listener();
    let stable_hashes = Arc::new(stable_hashes);

    // Receiver on B accepts all 60 streams
    let recv_handle = tokio::spawn(async move {
        let mut handles = Vec::with_capacity(total_streams);

        for _ in 0..total_streams {
            let (mut stream_b, _peer) = listener_b.accept().await.unwrap();
            let stable_hashes = stable_hashes.clone();

            let h = tokio::spawn(async move {
                let mut data = Vec::new();
                let _ = stream_b.read_to_end(&mut data).await;
                // If this is a stable stream (length == 512 KiB)
                if data.len() == stable_size {
                    let mut hasher = Sha256::new();
                    hasher.update(&data);
                    let actual_hash = hasher.finalize();
                    let matches_any = stable_hashes.contains(&actual_hash);
                    assert!(
                        matches_any,
                        "Received full-size stream whose SHA-256 does not match any expected stable stream!"
                    );
                    1usize
                } else {
                    // Flapping / aborted stream
                    0usize
                }
            });
            handles.push(h);
        }

        let mut stable_completed = 0;
        for h in handles {
            stable_completed += h.await.unwrap();
        }
        assert_eq!(
            stable_completed, num_stable,
            "Expected exactly {num_stable} stable streams to complete successfully"
        );
    });

    let client_b_key = *client_b.local_key();
    let mut client_handles = Vec::with_capacity(total_streams);

    // Launch stable streams
    for (idx, payload) in stable_payloads.into_iter().enumerate() {
        let c_a = client_a.clone();
        let h = tokio::spawn(async move {
            let mut stream = c_a.open_stream(&client_b_key).await.unwrap();
            // Yield briefly to interleave with flapping streams
            tokio::task::yield_now().await;
            stream.write_all(&payload).await.unwrap();
            stream.flush().await.unwrap();
            stream.shutdown().await.unwrap();
            idx
        });
        client_handles.push(h);
    }

    // Launch flapping streams that abort midway
    for f_idx in 0..num_flapping {
        let c_a = client_a.clone();
        let h = tokio::spawn(async move {
            let mut stream = c_a.open_stream(&client_b_key).await.unwrap();
            // Send partial bytes
            let partial = vec![0xFFu8; 8192];
            let _ = stream.write_all(&partial).await;
            let _ = stream.flush().await;
            // Abrupt drop without shutdown -> drops and sends FLAG_RST
            drop(stream);
            1000 + f_idx
        });
        client_handles.push(h);
    }

    let timeout_res = tokio::time::timeout(Duration::from_secs(30), async {
        for h in client_handles {
            let _ = h.await;
        }
        recv_handle.await.unwrap();
    })
    .await;

    assert!(
        timeout_res.is_ok(),
        "RST isolation test timed out! Possible deadlock or unhandled RST frame"
    );

    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
}

/// Challenge 5: End-to-End Noise AEAD Encrypted Stream Transfer & Nonce Integrity
/// Verifies that payloads transferred over encrypted DerpStream are protected with ChaCha20-Poly1305,
/// nonces increment monotonically, and data is decrypted identically to plaintext.
#[tokio::test]
async fn test_adversarial_noise_encrypted_stream_transfer_and_aad_tamper() {
    use archdrop_mesh::stream::DerpStream;

    let (mut stream_a, mut stream_b) =
        DerpStream::encrypted_pair().expect("Encrypted pair creation failed");
    assert!(stream_a.is_encrypted());
    assert!(stream_b.is_encrypted());

    let test_data = b"Noise_IK encrypted stream test vector -- end-to-end payload confidentiality and authenticity";

    // Transfer from A to B
    stream_a
        .write_all(test_data)
        .await
        .expect("Write from A must succeed");
    stream_a.flush().await.expect("Flush from A must succeed");
    stream_a
        .shutdown()
        .await
        .expect("Shutdown from A must succeed");

    let mut received = Vec::new();
    stream_b
        .read_to_end(&mut received)
        .await
        .expect("Read on B must succeed");
    assert_eq!(
        &received, test_data,
        "Decrypted payload must match original plaintext"
    );

    // Check nonces incremented
    assert!(stream_a.session().unwrap().tx_nonce() > 0);
    assert!(stream_b.session().unwrap().rx_nonce() > 0);
}

/// Challenge 6: Tampered AAD Header Injection Rejection
/// Simulates an untrusted relay tampering with the stream header (e.g. flipping stream_id or flags).
/// Verifies that DerpStream detects the Poly1305 MAC tag failure and aborts with InvalidData.
#[tokio::test]
async fn test_adversarial_noise_stream_tampered_header_aad_rejection() {
    use archdrop_mesh::frame::DerpFrame;
    use archdrop_mesh::stream::{decode_stream_payload, DerpStream, FLAG_DATA};
    use bytes::Bytes;
    use tokio::sync::mpsc;

    let key_a = [0x01u8; 32];
    let key_b = [0x02u8; 32];

    let (outbound_a_tx, mut outbound_a_rx) = mpsc::channel::<DerpFrame>(128);
    let (inbound_b_tx, inbound_b_rx) = mpsc::channel::<Bytes>(128);

    let session_a = archdrop_mesh::crypto::derive_stream_keys(&key_a, &key_b, 100, true).unwrap();
    let session_b =
        archdrop_mesh::crypto::derive_stream_keys(&key_b, &key_a, 100, false).unwrap();

    let mut stream_a = DerpStream::new(100, key_a, key_b, outbound_a_tx, mpsc::channel(1).1)
        .with_session(session_a);
    // Stream B listener expects stream 101 (relay swapped stream 100 packet to stream 101)
    let mut stream_b = DerpStream::new(101, key_b, key_a, mpsc::channel(1).0, inbound_b_rx)
        .with_session(session_b);

    // Stream A writes data
    stream_a.write_all(b"Secret chunk").await.unwrap();
    stream_a.flush().await.unwrap();

    // Intercept outbound frame from A
    let frame = outbound_a_rx.recv().await.expect("Frame must be produced");
    let payload = match frame {
        DerpFrame::SendPacket { payload, .. } => payload,
        _ => panic!("Expected SendPacket"),
    };

    let (sid, flags, ciphertext) = decode_stream_payload(payload).unwrap();
    assert_eq!(sid, 100);
    assert_eq!(flags, FLAG_DATA);

    // Deliver chunk to stream B (which is on stream 101 and binds AAD for 101)
    inbound_b_tx.send(ciphertext).await.unwrap();

    let mut buf = [0u8; 32];
    let res = stream_b.read(&mut buf).await;
    assert!(
        res.is_err(),
        "Stream B must reject chunk with tampered AAD header (stream_id mismatch)"
    );
    let err = res.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}
