use archdrop_core::ledger::{arbitrate_clipboard, Event, EventLedger};
use archdrop_mesh::sync::{ReconciliationEngine, SyncBatch, WireEvent};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{duplex, AsyncRead, AsyncWrite, ReadBuf};

/// Stream wrapper that can sever the connection when a condition is met (e.g. drop before ack).
struct SeverableStream<S> {
    inner: Option<S>,
    sever_on_write: Arc<AtomicBool>,
    severed: bool,
}

impl<S> SeverableStream<S> {
    fn new(inner: S, sever_on_write: Arc<AtomicBool>) -> Self {
        Self {
            inner: Some(inner),
            sever_on_write,
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
                "connection severed by network partition simulator",
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
        if self.sever_on_write.load(Ordering::SeqCst) || self.severed || self.inner.is_none() {
            self.severed = true;
            self.inner = None; // Drop inner duplex stream to simulate abrupt connection termination
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "connection abruptly dropped before ack transmission",
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

// =========================================================================================
// CHALLENGE 1: Split-Brain Simulation (100 concurrent clipboard events, identical timestamps)
// =========================================================================================

/// Split-brain challenge:
/// - Disconnect Peer A and Peer B.
/// - Peer A logs 100 clipboard events. Peer B logs 100 clipboard events concurrently with
///   overlapping and identical timestamps.
/// - Reconnect Peer A and Peer B and execute `run_sync_session`.
/// - Verify:
///   1. Zero dropped payloads: both nodes hold exactly 200 events in their ledgers.
///   2. Vector clocks on both nodes match: { PeerA: 100, PeerB: 100 }.
///   3. Single-state arbiter produces identical winning clipboard state on both nodes.
#[tokio::test]
async fn test_adversarial_split_brain_100_events_identical_timestamps() {
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_LAPTOP_A").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_PHONE_B").unwrap());

    let engine_a = ReconciliationEngine::new(ledger_a.clone());
    let engine_b = ReconciliationEngine::new(ledger_b.clone());

    let base_timestamp_ms = 1_731_000_000_000i64;

    // Disconnected state: Peer A and Peer B independently log 100 clipboard events
    // with exact pairwise identical timestamps for each sequence number (1..=100)
    let mut events_a = Vec::with_capacity(100);
    let mut events_b = Vec::with_capacity(100);

    for seq in 1..=100 {
        let ts = base_timestamp_ms + (seq * 10);
        let ea = Event::new_local(
            "NODE_LAPTOP_A",
            seq,
            "clipboard.text",
            format!("laptop clipboard item #{}", seq).into_bytes(),
            ts,
        );
        let eb = Event::new_local(
            "NODE_PHONE_B",
            seq,
            "clipboard.text",
            format!("phone clipboard item #{}", seq).into_bytes(),
            ts,
        );
        events_a.push(ea);
        events_b.push(eb);
    }

    // Apply to local ledgers while partitioned
    ledger_a.apply_batch(&events_a).unwrap();
    ledger_b.apply_batch(&events_b).unwrap();

    // Verify partitioned states before sync
    assert_eq!(ledger_a.count_events().unwrap(), 100);
    assert_eq!(ledger_b.count_events().unwrap(), 100);

    let clock_a_pre = ledger_a.get_vector_clock().unwrap();
    let clock_b_pre = ledger_b.get_vector_clock().unwrap();
    assert_eq!(clock_a_pre.get("NODE_LAPTOP_A"), Some(&100));
    assert!(clock_a_pre.get("NODE_PHONE_B").is_none());
    assert_eq!(clock_b_pre.get("NODE_PHONE_B"), Some(&100));
    assert!(clock_b_pre.get("NODE_LAPTOP_A").is_none());

    // Reconnect Peer A and Peer B via virtual bidirectional stream
    // Using batch limit of 15 to stress multiple batch exchanges (100 / 15 = 7 batches per direction)
    let (mut stream_a, mut stream_b) = duplex(128 * 1024);

    let engine_a_clone = engine_a.clone();
    let task_a = tokio::spawn(async move {
        engine_a_clone
            .run_sync_session(&mut stream_a, "NODE_PHONE_B", 15)
            .await
            .unwrap()
    });

    let engine_b_clone = engine_b.clone();
    let task_b = tokio::spawn(async move {
        engine_b_clone
            .run_sync_session(&mut stream_b, "NODE_LAPTOP_A", 15)
            .await
            .unwrap()
    });

    let (stats_a, stats_b) = tokio::join!(task_a, task_b);
    let stats_a = stats_a.unwrap();
    let stats_b = stats_b.unwrap();

    // Verify transmission stats: 100 events sent and 100 received each
    assert_eq!(stats_a.events_sent, 100);
    assert_eq!(stats_a.events_received, 100);
    assert_eq!(stats_b.events_sent, 100);
    assert_eq!(stats_b.events_received, 100);

    // 1. Invariant: ZERO DROPPED PAYLOADS
    // Both ledgers hold all 200 events (100 from A + 100 from B)
    assert_eq!(ledger_a.count_events().unwrap(), 200);
    assert_eq!(ledger_b.count_events().unwrap(), 200);

    // 2. Invariant: Vector clocks on both nodes match exactly { PeerA: 100, PeerB: 100 }
    let clock_a_post = ledger_a.get_vector_clock().unwrap();
    let clock_b_post = ledger_b.get_vector_clock().unwrap();

    assert_eq!(clock_a_post.get("NODE_LAPTOP_A"), Some(&100));
    assert_eq!(clock_a_post.get("NODE_PHONE_B"), Some(&100));
    assert_eq!(clock_b_post.get("NODE_LAPTOP_A"), Some(&100));
    assert_eq!(clock_b_post.get("NODE_PHONE_B"), Some(&100));
    assert_eq!(clock_a_post, clock_b_post);

    // 3. Invariant: Deterministic LWW Conflict Arbitration
    // Both nodes independently evaluate their 200 stored events
    let all_a = ledger_a.get_all_events().unwrap();
    let all_b = ledger_b.get_all_events().unwrap();

    assert_eq!(all_a.len(), 200);
    assert_eq!(all_b.len(), 200);

    let winner_a = arbitrate_clipboard(&all_a).expect("Node A must find a clipboard winner");
    let winner_b = arbitrate_clipboard(&all_b).expect("Node B must find a clipboard winner");

    // Single-state arbiter produces identical winning clipboard state on both nodes
    assert_eq!(winner_a, winner_b);

    // Tie-breaker verification:
    // Event 100 from both nodes has timestamp base_timestamp_ms + 1000.
    // Device ID comparison: "NODE_PHONE_B" > "NODE_LAPTOP_A" (P > L)
    // Therefore, NODE_PHONE_B seq 100 must deterministically win on both nodes.
    assert_eq!(winner_a.origin_device_id, "NODE_PHONE_B");
    assert_eq!(winner_a.sequence_number, 100);
    assert_eq!(winner_a.payload_str().unwrap(), "phone clipboard item #100");
    assert_eq!(winner_b.origin_device_id, "NODE_PHONE_B");
    assert_eq!(winner_b.sequence_number, 100);
    assert_eq!(winner_b.payload_str().unwrap(), "phone clipboard item #100");
}

/// Split-brain challenge with multi-threaded concurrent local writes:
/// 10 concurrent tasks logging 20 events on Node A, and 10 concurrent tasks logging
/// 20 events on Node B, followed by sync and deterministic state convergence.
#[tokio::test]
async fn test_adversarial_split_brain_multithreaded_concurrency() {
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_CONCURRENT_A").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_CONCURRENT_B").unwrap());

