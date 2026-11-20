use archdrop_audio::capture::MockAudioCapture;
use archdrop_audio::framing::{AudioCodec, AudioWireFrame, ADAU_MAGIC, HEADER_SIZE};
use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_transfer::TransferServer;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn setup_adversarial_server(
    paired_fingerprints: Vec<String>,
    attach_mock: bool,
    max_frames: Option<usize>,
) -> (u16, tokio::sync::watch::Sender<bool>, Arc<TransferServer>) {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = Config::default();
    config.security.paired_fingerprints = paired_fingerprints.into_iter().collect();
    let config = Arc::new(config);
    let identity = Arc::new(CryptoIdentity::generate().unwrap());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut server = TransferServer::new(
        config,
        identity,
        download_dir,
        port,
        false, // plain HTTP
    )
    .unwrap();

    if attach_mock {
        let mock = if let Some(max) = max_frames {
            MockAudioCapture::new().with_max_frames(max).with_pacing(false)
        } else {
            MockAudioCapture::new().with_pacing(true)
        };
        server.set_mock_audio_capture(mock);
        server.set_mock_virtual_sink();
    }

    let server_arc = Arc::new(server);
    let server_runner = server_arc.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = server_runner.run(shutdown_rx).await;
    });

    // Wait for server to bind
    tokio::time::sleep(Duration::from_millis(80)).await;

    (port, shutdown_tx, server_arc)
}

