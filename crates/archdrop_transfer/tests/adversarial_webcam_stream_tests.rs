use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_transfer::TransferServer;
use archdrop_webcam::{MockV4l2Sink, WebcamSink};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn make_valid_frame() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n");
    v.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
    v.extend_from_slice(b"\r\n--frame--\r\n");
    v
}

async fn setup_harness(
    paired_fps: Vec<String>,
) -> (
    u16,
    tokio::sync::watch::Sender<bool>,
    Arc<WebcamSink>,
    MockV4l2Sink,
    Arc<TransferServer>,
) {
    let temp_dir = TempDir::new().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = Config::default();
    config.security.paired_fingerprints = paired_fps.into_iter().collect();
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

    let mock = MockV4l2Sink::new(PathBuf::from("/dev/video-mock"));
    let mock_clone = mock.clone();
    let sink = Arc::new(WebcamSink::new(Box::new(mock)));
    server.set_webcam_sink(sink.clone());

    let server_arc = Arc::new(server);
    let server_runner = server_arc.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = server_runner.run(shutdown_rx).await;
    });

    tokio::time::sleep(Duration::from_millis(60)).await;

    (port, shutdown_tx, sink, mock_clone, server_arc)
}

#[tokio::test]
async fn test_adversarial_high_concurrency_race_condition_409() {
    let (port, shutdown_tx, _sink, _mock, server) = setup_harness(vec![]).await;

    let concurrency = 10;
    let mut handles = Vec::new();

    for i in 0..concurrency {
        let port = port;
        let handle = tokio::spawn(async move {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
            let req = format!(
                "POST /api/v2/webcam/stream HTTP/1.1\r\n\
                 Host: 127.0.0.1:{port}\r\n\
                 Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
                 Transfer-Encoding: chunked\r\n\
                 \r\n"
            );
            stream.write_all(req.as_bytes()).await.unwrap();

            // Hold open for a short moment
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Send single frame or terminate
            let mut chunk = Vec::new();
            chunk.extend_from_slice(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n");
            chunk.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
            chunk.extend_from_slice(b"\r\n--frame--\r\n");

            let _ = stream.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await;
            let _ = stream.write_all(&chunk).await;
            let _ = stream.write_all(b"\r\n0\r\n\r\n").await;
            let _ = stream.flush().await;

            let mut resp = [0u8; 512];
            let n = stream.read(&mut resp).await.unwrap_or(0);
            let resp_str = String::from_utf8_lossy(&resp[..n]).to_string();

            if resp_str.starts_with("HTTP/1.1 200 OK") {
                200
            } else if resp_str.starts_with("HTTP/1.1 409 Conflict") {
                409
            } else {
                eprintln!("Unexpected response for client {}: {}", i, resp_str);
                0
            }
        });
        handles.push(handle);
    }

    let mut count_200 = 0;
    let mut count_409 = 0;

    for h in handles {
        match h.await.unwrap() {
            200 => count_200 += 1,
            409 => count_409 += 1,
            other => panic!("Unexpected HTTP status code: {other}"),
        }
    }

    assert_eq!(count_200, 1, "Exactly one stream connection must succeed (200 OK)");
    assert_eq!(
        count_409,
        concurrency - 1,
        "All other concurrent attempts must be rejected with 409 Conflict"
    );

    // Wait for streaming flag to be released
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !server.is_webcam_streaming().load(Ordering::SeqCst),
        "Streaming flag must be false after all connections terminate"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_authentication_bypass_matrix() {
    let paired = "SECURE:DEVICE:1234".to_string();
    let (port, shutdown_tx, _sink, _mock, _server) =
        setup_harness(vec![paired.clone()]).await;

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/api/v2/webcam/stream");

    let valid_body = make_valid_frame();

    // 1. Missing header and query param
    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(valid_body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // 2. Empty string fingerprint
    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .header("X-Fingerprint", "")
        .body(valid_body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // 3. Whitespace fingerprint
    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .header("X-Fingerprint", "   ")
        .body(valid_body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // 4. Malicious path traversal / injection attempt in fingerprint
    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .header("X-Fingerprint", "../../etc/shadow")
        .body(valid_body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // 5. Query param with unpaired fingerprint
    let res = client
        .post(format!("{url}?fingerprint=UNPAIRED"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(valid_body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // 6. Query param with valid paired fingerprint
    let res = client
        .post(format!("{url}?fingerprint={paired}"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(valid_body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // 7. Alternate header name `fingerprint` (lowercase/legacy) with valid paired fingerprint
    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .header("fingerprint", &paired)
        .body(valid_body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // 8. GET method rejected (405 or 404)
    let res = client
        .get(&url)
        .header("X-Fingerprint", &paired)
        .send()
        .await
        .unwrap();
    assert_ne!(res.status(), reqwest::StatusCode::OK);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_abrupt_client_disconnect_cleanup() {
    let (port, shutdown_tx, sink, _mock, server) = setup_harness(vec![]).await;

    // Connect client 1 and abruptly disconnect after sending partial data
    {
        let mut stream1 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        let req1 = format!(
            "POST /api/v2/webcam/stream HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
             Transfer-Encoding: chunked\r\n\
             \r\n"
        );
        stream1.write_all(req1.as_bytes()).await.unwrap();

        // Send 1 frame
        let mut frame1 = Vec::new();
        frame1.extend_from_slice(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n");
        frame1.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
        frame1.extend_from_slice(b"\r\n");

        stream1
            .write_all(format!("{:x}\r\n", frame1.len()).as_bytes())
            .await
            .unwrap();
        stream1.write_all(&frame1).await.unwrap();
        stream1.write_all(b"\r\n").await.unwrap();
        stream1.flush().await.unwrap();

        // Send partial chunk of next frame then abruptly DROP TCP stream
        stream1.write_all(b"20\r\n--frame\r\nContent-Type: ").await.unwrap();
        stream1.flush().await.unwrap();

        // Abrupt drop!
        drop(stream1);
    }

    // Wait for server to process disconnect and drop guard
    tokio::time::sleep(Duration::from_millis(80)).await;

    assert!(
        !server.is_webcam_streaming().load(Ordering::SeqCst),
        "is_webcam_streaming flag must be reset to false after abrupt TCP drop"
    );

    // Client 2 connects immediately afterwards: MUST succeed with 200 OK
    let client = reqwest::Client::new();
    let body2 = make_valid_frame();
    let res2 = client
        .post(format!("http://127.0.0.1:{port}/api/v2/webcam/stream"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(body2)
        .send()
        .await
        .unwrap();

    assert_eq!(res2.status(), reqwest::StatusCode::OK);

    // Verify frames were received by sink
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(sink.stats().frames_received >= 2);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_malformed_mjpeg_stream_resets_lock() {
    let (port, shutdown_tx, _sink, _mock, server) = setup_harness(vec![]).await;

    let client = reqwest::Client::new();
    let mut bad_body = Vec::new();
    bad_body.extend_from_slice(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n");
    bad_body.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Bad SOI
    bad_body.extend_from_slice(b"\r\n--frame--\r\n");

    // 1. Send corrupted JPEG (missing SOI) -> must return 400 Bad Request
    let res1 = client
        .post(format!("http://127.0.0.1:{port}/api/v2/webcam/stream"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(bad_body)
        .send()
        .await
        .unwrap();

    assert_eq!(res1.status(), reqwest::StatusCode::BAD_REQUEST);

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !server.is_webcam_streaming().load(Ordering::SeqCst),
        "is_webcam_streaming flag must be reset to false after 400 error"
    );

    // 2. Subsequent valid stream succeeds
    let good_body = make_valid_frame();
    let res2 = client
        .post(format!("http://127.0.0.1:{port}/api/v2/webcam/stream"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(good_body)
        .send()
        .await
        .unwrap();

    assert_eq!(res2.status(), reqwest::StatusCode::OK);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_adversarial_boundary_variations_and_empty_stream() {
    let (port, shutdown_tx, sink, _mock, _server) = setup_harness(vec![]).await;

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/api/v2/webcam/stream");

    // 1. Quoted boundary
    let mut quoted_body = Vec::new();
    quoted_body.extend_from_slice(b"--custom_b\r\nContent-Length: 4\r\n\r\n");
    quoted_body.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
    quoted_body.extend_from_slice(b"\r\n--custom_b--\r\n");

    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=\"custom_b\"")
        .body(quoted_body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // 2. Empty stream (only terminating boundary)
    let empty_stream = "--frame--\r\n";
    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(empty_stream)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // 3. Boundary with semicolon parameters
    let mut param_body = Vec::new();
    param_body.extend_from_slice(b"--param_b\r\nContent-Length: 4\r\n\r\n");
    param_body.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
    param_body.extend_from_slice(b"\r\n--param_b--\r\n");

    let res = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=param_b; charset=utf-8")
        .body(param_body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(sink.stats().frames_received >= 2);

    let _ = shutdown_tx.send(true);
}
