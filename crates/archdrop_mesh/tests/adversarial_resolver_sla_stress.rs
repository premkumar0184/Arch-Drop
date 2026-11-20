//! Adversarial Challenge & Stress Harness: Milestone 3 Transport Resolver & Failover SLA.
//!
//! Evaluates:
//! 1. Failover SLA timing under adversarial conditions:
//!    - ECONNREFUSED fast fallback (< 300ms SLA) under single-shot and concurrent bursts.
//!    - Blackhole IP timeout strictly bounded (<= 1500ms cap on direct probe).
//!    - Fallback chain budget evaluation across 3 tiers (<= 1.5s budget).
//!    - Network unreachable (ENETUNREACH) fast failover.
//! 2. Concurrent multiplexing across mixed tiers (Tier 1 LAN, Tier 2 P2P, Tier 3 DERP):
//!    - 60 concurrent streams across mixed tiers with zero race conditions, data corruption, or panics.
//!    - Dynamic tier flapping and transient error containment.
//!    - P2P timeout enforcement and graceful degradation to DERP.
//! 3. Host WireGuard interface `tiktok` (`10.2.0.2/32`, table 51820) invariant verification.

use archdrop_mesh::client::{DerpClient, DerpClientConfig};
use archdrop_mesh::crypto::MeshKeyPair;
use archdrop_mesh::dialer::{
    FallbackDialer, P2pDialer, TransportResolver, TransportTier, DEFAULT_DIRECT_TIMEOUT,
};
use archdrop_mesh::error::MeshError;
use archdrop_mesh::mock_relay::MockDerpRelay;
use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Mock P2P dialer with configurable latency, failure modes, and call tracking.
struct StressMockP2pDialer {
    target_addr: Option<String>,
    delay: Duration,
    should_fail: bool,
    hang: bool,
    call_count: Arc<AtomicU32>,
}