    let mut handles_a = Vec::new();
    for t in 0..10 {
        let l = ledger_a.clone();
        handles_a.push(tokio::spawn(async move {
            for i in 0..20 {
                l.record_event("clipboard.text", format!("A_thread_{}_item_{}", t, i).as_bytes())
                    .unwrap();
            }
        }));
    }

    let mut handles_b = Vec::new();
    for t in 0..10 {
        let l = ledger_b.clone();
        handles_b.push(tokio::spawn(async move {
            for i in 0..20 {
                l.record_event("clipboard.text", format!("B_thread_{}_item_{}", t, i).as_bytes())
                    .unwrap();
            }
        }));
    }

    for h in handles_a {
        h.await.unwrap();
    }
    for h in handles_b {
        h.await.unwrap();
    }

    assert_eq!(ledger_a.count_events().unwrap(), 200);
    assert_eq!(ledger_b.count_events().unwrap(), 200);

    let engine_a = ReconciliationEngine::new(ledger_a.clone());
    let engine_b = ReconciliationEngine::new(ledger_b.clone());

    let (mut stream_a, mut stream_b) = duplex(128 * 1024);

    let engine_a_clone = engine_a.clone();
    let task_a = tokio::spawn(async move {
        engine_a_clone
            .run_sync_session(&mut stream_a, "NODE_CONCURRENT_B", 25)
            .await
            .unwrap()
    });

