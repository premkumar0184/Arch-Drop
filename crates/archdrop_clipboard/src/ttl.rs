use crate::cache::ClipboardPayloadCache;
use archdrop_core::models::clipboard::{ClearReason, ClipboardClearPacket, ClipboardPacket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::{debug, info};

pub const DEFAULT_TTL_DURATION: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub struct ClipboardTtlManager {
    device_id: String,
    last_update_epoch_ms: Arc<AtomicU64>,
    cache: ClipboardPayloadCache,
    packet_tx: broadcast::Sender<ClipboardPacket>,
}

impl ClipboardTtlManager {
    pub fn new(
        device_id: String,
        cache: ClipboardPayloadCache,
        packet_tx: broadcast::Sender<ClipboardPacket>,
    ) -> Self {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Self {
            device_id,
            last_update_epoch_ms: Arc::new(AtomicU64::new(now_ms)),
            cache,
            packet_tx,
        }
    }

    /// Records that a new clipboard payload has been synchronized, resetting the 120s timer.
    pub fn touch(&self) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_update_epoch_ms.store(now_ms, Ordering::Release);
        debug!("Reset 120-second clipboard TTL timer");
    }

    /// Zeroizes memory and emits a clear event due to session lock or TTL expiry.
    pub async fn zeroize(&self, reason: ClearReason) {
        info!(reason = ?reason, "Zeroizing volatile clipboard memory");
        self.cache.clear().await;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let clear_packet = ClipboardPacket::Clear(ClipboardClearPacket {
            version: 2,
            device_id: self.device_id.clone(),
            timestamp: now_ms,
            reason,
        });

        let _ = self.packet_tx.send(clear_packet);
    }

    /// Spawns the background monitor that prunes cache and checks the 120s expiration.
    pub fn spawn_ttl_loop(&self, mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
        let mgr = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // 1. Prune expired cache items
                        mgr.cache.prune_expired().await;

                        // 2. Check 120s activity threshold
                        let now_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let last_update = mgr.last_update_epoch_ms.load(Ordering::Acquire);

                        if now_ms.saturating_sub(last_update) >= DEFAULT_TTL_DURATION.as_millis() as u64
                            && mgr.cache.len().await > 0
                        {
                            info!("Clipboard TTL (120s) expired, purging volatile state");
                            mgr.zeroize(ClearReason::TtlExpired).await;
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });
    }
}