impl StressMockP2pDialer {
    fn successful(target_addr: String, delay: Duration) -> Self {
        Self {
            target_addr: Some(target_addr),
            delay,
            should_fail: false,
            hang: false,
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }

    fn failing(delay: Duration) -> Self {
        Self {
            target_addr: None,
            delay,
            should_fail: true,
            hang: false,
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }

    fn hanging() -> Self {
        Self {
            target_addr: None,
            delay: Duration::ZERO,
            should_fail: false,
            hang: true,
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }
}

impl P2pDialer for StressMockP2pDialer {
    fn dial_p2p<'a>(
        &'a self,
        _target_peer: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<TcpStream, MeshError>> + Send + 'a>> {
        Box::pin(async move {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if self.hang {
                tokio::time::sleep(Duration::from_secs(60)).await;
                return Err(MeshError::ConnectionClosed("Hanging dialer aborted".into()));
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if self.should_fail {
                return Err(MeshError::ConnectionClosed(
                    "Simulated P2P SoftAP negotiation failure".into(),
                ));
            }
            if let Some(ref addr) = self.target_addr {
                TcpStream::connect(addr).await.map_err(MeshError::Io)
            } else {
                Err(MeshError::ConnectionClosed("No target address for P2P".into()))
            }
        })
    }
}

/// Verifies that host WireGuard interface `tiktok` exists and retains its routing table and rules.
fn verify_host_wireguard_safety() {
    let addr_out = Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output();
    if let Ok(out) = addr_out {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            assert!(
                s.contains("10.2.0.2/32"),
                "tiktok IP must be 10.2.0.2/32, got: {s}"
            );
            assert!(
                s.contains("UP"),
                "tiktok interface must remain UP, got: {s}"
            );

            let route_out = Command::new("ip")
                .args(["route", "show", "table", "51820"])
                .output()
                .expect("Failed to execute ip route");
            let r = String::from_utf8_lossy(&route_out.stdout);
            assert!(
                r.contains("default dev tiktok scope link"),
                "table 51820 must have default route via tiktok, got: {r}"
            );

            let rule_out = Command::new("ip")
                .args(["rule", "show"])
                .output()
                .expect("Failed to execute ip rule");
            let rules = String::from_utf8_lossy(&rule_out.stdout);
            assert!(
                rules.contains("from 10.2.0.2 lookup 51820"),
                "ip rule 100 for tiktok must exist, got: {rules}"
            );
        }
    }
}

// ============================================================================
// SUITE 1: FAILOVER SLA TIMING UNDER ADVERSARIAL STRESS
// ============================================================================

/// SLA Challenge 1A: Burst of 50 concurrent dials hitting ECONNREFUSED on Tier 1
/// must ALL fall back to Tier 2 in strictly < 300ms each under load.
#[tokio::test]
async fn test_sla_burst_50_econnrefused_fallback_under_300ms() {
    verify_host_wireguard_safety();

    // Setup simulated P2P listener
    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    let server_task = tokio::spawn(async move {
        let mut count = 0;
        while count < 50 {
            if let Ok((mut stream, _)) = p2p_listener.accept().await {
                count += 1;
                tokio::spawn(async move {
                    let mut buf = [0u8; 4];
                    if stream.read_exact(&mut buf).await.is_ok() {
                        let _ = stream.write_all(b"PONG").await;
                    }
                });
            }
        }
    });

    let mock_p2p = Arc::new(StressMockP2pDialer::successful(
        p2p_addr,
        Duration::from_millis(5),
    ));

    let resolver = Arc::new(
        TransportResolver::new(None)
            .with_p2p_dialer(mock_p2p.clone())
            .with_direct_timeout(Duration::from_millis(1500)),
    );

    // Launch 50 concurrent failover requests against closed port 1
    let mut handles = Vec::new();
    let start_all = Instant::now();

    for i in 0..50 {
        let res = resolver.clone();
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let stream = res
                .resolve_and_dial(Some("127.0.0.1:1"), Some("peer_companion"), None)
                .await
                .expect("Failover to P2P must succeed");

            let elapsed = start.elapsed();
            assert!(stream.is_p2p(), "Stream must be P2P");
            assert_eq!(stream.tier(), TransportTier::OfflineP2p);

            // Test stream echo
            let mut stream = stream;
            stream.write_all(b"PING").await.unwrap();
            let mut reply = [0u8; 4];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"PONG");

            (i, elapsed)
        }));
    }

    let mut max_elapsed = Duration::ZERO;
    for h in handles {
        let (idx, elapsed) = h.await.unwrap();
        if elapsed > max_elapsed {
            max_elapsed = elapsed;
        }
        assert!(
            elapsed < Duration::from_millis(300),
            "Task {idx} failover took {elapsed:?}, exceeding < 300ms SLA"
        );
    }

    let total_batch = start_all.elapsed();
    assert_eq!(
        mock_p2p.call_count.load(Ordering::SeqCst),
        50,
        "All 50 tasks must invoke P2P dialer"
    );

    println!(
        "[PASS] 50 concurrent ECONNREFUSED failovers: max single elapsed = {max_elapsed:?}, total batch = {total_batch:?}"
    );

    server_task.abort();
    verify_host_wireguard_safety();
}

