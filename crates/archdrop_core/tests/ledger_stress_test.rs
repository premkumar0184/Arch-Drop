//! Adversarial stress test suite for `crates/archdrop_core::ledger`.
//!
//! Executed by Milestone 2 Challenger 1 to empirically stress-test:
//! 1. High-throughput multi-threaded local event generation under the same `origin_device_id`:
//!    verifying strictly monotonic sequence numbers without duplicates, gaps, or race conditions.
//! 2. Multi-origin concurrent insertion across 10 distinct origins:
//!    verifying compound primary key `(origin_device_id, sequence_number)` uniqueness and isolation.
//! 3. Idempotency & duplicate batch re-insertion (`INSERT OR IGNORE`):
//!    verifying idempotent state retention, duplicate accounting, and state integrity across repeated ingestions.
//! 4. Transactional atomicity under mid-batch failure:
//!    verifying full rollback of prior events in a batch and uncorrupted vector clocks when a mid-batch error occurs.
//! 5. On-disk SQLite WAL mode persistence under multi-threaded write pressure.
//! 6. Foreign key cascade deletion integrity between `events` and `outbox_queue`.
//! 7. Host WireGuard `tiktok` interface safety invariant.

use archdrop_core::ledger::{Event, EventLedger, LedgerError};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Barrier};
use std::thread;

// ============================================================================
// 1. HIGH-THROUGHPUT CONCURRENT SAME-ORIGIN EVENT GENERATION
// ============================================================================

/// Tests high-throughput concurrent writes by 20 threads to an in-memory ledger
/// generating events under the exact same `origin_device_id`.
/// Verifies strictly monotonic sequence numbers `1..=1000` with 0 duplicates and 0 gaps.
#[test]
fn test_stress_concurrent_same_origin_monotonic_sequences_in_memory() {
    let origin_id = "STRESS_ORIGIN_LOCAL";
    let ledger = Arc::new(EventLedger::open_in_memory(origin_id).expect("Failed to open in-memory ledger"));

    const THREAD_COUNT: usize = 20;
    const EVENTS_PER_THREAD: usize = 50;
    const TOTAL_EVENTS: usize = THREAD_COUNT * EVENTS_PER_THREAD; // 1,000 events

    let barrier = Arc::new(Barrier::new(THREAD_COUNT));
    let mut handles = Vec::with_capacity(THREAD_COUNT);

    for t in 0..THREAD_COUNT {
        let l = ledger.clone();
        let b = barrier.clone();
        let orig = origin_id.to_string();

        handles.push(thread::spawn(move || {
            b.wait(); // Synchronize all 20 threads to start hitting SQLite simultaneously
            let mut allocated = Vec::with_capacity(EVENTS_PER_THREAD);
            for i in 0..EVENTS_PER_THREAD {
                let payload = format!("concurrent payload from thread {t} item {i}").into_bytes();
                let (origin, seq) = l
                    .record_event("clipboard.text", &payload)
                    .unwrap_or_else(|e| panic!("Thread {t} record_event error at item {i}: {e:?}"));
                assert_eq!(origin, orig);
                allocated.push(seq);
            }
            allocated
        }));
    }

    let mut all_sequences = Vec::with_capacity(TOTAL_EVENTS);
    for h in handles {
        let seqs = h.join().expect("Worker thread panicked during write stress");
        all_sequences.extend(seqs);
    }

    assert_eq!(all_sequences.len(), TOTAL_EVENTS);

    // Verify compound primary key uniqueness: every sequence number must be unique
    let mut unique_seqs = HashSet::new();
    for seq in &all_sequences {
        assert!(
            unique_seqs.insert(*seq),
            "Duplicate sequence number detected: {seq}"
        );
    }

    // Sort and verify strict monotonicity: 1..=TOTAL_EVENTS with ZERO gaps
    all_sequences.sort();
    for (idx, seq) in all_sequences.iter().enumerate() {
        let expected_seq = (idx + 1) as i64;
        assert_eq!(
            *seq, expected_seq,
            "Sequence mismatch at index {idx}: expected {expected_seq}, got {seq}"
        );
    }

    // Verify total event count
    assert_eq!(
        ledger.count_events().unwrap(),
        TOTAL_EVENTS,
        "Total events count must equal 1000"
    );

    // Verify vector clock has exact high-water mark of TOTAL_EVENTS
    let clock = ledger.get_vector_clock().unwrap();
    assert_eq!(
        clock.get(origin_id).copied(),
        Some(TOTAL_EVENTS as i64),
        "Vector clock must reflect exactly 1000 for local origin"
    );
}