    let engine_b_clone = engine_b.clone();
    let task_b = tokio::spawn(async move {
        engine_b_clone
            .run_sync_session(&mut stream_b, "NODE_CONCURRENT_A", 25)
            .await
            .unwrap()
    });

    let (r_a, r_b) = tokio::join!(task_a, task_b);
    r_a.unwrap();
    r_b.unwrap();

    // Verify full convergence to 400 total events
    assert_eq!(ledger_a.count_events().unwrap(), 400);
    assert_eq!(ledger_b.count_events().unwrap(), 400);

    let clock_a = ledger_a.get_vector_clock().unwrap();
    let clock_b = ledger_b.get_vector_clock().unwrap();

    assert_eq!(*clock_a.get("NODE_CONCURRENT_A").unwrap(), 200);
    assert_eq!(*clock_a.get("NODE_CONCURRENT_B").unwrap(), 200);
    assert_eq!(clock_a, clock_b);

    // Arbiter check: Single-state arbiter produces identical winning clipboard state on both nodes
    let all_a = ledger_a.get_all_events().unwrap();
    let all_b = ledger_b.get_all_events().unwrap();
    let win_a = arbitrate_clipboard(&all_a).unwrap();
    let win_b = arbitrate_clipboard(&all_b).unwrap();
    assert_eq!(win_a, win_b);
}

// =========================================================================================
// CHALLENGE 2: Network Partition / Packet Loss Midway (Drop Before Ack & Reconnect Re-Sync)
// =========================================================================================

