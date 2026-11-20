use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_transfer::TransferServer;
use archdrop_webcam::{MockV4l2Sink, WebcamSink, MODPROBE_DIAGNOSTIC};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

async fn setup_test_server(
    paired_fingerprints: Vec<String>,
    attach_sink: bool,
) -> (u16, tokio::sync::watch::Sender<bool>, Option<Arc<WebcamSink>>, Option<MockV4l2Sink>) {
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

    let (sink_opt, mock_opt) = if attach_sink {
        let mock = MockV4l2Sink::new(PathBuf::from("/dev/video-mock"));
        let mock_clone = mock.clone();
        let sink = Arc::new(WebcamSink::new(Box::new(mock)));
        server.set_webcam_sink(sink.clone());
        (Some(sink), Some(mock_clone))
    } else {
        // Point sysfs_root to an empty directory to simulate absent v4l2loopback in test environment
        let empty_sysfs = temp_dir.path().join("empty_sysfs");
        let _ = tokio::fs::create_dir_all(&empty_sysfs).await;
        server.set_sysfs_root(empty_sysfs);
        (None, None)
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = server.run(shutdown_rx).await;
    });

    // Wait for server to bind
    tokio::time::sleep(Duration::from_millis(60)).await;

    (port, shutdown_tx, sink_opt, mock_opt)
}

#[tokio::test]
async fn test_webcam_stream_ingestion_success() {
    let (port, shutdown_tx, sink, mock) = setup_test_server(vec![], true).await;
    let sink = sink.unwrap();
    let mock = mock.unwrap();

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();

    let request_header = format!(
        "POST /api/v2/webcam/stream HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
         Transfer-Encoding: chunked\r\n\
         \r\n"
    );
    stream.write_all(request_header.as_bytes()).await.unwrap();

    // Frame 1
    let frame1 = b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n\xFF\xD8\xFF\xD9\r\n";
    stream
        .write_all(format!("{:x}\r\n", frame1.len()).as_bytes())
        .await
        .unwrap();
    stream.write_all(frame1).await.unwrap();
    stream.write_all(b"\r\n").await.unwrap();
    stream.flush().await.unwrap();

    // Wait 40ms to simulate real camera 25-30fps frame interval
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Frame 2
    let frame2 = b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 6\r\n\r\n\xFF\xD8\x01\x02\xFF\xD9\r\n--frame--\r\n";
    stream
        .write_all(format!("{:x}\r\n", frame2.len()).as_bytes())
        .await
        .unwrap();
    stream.write_all(frame2).await.unwrap();
    stream.write_all(b"\r\n").await.unwrap();
    // End of chunked stream
    stream.write_all(b"0\r\n\r\n").await.unwrap();
    stream.flush().await.unwrap();

    use tokio::io::AsyncReadExt;
    let mut response_buf = [0u8; 1024];
    let n = stream.read(&mut response_buf).await.unwrap();
    let response_str = String::from_utf8_lossy(&response_buf[..n]);
    assert!(response_str.starts_with("HTTP/1.1 200 OK"));

    // Allow sink worker task to finish writing frames
    tokio::time::sleep(Duration::from_millis(80)).await;

    let stats = sink.stats();
    assert_eq!(stats.frames_received, 2);
    assert_eq!(stats.frames_written, 2);

    let recorded = mock.written_frames();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0], vec![0xFF, 0xD8, 0xFF, 0xD9]);
    assert_eq!(recorded[1], vec![0xFF, 0xD8, 0x01, 0x02, 0xFF, 0xD9]);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_webcam_stream_authentication() {
    let paired_fp = "AA:BB:CC:DD:EE:FF".to_string();
    let (port, shutdown_tx, _sink, _mock) =
        setup_test_server(vec![paired_fp.clone()], true).await;

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/api/v2/webcam/stream");

    let mut body = Vec::new();
    body.extend_from_slice(b"--frame\r\n");
    body.extend_from_slice(b"Content-Type: image/jpeg\r\n");
    body.extend_from_slice(b"Content-Length: 4\r\n\r\n");
    body.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
    body.extend_from_slice(b"\r\n--frame--\r\n");

    // 1. Missing fingerprint header -> 403 Forbidden
    let resp1 = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), reqwest::StatusCode::FORBIDDEN);

    // 2. Untrusted / wrong fingerprint -> 403 Forbidden
    let resp2 = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .header("X-Fingerprint", "11:22:33:44:55:66")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), reqwest::StatusCode::FORBIDDEN);

    // 3. Trusted matching fingerprint -> 200 OK
    let resp3 = client
        .post(&url)
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .header("X-Fingerprint", &paired_fp)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), reqwest::StatusCode::OK);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_webcam_stream_mutual_exclusion_409() {
    let (port, shutdown_tx, _sink, _mock) = setup_test_server(vec![], true).await;

    // Connect Stream A via raw TCP to maintain persistent streaming connection
    let mut stream_a = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();

    let request_a_header = format!(
        "POST /api/v2/webcam/stream HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: multipart/x-mixed-replace; boundary=frame\r\n\
         Transfer-Encoding: chunked\r\n\
         \r\n"
    );
    stream_a.write_all(request_a_header.as_bytes()).await.unwrap();

    // Send first chunk (initial boundary)
    let chunk1 = b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n\xFF\xD8\xFF\xD9\r\n";
    let chunk1_header = format!("{:x}\r\n", chunk1.len());
    stream_a.write_all(chunk1_header.as_bytes()).await.unwrap();
    stream_a.write_all(chunk1).await.unwrap();
    stream_a.write_all(b"\r\n").await.unwrap();
    stream_a.flush().await.unwrap();

    // Allow server to process chunk and set streaming flag
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Attempt Stream B while Stream A is active -> must receive 409 Conflict
    let client = reqwest::Client::new();
    let resp_b = client
        .post(format!("http://127.0.0.1:{port}/api/v2/webcam/stream"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body("--frame--\r\n")
        .send()
        .await
        .unwrap();

    assert_eq!(resp_b.status(), reqwest::StatusCode::CONFLICT);

    // Now terminate Stream A
    drop(stream_a);
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Stream C should now succeed since Stream A was dropped and lock released
    let mut body_c = Vec::new();
    body_c.extend_from_slice(b"--frame\r\n");
    body_c.extend_from_slice(b"Content-Type: image/jpeg\r\n");
    body_c.extend_from_slice(b"Content-Length: 4\r\n\r\n");
    body_c.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
    body_c.extend_from_slice(b"\r\n--frame--\r\n");

    let resp_c = client
        .post(format!("http://127.0.0.1:{port}/api/v2/webcam/stream"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body(body_c)
        .send()
        .await
        .unwrap();

    assert_eq!(resp_c.status(), reqwest::StatusCode::OK);

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_webcam_stream_service_unavailable_without_loopback() {
    // When no sink is attached and v4l2loopback is not loaded, server returns 503
    let (port, shutdown_tx, _sink, _mock) = setup_test_server(vec![], false).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/api/v2/webcam/stream"))
        .header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        .body("--frame--\r\n")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let diag_header = resp
        .headers()
        .get("X-ArchDrop-Diagnostic")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(diag_header.contains("sudo modprobe v4l2loopback"));

    let body_text = resp.text().await.unwrap();
    assert!(body_text.contains(MODPROBE_DIAGNOSTIC));

    let _ = shutdown_tx.send(true);
}