/// Tests high-throughput concurrent writes by 10 threads to an on-disk SQLite WAL database file.
/// Verifies WAL mode resilience, locking, and monotonic sequence integrity on the filesystem.
#[test]
fn test_stress_concurrent_same_origin_monotonic_sequences_on_disk_wal() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let db_path = temp_dir.path().join("wal_stress_ledger.db");

    let origin_id = "ON_DISK_ORIGIN_A";
    let ledger = Arc::new(EventLedger::open(&db_path, origin_id).expect("Failed to open on-disk SQLite ledger"));

    const THREAD_COUNT: usize = 10;
    const EVENTS_PER_THREAD: usize = 50;
    const TOTAL_EVENTS: usize = THREAD_COUNT * EVENTS_PER_THREAD; // 500 events

    let barrier = Arc::new(Barrier::new(THREAD_COUNT));
    let mut handles = Vec::with_capacity(THREAD_COUNT);

    for t in 0..THREAD_COUNT {
        let l = ledger.clone();
        let b = barrier.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            let mut allocated = Vec::with_capacity(EVENTS_PER_THREAD);
            for i in 0..EVENTS_PER_THREAD {
                let payload = format!("disk wal thread {t} payload {i}").into_bytes();
                let (_, seq) = l
                    .record_event("transfer.status", &payload)
                    .unwrap_or_else(|e| panic!("Disk thread {t} record_event failed at {i}: {e:?}"));
                allocated.push(seq);
            }
            allocated
        }));
    }

    let mut all_sequences = Vec::with_capacity(TOTAL_EVENTS);
    for h in handles {
        let seqs = h.join().expect("Disk writer thread panicked");
        all_sequences.extend(seqs);
    }

    all_sequences.sort();
    assert_eq!(all_sequences.len(), TOTAL_EVENTS);
    for (idx, seq) in all_sequences.iter().enumerate() {
        assert_eq!(*seq, (idx + 1) as i64);
    }

    assert_eq!(ledger.count_events().unwrap(), TOTAL_EVENTS);

    // Verify persistent reload: open a new EventLedger pointing to the exact same file
    drop(ledger);
    let reloaded = EventLedger::open(&db_path, origin_id).expect("Failed to reload on-disk DB");
    assert_eq!(reloaded.count_events().unwrap(), TOTAL_EVENTS);

    let clock = reloaded.get_vector_clock().unwrap();
    assert_eq!(clock.get(origin_id).copied(), Some(TOTAL_EVENTS as i64));
}

// ============================================================================
// 2. CROSS-ORIGIN CONCURRENT INSERTION ACROSS 10 ORIGINS
// ============================================================================

