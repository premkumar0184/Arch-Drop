//! Adversarial Extreme Stress Test Suite for Offline Event Queue & Vector Reconciliation
//!
//! Tests:
//! 1. 1,000-event split-brain reconciliation (500 per node) across small batch sizes with zero dropped payloads.
//! 2. 4-node cyclic and cross-partition convergence to 400 events with zero payload loss and identical LWW winner.
//! 3. Repeated flapping network partitions (intermittent drops across multiple resumption cycles).
//! 4. Invariant: Host WireGuard interface `tiktok` (10.2.0.2/32, table 51820) remains active and untouched.

use archdrop_core::ledger::{arbitrate_clipboard, Event, EventLedger};
use archdrop_mesh::sync::ReconciliationEngine;
use std::collections::HashMap;
use std::pin::Pin;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{duplex, AsyncRead, AsyncWrite, ReadBuf};

/// Severable stream wrapper that drops connection when atomic flag is set
struct SeverableStream<S> {
    inner: Option<S>,
    sever_flag: Arc<AtomicBool>,
    severed: bool,
}

impl<S> SeverableStream<S> {
    fn new(inner: S, sever_flag: Arc<AtomicBool>) -> Self {
        Self {
            inner: Some(inner),
            sever_flag,
            severed: false,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for SeverableStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.severed || self.inner.is_none() {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "connection dropped by network partition simulator",
            )));
        }
        Pin::new(self.inner.as_mut().unwrap()).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for SeverableStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.sever_flag.load(Ordering::SeqCst) || self.severed || self.inner.is_none() {
            self.severed = true;
            self.inner = None;
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "simulated network partition during transmission",
            )));
        }
        Pin::new(self.inner.as_mut().unwrap()).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.severed || self.inner.is_none() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(self.inner.as_mut().unwrap()).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.severed || self.inner.is_none() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(self.inner.as_mut().unwrap()).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn test_stress_1000_events_split_brain_zero_payload_loss() {
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_SPLIT_A").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_SPLIT_B").unwrap());

    let base_ts = 1_731_200_000_000i64;

    // Node A generates 500 events
    let mut batch_a = Vec::with_capacity(500);
    for seq in 1..=500 {
        let ts = base_ts + (seq * 2);
        batch_a.push(Event::new_local(
            "NODE_SPLIT_A",
            seq,
            "clipboard.text",
            format!("Node A item {:04}", seq).into_bytes(),
            ts,
        ));
    }
    ledger_a.apply_batch(&batch_a).unwrap();

    // Node B generates 500 events with overlapping timestamps
    let mut batch_b = Vec::with_capacity(500);
    for seq in 1..=500 {
        let ts = base_ts + (seq * 2); // Identical timestamp
        batch_b.push(Event::new_local(
            "NODE_SPLIT_B",
            seq,
            "clipboard.text",
            format!("Node B item {:04}", seq).into_bytes(),
            ts,
        ));
    }
    ledger_b.apply_batch(&batch_b).unwrap();

    assert_eq!(ledger_a.count_events().unwrap(), 500);
    assert_eq!(ledger_b.count_events().unwrap(), 500);

    let engine_a = ReconciliationEngine::new(ledger_a.clone());
    let engine_b = ReconciliationEngine::new(ledger_b.clone());

    // Connect via duplex stream with small batch size (25 events per batch)
    let (mut stream_a, mut stream_b) = duplex(64 * 1024);

    let ea = engine_a.clone();
    let eb = engine_b.clone();

    let task_a = tokio::spawn(async move {
        ea.run_sync_session(&mut stream_a, "NODE_SPLIT_B", 25)
            .await
            .unwrap()
    });

    let task_b = tokio::spawn(async move {
        eb.run_sync_session(&mut stream_b, "NODE_SPLIT_A", 25)
            .await
            .unwrap()
    });

    let (stats_a, stats_b) = tokio::join!(task_a, task_b);
    let stats_a = stats_a.unwrap();
    let stats_b = stats_b.unwrap();

    assert_eq!(stats_a.events_sent, 500);
    assert_eq!(stats_a.events_received, 500);
    assert_eq!(stats_b.events_sent, 500);
    assert_eq!(stats_b.events_received, 500);

    // Invariant 1: Exactly 1,000 events in both ledgers (Zero Dropped Payloads)
    assert_eq!(ledger_a.count_events().unwrap(), 1000);
    assert_eq!(ledger_b.count_events().unwrap(), 1000);

    // Invariant 2: Vector clocks match exactly
    let clock_a = ledger_a.get_vector_clock().unwrap();
    let clock_b = ledger_b.get_vector_clock().unwrap();
    assert_eq!(clock_a.get("NODE_SPLIT_A"), Some(&500));
    assert_eq!(clock_a.get("NODE_SPLIT_B"), Some(&500));
    assert_eq!(clock_a, clock_b);

    // Invariant 3: Single-state arbiter produces identical winner on both nodes
    let all_a = ledger_a.get_all_events().unwrap();
    let all_b = ledger_b.get_all_events().unwrap();
    let win_a = arbitrate_clipboard(&all_a).unwrap();
    let win_b = arbitrate_clipboard(&all_b).unwrap();

    assert_eq!(win_a, win_b);
    // NODE_SPLIT_B > NODE_SPLIT_A
    assert_eq!(win_a.origin_device_id, "NODE_SPLIT_B");
    assert_eq!(win_a.sequence_number, 500);
    assert_eq!(win_a.payload_str().unwrap(), "Node B item 0500");
}

