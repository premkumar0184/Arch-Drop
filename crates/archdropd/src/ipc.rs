//! UNIX domain socket IPC server for Waybar streaming status and CLI management.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, watch, RwLock};
use tracing::{debug, info, warn};

pub use crate::events::WaybarPayload;

/// RAII Guard that removes the UNIX domain socket file when dropped.
#[derive(Debug)]
pub struct SocketGuard {
    path: PathBuf,
}

impl SocketGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            if let Err(e) = std::fs::remove_file(&self.path) {
                warn!("Failed to remove IPC socket {:?}: {e}", self.path);
            } else {
                debug!("Unlinked IPC socket: {:?}", self.path);
            }
        }
    }
}

/// Dynamic event payload streamed to Waybar and CLI clients over IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonStatusEvent {
    pub timestamp: u64,
    pub text: String,
    pub tooltip: String,
    pub class: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<f64>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub active_peers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_transfer: Option<String>,
}

#[allow(dead_code)]
impl DaemonStatusEvent {
    pub fn idle(active_peers: usize) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            timestamp: now,
            text: "󰄡 ArchDrop".to_string(),
            tooltip: format!("Arch Drop Daemon: Idle\nActive Peers: {}", active_peers),
            class: vec!["idle".to_string()],
            percentage: None,
            state: "idle".to_string(),
            active_peers,
            current_transfer: None,
        }
    }

    pub fn paired(peer_alias: &str, active_peers: usize) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            timestamp: now,
            text: format!("󰄡 {peer_alias}"),
            tooltip: format!("Arch Drop Daemon: Paired\nConnected to: {peer_alias}\nActive Peers: {active_peers}"),
            class: vec!["paired".to_string()],
            percentage: None,
            state: "paired".to_string(),
            active_peers,
            current_transfer: None,
        }
    }

    pub fn transferring(filename: &str, progress: f64, speed_bps: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let speed_mb = (speed_bps as f64) / (1024.0 * 1024.0);
        Self {
            timestamp: now,
            text: format!("󰄡 {:.0}%", progress),
            tooltip: format!("Transferring: {filename}\nProgress: {:.1}%\nSpeed: {:.2} MB/s", progress, speed_mb),
            class: vec!["transferring".to_string()],
            percentage: Some(progress),
            state: "transferring".to_string(),
            active_peers: 1,
            current_transfer: Some(filename.to_string()),
        }
    }

    pub fn to_waybar_payload(&self) -> WaybarPayload {
        WaybarPayload {
            text: self.text.clone(),
            tooltip: self.tooltip.clone(),
            class: self.class.clone(),
            percentage: self.percentage.map(|p| p.clamp(0.0, 100.0).round() as u8),
        }
    }
}

/// IPC server managing the UNIX domain socket lifecycle and client connections.
#[derive(Debug)]
pub struct IpcServer {
    listener: UnixListener,
    guard: SocketGuard,
    event_tx: broadcast::Sender<DaemonStatusEvent>,
    shutdown_rx: watch::Receiver<bool>,
    current_status: Arc<RwLock<DaemonStatusEvent>>,
}

