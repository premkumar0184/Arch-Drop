//! Freedesktop D-Bus Notification dispatch and interactive action handling (SwayNC & Dunst compatible).
//!
//! Uses existing workspace `zbus 5.0` via declarative `#[zbus::proxy]` without pulling in `notify-rust`
//! or conflicting `zbus 6`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, info, warn};
use zbus::proxy;
use zbus::zvariant::Value;

/// Declarative Freedesktop Notifications proxy interface.
#[proxy(
    default_service = "org.freedesktop.Notifications",
    interface = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
pub trait Notifications {
    /// Dispatches a notification to the desktop server.
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: &HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    /// Closes an active notification by notification ID.
    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    /// Signal emitted when an action button is clicked by the user.
    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: &str) -> zbus::Result<()>;

    /// Signal emitted when a notification is closed or expires.
    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

/// Notification service managing interactive desktop prompts and alert dispatch.
#[derive(Clone)]
pub struct NotificationService {
    proxy: Option<NotificationsProxy<'static>>,
    pending_actions: Arc<Mutex<HashMap<u32, oneshot::Sender<bool>>>>,
    folder_actions: Arc<Mutex<HashMap<u32, PathBuf>>>,
    default_download_dir: Arc<Mutex<Option<PathBuf>>>,
}

#[allow(dead_code)]
impl NotificationService {
    /// Connect to the user session D-Bus and initialize the notification proxy.
    pub async fn connect() -> Self {
        match zbus::Connection::session().await {
            Ok(conn) => match NotificationsProxy::new(&conn).await {
                Ok(proxy) => {
                    info!("Connected to Freedesktop Notifications D-Bus service");
                    let pending_actions = Arc::new(Mutex::new(HashMap::new()));
                    let folder_actions = Arc::new(Mutex::new(HashMap::new()));
                    let default_download_dir = Arc::new(Mutex::new(None));
                    let service = Self {
                        proxy: Some(proxy),
                        pending_actions,
                        folder_actions,
                        default_download_dir,
                    };
                    service.spawn_signal_listener();
                    service
                }
                Err(e) => {
                    warn!("Failed to create Notifications proxy: {e}");
                    Self::disabled()
                }
            },
            Err(e) => {
                warn!("Failed to connect to D-Bus session bus: {e}");
                Self::disabled()
            }
        }
    }

    /// Construct a disabled/noop notification service for headless or test environments.
    pub fn disabled() -> Self {
        Self {
            proxy: None,
            pending_actions: Arc::new(Mutex::new(HashMap::new())),
            folder_actions: Arc::new(Mutex::new(HashMap::new())),
            default_download_dir: Arc::new(Mutex::new(None)),
        }
    }

    /// Sets default download directory for notification action lookup.
    pub fn set_default_download_dir(&self, dir: PathBuf) {
        if let Ok(mut lock) = self.default_download_dir.try_lock() {
            *lock = Some(dir);
        }
    }

    /// Register a pending authorization action sender for a notification ID (used in auth flow and tests).
    pub async fn register_pending_action(&self, id: u32, sender: oneshot::Sender<bool>) {
        self.pending_actions.lock().await.insert(id, sender);
    }

    /// Register a target folder path for an open_folder action on a notification ID.
    pub async fn register_folder_action(&self, id: u32, path: PathBuf) {
        self.folder_actions.lock().await.insert(id, path);
    }

    /// Handle ActionInvoked D-Bus signal: processes accept/decline authorization and executes xdg-open for open_folder.
    pub async fn handle_action_invoked(&self, id: u32, key: &str) {
        debug!(id = id, action_key = %key, "Handling ActionInvoked signal");
        if let Some(sender) = self.pending_actions.lock().await.remove(&id) {
            let accepted = key.eq_ignore_ascii_case("accept");
            let _ = sender.send(accepted);
        }
        if let Some(folder_path) = self.folder_actions.lock().await.remove(&id) {
            if key.eq_ignore_ascii_case("open") || key.eq_ignore_ascii_case("open_folder") {
                info!(path = ?folder_path, "Spawning xdg-open for completed transfer folder");
                if let Err(e) = std::process::Command::new("xdg-open")
                    .arg(&folder_path)
                    .spawn()
                {
                    warn!("Failed to spawn xdg-open for {:?}: {e}", folder_path);
                }
            }
        }
    }

    /// Handle NotificationClosed D-Bus signal: cleans up pending authorization and folder actions.
    pub async fn handle_notification_closed(&self, id: u32, reason: u32) {
        debug!(id = id, reason = reason, "Handling NotificationClosed signal");
        if let Some(sender) = self.pending_actions.lock().await.remove(&id) {
            let _ = sender.send(false);
        }
        self.folder_actions.lock().await.remove(&id);
    }

    /// Spawn background D-Bus signal listener for interactive action responses.
    fn spawn_signal_listener(&self) {
        if let Some(ref proxy) = self.proxy {
            let proxy = proxy.clone();
            let this = self.clone();

            tokio::spawn(async move {
                use tokio_stream::StreamExt;
                let mut action_stream = match proxy.receive_action_invoked().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to subscribe to ActionInvoked signal: {e}");
                        return;
                    }
                };

                let mut closed_stream = match proxy.receive_notification_closed().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to subscribe to NotificationClosed signal: {e}");
                        return;
                    }
                };