/// Network Partition Challenge:
/// - Drop connection after Peer A sends batch but before Peer B acknowledges.
/// - Reconnect and re-sync: verify no duplicate records, gap detection resumes correctly,
///   and final state converges cleanly.
#[tokio::test]
async fn test_adversarial_network_partition_drop_before_ack_and_resync() {
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_LAPTOP_A").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_PHONE_B").unwrap());

    // Peer A logs 50 events while offline
    for i in 1..=50 {
        ledger_a
            .record_event("clipboard.text", format!("uncommitted item {}", i).as_bytes())
            .unwrap();
    }
    assert_eq!(ledger_a.count_events().unwrap(), 50);
    assert_eq!(ledger_b.count_events().unwrap(), 0);

    // Setup: Peer B stream will sever the connection right after ingesting the first batch,
    // before the Ack can be transmitted back to Peer A.
    let (stream_a, stream_b) = duplex(64 * 1024);

    let sever_on_write = Arc::new(AtomicBool::new(false));
    let sever_flag_for_listener = sever_on_write.clone();

    // Attach listener to Peer B: as soon as the first event is ingested, trigger the sever flag
    let engine_b = ReconciliationEngine::new(ledger_b.clone()).with_listener(move |_evt| {
        sever_flag_for_listener.store(true, Ordering::SeqCst);
    });

    let severable_stream_b = SeverableStream::new(stream_b, sever_on_write);

    let engine_a = ReconciliationEngine::new(ledger_a.clone());

    let mut stream_a_pinned = stream_a;
    let mut stream_b_pinned = severable_stream_b;

    let engine_a_clone = engine_a.clone();
    let task_a = tokio::spawn(async move {
        // Peer A attempts sync with batch limit 10
        engine_a_clone
            .run_sync_session(&mut stream_a_pinned, "NODE_PHONE_B", 10)
            .await
    });

    let engine_b_clone = engine_b.clone();
    let task_b = tokio::spawn(async move {
        engine_b_clone
            .run_sync_session(&mut stream_b_pinned, "NODE_LAPTOP_A", 10)
            .await
    });

    let (res_a, res_b) = tokio::join!(task_a, task_b);
    let res_a = res_a.unwrap();
    let res_b = res_b.unwrap();

    // Both sessions must report an error due to the abrupt midway connection drop
    assert!(res_a.is_err(), "Peer A must encounter partition error when ack is dropped");
    assert!(res_b.is_err(), "Peer B must encounter partition error on broken stream");

    // Inspect the partition state:
    // Peer B successfully ingested the first batch (10 events) into its SQLite table
    assert_eq!(ledger_b.count_events().unwrap(), 10);
    let clock_b_mid = ledger_b.get_vector_clock().unwrap();
    assert_eq!(clock_b_mid.get("NODE_LAPTOP_A"), Some(&10));

    // Peer A still has all 50 events
    assert_eq!(ledger_a.count_events().unwrap(), 50);

    // -------------------------------------------------------------------------------------
    // PARTITION HEALS: Reconnect over a fresh duplex stream and re-run sync session
    // -------------------------------------------------------------------------------------
    let (mut stream_a2, mut stream_b2) = duplex(64 * 1024);

    let engine_a_clone2 = engine_a.clone();
    let task_a2 = tokio::spawn(async move {
        engine_a_clone2
            .run_sync_session(&mut stream_a2, "NODE_PHONE_B", 10)
            .await
            .expect("Re-sync after reconnection must succeed")
    });

    let engine_b_clone2 = engine_b.clone();
    let task_b2 = tokio::spawn(async move {
        engine_b_clone2
            .run_sync_session(&mut stream_b2, "NODE_LAPTOP_A", 10)
            .await
            .expect("Re-sync after reconnection must succeed")
    });

    let (stats_a2, stats_b2) = tokio::join!(task_a2, task_b2);
    let stats_a2 = stats_a2.unwrap();
    let stats_b2 = stats_b2.unwrap();

    // In the second session:
    // Peer B advertised clock { NODE_LAPTOP_A: 10 }.
    // Peer A sent remaining 40 events (seq 11..50).
    assert_eq!(stats_a2.events_sent, 40);
    assert_eq!(stats_b2.events_received, 40);

    // Verify Invariant: NO DUPLICATE RECORDS
    // Peer B must hold EXACTLY 50 events (not 60, not 40)
    assert_eq!(ledger_b.count_events().unwrap(), 50);
    assert_eq!(ledger_a.count_events().unwrap(), 50);

    // Verify Invariant: VECTOR CLOCKS MATCH
    let clock_a_final = ledger_a.get_vector_clock().unwrap();
    let clock_b_final = ledger_b.get_vector_clock().unwrap();
    assert_eq!(clock_a_final.get("NODE_LAPTOP_A"), Some(&50));
    assert_eq!(clock_b_final.get("NODE_LAPTOP_A"), Some(&50));
    assert_eq!(clock_a_final, clock_b_final);

    // Verify Data Integrity: Every single event in Peer B matches Peer A bit-for-bit
    let events_a_all = ledger_a.get_all_events().unwrap();
    let events_b_all = ledger_b.get_all_events().unwrap();
    assert_eq!(events_a_all.len(), 50);
    assert_eq!(events_b_all.len(), 50);
    for (ea, eb) in events_a_all.iter().zip(events_b_all.iter()) {
        assert_eq!(ea, eb);
    }
}

