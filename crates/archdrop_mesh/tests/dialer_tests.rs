use archdrop_mesh::client::{DerpClient, DerpClientConfig};
use archdrop_mesh::crypto::MeshKeyPair;
use archdrop_mesh::dialer::{FallbackDialer, P2pDialer, TransportTier};
use archdrop_mesh::error::MeshError;
use archdrop_mesh::mock_relay::MockDerpRelay;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Mock P2P dialer that connects to a local listener to simulate successful LOHS SoftAP connection.
struct MockP2pDialer {
    target_addr: String,
    call_count: Arc<AtomicU32>,
    delay: Duration,
    should_fail: bool,
}

impl MockP2pDialer {
    fn new(target_addr: String, delay: Duration) -> Self {
        Self {
            target_addr,
            call_count: Arc::new(AtomicU32::new(0)),
            delay,
            should_fail: false,
        }
    }

    fn failing(delay: Duration) -> Self {
        Self {
            target_addr: "127.0.0.1:1".to_string(),
            call_count: Arc::new(AtomicU32::new(0)),
            delay,
            should_fail: true,
        }
    }
}

impl P2pDialer for MockP2pDialer {
    fn dial_p2p<'a>(
        &'a self,
        _target_peer: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<TcpStream, MeshError>> + Send + 'a>> {
        Box::pin(async move {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if self.should_fail {
                return Err(MeshError::ConnectionClosed("Simulated P2P failure".to_string()));
            }
            TcpStream::connect(&self.target_addr)
                .await
                .map_err(MeshError::Io)
        })
    }
}

#[tokio::test]
async fn test_direct_tcp_success_fast() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"direct_ok").await;
        }
    });

    let dialer = FallbackDialer::new(None);
    let start = Instant::now();
    let stream = dialer
        .dial_with_fallback(Some(&addr), None)
        .await
        .expect("Direct TCP dial must succeed");

    assert!(stream.is_direct(), "Expected Direct stream");
    assert_eq!(stream.tier(), TransportTier::DirectLan);
    assert_eq!(stream.tier().label(), "Direct LAN");
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "Direct connection should take < 500ms"
    );

    let mut stream = stream;
    let mut buf = vec![0u8; 9];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"direct_ok");
}

#[tokio::test]
async fn test_direct_tcp_failure_no_derp() {
    // Port 1 is reserved and immediately refused on localhost
    let dialer = FallbackDialer::new(None).with_direct_timeout(Duration::from_millis(200));
    let err = dialer
        .dial_with_fallback(Some("127.0.0.1:1"), None)
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("All 3 transport tiers failed"),
        "Expected peer unreachable error: {err}"
    );
}

#[tokio::test]
async fn test_transparent_fallback_to_derp_under_2_seconds() {
    // 1. Start mock DERP relay
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_url = relay.relay_url();

    // 2. Client A and Client B
    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .unwrap();

    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .unwrap();

    let client_a_key = *client_a.local_key();
    let listener_b = client_b.listener();

    // 3. Receiver task on B
    let recv_handle = tokio::spawn(async move {
        let (mut derp_stream, remote_peer) = listener_b.accept().await.unwrap();
        assert_eq!(&remote_peer, &client_a_key);

        let mut buf = vec![0u8; 17];
        derp_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"fallback_wan_ping");
        derp_stream.write_all(b"fallback_wan_pong").await.unwrap();
        derp_stream.flush().await.unwrap();
    });

    // 4. Dial with Fallback on A:
    // Specify a blackhole IP (198.51.100.1 is RFC 5737 TEST-NET-2, unroutable)
    // with exact 1500 ms direct timeout
    let dialer = FallbackDialer::new(Some(client_a.clone()))
        .with_direct_timeout(Duration::from_millis(1500));

    let start = Instant::now();
    let stream_res = dialer
        .dial_with_fallback(Some("198.51.100.1:53317"), Some(client_b.local_key()))
        .await;

    let elapsed = start.elapsed();
    let mut stream = stream_res.expect("Fallback to DERP relay must succeed");

    assert!(stream.is_relayed(), "Stream must be a relayed DERP stream");
    assert_eq!(stream.tier(), TransportTier::WanRelay);
    assert_eq!(stream.tier().label(), "Cloud Relay");

    // Assert that total elapsed time meets the < 2.0 seconds requirement,
    // while honoring the 1500 ms direct attempt.
    assert!(
        elapsed >= Duration::from_millis(1450),
        "Direct connection timeout must have elapsed at least ~1500ms (got {elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_millis(2000),
        "Total fallback time must be strictly under 2.0 seconds (got {elapsed:?})"
    );

    // Verify virtual stream data transfer works over the fallback stream
    stream.write_all(b"fallback_wan_ping").await.unwrap();
    stream.flush().await.unwrap();

    let mut response = vec![0u8; 17];
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"fallback_wan_pong");

    recv_handle.await.unwrap();
    relay.stop().await;
}