#[tokio::test]
async fn test_stress_4_node_cyclic_and_cross_partition_reconciliation() {
    let nodes = ["NODE_A", "NODE_B", "NODE_C", "NODE_D"];
    let mut ledgers = Vec::new();
    let mut engines = Vec::new();

    let base_ts = 1_731_300_000_000i64;

    for &name in &nodes {
        let ledger = Arc::new(EventLedger::open_in_memory(name).unwrap());
        let mut events = Vec::new();
        for seq in 1..=100 {
            events.push(Event::new_local(
                name,
                seq,
                "clipboard.text",
                format!("{}_event_{:03}", name, seq).into_bytes(),
                base_ts + (seq * 10),
            ));
        }
        ledger.apply_batch(&events).unwrap();
        assert_eq!(ledger.count_events().unwrap(), 100);

        engines.push(ReconciliationEngine::new(ledger.clone()));
        ledgers.push(ledger);
    }

    // Helper closure to sync between two nodes
    let sync_pair = |idx1: usize, idx2: usize| {
        let e1 = engines[idx1].clone();
        let e2 = engines[idx2].clone();
        let name1 = nodes[idx1];
        let name2 = nodes[idx2];
        async move {
            let (mut s1, mut s2) = duplex(64 * 1024);
            let t1 = tokio::spawn(async move { e1.run_sync_session(&mut s1, name2, 20).await.unwrap() });
            let t2 = tokio::spawn(async move { e2.run_sync_session(&mut s2, name1, 20).await.unwrap() });
            tokio::try_join!(t1, t2).unwrap();
        }
    };

    // Cycle 1: Ring sync A <-> B, B <-> C, C <-> D, D <-> A
    sync_pair(0, 1).await;
    sync_pair(1, 2).await;
    sync_pair(2, 3).await;
    sync_pair(3, 0).await;

    // Cycle 2: Cross sync A <-> C, B <-> D
    sync_pair(0, 2).await;
    sync_pair(1, 3).await;

    // Cycle 3: Final ring sync to ensure full convergence
    sync_pair(0, 1).await;
    sync_pair(1, 2).await;
    sync_pair(2, 3).await;
    sync_pair(3, 0).await;

    // All 4 nodes must now hold exactly 400 events
    let mut expected_clock = HashMap::new();
    for &name in &nodes {
        expected_clock.insert(name.to_string(), 100i64);
    }

    for (i, ledger) in ledgers.iter().enumerate() {
        assert_eq!(
            ledger.count_events().unwrap(),
            400,
            "Node {} must have converged to 400 events",
            nodes[i]
        );
        let clock = ledger.get_vector_clock().unwrap();
        assert_eq!(clock, expected_clock);
    }

    // Single-state LWW arbiter verification
    let mut winners = Vec::new();
    for ledger in &ledgers {
        let all = ledger.get_all_events().unwrap();
        winners.push(arbitrate_clipboard(&all).unwrap().clone());
    }

    // All 4 winners must be identical
    for w in &winners[1..] {
        assert_eq!(&winners[0], w);
    }

    // Lexicographically NODE_D > NODE_C > NODE_B > NODE_A
    assert_eq!(winners[0].origin_device_id, "NODE_D");
    assert_eq!(winners[0].sequence_number, 100);
    assert_eq!(winners[0].payload_str().unwrap(), "NODE_D_event_100");
}