/// Network Partition Challenge with Bidirectional Mid-Stream Drop:
/// Peer A has 40 events, Peer B has 40 events.
/// Drop occurs halfway through bidirectional transfer. Reconnect and verify complete convergence.
#[tokio::test]
async fn test_adversarial_bidirectional_partition_midway_and_clean_resync() {
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_LAPTOP_A").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_PHONE_B").unwrap());

    for i in 1..=40 {
        ledger_a
            .record_event("clipboard.text", format!("laptop text {}", i).as_bytes())
            .unwrap();
        ledger_b
            .record_event("clipboard.text", format!("phone text {}", i).as_bytes())
            .unwrap();
    }

    let (stream_a, stream_b) = duplex(64 * 1024);

    let sever_flag = Arc::new(AtomicBool::new(false));
    let sever_flag_for_listener = sever_flag.clone();

    // Sever after Peer B ingests 15 events from Peer A
    let ingested_count = Arc::new(AtomicUsize::new(0));
    let count_clone = ingested_count.clone();

    let engine_b = ReconciliationEngine::new(ledger_b.clone()).with_listener(move |_evt| {
        let c = count_clone.fetch_add(1, Ordering::SeqCst);
        if c >= 14 {
            sever_flag_for_listener.store(true, Ordering::SeqCst);
        }
    });

    let engine_a = ReconciliationEngine::new(ledger_a.clone());

    let mut stream_a_pin = stream_a;
    let mut stream_b_pin = SeverableStream::new(stream_b, sever_flag);

    let engine_a_clone = engine_a.clone();
    let t_a = tokio::spawn(async move {
        engine_a_clone
            .run_sync_session(&mut stream_a_pin, "NODE_PHONE_B", 10)
            .await
    });

    let engine_b_clone = engine_b.clone();
    let t_b = tokio::spawn(async move {
        engine_b_clone
            .run_sync_session(&mut stream_b_pin, "NODE_LAPTOP_A", 10)
            .await
    });

    let (res_a, res_b) = tokio::join!(t_a, t_b);
    let _ = res_a.unwrap();
    let _ = res_b.unwrap();

    // Reconnect cleanly
    let (mut stream_a2, mut stream_b2) = duplex(64 * 1024);

    let engine_a_clone2 = engine_a.clone();
    let t_a2 = tokio::spawn(async move {
        engine_a_clone2
            .run_sync_session(&mut stream_a2, "NODE_PHONE_B", 10)
            .await
            .unwrap()
    });

    let engine_b_clone2 = engine_b.clone();
    let t_b2 = tokio::spawn(async move {
        engine_b_clone2
            .run_sync_session(&mut stream_b2, "NODE_LAPTOP_A", 10)
            .await
            .unwrap()
    });

    let (s_a, s_b) = tokio::join!(t_a2, t_b2);
    s_a.unwrap();
    s_b.unwrap();

    // Verify total events: 80 events on both nodes
    assert_eq!(ledger_a.count_events().unwrap(), 80);
    assert_eq!(ledger_b.count_events().unwrap(), 80);

    let clock_a = ledger_a.get_vector_clock().unwrap();
    let clock_b = ledger_b.get_vector_clock().unwrap();

    assert_eq!(*clock_a.get("NODE_LAPTOP_A").unwrap(), 40);
    assert_eq!(*clock_a.get("NODE_PHONE_B").unwrap(), 40);
    assert_eq!(clock_a, clock_b);

    // Deterministic LWW arbiter produces identical winner on both nodes
    let all_a = ledger_a.get_all_events().unwrap();
    let all_b = ledger_b.get_all_events().unwrap();
    let win_a = arbitrate_clipboard(&all_a).unwrap();
    let win_b = arbitrate_clipboard(&all_b).unwrap();
    assert_eq!(win_a, win_b);
}

// =========================================================================================
// CHALLENGE 3: Gap Detection, Cursor Rewind, & Duplicate Idempotence Stress
// =========================================================================================

