use archdrop_core::ledger::{arbitrate_clipboard, Event, EventLedger};
use archdrop_mesh::sync::{ReconciliationEngine, SyncBatch};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::duplex;


#[tokio::test]
async fn test_reconciliation_offline_catchup_5_step() {
    // Setup Node A and Node B with isolated in-memory ledgers
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_LAPTOP_A").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_PHONE_B").unwrap());

    let engine_a = ReconciliationEngine::new(ledger_a.clone());
    let engine_b = ReconciliationEngine::new(ledger_b.clone());

    // Step 0: While offline, Node A generates 10 events (1..10)
    for i in 1..=10 {
        ledger_a
            .record_event("clipboard.text", format!("laptop text {}", i).as_bytes())
            .unwrap();
    }

    // While offline, Node B generates 5 events (1..5)
    for i in 1..=5 {
        ledger_b
            .record_event("transfer.status", format!("phone transfer {}", i).as_bytes())
            .unwrap();
    }

    // Verify offline state
    let clock_a_before = ledger_a.get_vector_clock().unwrap();
    assert_eq!(*clock_a_before.get("NODE_LAPTOP_A").unwrap(), 10);
    assert!(clock_a_before.get("NODE_PHONE_B").is_none());

    let clock_b_before = ledger_b.get_vector_clock().unwrap();
    assert_eq!(*clock_b_before.get("NODE_PHONE_B").unwrap(), 5);
    assert!(clock_b_before.get("NODE_LAPTOP_A").is_none());

    // Now connect via virtual bidirectional duplex stream
    let (mut stream_a, mut stream_b) = duplex(64 * 1024);

    let engine_a_clone = engine_a.clone();
    let task_a = tokio::spawn(async move {
        engine_a_clone
            .run_sync_session(&mut stream_a, "NODE_PHONE_B", 5)
            .await
            .unwrap()
    });

    let engine_b_clone = engine_b.clone();
    let task_b = tokio::spawn(async move {
        engine_b_clone
            .run_sync_session(&mut stream_b, "NODE_LAPTOP_A", 5)
            .await
            .unwrap()
    });

    let (stats_a, stats_b) = tokio::join!(task_a, task_b);
    let stats_a = stats_a.unwrap();
    let stats_b = stats_b.unwrap();

    // Verify transmission stats
    assert_eq!(stats_a.events_sent, 10);
    assert_eq!(stats_a.events_received, 5);
    assert_eq!(stats_b.events_sent, 5);
    assert_eq!(stats_b.events_received, 10);

    // Verify both ledgers have converged to { NODE_LAPTOP_A: 10, NODE_PHONE_B: 5 }
    let clock_a_after = ledger_a.get_vector_clock().unwrap();
    let clock_b_after = ledger_b.get_vector_clock().unwrap();

    assert_eq!(*clock_a_after.get("NODE_LAPTOP_A").unwrap(), 10);
    assert_eq!(*clock_a_after.get("NODE_PHONE_B").unwrap(), 5);

    assert_eq!(*clock_b_after.get("NODE_LAPTOP_A").unwrap(), 10);
    assert_eq!(*clock_b_after.get("NODE_PHONE_B").unwrap(), 5);

    // Verify total events: each ledger holds all 15 events (zero dropped payloads!)
    assert_eq!(ledger_a.count_events().unwrap(), 15);
    assert_eq!(ledger_b.count_events().unwrap(), 15);
}