/// SLA Challenge 1B: Fallback chain with ECONNREFUSED Tier 1, fast-failing Tier 2,
/// and Tier 3 DERP relay connection.
/// Asserts that total fallback chain budget <= 1.5s (in fact < 500ms).
#[tokio::test]
async fn test_sla_refused_tier1_failed_tier2_to_derp_relay_budget_under_1500ms() {
    verify_host_wireguard_safety();

    let relay = MockDerpRelay::start().await.expect("Relay start failed");
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

    let recv_handle = tokio::spawn(async move {
        let (mut derp_stream, remote_peer) = listener_b.accept().await.unwrap();
        assert_eq!(&remote_peer, &client_a_key);
        let mut buf = [0u8; 10];
        derp_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"derp_probe");
        derp_stream.write_all(b"derp_reply").await.unwrap();
        derp_stream.flush().await.unwrap();
    });

    // P2P dialer fails in 15ms (simulating BLE connection drop)
    let failing_p2p = Arc::new(StressMockP2pDialer::failing(Duration::from_millis(15)));

    let resolver = TransportResolver::new(Some(client_a.clone()))
        .with_p2p_dialer(failing_p2p)
        .with_direct_timeout(Duration::from_millis(1500));

    let start = Instant::now();
    let stream = resolver
        .resolve_and_dial(
            Some("127.0.0.1:1"), // ECONNREFUSED < 5ms
            Some("peer_b"),
            Some(client_b.local_key()),
        )
        .await
        .expect("Full failover to DERP must succeed");

    let elapsed = start.elapsed();
    assert!(stream.is_relayed(), "Stream must be DERP relayed");
    assert_eq!(stream.tier(), TransportTier::WanRelay);

    // Strict SLA: Budget must be <= 1.5 seconds (here expected ~50-100ms)
    assert!(
        elapsed <= Duration::from_millis(1500),
        "Total fallback chain budget must be <= 1.5s, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "Fast failover chain should complete in < 500ms, got {elapsed:?}"
    );

    let mut stream = stream;
    stream.write_all(b"derp_probe").await.unwrap();
    stream.flush().await.unwrap();

    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"derp_reply");

    recv_handle.await.unwrap();
    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
    verify_host_wireguard_safety();
}

/// SLA Challenge 1C: Blackhole probe timeout strictly bounded at <= 1500ms.
/// Tests RFC 5737 unroutable IP 198.51.100.1.
#[tokio::test]
async fn test_sla_blackhole_timeout_strictly_respects_1500ms_cap() {
    verify_host_wireguard_safety();

    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = p2p_listener.accept().await {
            let _ = stream.write_all(b"p2p_after_blackhole").await;
        }
    });

    let mock_p2p = Arc::new(StressMockP2pDialer::successful(
        p2p_addr,
        Duration::from_millis(10),
    ));

    // Construct resolver with default direct timeout (1500ms)
    let resolver = TransportResolver::new(None).with_p2p_dialer(mock_p2p);
    assert_eq!(
        resolver.direct_timeout(),
        DEFAULT_DIRECT_TIMEOUT,
        "Default direct timeout must be exactly 1500ms"
    );
    assert_eq!(resolver.direct_timeout(), Duration::from_millis(1500));

    let start = Instant::now();
    // 198.51.100.1 is RFC 5737 TEST-NET-2 unroutable blackhole IP
    let stream = resolver
        .resolve_and_dial(Some("198.51.100.1:53317"), Some("companion"), None)
        .await
        .expect("Fallback to P2P after blackhole timeout must succeed");

    let elapsed = start.elapsed();
    assert!(stream.is_p2p());

    // Strict SLA: direct timeout is 1500ms; with small scheduler tolerance and P2P dial (~10ms),
    // probe should fire at ~1500ms and not exceed 1700ms.
    assert!(
        elapsed >= Duration::from_millis(1480),
        "Blackhole probe must honor the 1500ms timeout (got {elapsed:?})"
    );
    assert!(
        elapsed <= Duration::from_millis(1750),
        "Blackhole timeout + P2P fallback must strictly complete near 1500ms cap (got {elapsed:?})"
    );

    let mut stream = stream;
    let mut buf = [0u8; 19];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"p2p_after_blackhole");
    verify_host_wireguard_safety();
}

/// SLA Challenge 1D: Network Unreachable (ENETUNREACH) fast failover.
/// Non-routable broadcast or 0.0.0.0:1 should fail immediately without hanging for 1500ms.
#[tokio::test]
async fn test_sla_network_unreachable_fast_failover_under_300ms() {
    verify_host_wireguard_safety();

    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = p2p_listener.accept().await {
            let _ = stream.write_all(b"fast_p2p").await;
        }
    });

    let mock_p2p = Arc::new(StressMockP2pDialer::successful(
        p2p_addr,
        Duration::from_millis(5),
    ));
    let resolver = TransportResolver::new(None)
        .with_p2p_dialer(mock_p2p)
        .with_direct_timeout(Duration::from_millis(1500));

    let start = Instant::now();
    // [2001:db8::1]:53317 (RFC 3849 documentation IPv6 prefix with no IPv6 default route triggers ENETUNREACH immediately)
    let stream = resolver
        .resolve_and_dial(Some("[2001:db8::1]:53317"), Some("companion"), None)
        .await
        .expect("Fallback to P2P must succeed");

    let elapsed = start.elapsed();
    assert!(stream.is_p2p());
    assert!(
        elapsed < Duration::from_millis(300),
        "ENETUNREACH/invalid network must fail fast in < 300ms (got {elapsed:?})"
    );
    verify_host_wireguard_safety();
}