#[tokio::test]
async fn test_three_tier_tier1_lan_success_fast() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"lan_ok").await;
        }
    });

    let dialer = FallbackDialer::new(None);
    let start = Instant::now();
    let stream = dialer
        .dial_with_fallback(Some(&addr), None)
        .await
        .expect("Tier 1 direct TCP dial must succeed");

    assert!(stream.is_direct(), "Stream must be Direct");
    assert_eq!(stream.tier(), TransportTier::DirectLan);
    assert_eq!(stream.tier().label(), "Direct LAN");
    assert_eq!(stream.tier().to_string(), "Direct LAN");
    assert!(
        start.elapsed() < Duration::from_millis(300),
        "Direct LAN connect must take < 300ms (got {:?})",
        start.elapsed()
    );

    let mut stream = stream;
    let mut buf = vec![0u8; 6];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"lan_ok");
}

#[tokio::test]
async fn test_three_tier_tier1_econnrefused_fast_fail_to_tier2_p2p() {
    // 1. Prepare simulated LOHS peer listener
    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = p2p_listener.accept().await {
            let _ = stream.write_all(b"p2p_lohs_ok").await;
        }
    });

    let mock_p2p = Arc::new(MockP2pDialer::new(p2p_addr, Duration::from_millis(20)));
    let p2p_calls = mock_p2p.call_count.clone();

    // 2. Direct LAN points to port 1 (which immediately returns ECONNREFUSED on localhost)
    let dialer = FallbackDialer::new(None)
        .with_p2p_dialer(mock_p2p)
        .with_direct_timeout(Duration::from_millis(1500));

    let start = Instant::now();
    let stream = dialer
        .dial_with_p2p_fallback(Some("127.0.0.1:1"), Some("companion_device"), None)
        .await
        .expect("Failover to Tier 2 P2P must succeed");

    let elapsed = start.elapsed();
    assert!(stream.is_p2p(), "Stream must be P2P SoftAP");
    assert_eq!(stream.tier(), TransportTier::OfflineP2p);
    assert_eq!(stream.tier().label(), "Peer-to-Peer");
    assert_eq!(stream.tier().to_string(), "Peer-to-Peer");
    assert_eq!(
        p2p_calls.load(Ordering::SeqCst),
        1,
        "P2P dialer must be invoked exactly once"
    );

    // SLA Check: Fail fast on ECONNREFUSED (< 300ms) plus P2P connect time (< 100ms)
    assert!(
        elapsed < Duration::from_millis(300),
        "Fail fast on ECONNREFUSED + Tier 2 dial must complete within < 300ms SLA (got {:?})",
        elapsed
    );

    let mut stream = stream;
    let mut buf = vec![0u8; 11];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"p2p_lohs_ok");
}

#[tokio::test]
async fn test_three_tier_dial_3tier_method() {
    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = p2p_listener.accept().await {
            let _ = stream.write_all(b"dial_3tier_ok").await;
        }
    });

    let mock_p2p = MockP2pDialer::new(p2p_addr, Duration::from_millis(10));
    let dialer = FallbackDialer::new(None).with_direct_timeout(Duration::from_millis(1500));

    let start = Instant::now();
    let stream = dialer
        .dial_3tier(Some("127.0.0.1:1"), Some(&mock_p2p), None)
        .await
        .expect("dial_3tier with external P2pDialer must succeed");

    assert!(stream.is_p2p());
    assert!(start.elapsed() < Duration::from_millis(300));
}