/// Tests concurrent writes from 10 distinct origins simultaneously.
/// Verifies compound primary key `(origin_device_id, sequence_number)` uniqueness,
/// per-origin sequence monotonicity, and absence of cross-talk.
#[test]
fn test_stress_cross_origin_concurrent_10_origins() {
    let ledger = Arc::new(EventLedger::open_in_memory("PRIMARY_LOCAL").expect("Failed to open ledger"));

    const ORIGIN_COUNT: usize = 10;
    const EVENTS_PER_ORIGIN: usize = 100;
    const TOTAL_EVENTS: usize = ORIGIN_COUNT * EVENTS_PER_ORIGIN; // 1,000 events

    let barrier = Arc::new(Barrier::new(ORIGIN_COUNT));
    let mut handles = Vec::with_capacity(ORIGIN_COUNT);

    for o_idx in 0..ORIGIN_COUNT {
        let l = ledger.clone();
        let b = barrier.clone();
        let origin_name = format!("ORIGIN_DEVICE_{:02}", o_idx);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut seqs = Vec::with_capacity(EVENTS_PER_ORIGIN);
            for i in 0..EVENTS_PER_ORIGIN {
                let payload = format!("event for origin {origin_name} idx {i}").into_bytes();
                let (orig, seq) = l
                    .record_event_for_origin(&origin_name, "clipboard.text", &payload)
                    .unwrap_or_else(|e| panic!("Origin {origin_name} failed at {i}: {e:?}"));
                assert_eq!(orig, origin_name);
                seqs.push(seq);
            }
            (origin_name, seqs)
        }));
    }

    let mut origin_results = HashMap::new();
    for h in handles {
        let (origin_name, seqs) = h.join().expect("Cross-origin worker thread panicked");
        origin_results.insert(origin_name, seqs);
    }

    assert_eq!(origin_results.len(), ORIGIN_COUNT);

    // Verify each origin independently produced sequences 1..=100
    for o_idx in 0..ORIGIN_COUNT {
        let origin_name = format!("ORIGIN_DEVICE_{:02}", o_idx);
        let seqs = origin_results.get(&origin_name).expect("Missing origin result");
        assert_eq!(seqs.len(), EVENTS_PER_ORIGIN);

        let mut sorted_seqs = seqs.clone();
        sorted_seqs.sort();
        for (idx, seq) in sorted_seqs.iter().enumerate() {
            assert_eq!(
                *seq,
                (idx + 1) as i64,
                "Origin {origin_name} sequence gap or corruption at index {idx}"
            );
        }

        // Query ledger specifically for this origin's events
        let events = ledger
            .get_events_since(&origin_name, 0, None)
            .unwrap_or_else(|e| panic!("Failed to get events for {origin_name}: {e:?}"));
        assert_eq!(events.len(), EVENTS_PER_ORIGIN);
        for (idx, ev) in events.iter().enumerate() {
            assert_eq!(ev.origin_device_id, origin_name);
            assert_eq!(ev.sequence_number, (idx + 1) as i64);
        }
    }

    // Verify global count
    assert_eq!(ledger.count_events().unwrap(), TOTAL_EVENTS);

    // Verify vector clock has all 10 origins with max_seq = 100
    let clock = ledger.get_vector_clock().unwrap();
    assert_eq!(clock.len(), ORIGIN_COUNT);
    for o_idx in 0..ORIGIN_COUNT {
        let origin_name = format!("ORIGIN_DEVICE_{:02}", o_idx);
        assert_eq!(
            clock.get(&origin_name).copied(),
            Some(EVENTS_PER_ORIGIN as i64),
            "Vector clock mismatch for {origin_name}"
        );
    }
}

// ============================================================================
// 3. IDEMPOTENCY & DUPLICATE BATCH RE-INSERTION (`INSERT OR IGNORE`)
// ============================================================================