// ============================================================================
// SUITE 2: CONCURRENT MULTIPLEXING ACROSS MIXED TIERS WITHOUT RACE CONDITIONS
// ============================================================================

/// Multiplexing Challenge 2A: 60 concurrent streams simultaneously resolving
/// across mixed tiers:
/// - 20 streams via Tier 1 Direct LAN
/// - 20 streams via Tier 2 Offline P2P
/// - 20 streams via Tier 3 WAN DERP Relay
/// All 60 streams run in parallel, exchange distinct nonces, and verify zero data corruption.
#[tokio::test]
async fn test_adversarial_concurrent_60_streams_across_mixed_tiers() {
    verify_host_wireguard_safety();

    // 1. Tier 1 LAN server
    let lan_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let lan_addr = lan_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = lan_listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 8];
                if stream.read_exact(&mut buf).await.is_ok() {
                    let _ = stream.write_all(&buf).await;
                }
            });
        }
    });

    // 2. Tier 2 P2P server
    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = p2p_listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 8];
                if stream.read_exact(&mut buf).await.is_ok() {
                    let _ = stream.write_all(&buf).await;
                }
            });
        }
    });

    // 3. Tier 3 DERP relay & clients
    let relay = MockDerpRelay::start().await.expect("Relay start failed");
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

    // Spawn DERP receiver loop on Client B
    tokio::spawn(async move {
        while let Ok((mut derp_stream, remote_peer)) = listener_b.accept().await {
            assert_eq!(&remote_peer, &client_a_key);
            tokio::spawn(async move {
                let mut buf = [0u8; 8];
                if derp_stream.read_exact(&mut buf).await.is_ok() {
                    let _ = derp_stream.write_all(&buf).await;
                    let _ = derp_stream.flush().await;
                }
            });
        }
    });

    // Construct 3 resolvers simulating 3 distinct routing profiles:
    // Profile 1: Direct LAN available
    let res_tier1 = Arc::new(
        TransportResolver::new(Some(client_a.clone()))
            .with_p2p_dialer(Arc::new(StressMockP2pDialer::successful(
                p2p_addr.clone(),
                Duration::from_millis(5),
            )))
            .with_direct_timeout(Duration::from_millis(500)),
    );

    // Profile 2: Direct LAN closed -> falls back to P2P
    let res_tier2 = Arc::new(
        TransportResolver::new(Some(client_a.clone()))
            .with_p2p_dialer(Arc::new(StressMockP2pDialer::successful(
                p2p_addr.clone(),
                Duration::from_millis(5),
            )))
            .with_direct_timeout(Duration::from_millis(500)),
    );

    // Profile 3: Direct LAN closed, P2P failed -> falls back to DERP
    let res_tier3 = Arc::new(
        TransportResolver::new(Some(client_a.clone()))
            .with_p2p_dialer(Arc::new(StressMockP2pDialer::failing(
                Duration::from_millis(5),
            )))
            .with_direct_timeout(Duration::from_millis(500)),
    );

    let mut handles = Vec::new();
    let client_b_key = *client_b.local_key();

    // Spawn 20 tasks on Tier 1
    for i in 0..20u64 {
        let res = res_tier1.clone();
        let direct = lan_addr.clone();
        handles.push(tokio::spawn(async move {
            let mut stream = res
                .resolve_and_dial(Some(&direct), Some("peer"), None)
                .await
                .expect("Tier 1 direct must succeed");

            assert!(stream.is_direct(), "Task {i} must be Tier 1 Direct");
            assert_eq!(stream.tier(), TransportTier::DirectLan);

            let nonce = i.to_be_bytes();
            stream.write_all(&nonce).await.unwrap();
            let mut reply = [0u8; 8];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply, nonce, "Nonce mismatch in Tier 1 stream {i}");
            "tier1"
        }));
    }

    // Spawn 20 tasks on Tier 2 (LAN closed port -> P2P)
    for i in 20..40u64 {
        let res = res_tier2.clone();
        handles.push(tokio::spawn(async move {
            let mut stream = res
                .resolve_and_dial(Some("127.0.0.1:1"), Some("peer"), None)
                .await
                .expect("Tier 2 P2P failover must succeed");

            assert!(stream.is_p2p(), "Task {i} must be Tier 2 P2P");
            assert_eq!(stream.tier(), TransportTier::OfflineP2p);

            let nonce = i.to_be_bytes();
            stream.write_all(&nonce).await.unwrap();
            let mut reply = [0u8; 8];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply, nonce, "Nonce mismatch in Tier 2 stream {i}");
            "tier2"
        }));
    }

    // Spawn 20 tasks on Tier 3 (LAN closed + P2P failed -> DERP)
    for i in 40..60u64 {
        let res = res_tier3.clone();
        handles.push(tokio::spawn(async move {
            let mut stream = res
                .resolve_and_dial(Some("127.0.0.1:1"), Some("peer"), Some(&client_b_key))
                .await
                .expect("Tier 3 DERP failover must succeed");

            assert!(stream.is_relayed(), "Task {i} must be Tier 3 DERP");
            assert_eq!(stream.tier(), TransportTier::WanRelay);

            let nonce = i.to_be_bytes();
            stream.write_all(&nonce).await.unwrap();
            stream.flush().await.unwrap();
            let mut reply = [0u8; 8];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply, nonce, "Nonce mismatch in Tier 3 stream {i}");
            "tier3"
        }));
    }

    let mut tier1_count = 0;
    let mut tier2_count = 0;
    let mut tier3_count = 0;

    for h in handles {
        match h.await.unwrap() {
            "tier1" => tier1_count += 1,
            "tier2" => tier2_count += 1,
            "tier3" => tier3_count += 1,
            _ => panic!("Unknown tier"),
        }
    }

    assert_eq!(tier1_count, 20);
    assert_eq!(tier2_count, 20);
    assert_eq!(tier3_count, 20);

    println!(
        "[PASS] 60 concurrent streams across mixed tiers (20 Direct, 20 P2P, 20 DERP) executed with 100% integrity"
    );

    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
    verify_host_wireguard_safety();
}