#[tokio::test]
async fn test_three_tier_tier1_timeout_failover_to_tier2_p2p() {
    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = p2p_listener.accept().await {
            let _ = stream.write_all(b"p2p_after_timeout").await;
        }
    });

    let mock_p2p = Arc::new(MockP2pDialer::new(p2p_addr, Duration::from_millis(10)));

    // Set direct timeout to 400ms for test speed
    let dialer = FallbackDialer::new(None)
        .with_p2p_dialer(mock_p2p)
        .with_direct_timeout(Duration::from_millis(400));

    let start = Instant::now();
    // 198.51.100.1 is RFC 5737 TEST-NET-2 unroutable blackhole IP
    let stream = dialer
        .dial_with_p2p_fallback(Some("198.51.100.1:53317"), None, None)
        .await
        .expect("Fallback to Tier 2 P2P after timeout must succeed");

    let elapsed = start.elapsed();
    assert!(stream.is_p2p());
    assert!(
        elapsed >= Duration::from_millis(390),
        "Must wait for Tier 1 direct timeout (got {:?})",
        elapsed
    );
    assert!(
        elapsed < Duration::from_millis(800),
        "P2P fallback must succeed promptly after timeout"
    );

    let mut stream = stream;
    let mut buf = vec![0u8; 17];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"p2p_after_timeout");
}

#[tokio::test]
async fn test_three_tier_full_failover_to_tier3_derp_relay() {
    // 1. Setup Mock DERP Relay and clients
    let relay = MockDerpRelay::start().await.unwrap();
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .unwrap();
    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .unwrap();

    let client_a_key = *client_a.local_key();
    let listener_b = client_b.listener();

    tokio::spawn(async move {
        let (mut derp_stream, remote_peer) = listener_b.accept().await.unwrap();
        assert_eq!(&remote_peer, &client_a_key);
        let mut buf = vec![0u8; 8];
        derp_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"tier3_in");
        derp_stream.write_all(b"tier3_out").await.unwrap();
        derp_stream.flush().await.unwrap();
    });

    // 2. Setup failing P2P dialer (Tier 2 fails)
    let failing_p2p = Arc::new(MockP2pDialer::failing(Duration::from_millis(20)));

    // 3. Configure FallbackDialer with failing Tier 1 and failing Tier 2
    let dialer = FallbackDialer::new(Some(client_a.clone()))
        .with_p2p_dialer(failing_p2p)
        .with_direct_timeout(Duration::from_millis(300));

    let start = Instant::now();
    // Port 1 gives fast ECONNREFUSED (< 10ms), P2P fails in 20ms, Tier 3 DERP opens in ~50ms
    let stream = dialer
        .dial_with_p2p_fallback(Some("127.0.0.1:1"), Some("peer_b"), Some(client_b.local_key()))
        .await
        .expect("Full 3-tier fallback to DERP relay must succeed");

    let elapsed = start.elapsed();
    assert!(stream.is_relayed(), "Stream must be Relayed via DERP");
    assert_eq!(stream.tier(), TransportTier::WanRelay);
    assert_eq!(stream.tier().label(), "Cloud Relay");
    assert_eq!(stream.tier().to_string(), "Cloud Relay");

    // Total failover SLA check: fast refusal (< 300ms) + fast P2P fail (< 50ms) + DERP stream (< 200ms) < 1.5s SLA
    assert!(
        elapsed < Duration::from_millis(1500),
        "Full failover to DERP on refused LAN must complete within < 1.5s SLA (got {:?})",
        elapsed
    );

    let mut stream = stream;
    stream.write_all(b"tier3_in").await.unwrap();
    stream.flush().await.unwrap();

    let mut buf = vec![0u8; 9];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"tier3_out");

    relay.stop().await;
}

#[tokio::test]
async fn test_three_tier_all_transports_exhausted_error() {
    let failing_p2p = Arc::new(MockP2pDialer::failing(Duration::from_millis(5)));
    let dialer = FallbackDialer::new(None)
        .with_p2p_dialer(failing_p2p)
        .with_direct_timeout(Duration::from_millis(100));

    let err = dialer
        .dial_with_p2p_fallback(Some("127.0.0.1:1"), Some("peer_none"), None)
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("All 3 transport tiers failed"),
        "Error must clearly indicate exhaustion of all 3 tiers: {err}"
    );
}
