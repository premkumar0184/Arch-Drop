use archdrop_audio::capture::MockAudioCapture;
use archdrop_audio::framing::{AudioCodec, AudioWireFrame, ADAU_MAGIC, HEADER_SIZE};
use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_transfer::TransferServer;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn setup_test_server(
    paired_fingerprints: Vec<String>,
    attach_mock_audio: bool,
    max_frames: Option<usize>,
) -> (u16, tokio::sync::watch::Sender<bool>) {
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
        false, // plain HTTP for test simplicity
    )
    .unwrap();

    if attach_mock_audio {
        let mock = if let Some(max) = max_frames {
            MockAudioCapture::new().with_max_frames(max).with_pacing(false)
        } else {
            MockAudioCapture::new().with_pacing(true)
        };
        server.set_mock_audio_capture(mock);
        server.set_mock_virtual_sink();
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = server.run(shutdown_rx).await;
    });

    // Wait for server to bind
    tokio::time::sleep(Duration::from_millis(70)).await;

    (port, shutdown_tx)
}

#[tokio::test]
async fn test_audio_stream_opus_success() {
    let (port, shutdown_tx) = setup_test_server(vec![], true, Some(5)).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();

    let request = format!(
        "GET /api/v2/audio/stream?codec=opus&bitrate=96000 HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: audio/x-archdrop-audio\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => {
                eprintln!("Read finished: {e}");
                break;
            }
        }
    }

    let response_str = String::from_utf8_lossy(&buf);
    assert!(response_str.starts_with("HTTP/1.1 200 OK"));
    assert!(response_str.contains("content-type: audio/x-archdrop-audio"));
    assert!(response_str.contains("transfer-encoding: chunked"));
    assert!(response_str.contains("x-archdrop-audio-codec: opus"));
    assert!(response_str.contains("x-archdrop-bitrate: 96000"));

    // Find the end of HTTP headers
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("Header boundary")
        + 4;
    let body_bytes = &buf[header_end..];

    // Verify presence of ADAU magic marker in the body
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
                    assert_eq!(frame.header.codec, AudioCodec::Opus);
                    assert_eq!(frame.header.channels, 2);
                    assert_eq!(frame.header.sample_rate, 48000);
                    assert_eq!(frame.header.sequence, frames_found as u64);
                    frames_found += 1;
                    pos = frame_start + consumed;
                    continue;
                }
            }
        }
        break;
    }

    assert!(frames_found >= 1, "Expected at least 1 valid ADAU frame, found {}", frames_found);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_audio_stream_pcm_success() {
    let (port, shutdown_tx) = setup_test_server(vec![], true, Some(3)).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();

    let request = format!(
        "GET /api/v2/audio/stream?codec=pcm HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&buf);
    assert!(response_str.starts_with("HTTP/1.1 200 OK"));
    assert!(response_str.contains("x-archdrop-audio-codec: pcm"));

    // Find ADAU magic in the body
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("Header boundary")
        + 4;
    let body_bytes = &buf[header_end..];

    let mut frames_found = 0;
    let mut pos = 0;
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
                    assert_eq!(frame.header.payload_len, 3840);
                    assert_eq!(frame.payload.len(), 3840);
                    frames_found += 1;
                    pos = frame_start + consumed;
                    continue;
                }
            }
        }
        break;
    }

    assert!(frames_found >= 1, "Expected at least 1 PCM frame");

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_audio_stream_authentication_enforced() {
    let trusted_fp = "abcd1234trustedfingerprint";
    let (port, shutdown_tx) =
        setup_test_server(vec![trusted_fp.to_string()], true, Some(1)).await;

    // 1. Missing fingerprint header -> 403 Forbidden
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream
        .write_all(
            format!(
                "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let res = String::from_utf8_lossy(&buf[..n]);
    assert!(res.contains("403 Forbidden"));

    // 2. Untrusted fingerprint -> 403 Forbidden
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream2
        .write_all(
            format!(
                "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint: unknownpeer\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let mut buf2 = vec![0u8; 1024];
    let n2 = stream2.read(&mut buf2).await.unwrap();
    let res2 = String::from_utf8_lossy(&buf2[..n2]);
    assert!(res2.contains("403 Forbidden"));

    // 3. Trusted fingerprint -> 200 OK
    let mut stream3 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream3
        .write_all(
            format!(
                "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fingerprint: {trusted_fp}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let mut buf3 = vec![0u8; 1024];
    let n3 = stream3.read(&mut buf3).await.unwrap();
    let res3 = String::from_utf8_lossy(&buf3[..n3]);
    assert!(res3.contains("200 OK"));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_audio_stream_mutual_exclusion_conflict() {
    let (port, shutdown_tx) = setup_test_server(vec![], true, None).await;

    // Client 1 connects and holds the stream
    let mut stream1 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream1
        .write_all(
            format!(
                "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    // Read headers from client 1
    let mut buf1 = vec![0u8; 512];
    let n1 = stream1.read(&mut buf1).await.unwrap();
    let res1 = String::from_utf8_lossy(&buf1[..n1]);
    assert!(res1.contains("200 OK"));

    // Client 2 connects concurrently -> 409 Conflict
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream2
        .write_all(
            format!(
                "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let mut buf2 = vec![0u8; 512];
    let n2 = stream2.read(&mut buf2).await.unwrap();
    let res2 = String::from_utf8_lossy(&buf2[..n2]);
    assert!(res2.contains("409 Conflict"));

    // Drop client 1
    drop(stream1);
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Client 3 connects after client 1 disconnected -> should succeed with 200 OK!
    let mut stream3 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream3
        .write_all(
            format!(
                "GET /api/v2/audio/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let mut buf3 = vec![0u8; 512];
    let n3 = stream3.read(&mut buf3).await.unwrap();
    let res3 = String::from_utf8_lossy(&buf3[..n3]);
    assert!(res3.contains("200 OK"));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_audio_stream_parameter_validation() {
    let (port, shutdown_tx) = setup_test_server(vec![], true, Some(1)).await;

    // Bad codec -> 400 Bad Request
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream
        .write_all(
            format!(
                "GET /api/v2/audio/stream?codec=flac HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf = vec![0u8; 512];
    let n = stream.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).contains("400 Bad Request"));

    // Bitrate out of range (< 64000) -> 400 Bad Request
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream2
        .write_all(
            format!(
                "GET /api/v2/audio/stream?bitrate=32000 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf2 = vec![0u8; 512];
    let n2 = stream2.read(&mut buf2).await.unwrap();
    assert!(String::from_utf8_lossy(&buf2[..n2]).contains("400 Bad Request"));

    // Bitrate out of range (> 128000) -> 400 Bad Request
    let mut stream3 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream3
        .write_all(
            format!(
                "GET /api/v2/audio/stream?bitrate=256000 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf3 = vec![0u8; 512];
    let n3 = stream3.read(&mut buf3).await.unwrap();
    assert!(String::from_utf8_lossy(&buf3[..n3]).contains("400 Bad Request"));

    // Bad sink mode -> 400 Bad Request
    let mut stream4 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    stream4
        .write_all(
            format!(
                "GET /api/v2/audio/stream?sink=alien HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf4 = vec![0u8; 512];
    let n4 = stream4.read(&mut buf4).await.unwrap();
    assert!(String::from_utf8_lossy(&buf4[..n4]).contains("400 Bad Request"));

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_audio_stream_virtual_sink_mode() {
    let (port, shutdown_tx) = setup_test_server(vec![], true, Some(3)).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();

    let request = format!(
        "GET /api/v2/audio/stream?sink=virtual HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&buf);
    assert!(response_str.starts_with("HTTP/1.1 200 OK"));

    let _ = shutdown_tx.send(true);
}