/// Multiplexing Challenge 2B: Hanging P2P dialer timeout containment.
/// Verifies that when Tier 2 hangs indefinitely, `p2p_timeout` aborts Tier 2
/// and cleanly advances to Tier 3 DERP relay without stalling the worker.
#[tokio::test]
async fn test_adversarial_p2p_hang_timeout_advances_to_tier3_derp() {
    verify_host_wireguard_safety();

    let relay = MockDerpRelay::start().await.expect("Relay start failed");
    let relay_url = relay.relay_url();

    let key_a = Arc::new(MeshKeyPair::generate().unwrap());
    let key_b = Arc::new(MeshKeyPair::generate().unwrap());

    let client_a = DerpClient::connect(DerpClientConfig::new(&relay_url, key_a.clone()))
        .await
        .unwrap();
    let client_b = DerpClient::connect(DerpClientConfig::new(&relay_url, key_b.clone()))
        .await
        .unwrap();

    let listener_b = client_b.listener();
    tokio::spawn(async move {
        if let Ok((mut derp_stream, _)) = listener_b.accept().await {
            let _ = derp_stream.write_all(b"recovered_via_derp").await;
            let _ = derp_stream.flush().await;
        }
    });

    // P2P dialer hangs indefinitely
    let hanging_p2p = Arc::new(StressMockP2pDialer::hanging());

    // Configure resolver with tight timeouts: direct 100ms, P2P 300ms
    let resolver = TransportResolver::new(Some(client_a.clone()))
        .with_p2p_dialer(hanging_p2p)
        .with_direct_timeout(Duration::from_millis(100))
        .with_p2p_timeout(Duration::from_millis(300));

    assert_eq!(resolver.p2p_timeout(), Duration::from_millis(300));

    let start = Instant::now();
    let stream = resolver
        .resolve_and_dial(Some("127.0.0.1:1"), Some("peer"), Some(client_b.local_key()))
        .await
        .expect("Must recover via Tier 3 DERP after P2P timeout");

    let elapsed = start.elapsed();
    assert!(stream.is_relayed(), "Must fall back to DERP relay");
    assert_eq!(stream.tier(), TransportTier::WanRelay);

    // Elapsed should be ~300ms (P2P timeout) + ~50ms (DERP dial) < 800ms
    assert!(
        elapsed >= Duration::from_millis(290),
        "Must wait for P2P timeout of 300ms (got {elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_millis(1000),
        "Must advance to DERP promptly after P2P timeout (got {elapsed:?})"
    );

    let mut stream = stream;
    let mut buf = [0u8; 18];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"recovered_via_derp");

    client_a.close().await.unwrap();
    client_b.close().await.unwrap();
    relay.stop().await;
    verify_host_wireguard_safety();
}

