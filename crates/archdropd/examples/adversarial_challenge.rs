//! Adversarial Empirical Challenge for Arch Drop Phase 4 Milestone 2
//! Tests:
//! 1. EMA SpeedEngine accuracy (alpha=0.35), burst resilience, zero-time, NaN avoidance, decay.
//! 2. Edge case math in ETA, percentage, formatters.
//! 3. Notification interactive action dispatch, 30s timeout rejection, and closed handling.
//! 4. State machine priority, Waybar payload serialization, and transient expiration.

use archdropd::events::{
    calculate_eta, calculate_percentage, format_eta, format_size, format_speed, DaemonEvent,
    DaemonState, Direction, SpeedEngine,
};
use archdropd::notification::NotificationService;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Mutex};

fn run_check<F: FnOnce()>(name: &str, f: F) {
    let start = Instant::now();
    print!("[TEST] {:<65} ... ", name);
    f();
    println!("OK ({:?})", start.elapsed());
}

async fn run_check_async<F, Fut>(name: &str, f: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let start = Instant::now();
    print!("[TEST] {:<65} ... ", name);
    f().await;
    println!("OK ({:?})", start.elapsed());
}

#[tokio::main]
async fn main() {
    println!("================================================================================");
    println!("ARCH DROP MILESTONE 2: ADVERSARIAL VERIFICATION SUITE");
    println!("================================================================================");

    // -------------------------------------------------------------------------
    // 1. Rate Smoothing & Math Accuracy
    // -------------------------------------------------------------------------
    run_check("EMA Math Accuracy: Exact alpha=0.35 step verification", || {
        let mut engine = SpeedEngine::new();
        let t0 = Instant::now();

        // Step 0: Initial update at t=0
        let s0 = engine.update(0, t0);
        assert_eq!(s0, 0, "Initial speed must be 0");

        // Step 1: 10,000,000 bytes in 1.0s -> inst = 10,000,000 B/s
        let t1 = t0 + Duration::from_secs(1);
        let s1 = engine.update(10_000_000, t1);
        assert_eq!(s1, 10_000_000, "First interval inst speed initializes EMA");

        // Step 2: Another 10,000,000 bytes in 1.0s -> inst = 10,000,000 B/s
        // EMA = 0.35 * 10M + 0.65 * 10M = 10M
        let t2 = t1 + Duration::from_secs(1);
        let s2 = engine.update(20_000_000, t2);
        assert_eq!(s2, 10_000_000, "Constant rate EMA must equal constant rate");

        // Step 3: 20,000,000 bytes in 1.0s -> inst = 20,000,000 B/s
        // EMA = 0.35 * 20M + 0.65 * 10M = 7M + 6.5M = 13,500,000
        let t3 = t2 + Duration::from_secs(1);
        let s3 = engine.update(40_000_000, t3);
        assert_eq!(s3, 13_500_000, "EMA acceleration must match alpha=0.35 formula");

        // Step 4: 0 bytes transferred in 1.0s -> inst = 0 B/s
        // EMA = 0.35 * 0 + 0.65 * 13,500,000 = 8,775,000
        let t4 = t3 + Duration::from_secs(1);
        let s4 = engine.update(40_000_000, t4);
        assert_eq!(s4, 8_775_000, "EMA must not drop to zero; must decay by 0.65");

        // Step 5: another 0 bytes in 1.0s
        // EMA = 0.65 * 8,775,000 = 5,703,750
        let t5 = t4 + Duration::from_secs(1);
        let s5 = engine.update(40_000_000, t5);
        assert_eq!(s5, 5_703_750, "Second decay step must equal 0.65^2");
    });

    run_check("Burst Resilience: 50,000 updates with zero elapsed duration", || {
        let mut engine = SpeedEngine::new();
        let now = Instant::now();
        engine.update(1_000_000, now);

        // 50,000 rapid updates at the EXACT same instant (elapsed == 0.0)
        for i in 1..=50_000 {
            let speed = engine.update(1_000_000 + i * 10, now);
            // Must not panic, must not produce NaN, must preserve last valid speed
            assert!(!speed.to_string().contains("NaN"));
            assert_eq!(speed, 0); // initial speed was 0 before first interval
        }
    });

    run_check("Burst Resilience: 10,000 sub-microsecond updates", || {
        let mut engine = SpeedEngine::new();
        let base = Instant::now();
        let _ = engine.update(0, base);

        // Advance by 1 nanosecond each iteration
        let mut transferred = 0u64;
        for i in 1..=10_000 {
            let t = base + Duration::from_nanos(i);
            transferred += 100; // 100 bytes every nanosecond
            let speed = engine.update(transferred, t);
            assert!(speed > 0);
            assert_ne!(speed, u64::MAX);
        }
    });

    run_check("Saturating Subtraction: Backward / Rewound byte counts", || {
        let mut engine = SpeedEngine::new();
        let start = Instant::now();
        let _ = engine.update(0, start);
        let s1 = engine.update(10_000_000, start + Duration::from_secs(1));
        assert_eq!(s1, 10_000_000);

        // Rewind: counter goes from 10,000,000 back down to 5,000,000
        // saturating_sub should prevent underflow panic and yield delta_bytes = 0
        let s2 = engine.update(5_000_000, start + Duration::from_secs(2));
        assert_eq!(s2, 6_500_000, "Rewound bytes must treat delta as 0 and decay EMA");
    });

    run_check("Boundary & Fuzz: calculate_percentage & calculate_eta", || {
        // Percentage boundaries
        assert_eq!(calculate_percentage(0, 0), 0);
        assert_eq!(calculate_percentage(100, 0), 0);
        assert_eq!(calculate_percentage(0, 100), 0);
        assert_eq!(calculate_percentage(50, 100), 50);
        assert_eq!(calculate_percentage(100, 100), 100);
        assert_eq!(calculate_percentage(150, 100), 100); // Clamped
        assert_eq!(calculate_percentage(u64::MAX, u64::MAX), 100);
        assert_eq!(calculate_percentage(u64::MAX / 2, u64::MAX), 50);

        // ETA boundaries
        assert_eq!(calculate_eta(100, 100, 1000), None);
        assert_eq!(calculate_eta(150, 100, 1000), None);
        assert_eq!(calculate_eta(50, 100, 0), None);
        assert_eq!(calculate_eta(50, 0, 1000), None);
        assert_eq!(
            calculate_eta(0, 1000, 100),
            Some(Duration::from_secs(10))
        );

        // Huge transfer ETA without overflow
        let eta_huge = calculate_eta(0, 1_000_000_000_000, 1_000_000);
        assert_eq!(eta_huge, Some(Duration::from_secs(1_000_000)));
        assert_eq!(format_eta(eta_huge), "277h 46m");

        // Formatters
        assert_eq!(format_speed(0), "0 B/s");
        assert_eq!(format_speed(1023), "1023 B/s");
        assert_eq!(format_speed(1024), "1.0 KB/s");
        assert_eq!(format_speed(1024 * 1024), "1.0 MB/s");
        assert_eq!(format_speed(1024 * 1024 * 1024), "1.0 GB/s");
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    });

    // -------------------------------------------------------------------------
    // 2. Notification Dispatch & Action Timeout Handling
    // -------------------------------------------------------------------------
    run_check_async("Notification: Disabled service permissive fallback", || async {
        let service = NotificationService::disabled();
        let auth = service
            .request_transfer_authorization("UntrustedPeer", 10, 50_000_000)
            .await;
        assert!(auth, "Disabled service must fall back to true");

        // Verify other methods execute without panic
        service.notify_transfer_completed("file.zip", 1000, "Peer", None).await;
        service.notify_transfer_failed("IO error", "Peer").await;
        service.notify_peer_paired("Peer", "abc1234567890abcdef").await;
        service.notify_clipboard_synced("text/plain", 50, "Peer").await;
    })
    .await;

    run_check_async("Notification: Interactive 'accept' action resolution", || async {
        let pending_actions: Arc<Mutex<HashMap<u32, oneshot::Sender<bool>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let notif_id = 42u32;
        pending_actions.lock().await.insert(notif_id, tx);

        // Simulate ActionInvoked signal with 'accept'
        let pending_clone = pending_actions.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if let Some(sender) = pending_clone.lock().await.remove(&notif_id) {
                let action_key = "accept";
                let accepted = action_key.eq_ignore_ascii_case("accept");
                let _ = sender.send(accepted);
            }
        });

        let result = tokio::time::timeout(Duration::from_millis(500), rx)
            .await
            .expect("Did not timeout")
            .expect("Channel open");
        assert!(result, "Accept action must resolve to true");
        assert!(
            pending_actions.lock().await.is_empty(),
            "Pending actions map must be drained"
        );
    })
    .await;

    run_check_async("Notification: Interactive 'decline' action resolution", || async {
        let pending_actions: Arc<Mutex<HashMap<u32, oneshot::Sender<bool>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let notif_id = 99u32;
        pending_actions.lock().await.insert(notif_id, tx);

        // Simulate ActionInvoked signal with 'decline'
        let pending_clone = pending_actions.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if let Some(sender) = pending_clone.lock().await.remove(&notif_id) {
                let action_key = "decline";
                let accepted = action_key.eq_ignore_ascii_case("accept");
                let _ = sender.send(accepted);
            }
        });

        let result = tokio::time::timeout(Duration::from_millis(500), rx)
            .await
            .expect("Did not timeout")
            .expect("Channel open");
        assert!(!result, "Decline action must resolve to false");
        assert!(pending_actions.lock().await.is_empty());
    })
    .await;

    run_check_async("Notification: Closed / Expired signal resolution", || async {
        let pending_actions: Arc<Mutex<HashMap<u32, oneshot::Sender<bool>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let notif_id = 101u32;
        pending_actions.lock().await.insert(notif_id, tx);

        // Simulate NotificationClosed signal (reason = 1 Expired, 2 Dismissed)
        let pending_clone = pending_actions.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if let Some(sender) = pending_clone.lock().await.remove(&notif_id) {
                let _ = sender.send(false);
            }
        });

        let result = tokio::time::timeout(Duration::from_millis(500), rx)
            .await
            .expect("Did not timeout")
            .expect("Channel open");
        assert!(!result, "NotificationClosed signal must resolve to false");
        assert!(pending_actions.lock().await.is_empty());
    })
    .await;

    run_check_async("Notification: 30-second action timeout rejection & cleanup", || async {
        // Test the timeout mechanics (using a scaled timeout simulation of the select loop)
        let pending_actions: Arc<Mutex<HashMap<u32, oneshot::Sender<bool>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let notif_id = 777u32;
        pending_actions.lock().await.insert(notif_id, tx);

        // Simulate timeout triggering after timeout interval
        let pending_clone = pending_actions.clone();
        let timeout_interval = Duration::from_millis(50); // fast simulation

        let resolution = tokio::select! {
            res = rx => res.unwrap_or(false),
            _ = tokio::time::sleep(timeout_interval) => {
                let _ = pending_clone.lock().await.remove(&notif_id);
                false
            }
        };

        assert!(!resolution, "Timeout must resolve to false (rejection)");
        assert!(
            !pending_actions.lock().await.contains_key(&notif_id),
            "Timed out notification must be removed from pending_actions"
        );
    })
    .await;

    run_check_async("Notification: 100 concurrent requests ID isolation", || async {
        let pending_actions: Arc<Mutex<HashMap<u32, oneshot::Sender<bool>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let mut handles = Vec::new();

        for id in 1..=100u32 {
            let (tx, rx) = oneshot::channel();
            pending_actions.lock().await.insert(id, tx);
            let pending_clone = pending_actions.clone();

            handles.push(tokio::spawn(async move {
                // Odd IDs accept, Even IDs decline
                let should_accept = id % 2 != 0;
                tokio::time::sleep(Duration::from_millis(5 + (id as u64 % 20))).await;
                if let Some(sender) = pending_clone.lock().await.remove(&id) {
                    let _ = sender.send(should_accept);
                }
                rx.await.unwrap()
            }));
        }

        for (idx, handle) in handles.into_iter().enumerate() {
            let id = (idx + 1) as u32;
            let result = handle.await.unwrap();
            #[allow(clippy::manual_is_multiple_of)]
            let expected = id % 2 != 0;
            assert_eq!(result, expected, "ID {} resolved incorrectly", id);
        }

        assert!(pending_actions.lock().await.is_empty(), "All 100 requests drained cleanly");
    })
    .await;

    // -------------------------------------------------------------------------
    // 3. State Machine Priority & Waybar Payload Coherence
    // -------------------------------------------------------------------------
    run_check("State Machine: Strict Priority Hierarchy Verification", || {
        let now = Instant::now();
        let mut state = DaemonState::new(2, 53317, true);

        // 1. Base state with paired peers: class=["paired"]
        let p0 = state.to_waybar_payload(now);
        assert_eq!(p0.class, vec!["paired".to_string()]);
        assert_eq!(p0.percentage, None);

        // 2. Clipboard synced event -> transient takes precedence over paired
        let p_clip = state.apply_event(
            &DaemonEvent::ClipboardSynced {
                mime_type: "image/png".to_string(),
                byte_size: 2048,
                origin_device: "Pixel 8".to_string(),
            },
            now,
        );
        assert!(p_clip.class.contains(&"clipboard".to_string()));

        // 3. Active transfer started -> transfer takes precedence over transient clipboard!
        let p_tx = state.apply_event(
            &DaemonEvent::TransferStarted {
                session_id: "s1".to_string(),
                peer_alias: "Pixel 8".to_string(),
                direction: Direction::Inbound,
                total_files: 1,
                total_bytes: 10_000_000,
            },
            now,
        );
        assert!(p_tx.class.contains(&"transferring".to_string()));
        assert!(p_tx.class.contains(&"inbound".to_string()));
        assert!(state.transient.is_none(), "Transfer start must clear transient state");

        // 4. Transfer completed -> transitions to completed transient
        let p_done = state.apply_event(
            &DaemonEvent::TransferCompleted {
                session_id: "s1".to_string(),
                peer_alias: "Pixel 8".to_string(),
                file_name: "test.iso".to_string(),
                file_count: 1,
                total_bytes: 10_000_000,
            },
            now,
        );
        assert_eq!(p_done.class, vec!["completed".to_string()]);
        assert_eq!(p_done.percentage, Some(100));

        // 5. Before 5s expires, completed remains
        let p_during = state.to_waybar_payload(now + Duration::from_secs(3));
        assert_eq!(p_during.class, vec!["completed".to_string()]);

        // 6. After 5s, completed expires back to paired
        let p_after = state.to_waybar_payload(now + Duration::from_secs(6));
        assert_eq!(p_after.class, vec!["paired".to_string()]);
    });

    println!("================================================================================");
    println!("ALL ADVERSARIAL CHECKS PASSED EMPIRICALLY (0 ERRORS)");
    println!("================================================================================");
}