/// Test gap detection and cursor rewinding during live sync session:
/// When an unexpected gap is presented to the receiver, the receiver returns GAP_DETECTED
/// with the exact sequence expected, and the sender rewinds its cursor and recovers.
#[tokio::test]
async fn test_adversarial_gap_detection_and_resumption() {
    let ledger_receiver = Arc::new(EventLedger::open_in_memory("NODE_RECV").unwrap());
    let engine = ReconciliationEngine::new(ledger_receiver.clone());

    // Ingest seq 1..5
    for i in 1..=5 {
        let e = Event::new_local("REMOTE_DEV", i, "clipboard.text", b"item".to_vec(), 1000 + i);
        ledger_receiver.apply_batch(&[e]).unwrap();
    }
    assert_eq!(ledger_receiver.count_events().unwrap(), 5);

    // Adversarial sender skips sequences 6..9 and sends sequence 10
    let e10 = Event::new_local("REMOTE_DEV", 10, "clipboard.text", b"item 10".to_vec(), 2000);
    let corrupt_batch = SyncBatch::new("REMOTE_DEV", 10, 10, false, vec![e10.into()]);

    let ack = engine.ingest_batch(&corrupt_batch).unwrap();
    assert!(ack.is_gap(), "Batch skipping sequence 6 must be flagged as a gap");
    assert_eq!(ack.acknowledged_sequence, 5);
    assert_eq!(ack.expected_sequence, Some(6));

    // Ledger still at 5 events (gap not ingested)
    assert_eq!(ledger_receiver.count_events().unwrap(), 5);

    // Resend from sequence 6..10
    let mut recovery_events = Vec::new();
    for i in 6..=10 {
        recovery_events.push(Event::new_local(
            "REMOTE_DEV",
            i,
            "clipboard.text",
            format!("recovered {}", i).into_bytes(),
            1000 + i,
        ));
    }
    let wire_events: Vec<WireEvent> = recovery_events.into_iter().map(WireEvent::from).collect();
    let recovery_batch = SyncBatch::new("REMOTE_DEV", 6, 10, false, wire_events);

    let recovery_ack = engine.ingest_batch(&recovery_batch).unwrap();
    assert!(recovery_ack.is_success());
    assert_eq!(recovery_ack.acknowledged_sequence, 10);
    assert_eq!(ledger_receiver.count_events().unwrap(), 10);
}

/// Test duplicate batch retransmission idempotence:
/// Re-sending already acknowledged batches must not create duplicate entries in SQLite
/// or alter the vector clock.
#[tokio::test]
async fn test_adversarial_duplicate_batch_retransmission_idempotence() {
    let ledger = Arc::new(EventLedger::open_in_memory("NODE_IDEMPOTENT").unwrap());
    let engine = ReconciliationEngine::new(ledger.clone());

    let mut events = Vec::new();
    for i in 1..=20 {
        events.push(Event::new_local(
            "ORIGIN_ALPHA",
            i,
            "clipboard.text",
            format!("payload {}", i).into_bytes(),
            1000 + i,
        ));
    }

    let batch = SyncBatch::new(
        "ORIGIN_ALPHA",
        1,
        20,
        false,
        events.iter().cloned().map(WireEvent::from).collect(),
    );

    // Initial ingestion
    let ack1 = engine.ingest_batch(&batch).unwrap();
    assert!(ack1.is_success());
    assert_eq!(ack1.acknowledged_sequence, 20);
    assert_eq!(ledger.count_events().unwrap(), 20);

    // Re-send the exact same batch 3 times (network duplicate retransmission)
    for _ in 0..3 {
        let ack_dup = engine.ingest_batch(&batch).unwrap();
        assert!(ack_dup.is_success());
        assert_eq!(ack_dup.acknowledged_sequence, 20);
        // Count must remain exactly 20!
        assert_eq!(ledger.count_events().unwrap(), 20);
    }

    let clock = ledger.get_vector_clock().unwrap();
    assert_eq!(clock.get("ORIGIN_ALPHA"), Some(&20));
}

// =========================================================================================
// CHALLENGE 4: 3-Node Triangular Partition & Eventual Convergence
// =========================================================================================