/// Tests batch re-insertion idempotency:
/// 1. Ingest initial batch of 100 events across 5 origins.
/// 2. Re-insert the identical batch multiple times.
/// 3. Ingest partially overlapping batches (50 old + 50 new).
/// 4. Concurrent duplicate batch submissions from multiple threads.
#[test]
fn test_stress_idempotency_and_duplicate_batch_reinsertion() {
    let ledger = EventLedger::open_in_memory("LOCAL_RECEIVER").expect("Failed to open ledger");

    // Construct 100 events across 5 origins (20 events per origin)
    let mut initial_batch = Vec::new();
    for o in 0..5 {
        let origin = format!("PEER_ORIGIN_{o}");
        for s in 1..=20 {
            let payload = format!("payload {origin} seq {s}").into_bytes();
            let event = Event::new_local(&origin, s, "clipboard.text", payload, 1000 + s * 10);
            initial_batch.push(event);
        }
    }

    // First ingestion: all 100 applied, 0 duplicates
    let summary1 = ledger.apply_batch(&initial_batch).unwrap();
    assert_eq!(summary1.applied_count, 100);
    assert_eq!(summary1.duplicate_count, 0);
    assert_eq!(ledger.count_events().unwrap(), 100);

    // Second ingestion with identical batch: 0 applied, 100 duplicates
    let summary2 = ledger.apply_batch(&initial_batch).unwrap();
    assert_eq!(summary2.applied_count, 0);
    assert_eq!(summary2.duplicate_count, 100);
    assert_eq!(ledger.count_events().unwrap(), 100);

    // Third ingestion with identical batch: 0 applied, 100 duplicates
    let summary3 = ledger.apply_batch(&initial_batch).unwrap();
    assert_eq!(summary3.applied_count, 0);
    assert_eq!(summary3.duplicate_count, 100);
    assert_eq!(ledger.count_events().unwrap(), 100);

    // Overlapping batch: 50 existing events (PEER_ORIGIN_0..PEER_ORIGIN_2, seq 11..20)
    // plus 50 new events (PEER_ORIGIN_0..PEER_ORIGIN_4, seq 21..30)
    let mut mixed_batch = Vec::new();
    // 50 duplicates:
    for o in 0..5 {
        let origin = format!("PEER_ORIGIN_{o}");
        for s in 11..=20 {
            let payload = format!("payload {origin} seq {s}").into_bytes();
            let event = Event::new_local(&origin, s, "clipboard.text", payload, 1000 + s * 10);
            mixed_batch.push(event);
        }
    }
    // 50 new events:
    for o in 0..5 {
        let origin = format!("PEER_ORIGIN_{o}");
        for s in 21..=30 {
            let payload = format!("new payload {origin} seq {s}").into_bytes();
            let event = Event::new_local(&origin, s, "clipboard.text", payload, 2000 + s * 10);
            mixed_batch.push(event);
        }
    }

    let summary4 = ledger.apply_batch(&mixed_batch).unwrap();
    assert_eq!(summary4.applied_count, 50, "Expected 50 new events applied");
    assert_eq!(summary4.duplicate_count, 50, "Expected 50 duplicates ignored");
    assert_eq!(ledger.count_events().unwrap(), 150);

    // Vector clock should reflect max sequence 30 for all 5 origins
    let clock = ledger.get_vector_clock().unwrap();
    for o in 0..5 {
        let origin = format!("PEER_ORIGIN_{o}");
        assert_eq!(clock.get(&origin).copied(), Some(30));
    }

    // Concurrent duplicate barrage: 10 threads pounding the ledger with identical batches
    let ledger_arc = Arc::new(ledger);
    let barrier = Arc::new(Barrier::new(10));
    let mut handles = Vec::new();

    for t in 0..10 {
        let l = ledger_arc.clone();
        let b = barrier.clone();
        let batch_clone = mixed_batch.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            let summary = l
                .apply_batch(&batch_clone)
                .unwrap_or_else(|e| panic!("Concurrent duplicate ingestion failed on thread {t}: {e:?}"));
            // All items should be treated as duplicates
            assert_eq!(summary.applied_count, 0);
            assert_eq!(summary.duplicate_count, 100);
        }));
    }

    for h in handles {
        h.join().expect("Concurrent duplicate thread panicked");
    }

    // Ledger count remains exactly 150
    assert_eq!(ledger_arc.count_events().unwrap(), 150);
}

// ============================================================================
// 4. TRANSACTIONAL ATOMICITY UNDER MID-BATCH FAILURE
// ============================================================================