                loop {
                    tokio::select! {
                        Some(action) = action_stream.next() => {
                            if let Ok(args) = action.args() {
                                this.handle_action_invoked(args.id, args.action_key).await;
                            }
                        }
                        Some(closed) = closed_stream.next() => {
                            if let Ok(args) = closed.args() {
                                this.handle_notification_closed(args.id, args.reason).await;
                            }
                        }
                        else => break,
                    }
                }
            });
        }
    }

    /// Prompt user with interactive "Accept" / "Decline" notification buttons for incoming file transfer.
    pub async fn request_transfer_authorization(
        &self,
        peer_alias: &str,
        file_count: usize,
        total_bytes: u64,
    ) -> bool {
        let size_str = crate::events::format_size(total_bytes);
        let summary = "Incoming File Transfer";
        let body = format!("<b>{peer_alias}</b> wants to send {file_count} file(s) ({size_str})");
        let actions = ["accept", "Accept", "decline", "Decline"];

        let mut hints = HashMap::new();
        hints.insert("urgency", Value::from(1u8)); // Normal/High urgency
        hints.insert("category", Value::from("transfer"));

        if let Some(ref proxy) = self.proxy {
            let (tx, rx) = oneshot::channel();
            match proxy
                .notify(
                    "Arch Drop",
                    0,
                    "document-send",
                    summary,
                    &body,
                    &actions,
                    &hints,
                    30_000, // 30s timeout
                )
                .await
            {
                Ok(id) => {
                    self.pending_actions.lock().await.insert(id, tx);
                    // Wait up to 30s for user action click or timeout
                    tokio::select! {
                        res = rx => res.unwrap_or(false),
                        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                            let _ = self.pending_actions.lock().await.remove(&id);
                            let _ = proxy.close_notification(id).await;
                            false
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to dispatch transfer authorization notification: {e}");
                    true // Fallback to accept if notification service is uncontactable
                }
            }
        } else {
            true // No notification service available -> allow transfer
        }
    }

    /// Notify user when a file transfer successfully completes.
    pub async fn notify_transfer_completed(
        &self,
        file_name: &str,
        total_bytes: u64,
        peer_alias: &str,
        save_dir: Option<&Path>,
    ) {
        if let Some(ref proxy) = self.proxy {
            let size_str = crate::events::format_size(total_bytes);
            let summary = "File Transfer Complete";
            let body =
                format!("Successfully received <b>{file_name}</b> ({size_str}) from {peer_alias}");
            let mut hints = HashMap::new();
            hints.insert("urgency", Value::from(1u8));
            hints.insert("category", Value::from("transfer.complete"));
            let actions = ["open", "Open Folder"];

            match proxy
                .notify(
                    "Arch Drop",
                    0,
                    "document-save",
                    summary,
                    &body,
                    &actions,
                    &hints,
                    5_000,
                )
                .await
            {
                Ok(id) => {
                    let target = save_dir
                        .map(|p| p.to_path_buf())
                        .or_else(|| self.default_download_dir.try_lock().ok().and_then(|d| d.clone()))
                        .unwrap_or_else(|| PathBuf::from("."));
                    self.folder_actions.lock().await.insert(id, target);
                }
                Err(e) => {
                    warn!("Failed to dispatch transfer completed notification: {e}");
                }
            }
        }
    }

    /// Notify user when a file transfer fails.
    pub async fn notify_transfer_failed(&self, error: &str, peer_alias: &str) {
        if let Some(ref proxy) = self.proxy {
            let summary = "Transfer Failed";
            let body = format!("Transfer from <b>{peer_alias}</b> failed: {error}");
            let mut hints = HashMap::new();
            hints.insert("urgency", Value::from(2u8)); // Critical urgency
            let actions = ["dismiss", "Dismiss"];

            if let Err(e) = proxy
                .notify(
                    "Arch Drop",
                    0,
                    "dialog-error",
                    summary,
                    &body,
                    &actions,
                    &hints,
                    10_000,
                )
                .await
            {
                warn!("Failed to dispatch transfer failed notification: {e}");
            }
        }
    }

    /// Notify user when a new trusted peer device is paired.
    pub async fn notify_peer_paired(&self, alias: &str, fingerprint: &str) {
        if let Some(ref proxy) = self.proxy {
            let summary = "Device Paired";
            let fp_short = if fingerprint.len() > 16 {
                &fingerprint[..16]
            } else {
                fingerprint
            };
            let body = format!("Trusted peer <b>{alias}</b> ({fp_short}...) paired successfully");
            let mut hints = HashMap::new();
            hints.insert("urgency", Value::from(0u8)); // Low urgency
            let actions: [&str; 0] = [];

            if let Err(e) = proxy
                .notify(
                    "Arch Drop",
                    0,
                    "security-high",
                    summary,
                    &body,
                    &actions,
                    &hints,
                    6_000,
                )
                .await
            {
                warn!("Failed to dispatch peer paired notification: {e}");
            }
        }
    }

    /// Notify user when universal clipboard content is synchronized.
    pub async fn notify_clipboard_synced(&self, mime: &str, bytes: usize, peer_alias: &str) {
        if let Some(ref proxy) = self.proxy {
            let size_str = crate::events::format_size(bytes as u64);
            let summary = "Clipboard Synced";
            let body = format!("{size_str} ({mime}) synced from <b>{peer_alias}</b>");
            let mut hints = HashMap::new();
            hints.insert("urgency", Value::from(0u8)); // Low urgency
            let actions: [&str; 0] = [];

            if let Err(e) = proxy
                .notify(
                    "Arch Drop",
                    0,
                    "edit-paste",
                    summary,
                    &body,
                    &actions,
                    &hints,
                    2_500,
                )
                .await
            {
                warn!("Failed to dispatch clipboard notification: {e}");
            }
        }
    }
}