/// 3-Node split-brain partition challenge:
/// Nodes A, B, and C are partitioned from each other.
/// Each logs 30 clipboard events with overlapping timestamps.
/// Reconnect pairwise: A <-> B, then B <-> C, then A <-> C.
/// Verify all three nodes converge to 90 events, identical vector clocks, and identical LWW winner.
#[tokio::test]
async fn test_adversarial_three_node_partition_eventual_convergence() {
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_A").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_B").unwrap());
    let ledger_c = Arc::new(EventLedger::open_in_memory("NODE_C").unwrap());

    let base_ts = 1_731_111_000_000i64;

    for i in 1..=30 {
        let ts = base_ts + (i * 10);
        ledger_a.apply_batch(&[Event::new_local("NODE_A", i, "clipboard.text", format!("A_{}", i).into_bytes(), ts)]).unwrap();
        ledger_b.apply_batch(&[Event::new_local("NODE_B", i, "clipboard.text", format!("B_{}", i).into_bytes(), ts)]).unwrap();
        ledger_c.apply_batch(&[Event::new_local("NODE_C", i, "clipboard.text", format!("C_{}", i).into_bytes(), ts)]).unwrap();
    }

    let engine_a = ReconciliationEngine::new(ledger_a.clone());
    let engine_b = ReconciliationEngine::new(ledger_b.clone());
    let engine_c = ReconciliationEngine::new(ledger_c.clone());

    // Pairwise Sync 1: A <-> B
    {
        let (mut s_a, mut s_b) = duplex(64 * 1024);
        let ea = engine_a.clone();
        let eb = engine_b.clone();
        let t_a = tokio::spawn(async move { ea.run_sync_session(&mut s_a, "NODE_B", 10).await.unwrap() });
        let t_b = tokio::spawn(async move { eb.run_sync_session(&mut s_b, "NODE_A", 10).await.unwrap() });
        tokio::try_join!(t_a, t_b).unwrap();
    }

    // Pairwise Sync 2: B <-> C (B propagates A's events to C)
    {
        let (mut s_b, mut s_c) = duplex(64 * 1024);
        let eb = engine_b.clone();
        let ec = engine_c.clone();
        let t_b = tokio::spawn(async move { eb.run_sync_session(&mut s_b, "NODE_C", 10).await.unwrap() });
        let t_c = tokio::spawn(async move { ec.run_sync_session(&mut s_c, "NODE_B", 10).await.unwrap() });
        tokio::try_join!(t_b, t_c).unwrap();
    }

    // Pairwise Sync 3: A <-> C (C propagates B & C events to A)
    {
        let (mut s_a, mut s_c) = duplex(64 * 1024);
        let ea = engine_a.clone();
        let ec = engine_c.clone();
        let t_a = tokio::spawn(async move { ea.run_sync_session(&mut s_a, "NODE_C", 10).await.unwrap() });
        let t_c = tokio::spawn(async move { ec.run_sync_session(&mut s_c, "NODE_A", 10).await.unwrap() });
        tokio::try_join!(t_a, t_c).unwrap();
    }

    // Pairwise Sync 4: A <-> B (final convergence check)
    {
        let (mut s_a, mut s_b) = duplex(64 * 1024);
        let ea = engine_a.clone();
        let eb = engine_b.clone();
        let t_a = tokio::spawn(async move { ea.run_sync_session(&mut s_a, "NODE_B", 10).await.unwrap() });
        let t_b = tokio::spawn(async move { eb.run_sync_session(&mut s_b, "NODE_A", 10).await.unwrap() });
        tokio::try_join!(t_a, t_b).unwrap();
    }

    // All three nodes must now hold exactly 90 events!
    assert_eq!(ledger_a.count_events().unwrap(), 90);
    assert_eq!(ledger_b.count_events().unwrap(), 90);
    assert_eq!(ledger_c.count_events().unwrap(), 90);

    let clock_a = ledger_a.get_vector_clock().unwrap();
    let clock_b = ledger_b.get_vector_clock().unwrap();
    let clock_c = ledger_c.get_vector_clock().unwrap();

    let expected_clock: HashMap<String, i64> = [
        ("NODE_A".to_string(), 30),
        ("NODE_B".to_string(), 30),
        ("NODE_C".to_string(), 30),
    ].into_iter().collect();

    assert_eq!(clock_a, expected_clock);
    assert_eq!(clock_b, expected_clock);
    assert_eq!(clock_c, expected_clock);

    // LWW Arbiter on all three nodes must yield the exact same winner:
    // Timestamp base_ts + 300, tie breaker NODE_C > NODE_B > NODE_A
    let all_a = ledger_a.get_all_events().unwrap();
    let all_b = ledger_b.get_all_events().unwrap();
    let all_c = ledger_c.get_all_events().unwrap();

    let win_a = arbitrate_clipboard(&all_a).unwrap();
    let win_b = arbitrate_clipboard(&all_b).unwrap();
    let win_c = arbitrate_clipboard(&all_c).unwrap();

    assert_eq!(win_a, win_b);
    assert_eq!(win_b, win_c);
    assert_eq!(win_a.origin_device_id, "NODE_C");
    assert_eq!(win_a.sequence_number, 30);
    assert_eq!(win_a.payload_str().unwrap(), "C_30");
}