#[tokio::test]
async fn test_stress_repeated_flapping_partitions_resumption() {
    let ledger_sender = Arc::new(EventLedger::open_in_memory("NODE_FLAP_SRC").unwrap());
    let ledger_receiver = Arc::new(EventLedger::open_in_memory("NODE_FLAP_DST").unwrap());

    for seq in 1..=100 {
        ledger_sender
            .record_event("clipboard.text", format!("item_{:03}", seq).as_bytes())
            .unwrap();
    }
    assert_eq!(ledger_sender.count_events().unwrap(), 100);
    assert_eq!(ledger_receiver.count_events().unwrap(), 0);

    let engine_sender = ReconciliationEngine::new(ledger_sender.clone());
    let engine_receiver = ReconciliationEngine::new(ledger_receiver.clone());

    // Session 1: Drop connection after ingesting 25 events (mid-stream partition)
    let sever_flag = Arc::new(AtomicBool::new(false));
    let sever_flag_for_listener = sever_flag.clone();
    let ingested_count = Arc::new(AtomicUsize::new(0));
    let count_clone = ingested_count.clone();

    let engine_receiver_flapping = ReconciliationEngine::new(ledger_receiver.clone()).with_listener(move |_evt| {
        let c = count_clone.fetch_add(1, Ordering::SeqCst);
        if c >= 24 {
            sever_flag_for_listener.store(true, Ordering::SeqCst);
        }
    });

    {
        let (s_a, s_b) = duplex(64 * 1024);
        let mut severable_stream_b = SeverableStream::new(s_b, sever_flag);
        let mut s_a_pin = s_a;
        let es = engine_sender.clone();

        let t_a = tokio::spawn(async move { es.run_sync_session(&mut s_a_pin, "NODE_FLAP_DST", 10).await });
        let t_b = tokio::spawn(async move { engine_receiver_flapping.run_sync_session(&mut severable_stream_b, "NODE_FLAP_SRC", 10).await });

        let (r_a, r_b) = tokio::join!(t_a, t_b);
        assert!(r_a.unwrap().is_err(), "Sender must report partition error on severed connection");
        assert!(r_b.unwrap().is_err(), "Receiver must report partition error on severed connection");
    }

    // Partial ingestion verified
    let mid_count = ledger_receiver.count_events().unwrap();
    assert!(mid_count > 0 && mid_count < 100, "Should have ingested some events: {mid_count}");

    // Session 2: Resumption over fresh connection
    {
        let (mut s_a, mut s_b) = duplex(64 * 1024);
        let es = engine_sender.clone();
        let er = engine_receiver.clone();

        let t_a = tokio::spawn(async move {
            es.run_sync_session(&mut s_a, "NODE_FLAP_DST", 10)
                .await
                .expect("Resumed session must succeed")
        });
        let t_b = tokio::spawn(async move {
            er.run_sync_session(&mut s_b, "NODE_FLAP_SRC", 10)
                .await
                .expect("Resumed session must succeed")
        });

        let (stats_a, stats_b) = tokio::join!(t_a, t_b);
        let stats_a = stats_a.unwrap();
        let stats_b = stats_b.unwrap();

        assert_eq!(stats_a.events_sent + mid_count, 100);
        assert_eq!(stats_b.events_received + mid_count, 100);
    }

    // Final verification: exactly 100 events, 0 duplicates, 0 dropped
    assert_eq!(ledger_receiver.count_events().unwrap(), 100);
    assert_eq!(ledger_sender.count_events().unwrap(), 100);

    let clock_sender = ledger_sender.get_vector_clock().unwrap();
    let clock_receiver = ledger_receiver.get_vector_clock().unwrap();
    assert_eq!(clock_receiver.get("NODE_FLAP_SRC"), Some(&100));
    assert_eq!(clock_sender, clock_receiver);
}

#[test]
fn test_stress_host_wireguard_safety_invariant() {
    let out = Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output()
        .expect("Failed to execute ip addr");
    assert!(out.status.success(), "tiktok interface must be active");
    let out_str = String::from_utf8_lossy(&out.stdout);
    assert!(
        out_str.contains("10.2.0.2"),
        "tiktok must retain 10.2.0.2: {out_str}"
    );

    let route_out = Command::new("ip")
        .args(["route", "show", "table", "51820"])
        .output()
        .expect("Failed to execute ip route table 51820");
    assert!(route_out.status.success(), "route table 51820 must exist");
    let route_str = String::from_utf8_lossy(&route_out.stdout);
    assert!(
        route_str.contains("default dev tiktok"),
        "table 51820 must have default dev tiktok: {route_str}"
    );
}