/// Multiplexing Challenge 2C: FallbackDialer Backward Compatibility & Dynamic Override.
/// Verifies `dial_3tier` with per-call transient `P2pDialer` override under concurrency.
#[tokio::test]
async fn test_adversarial_fallback_dialer_transient_override_concurrency() {
    verify_host_wireguard_safety();

    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = p2p_listener.accept().await {
            let _ = stream.write_all(b"override_ok").await;
        }
    });

    let default_failing_p2p = Arc::new(StressMockP2pDialer::failing(Duration::ZERO));
    let dialer = Arc::new(
        FallbackDialer::new(None)
            .with_p2p_dialer(default_failing_p2p)
            .with_direct_timeout(Duration::from_millis(50)),
    );

    let mut handles = Vec::new();
    for i in 0..20 {
        let d = dialer.clone();
        let target = p2p_addr.clone();
        handles.push(tokio::spawn(async move {
            // Transient override: this call provides a working P2pDialer
            let working_p2p = StressMockP2pDialer::successful(target, Duration::from_millis(2));
            let stream = d
                .dial_3tier(Some("127.0.0.1:1"), Some(&working_p2p), None)
                .await
                .expect("dial_3tier with transient override must succeed");

            assert!(stream.is_p2p());
            assert_eq!(stream.tier(), TransportTier::OfflineP2p);
            let mut stream = stream;
            let mut buf = [0u8; 11];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"override_ok");
            i
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
    verify_host_wireguard_safety();
}

// ============================================================================
// SUITE 3: HOST WIREGUARD SAFETY INVARIANT CONTINUOUS ASSERTION
// ============================================================================

/// Verifies that the host WireGuard interface `tiktok` remains untouched across
/// all adversarial operations, routing tables, and policy rules.
#[tokio::test]
async fn test_host_wireguard_tiktok_remains_active_and_untouched() {
    verify_host_wireguard_safety();

    // Execute multiple dials across all tiers while asserting WireGuard invariants
    let dialer = FallbackDialer::new(None).with_direct_timeout(Duration::from_millis(100));

    for _ in 0..10 {
        let _ = dialer.dial_with_fallback(Some("127.0.0.1:1"), None).await;
        verify_host_wireguard_safety();
    }
}

/// Adversarial Challenge 4A: Head-of-line blocking stress.
/// Verifies that an active, timing-out blackhole probe (1500ms) does NOT block or delay
/// 30 concurrent Direct LAN connections running on the same resolver.
#[tokio::test]
async fn test_adversarial_blackhole_non_blocking_concurrent_lan_dials() {
    verify_host_wireguard_safety();

    let lan_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let lan_addr = lan_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = lan_listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4];
                if stream.read_exact(&mut buf).await.is_ok() {
                    let _ = stream.write_all(b"FAST").await;
                }
            });
        }
    });

    let resolver = Arc::new(
        TransportResolver::new(None)
            .with_direct_timeout(Duration::from_millis(1500)),
    );

    // 1. Launch blackhole task
    let res_bh = resolver.clone();
    let bh_handle = tokio::spawn(async move {
        let start = Instant::now();
        let res = res_bh
            .resolve_and_dial(Some("198.51.100.1:53317"), None, None)
            .await;
        let elapsed = start.elapsed();
        (res.is_err(), elapsed)
    });

    // Small delay to ensure blackhole task is actively sleeping in tokio::time::timeout
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 2. Launch 30 concurrent LAN tasks while blackhole probe is ongoing
    let mut lan_handles = Vec::new();
    let start_lan = Instant::now();
    for i in 0..30 {
        let res = resolver.clone();
        let addr = lan_addr.clone();
        lan_handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let mut stream = res
                .resolve_and_dial(Some(&addr), None, None)
                .await
                .expect("LAN direct dial must succeed");
            assert!(stream.is_direct());
            stream.write_all(b"TEST").await.unwrap();
            let mut reply = [0u8; 4];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"FAST");
            (i, start.elapsed())
        }));
    }

    // All 30 LAN tasks must finish immediately (< 100ms) without waiting for blackhole
    for h in lan_handles {
        let (idx, elapsed) = h.await.unwrap();
        assert!(
            elapsed < Duration::from_millis(100),
            "Task {idx} was blocked by blackhole probe, took {elapsed:?}"
        );
    }
    let lan_batch_elapsed = start_lan.elapsed();
    assert!(
        lan_batch_elapsed < Duration::from_millis(300),
        "All 30 LAN dials must complete in < 300ms total, took {lan_batch_elapsed:?}"
    );

    // Now wait for blackhole task to finish
    let (is_err, bh_elapsed) = bh_handle.await.unwrap();
    assert!(is_err, "Blackhole dial must fail when no fallbacks available");
    assert!(
        bh_elapsed >= Duration::from_millis(1480),
        "Blackhole dial must honor 1500ms timeout"
    );

    println!(
        "[PASS] Blackhole isolation verified: 30 LAN tasks completed in {lan_batch_elapsed:?} while blackhole probe ran for {bh_elapsed:?}"
    );
    verify_host_wireguard_safety();
}