#[tokio::test]
async fn test_adversarial_burst_concurrency_race_condition_409() {
    let (port, shutdown_tx, server) = setup_adversarial_server(vec![], true, None).await;

    let concurrency = 20;
    let mut handles = Vec::new();

    for i in 0..concurrency {
        let port = port;
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
            let req = format!(
                "GET /api/v2/audio/stream HTTP/1.1\r\n\
                 Host: 127.0.0.1:{port}\r\n\
                 Connection: close\r\n\
                 \r\n"
            );
            stream.write_all(req.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();

            let mut buf = vec![0u8; 512];
            let n = stream.read(&mut buf).await.unwrap();
            let res = String::from_utf8_lossy(&buf[..n]).to_string();

            // Keep connection alive for winners for a moment to enforce race barrier
            if res.contains("200 OK") {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            (i, res, stream)
        });
        handles.push(handle);
    }

    let mut ok_count = 0;
    let mut conflict_count = 0;
    let mut winning_stream = None;

    for handle in handles {
        let (id, res, stream) = handle.await.unwrap();
        if res.contains("200 OK") {
            ok_count += 1;
            winning_stream = Some((id, stream));
        } else if res.contains("409 Conflict") {
            conflict_count += 1;
        } else {
            panic!("Unexpected response for client {id}: {res}");
        }
    }

    assert_eq!(
        ok_count, 1,
        "CRITICAL MUTUAL EXCLUSION VIOLATION: Exactly 1 client must receive 200 OK, got {ok_count}"
    );
    assert_eq!(
        conflict_count,
        concurrency - 1,
        "All other clients must receive 409 Conflict"
    );

    // Drop the winner
    drop(winning_stream);

    // Wait for Hyper to close and guard drop to run
    let mut cleared = false;
    for _ in 0..30 {
        if !server.is_audio_streaming().load(Ordering::SeqCst) {
            cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(cleared, "Streaming flag must be released after winner drops connection");

    // Sequential client should now succeed
    let mut stream_after = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream_after
        .write_all(
            format!("GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();

    let mut buf = vec![0u8; 512];
    let n = stream_after.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_auth_fuzzing_and_lock_immunity() {
    let trusted_1 = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
    let trusted_2 = "11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff";
    let (port, shutdown_tx, server) = setup_adversarial_server(
        vec![trusted_1.to_string(), trusted_2.to_string()],
        true,
        Some(1),
    )
    .await;

    let adversarial_attacks = vec![
        // 1. Missing header entirely
        format!("GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
        // 2. Empty header
        format!("GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint: \r\nConnection: close\r\n\r\n"),
        // 3. Whitespace header
        format!("GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint:    \r\nConnection: close\r\n\r\n"),
        // 4. Untrusted fingerprint
        format!("GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint: deadbeef00112233\r\nConnection: close\r\n\r\n"),
        // 5. Truncated fingerprint
        format!("GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint: aabbccddeeff0011\r\nConnection: close\r\n\r\n"),
        // 6. Injection attempt in header
        format!("GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint: ' OR '1'='1' --\r\nConnection: close\r\n\r\n"),
        // 7. Path traversal in query string
        format!("GET /api/v2/audio/stream?fingerprint=../../etc/passwd HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
        // 8. Empty query string fingerprint
        format!("GET /api/v2/audio/stream?fingerprint= HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    ];

    for (idx, attack) in adversarial_attacks.into_iter().enumerate() {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        stream.write_all(attack.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        let res = String::from_utf8_lossy(&buf[..n]);

        assert!(
            res.contains("403 Forbidden"),
            "Attack #{idx} should have been rejected with 403 Forbidden, got: {res}"
        );

        // Crucial invariant: 403 response must never leave server in streaming state
        assert!(
            !server.is_audio_streaming().load(Ordering::SeqCst),
            "Server lock must not be held after failed auth on attack #{idx}"
        );
    }

    // Now verify legitimate authentication methods:
    // A. X-Fingerprint header with trusted_1
    {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint: {trusted_1}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    // B. Fingerprint header (lowercase) with trusted_2
    {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nfingerprint: {trusted_2}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    // C. Query param fingerprint with trusted_1
    {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream?fingerprint={trusted_1} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_parameter_boundary_and_fuzzing() {
    let (port, shutdown_tx, server) = setup_adversarial_server(vec![], true, Some(1)).await;

    let bad_queries = vec![
        // Codec invalid values
        ("codec=flac", "Invalid codec flac"),
        ("codec=mp3", "Invalid codec mp3"),
        ("codec=aac", "Invalid codec aac"),
        ("codec=OPUS", "Case sensitivity rejected OPUS"),
        ("codec=PCM", "Case sensitivity rejected PCM"),
        ("codec=", "Empty codec"),
        // Bitrate boundaries (< 64000 or > 128000)
        ("bitrate=63999", "Bitrate boundary 63999 (< 64000)"),
        ("bitrate=128001", "Bitrate boundary 128001 (> 128000)"),
        ("bitrate=0", "Bitrate 0"),
        ("bitrate=-50", "Negative bitrate"),
        ("bitrate=99999999999999999999", "Integer overflow bitrate"),
        ("bitrate=non_numeric", "Non-numeric bitrate"),
        ("bitrate=", "Empty bitrate"),
        // Sink invalid values
        ("sink=speaker", "Invalid sink speaker"),
        ("sink=headset", "Invalid sink headset"),
        ("sink=default_sink", "Invalid sink default_sink"),
        ("sink=MONITOR", "Case sensitivity rejected MONITOR"),
        ("sink=", "Empty sink"),
    ];

    for (query, desc) in bad_queries {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream?{query} HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Connection: close\r\n\
             \r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        let res = String::from_utf8_lossy(&buf[..n]);

        assert!(
            res.contains("400 Bad Request"),
            "Failed validation for '{desc}' (query '{query}'), response: {res}"
        );

        // Ensure lock not held
        assert!(
            !server.is_audio_streaming().load(Ordering::SeqCst),
            "Lock leaked on 400 Bad Request for '{desc}'"
        );
    }

    // Now test valid boundaries
    let valid_queries = vec![
        ("codec=opus&bitrate=64000", "Min valid bitrate 64000"),
        ("codec=opus&bitrate=128000", "Max valid bitrate 128000"),
        ("codec=opus&bitrate=96000", "Default bitrate 96000"),
        ("codec=pcm", "Raw PCM mode"),
        ("sink=monitor", "Monitor sink mode"),
        ("sink=virtual", "Virtual sink mode"),
        (
            "codec=opus&bitrate=80000&sink=virtual",
            "Combined valid parameters",
        ),
    ];

    for (query, desc) in valid_queries {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream?{query} HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Connection: close\r\n\
             \r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        let res = String::from_utf8_lossy(&buf[..n]);

        assert!(
            res.contains("200 OK"),
            "Valid parameter set '{desc}' unexpectedly failed: {res}"
        );

        drop(stream);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_rapid_disconnect_reconnect_stress() {
    let (port, shutdown_tx, server) = setup_adversarial_server(vec![], true, None).await;

    let cycles = 15;
    for cycle in 0..cycles {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream?codec=pcm HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Connection: close\r\n\
             \r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        // Read response headers
        let mut header_buf = vec![0u8; 1024];
        let n = stream.read(&mut header_buf).await.unwrap();
        let header_str = String::from_utf8_lossy(&header_buf[..n]);
        assert!(
            header_str.contains("200 OK"),
            "Cycle #{cycle} failed to get 200 OK: {header_str}"
        );

        // Verify server is now in streaming state
        assert!(
            server.is_audio_streaming().load(Ordering::SeqCst),
            "Server must report streaming state true in cycle #{cycle}"
        );

        // Abruptly drop connection
        drop(stream);

        // Wait for teardown
        let mut cleared = false;
        for _ in 0..25 {
            if !server.is_audio_streaming().load(Ordering::SeqCst) {
                cleared = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        assert!(
            cleared,
            "Cycle #{cycle}: Lock was not released after client socket drop within 375ms"
        );
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_framing_oracle_field_validation() {
    let (port, shutdown_tx, _server) = setup_adversarial_server(vec![], true, Some(10)).await;

    // Test 1: Opus framing
    {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream?codec=opus&bitrate=96000 HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Connection: close\r\n\
             \r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }

        let header_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("Header boundary")
            + 4;
        let body_bytes = &buf[header_end..];

        let mut pos = 0;
        let mut frames_found = 0;
        let mut last_seq = None;

        while pos + HEADER_SIZE <= body_bytes.len() {
            if let Some(magic_idx) = body_bytes[pos..]
                .windows(4)
                .position(|w| w == &ADAU_MAGIC)
            {
                let frame_start = pos + magic_idx;
                if frame_start + HEADER_SIZE <= body_bytes.len() {
                    if let Ok((frame, consumed)) = AudioWireFrame::decode(&body_bytes[frame_start..]) {
                        assert_eq!(frame.header.magic, ADAU_MAGIC);
                        assert_eq!(frame.header.codec, AudioCodec::Opus);
                        assert_eq!(frame.header.channels, 2);
                        assert_eq!(frame.header.sample_rate, 48000);
                        assert!(frame.header.timestamp_ms > 1_700_000_000_000, "Timestamp must be valid epoch ms");
                        assert!(frame.header.payload_len > 0, "Opus payload cannot be empty");

                        if let Some(prev) = last_seq {
                            assert_eq!(frame.header.sequence, prev + 1, "Monotonic sequence check");
                        } else {
                            assert_eq!(frame.header.sequence, 0, "Initial sequence check");
                        }
                        last_seq = Some(frame.header.sequence);
                        frames_found += 1;
                        pos = frame_start + consumed;
                        continue;
                    }
                }
            }
            break;
        }

        assert!(frames_found >= 3, "Expected at least 3 Opus frames, got {frames_found}");
    }

    // Test 2: PCM framing exact size
    {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "GET /api/v2/audio/stream?codec=pcm HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Connection: close\r\n\
             \r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }

        let header_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("Header boundary")
            + 4;
        let body_bytes = &buf[header_end..];

        let mut pos = 0;
        let mut frames_found = 0;

        while pos + HEADER_SIZE <= body_bytes.len() {
            if let Some(magic_idx) = body_bytes[pos..]
                .windows(4)
                .position(|w| w == &ADAU_MAGIC)
            {
                let frame_start = pos + magic_idx;
                if frame_start + HEADER_SIZE <= body_bytes.len() {
                    if let Ok((frame, consumed)) = AudioWireFrame::decode(&body_bytes[frame_start..]) {
                        assert_eq!(frame.header.codec, AudioCodec::Pcm);
                        assert_eq!(frame.header.channels, 2);
                        assert_eq!(frame.header.sample_rate, 48000);
                        assert_eq!(frame.header.payload_len, 3840, "PCM frame payload must be exactly 3840 bytes (960 stereo samples * 4 bytes)");
                        assert_eq!(frame.payload.len(), 3840);
                        frames_found += 1;
                        pos = frame_start + consumed;
                        continue;
                    }
                }
            }
            break;
        }

        assert!(frames_found >= 3, "Expected at least 3 PCM frames, got {frames_found}");
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_invalid_http_verbs_and_lock_immunity() {
    let (port, shutdown_tx, server) = setup_adversarial_server(vec![], true, Some(1)).await;

    let bad_methods = vec!["POST", "PUT", "DELETE", "PATCH", "HEAD"];
    for method in bad_methods {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req = format!(
            "{method} /api/v2/audio/stream HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Connection: close\r\n\
             \r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; 512];
        let n = stream.read(&mut buf).await.unwrap();
        let res = String::from_utf8_lossy(&buf[..n]);

        assert!(
            res.contains("404 Not Found"),
            "Invalid HTTP method {method} should be rejected with 404, got: {res}"
        );

        assert!(
            !server.is_audio_streaming().load(Ordering::SeqCst),
            "Streaming lock must not be held after invalid HTTP method {method}"
        );
    }

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_slow_client_resilience_and_teardown() {
    // 150 unpaced frames to guarantee backpressure on channel capacity 32
    let (port, shutdown_tx, server) = setup_adversarial_server(vec![], true, Some(150)).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    let req = format!(
        "GET /api/v2/audio/stream?codec=pcm HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();

    // Read response headers
    let mut header_buf = [0u8; 1024];
    let mut header_bytes = Vec::new();
    loop {
        let n = stream.read(&mut header_buf).await.unwrap();
        if n == 0 {
            break;
        }
        header_bytes.extend_from_slice(&header_buf[..n]);
        if header_bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    // Read body in small chunks with intermittent delays to induce drops followed by recovered delivery
    let mut body_bytes = Vec::new();
    let mut chunk = [0u8; 2048];
    for read_idx in 0..150 {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                body_bytes.extend_from_slice(&chunk[..n]);
                // Artificially delay every 3 reads to allow generator to overrun buffer and set discontinuity
                if read_idx % 3 == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            Err(_) => break,
        }
    }

    let mut pos = 0;
    let mut decoded_frames = Vec::new();

    while pos + HEADER_SIZE <= body_bytes.len() {
        if let Some(magic_idx) = body_bytes[pos..]
            .windows(4)
            .position(|w| w == &ADAU_MAGIC)
        {
            let frame_start = pos + magic_idx;
            if frame_start + HEADER_SIZE <= body_bytes.len() {
                if let Ok((frame, consumed)) = AudioWireFrame::decode(&body_bytes[frame_start..]) {
                    decoded_frames.push((frame.header.sequence, frame.header.flags));
                    pos = frame_start + consumed;
                    continue;
                }
            }
        }
        break;
    }

    // Verify monotonic sequencing and clean stream delivery under slow consumption
    assert!(
        decoded_frames.len() >= 30,
        "Slow consumer should receive at least 30 frames"
    );
    for window in decoded_frames.windows(2) {
        assert_eq!(
            window[1].0,
            window[0].0 + 1,
            "Sequencing must remain strictly monotonic"
        );
    }

    // Drop slow client
    drop(stream);

    // Verify teardown
    let mut cleared = false;
    for _ in 0..25 {
        if !server.is_audio_streaming().load(Ordering::SeqCst) {
            cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(cleared, "Streaming lock must be released after slow client disconnect");

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_half_close_drop() {
    let (port, shutdown_tx, server) = setup_adversarial_server(vec![], true, None).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    let req = format!(
        "GET /api/v2/audio/stream HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    // Read headers
    let mut buf = vec![0u8; 512];
    let n = stream.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));

    // Verify streaming is active
    assert!(server.is_audio_streaming().load(Ordering::SeqCst));

    // Shutdown write half
    stream.shutdown().await.unwrap();

    // Then drop the stream entirely
    drop(stream);

    // Verify clean teardown
    let mut cleared = false;
    for _ in 0..25 {
        if !server.is_audio_streaming().load(Ordering::SeqCst) {
            cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(cleared, "Server must cleanly clear streaming lock after half-close drop");

    let _ = shutdown_tx.send(true);
}