#[tokio::test]
async fn test_reconciliation_gap_detection_and_recovery() {
    let ledger_receiver = Arc::new(EventLedger::open_in_memory("NODE_RECEIVER").unwrap());
    let engine = ReconciliationEngine::new(ledger_receiver.clone());

    // Node receiver has ingested events 1 and 2
    let e1 = Event::new_local("ORIGIN_X", 1, "clipboard.text", b"item 1".to_vec(), 1000);
    let e2 = Event::new_local("ORIGIN_X", 2, "clipboard.text", b"item 2".to_vec(), 2000);
    ledger_receiver.apply_batch(&[e1, e2]).unwrap();

    // Peer mistakenly sends a batch starting at sequence 4 (skipping sequence 3!)
    let e4 = Event::new_local("ORIGIN_X", 4, "clipboard.text", b"item 4".to_vec(), 4000);
    let e5 = Event::new_local("ORIGIN_X", 5, "clipboard.text", b"item 5".to_vec(), 5000);
    let corrupt_batch = SyncBatch::new("ORIGIN_X", 4, 5, false, vec![e4.into(), e5.into()]);

    let ack = engine.ingest_batch(&corrupt_batch).unwrap();

    // Verify receiver rejects the batch with GAP_DETECTED
    assert!(ack.is_gap());
    assert_eq!(ack.acknowledged_sequence, 2);
    assert_eq!(ack.expected_sequence, Some(3));

    // Ledger still only has 2 events (gap was not ingested)
    assert_eq!(ledger_receiver.count_events().unwrap(), 2);

    // Now peer recovers and sends the missing range starting at sequence 3
    let e3 = Event::new_local("ORIGIN_X", 3, "clipboard.text", b"item 3".to_vec(), 3000);
    let e4_retry = Event::new_local("ORIGIN_X", 4, "clipboard.text", b"item 4".to_vec(), 4000);
    let e5_retry = Event::new_local("ORIGIN_X", 5, "clipboard.text", b"item 5".to_vec(), 5000);
    let valid_batch = SyncBatch::new(
        "ORIGIN_X",
        3,
        5,
        false,
        vec![e3.into(), e4_retry.into(), e5_retry.into()],
    );

    let recovery_ack = engine.ingest_batch(&valid_batch).unwrap();
    assert!(recovery_ack.is_success());
    assert_eq!(recovery_ack.acknowledged_sequence, 5);
    assert_eq!(ledger_receiver.count_events().unwrap(), 5);
}

#[tokio::test]
async fn test_split_brain_clipboard_arbitration_zero_dropped_payloads() {
    let ledger_a = Arc::new(EventLedger::open_in_memory("NODE_AAA").unwrap());
    let ledger_b = Arc::new(EventLedger::open_in_memory("NODE_BBB").unwrap());

    // Both nodes write clipboard items at identical timestamp (split-brain collision)
    let collision_time = 1731062400000;

    let event_phone = Event::new_local(
        "E8C2D3A1B4C5D6",
        1,
        "clipboard.text",
        b"phone copied text".to_vec(),
        collision_time,
    );

    let event_laptop = Event::new_local(
        "F4A7B1C2D3E4F5",
        1,
        "clipboard.text",
        b"laptop copied text".to_vec(),
        collision_time,
    );

    // Apply both events to Node A and Node B
    ledger_a.apply_batch(&[event_phone.clone(), event_laptop.clone()]).unwrap();
    ledger_b.apply_batch(&[event_laptop.clone(), event_phone.clone()]).unwrap();

    // Verify Invariant: ZERO DROPPED PAYLOADS (both rows exist in both ledgers)
    assert_eq!(ledger_a.count_events().unwrap(), 2);
    assert_eq!(ledger_b.count_events().unwrap(), 2);

    let all_a = ledger_a.get_all_events().unwrap();
    let all_b = ledger_b.get_all_events().unwrap();

    // Verify LWW Arbitration:
    // With identical timestamps, tie-breaker compares uppercase device IDs:
    // "F4A7B1C2D3E4F5" > "E8C2D3A1B4C5D6"
    let winner_a = arbitrate_clipboard(&all_a).unwrap();
    let winner_b = arbitrate_clipboard(&all_b).unwrap();

    assert_eq!(winner_a.origin_device_id, "F4A7B1C2D3E4F5");
    assert_eq!(winner_a.payload_str().unwrap(), "laptop copied text");

    assert_eq!(winner_b.origin_device_id, "F4A7B1C2D3E4F5");
    assert_eq!(winner_b.payload_str().unwrap(), "laptop copied text");

    // Both independent nodes reached IDENTICAL active clipboard state without consensus rounds!
    assert_eq!(winner_a, winner_b);
}

#[tokio::test]
async fn test_reconciliation_listeners_triggered_on_ingestion() {
    let ledger = Arc::new(EventLedger::open_in_memory("LOCAL_RECEIVER").unwrap());
    let counter = Arc::new(AtomicUsize::new(0));

    let counter_clone = counter.clone();
    let engine = ReconciliationEngine::new(ledger.clone()).with_listener(move |_event| {
        counter_clone.fetch_add(1, Ordering::SeqCst);
    });

    let e1 = Event::new_local("REMOTE", 1, "clipboard.text", b"event 1".to_vec(), 1000);
    let e2 = Event::new_local("REMOTE", 2, "clipboard.text", b"event 2".to_vec(), 2000);
    let batch = SyncBatch::new("REMOTE", 1, 2, false, vec![e1.into(), e2.into()]);

    let ack = engine.ingest_batch(&batch).unwrap();
    assert!(ack.is_success());
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}
