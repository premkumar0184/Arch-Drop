//! Asynchronous 60-second inactivity watchdog coordinator for active LOHS sessions.
//!
//! Wraps `ActiveLohsSession` to prevent Wi-Fi connection flapping and ensure single-synthesizer
//! driver safety for Realtek RTL8852BE (`rtw89_8852be`). Auto-deactivates the volatile NetworkManager
//! profile after 60 seconds of silence, restoring the baseline Wi-Fi connection (e.g. FTTH-AB0D).

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use zbus::zvariant::OwnedObjectPath;

use super::client::ActiveLohsSession;
use crate::error::WirelessError;

/// Default inactivity timeout before auto-teardown (60 seconds).
pub const DEFAULT_LOHS_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Coordinates an active LOHS station session with an asynchronous inactivity watchdog.
#[derive(Clone)]
pub struct LohsSessionCoordinator {
    inner: Arc<CoordinatorInner>,
}

struct CoordinatorInner {
    session: tokio::sync::Mutex<Option<ActiveLohsSession>>,
    watchdog_timeout: Duration,
    last_activity: Mutex<Instant>,
    activity_notify: Notify,
    cancel_notify: Notify,
    deactivation_notify: Arc<Notify>,
    is_active: AtomicBool,
    gateway_ip: Ipv4Addr,
    local_ip: Ipv4Addr,
    active_path: OwnedObjectPath,
    connection_path: OwnedObjectPath,
}

impl LohsSessionCoordinator {
    /// Spawns a new coordinator wrapping the active LOHS session with the specified idle timeout.
    pub fn spawn(
        session: ActiveLohsSession,
        watchdog_timeout: Duration,
    ) -> (Self, JoinHandle<()>) {
        let gateway_ip = session.gateway_ip();
        let local_ip = session.local_ip();
        let active_path = session.active_path().clone();
        let connection_path = session.connection_path().clone();

        let inner = Arc::new(CoordinatorInner {
            session: tokio::sync::Mutex::new(Some(session)),
            watchdog_timeout,
            last_activity: Mutex::new(Instant::now()),
            activity_notify: Notify::new(),
            cancel_notify: Notify::new(),
            deactivation_notify: Arc::new(Notify::new()),
            is_active: AtomicBool::new(true),
            gateway_ip,
            local_ip,
            active_path,
            connection_path,
        });

        let coordinator = Self {
            inner: inner.clone(),
        };

        let handle = tokio::spawn(async move {
            inner.run_watchdog_loop().await;
        });

        (coordinator, handle)
    }

    /// Resets the inactivity watchdog timer to full duration.
    /// MUST be called on incoming or outgoing HTTP transfer/clipboard activity on port 53317.
    pub fn record_activity(&self) {
        if !self.inner.is_active.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut lock) = self.inner.last_activity.lock() {
            *lock = Instant::now();
        }
        self.inner.activity_notify.notify_one();
    }

    /// Explicitly deactivates the LOHS session, cancelling the watchdog and restoring baseline Wi-Fi.
    pub async fn deactivate(&self) -> Result<(), WirelessError> {
        self.inner.cancel_notify.notify_one();
        self.inner.deactivate_internal().await;
        Ok(())
    }

    /// Returns true if the session is currently active and within the watchdog window.
    pub fn is_active(&self) -> bool {
        self.inner.is_active.load(Ordering::SeqCst)
    }

    /// Gateway IP address of the Android SoftAP.
    pub fn gateway_ip(&self) -> Ipv4Addr {
        self.inner.gateway_ip
    }

    /// Local interface IP address on the LOHS subnet.
    pub fn local_ip(&self) -> Ipv4Addr {
        self.inner.local_ip
    }

    /// D-Bus object path of the active connection.
    pub fn active_path(&self) -> &OwnedObjectPath {
        &self.inner.active_path
    }

    /// D-Bus object path of the connection settings profile.
    pub fn connection_path(&self) -> &OwnedObjectPath {
        &self.inner.connection_path
    }

    /// Time remaining before inactivity auto-deactivation triggers.
    pub fn remaining_time(&self) -> Duration {
        if !self.is_active() {
            return Duration::ZERO;
        }
        let elapsed = self
            .inner
            .last_activity
            .lock()
            .map(|l| l.elapsed())
            .unwrap_or(self.inner.watchdog_timeout);
        self.inner.watchdog_timeout.saturating_sub(elapsed)
    }

    /// Returns a notification handle that fires when auto-deactivation triggers.
    pub fn deactivation_notify(&self) -> Arc<Notify> {
        self.inner.deactivation_notify.clone()
    }
}

impl CoordinatorInner {
    async fn run_watchdog_loop(&self) {
        info!(
            timeout_secs = self.watchdog_timeout.as_secs(),
            gateway = %self.gateway_ip,
            "LOHS session inactivity watchdog armed"
        );

        loop {
            let elapsed = self
                .last_activity
                .lock()
                .map(|l| l.elapsed())
                .unwrap_or(self.watchdog_timeout);

            if elapsed >= self.watchdog_timeout {
                info!(
                    idle_secs = elapsed.as_secs(),
                    "LOHS session reached 60s idle threshold. Auto-deactivating volatile profile to restore baseline Wi-Fi..."
                );
                self.deactivate_internal().await;
                break;
            }

            let sleep_duration = self.watchdog_timeout.saturating_sub(elapsed);

            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {}
                _ = self.activity_notify.notified() => {
                    debug!("LOHS watchdog countdown reset via HTTP activity on port 53317");
                }
                _ = self.cancel_notify.notified() => {
                    debug!("LOHS watchdog loop cancelled by explicit deactivation");
                    break;
                }
            }
        }
    }

    async fn deactivate_internal(&self) {
        if !self.is_active.swap(false, Ordering::SeqCst) {
            return;
        }

        let mut lock = self.session.lock().await;
        if let Some(mut session) = lock.take() {
            info!("Auto-deactivating LOHS session: {}", self.active_path.as_str());
            if let Err(e) = session.deactivate().await {
                warn!("Failed to cleanly deactivate LOHS session: {e}");
            } else {
                info!("Volatile LOHS profile deactivated; baseline Wi-Fi connection restored.");
            }
        }
        self.deactivation_notify.notify_waiters();
    }
}