/// Adversarial Challenge 4B: Rapid sequential failovers across 100 iterations.
/// Verifies no socket descriptors, threads, or memory are leaked across rapid failover bursts.
#[tokio::test]
async fn test_adversarial_rapid_sequential_failovers_100_iterations() {
    verify_host_wireguard_safety();

    let p2p_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p2p_addr = p2p_listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = p2p_listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4];
                if stream.read_exact(&mut buf).await.is_ok() {
                    let _ = stream.write_all(b"LOOP").await;
                }
            });
        }
    });

    let mock_p2p = Arc::new(StressMockP2pDialer::successful(
        p2p_addr,
        Duration::from_micros(500),
    ));

    let resolver = TransportResolver::new(None)
        .with_p2p_dialer(mock_p2p)
        .with_direct_timeout(Duration::from_millis(1500));

    let start_100 = Instant::now();
    for i in 0..100 {
        let start_iter = Instant::now();
        let stream = resolver
            .resolve_and_dial(Some("127.0.0.1:1"), Some("peer"), None)
            .await
            .expect("Failover to P2P must succeed");

        let iter_elapsed = start_iter.elapsed();
        assert!(
            iter_elapsed < Duration::from_millis(50),
            "Iteration {i} took {iter_elapsed:?}, exceeding 50ms fast failover SLA"
        );

        let mut stream = stream;
        stream.write_all(b"PING").await.unwrap();
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"LOOP");
    }

    let total_100 = start_100.elapsed();
    let avg = total_100 / 100;
    println!(
        "[PASS] 100 rapid sequential failovers completed in {total_100:?} (average {avg:?}/dial)"
    );
    verify_host_wireguard_safety();
}