impl IpcServer {
    /// Resolve default socket path:
    /// 1. $XDG_RUNTIME_DIR/archdrop/archdropd.sock (mode 0o700 directory)
    /// 2. /tmp/archdrop-$UID/archdropd.sock (fallback)
    pub fn default_socket_path() -> PathBuf {
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            let trimmed = runtime.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("archdrop").join("archdropd.sock");
            }
        }
        #[cfg(unix)]
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = 1000;
        PathBuf::from(format!("/tmp/archdrop-{}", uid)).join("archdropd.sock")
    }

    #[allow(dead_code)]
    pub fn bind_default(
        event_tx: broadcast::Sender<DaemonStatusEvent>,
        shutdown_rx: watch::Receiver<bool>,
        current_status: Arc<RwLock<DaemonStatusEvent>>,
    ) -> Result<Self> {
        let path = Self::default_socket_path();
        Self::bind(&path, event_tx, shutdown_rx, current_status)
    }

    pub fn bind(
        path: &Path,
        event_tx: broadcast::Sender<DaemonStatusEvent>,
        shutdown_rx: watch::Receiver<bool>,
        current_status: Arc<RwLock<DaemonStatusEvent>>,
    ) -> Result<Self> {
        // 1. Ensure parent directory exists with 0o700 permissions
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create IPC parent directory {:?}", parent))?;
            #[cfg(unix)]
            {
                if let Err(e) = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)) {
                    warn!("Failed to set permissions 0o700 on IPC dir {:?}: {e}", parent);
                }
            }
        }

        // 2. Probe existing socket file for active daemon vs stale file
        if path.exists() {
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    anyhow::bail!("archdropd daemon is already running (socket {:?} responsive)", path);
                }
                Err(e) => {
                    warn!(
                        socket = ?path,
                        err = %e,
                        "Detected unresponsive/stale IPC socket; unlinking"
                    );
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        // 3. Bind UNIX domain socket listener
        let listener = UnixListener::bind(path)
            .with_context(|| format!("Failed to bind UNIX domain socket at {:?}", path))?;

        // 4. Enforce strict 0o600 permissions (owner read/write only)
        #[cfg(unix)]
        {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Failed to set permissions 0o600 on {:?}", path))?;
        }

        let guard = SocketGuard::new(path.to_path_buf());
        info!(socket = ?path, "Bound IPC UNIX domain socket");

        Ok(Self {
            listener,
            guard,
            event_tx,
            shutdown_rx,
            current_status,
        })
    }

    pub fn socket_path(&self) -> &Path {
        self.guard.path()
    }

    pub async fn run(self) -> Result<()> {
        let mut shutdown = self.shutdown_rx;
        loop {
            tokio::select! {
                res = self.listener.accept() => {
                    match res {
                        Ok((stream, _)) => {
                            let rx = self.event_tx.subscribe();
                            let current_status = self.current_status.clone();
                            tokio::spawn(Self::handle_client(stream, rx, current_status));
                        }
                        Err(e) => {
                            warn!("IPC accept error: {e}");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("IPC server stopping on shutdown signal");
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_client(
        stream: UnixStream,
        mut rx: broadcast::Receiver<DaemonStatusEvent>,
        current_status: Arc<RwLock<DaemonStatusEvent>>,
    ) {
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let mut waybar_mode = false;

        // 1. Probe for an immediate client command (e.g. within 20ms)
        let initial_res = tokio::select! {
            cmd = lines.next_line() => Some(cmd),
            _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => None,
        };

        match initial_res {
            Some(Ok(Some(line))) => {
                let trimmed = line.trim();
                if Self::is_follow_cmd(trimmed) {
                    waybar_mode = true;
                    let payload = current_status.read().await.to_waybar_payload();
                    if let Ok(json) = serde_json::to_string(&payload) {
                        if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                            return;
                        }
                    }
                } else if trimmed.eq_ignore_ascii_case("status") {
                    let snapshot = current_status.read().await.clone();
                    if let Ok(json) = serde_json::to_string(&snapshot) {
                        let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
                    }
                } else if trimmed.eq_ignore_ascii_case("ping") {
                    let pong = serde_json::json!({
                        "type": "pong",
                        "timestamp": std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                    });
                    let _ = writer.write_all(format!("{pong}\n").as_bytes()).await;
                } else {
                    let snapshot = current_status.read().await.clone();
                    if let Ok(json) = serde_json::to_string(&snapshot) {
                        if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Some(Ok(None)) => {
                // Client disconnected immediately
                return;
            }
            Some(Err(e)) => {
                debug!("IPC client read error: {e}");
                return;
            }
            None => {
                // No command received within 20ms: send initial status snapshot (M1 default)
                let snapshot = current_status.read().await.clone();
                if let Ok(json) = serde_json::to_string(&snapshot) {
                    if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                        return;
                    }
                }
            }
        }

        // 2. Concurrently process client commands and broadcast event notifications
        loop {
            tokio::select! {
                line_res = lines.next_line() => {
                    match line_res {
                        Ok(Some(line)) => {
                            let trimmed = line.trim();
                            if Self::is_follow_cmd(trimmed) {
                                if !waybar_mode {
                                    waybar_mode = true;
                                    let payload = current_status.read().await.to_waybar_payload();
                                    if let Ok(json) = serde_json::to_string(&payload) {
                                        if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            } else if trimmed.eq_ignore_ascii_case("ping") {
                                let pong = serde_json::json!({
                                    "type": "pong",
                                    "timestamp": std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                });
                                if writer.write_all(format!("{pong}\n").as_bytes()).await.is_err() {
                                    break;
                                }
                            } else if trimmed.eq_ignore_ascii_case("status") {
                                if waybar_mode {
                                    let payload = current_status.read().await.to_waybar_payload();
                                    if let Ok(json) = serde_json::to_string(&payload) {
                                        if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                } else {
                                    let snapshot = current_status.read().await.clone();
                                    if let Ok(json) = serde_json::to_string(&snapshot) {
                                        if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            } else if !trimmed.is_empty() {
                                debug!("Received unhandled IPC query: {trimmed}");
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            debug!("IPC client read error: {e}");
                            break;
                        }
                    }
                }
                event_res = rx.recv() => {
                    match event_res {
                        Ok(event) => {
                            if waybar_mode {
                                let payload = event.to_waybar_payload();
                                if let Ok(json) = serde_json::to_string(&payload) {
                                    if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                                        break;
                                    }
                                }
                            } else if let Ok(json) = serde_json::to_string(&event) {
                                if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            debug!("IPC client lagged behind by {skipped} events");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
            }
        }
    }

    fn is_follow_cmd(cmd: &str) -> bool {
        let lower = cmd.to_ascii_lowercase();
        lower == "status --follow"
            || lower == "follow"
            || lower == "subscribe"
            || lower.contains("waybar")
            || lower.contains("\"subscribe\"")
            || lower.contains("\"follow\"")
    }
}