/// Tests transactional atomicity when a batch contains a corrupted event midway.
/// Invariant: If event 26 in a 50-event batch fails SHA-256 validation,
/// the ENTIRE transaction rolls back: 0 events are added, and the vector clock is unchanged.
#[test]
fn test_stress_transactional_atomicity_mid_batch_failure_empty_ledger() {
    let ledger = EventLedger::open_in_memory("LOCAL_NODE").expect("Failed to open ledger");

    // Construct a 50-event batch
    let mut batch = Vec::new();
    for i in 1..=50 {
        let payload = format!("item {i}").into_bytes();
        let event = Event::new_local("REMOTE_ORIGIN", i, "clipboard.text", payload, 1000 + i);
        batch.push(event);
    }

    // Corrupt the 26th event (index 25): tamper with payload while leaving payload_hash unchanged
    batch[25].payload = b"TAMPERED_CONTENT_CORRUPTED".to_vec();

    // Attempt to ingest batch
    let result = ledger.apply_batch(&batch);
    assert!(result.is_err(), "apply_batch must fail on tampered payload hash");

    match result.unwrap_err() {
        LedgerError::PayloadHashMismatch { sequence, .. } => {
            assert_eq!(sequence, 26, "Expected mismatch at sequence 26");
        }
        other => panic!("Expected PayloadHashMismatch error, got {other:?}"),
    }

    // ATOMICITY VERIFICATION:
    // 1. Exactly 0 events must be in the ledger (events 1..25 must NOT have leaked into DB)
    assert_eq!(
        ledger.count_events().unwrap(),
        0,
        "Transaction rollback failed: events leaked into database after mid-batch error!"
    );

    // 2. Vector clock must be completely empty (no high-water mark advancement)
    let clock = ledger.get_vector_clock().unwrap();
    assert!(
        !clock.contains_key("REMOTE_ORIGIN"),
        "Vector clock was updated despite transaction failure: {:?}",
        clock
    );

    // 3. Querying get_events_since must return empty vec
    let events = ledger.get_events_since("REMOTE_ORIGIN", 0, None).unwrap();
    assert!(events.is_empty());
}

/// Tests transactional atomicity on an already populated ledger.
/// Pre-existing state must remain intact, and failed batch must leave zero trace.
#[test]
fn test_stress_transactional_atomicity_on_populated_ledger() {
    let ledger = EventLedger::open_in_memory("LOCAL_NODE").expect("Failed to open ledger");

    // 1. Ingest 20 valid pre-existing events
    let mut initial_batch = Vec::new();
    for i in 1..=20 {
        let payload = format!("initial item {i}").into_bytes();
        initial_batch.push(Event::new_local("ORIGIN_A", i, "clipboard.text", payload, 1000 + i));
    }
    ledger.apply_batch(&initial_batch).expect("Initial batch ingestion failed");

    assert_eq!(ledger.count_events().unwrap(), 20);
    let initial_clock = ledger.get_vector_clock().unwrap();
    assert_eq!(initial_clock.get("ORIGIN_A").copied(), Some(20));

    // 2. Submit a new batch: items 21..30 valid, item 31 corrupted, items 32..40 valid
    let mut failing_batch = Vec::new();
    for i in 21..=40 {
        let payload = format!("second batch item {i}").into_bytes();
        let mut ev = Event::new_local("ORIGIN_A", i, "clipboard.text", payload, 2000 + i);
        if i == 31 {
            // Corrupt hash
            ev.payload_hash = "deadbeef00000000000000000000000000000000000000000000000000000000".to_string();
        }
        failing_batch.push(ev);
    }

    let err = ledger.apply_batch(&failing_batch).unwrap_err();
    assert!(matches!(err, LedgerError::PayloadHashMismatch { sequence: 31, .. }));

    // Invariant: Ledger count is still exactly 20 (items 21..30 were rolled back)
    assert_eq!(
        ledger.count_events().unwrap(),
        20,
        "Partial commit detected! Items 21..30 were committed despite failure at 31"
    );

    // Vector clock for ORIGIN_A remains at 20
    let post_fail_clock = ledger.get_vector_clock().unwrap();
    assert_eq!(
        post_fail_clock.get("ORIGIN_A").copied(),
        Some(20),
        "Vector clock advanced during a rolled back transaction!"
    );

    // Now fix item 31 and re-submit: all 20 new items should ingest cleanly
    failing_batch[10].payload_hash = hex::encode(Sha256::digest(&failing_batch[10].payload));
    let recovery_summary = ledger.apply_batch(&failing_batch).expect("Recovery batch must succeed");
    assert_eq!(recovery_summary.applied_count, 20);
    assert_eq!(ledger.count_events().unwrap(), 40);

    let final_clock = ledger.get_vector_clock().unwrap();
    assert_eq!(final_clock.get("ORIGIN_A").copied(), Some(40));
}

// ============================================================================
// 5. FOREIGN KEY CASCADE & OUTBOX INTEGRITY STRESS
// ============================================================================

/// Tests that deleting events concurrently triggers foreign key cascades,
/// leaving zero orphaned outbox records without locking deadlocks.
#[test]
fn test_stress_outbox_foreign_key_cascade_deletions() {
    let ledger = Arc::new(EventLedger::open_in_memory("LOCAL_NODE").expect("Failed to open ledger"));
    let peers = vec![
        "PEER_A".to_string(),
        "PEER_B".to_string(),
        "PEER_C".to_string(),
    ];

    // Create 100 events, each targeting 3 peers (300 outbox entries)
    let mut event_keys = Vec::new();
    for i in 0..100 {
        let payload = format!("secret message {i}").into_bytes();
        let ev = ledger.record_event_with_peers("clipboard.text", &payload, &peers).unwrap();
        event_keys.push((ev.origin_device_id, ev.sequence_number));
    }

    assert_eq!(ledger.count_events().unwrap(), 100);
    assert_eq!(ledger.count_outbox().unwrap(), 300);

    // Delete 50 events concurrently across 5 threads
    let barrier = Arc::new(Barrier::new(5));
    let mut handles = Vec::new();

    let chunks: Vec<_> = event_keys[..50].chunks(10).map(|c| c.to_vec()).collect();

    for (t, chunk) in chunks.into_iter().enumerate() {
        let l = ledger.clone();
        let b = barrier.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            for (origin, seq) in chunk {
                let deleted = l.delete_event(&origin, seq).unwrap_or_else(|e| {
                    panic!("Thread {t} delete failed for ({origin}, {seq}): {e:?}")
                });
                assert!(deleted);
            }
        }));
    }

    for h in handles {
        h.join().expect("Delete thread panicked");
    }

    // 50 events remaining, exactly 150 outbox entries remaining (foreign key cascade verified!)
    assert_eq!(ledger.count_events().unwrap(), 50);
    assert_eq!(
        ledger.count_outbox().unwrap(),
        150,
        "Orphaned outbox entries detected! FOREIGN KEY CASCADE failed"
    );
}

// ============================================================================
// 6. HOST WIREGUARD SAFETY INVARIANT
// ============================================================================

/// Verifies that the host WireGuard interface `tiktok` remains 100% active and untouched.
#[test]
fn test_stress_host_wireguard_safety_invariant() {
    let output = std::process::Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output()
        .expect("Failed to query ip addr");

    assert!(
        output.status.success(),
        "Interface tiktok must be active and present"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("10.2.0.2/32"),
        "Interface tiktok must have IP 10.2.0.2/32, got: {stdout}"
    );

    // Check routing rule 51820
    let rule_output = std::process::Command::new("ip")
        .args(["rule", "show"])
        .output()
        .expect("Failed to query ip rule");

    assert!(rule_output.status.success());
    let rule_stdout = String::from_utf8_lossy(&rule_output.stdout);
    assert!(
        rule_stdout.contains("51820"),
        "Routing table 51820 rule must remain active, got: {rule_stdout}"
    );
}
