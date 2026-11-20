//! Daemon orchestrator coordinating server, clipboard, mesh, and IPC loops.

use anyhow::{Context, Result};
use archdrop_clipboard::{
    ClipboardPayloadCache, ClipboardTtlManager, WaylandClipboardAdapter,
};
use archdrop_core::config::{expand_tilde, AppConfig, Config};
use archdrop_core::crypto::CryptoIdentity;
use archdrop_core::models::clipboard::ClipboardPacket;
use archdrop_transfer::TransferServer;
use archdrop_wireless::{
    BleClipboardPacket, BleConfig, BleController, GattServerState,
    CLIPBOARD_MIME_TEXT_PLAIN, CLIPBOARD_MIME_URI_LIST, MAX_BLE_CLIPBOARD_PAYLOAD,
};
use clap::Args;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch, RwLock};
use tracing::{debug, error, info, warn};
use crate::events::{DaemonEvent, DaemonState};
use crate::ipc::{DaemonStatusEvent, IpcServer};
use crate::notification::NotificationService;

#[derive(Args, Debug, Clone, Default)]
pub struct DaemonArgs {
    /// Override listening port (default: 53317)
    #[arg(short, long)]
    pub port: Option<u16>,

    /// Override bind address (default: 0.0.0.0)
    #[arg(short, long)]
    pub bind: Option<String>,

    /// Disable TLS encryption and run plain HTTP
    #[arg(long)]
    pub no_tls: bool,

    /// Connect to userspace DERP relay for WAN reachability
    #[arg(long)]
    pub relay: bool,

    /// Override DERP relay URL (e.g. wss://derp.archdrop.net:443/derp)
    #[arg(long)]
    pub relay_url: Option<String>,

    /// Custom UNIX domain socket path for IPC
    #[arg(long)]
    pub socket_path: Option<PathBuf>,
}

/// Dynamically resolves the paired BLE secret from Config or falls back to default.
pub fn resolve_paired_ble_secret_from_config(cfg: &Config, target_peer: Option<&str>) -> Vec<u8> {
    if let Some(target) = target_peer {
        let cleaned = target.replace([':', ' ', '-'], "");
        if cfg.is_peer_paired(&cleaned) || cfg.is_mesh_key_paired(&cleaned) {
            return archdrop_wireless::derive_paired_key(&cleaned).to_vec();
        }
    }
    if let Some(mesh_key) = cfg.mesh.paired_mesh_keys.first() {
        return archdrop_wireless::derive_paired_key(mesh_key).to_vec();
    }
    if let Some(fp) = cfg.security.paired_fingerprints.iter().next() {
        return archdrop_wireless::derive_paired_key(fp).to_vec();
    }
    if let Some(ref pin) = cfg.security.pin {
        return archdrop_wireless::derive_paired_key(pin).to_vec();
    }
    b"ARCHDROP_SECRET".to_vec()
}

/// Collects all candidate paired symmetric keys for authenticating incoming BLE GATT writes.
pub fn candidate_paired_keys(cfg: &Config, identity: Option<&CryptoIdentity>) -> Vec<Vec<u8>> {
    let mut keys = Vec::new();
    if let Some(id) = identity {
        keys.push(archdrop_wireless::derive_paired_key(&id.fingerprint).to_vec());
    }
    for fp in &cfg.security.paired_fingerprints {
        keys.push(archdrop_wireless::derive_paired_key(fp).to_vec());
    }
    for mk in &cfg.mesh.paired_mesh_keys {
        keys.push(archdrop_wireless::derive_paired_key(mk).to_vec());
    }
    if let Some(ref pin) = cfg.security.pin {
        keys.push(archdrop_wireless::derive_paired_key(pin).to_vec());
    }
    keys.push(b"ARCHDROP_SECRET".to_vec());
    keys
}

/// Pre-registered OS shutdown signal streams to ensure zero window for lost signals.
#[cfg(unix)]
pub struct ShutdownSignals {
    sigterm: tokio::signal::unix::Signal,
    sigint: tokio::signal::unix::Signal,
    sighup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    /// Register signal streams immediately with the OS runtime.
    pub fn register() -> Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};

        let sigterm = signal(SignalKind::terminate())
            .context("Failed to register SIGTERM signal handler")?;
        let sigint = signal(SignalKind::interrupt())
            .context("Failed to register SIGINT signal handler")?;
        let sighup = signal(SignalKind::hangup())
            .context("Failed to register SIGHUP signal handler")?;

        Ok(Self {
            sigterm,
            sigint,
            sighup,
        })
    }

    /// Wait for any of the registered shutdown signals.
    pub async fn wait(mut self) -> Result<()> {
        tokio::select! {
            _ = self.sigterm.recv() => {
                info!("SIGTERM received from systemd/init, initiating graceful shutdown");
            }
            _ = self.sigint.recv() => {
                info!("SIGINT (Ctrl+C) received, initiating graceful shutdown");
            }
            _ = self.sighup.recv() => {
                info!("SIGHUP received, initiating graceful shutdown");
            }
        }
        Ok(())
    }
}

#[cfg(not(unix))]
pub struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    pub fn register() -> Result<Self> {
        Ok(Self)
    }

    pub async fn wait(self) -> Result<()> {
        tokio::signal::ctrl_c()
            .await
            .context("Failed to listen for Ctrl+C signal")?;
        info!("Ctrl+C received, initiating graceful shutdown");
        Ok(())
    }
}

pub struct DaemonOrchestrator {
    app_config: AppConfig,
    identity: Arc<CryptoIdentity>,
    args: DaemonArgs,
}

impl DaemonOrchestrator {
    pub fn new(
        app_config: AppConfig,
        identity: CryptoIdentity,
        args: DaemonArgs,
    ) -> Self {
        Self {
            app_config,
            identity: Arc::new(identity),
            args,
        }
    }

    pub async fn run(self) -> Result<()> {
        let signals = ShutdownSignals::register()?;
        self.run_with_signals(signals).await
    }

    pub async fn run_with_signals(self, signals: ShutdownSignals) -> Result<()> {
        info!(
            alias = %self.app_config.config().effective_alias(),
            fingerprint = %self.identity.fingerprint,
            "Initializing Arch Drop Headless Daemon"
        );

        let config = Arc::new(self.app_config.config().clone());
        let port = self.args.port.unwrap_or(config.network.port);
        let use_tls = !self.args.no_tls && config.security.require_encryption;
        let download_dir = expand_tilde(&config.storage.download_dir);

        // 1. Initialize Transfer Server & Notification Service
        let mut transfer_server = TransferServer::new(
            config.clone(),
            self.identity.clone(),
            download_dir.clone(),
            port,
            use_tls,
        )
        .context("Failed to build TransferServer")?;

        let notification_service = NotificationService::connect().await;
        notification_service.set_default_download_dir(download_dir.clone());
        let (internal_event_tx, mut internal_event_rx) = broadcast::channel::<DaemonEvent>(64);

        let progress_tx = internal_event_tx.clone();
        transfer_server.set_progress_callback(Arc::new(move |p| {
            let _ = progress_tx.send(DaemonEvent::TransferProgress {
                session_id: p.session_id,
                file_name: p.file_name,
                bytes_transferred: p.current_bytes,
                total_bytes: p.total_bytes,
                speed_bps: 0,
                percentage: 0,
            });
        }));

        let completion_tx = internal_event_tx.clone();
        transfer_server.set_completion_callback(Arc::new(move |c| {
            let _ = completion_tx.send(DaemonEvent::TransferCompleted {
                session_id: c.session_id,
                peer_alias: c.peer_alias,
                file_name: c.file_name,
                file_count: 1,
                total_bytes: c.total_bytes,
            });
        }));

        let staged_notif = notification_service.clone();
        transfer_server.set_staged_clipboard_callback(Arc::new(move |_file_name, _final_path, total_bytes, peer_alias| {
            let notif = staged_notif.clone();
            Box::pin(async move {
                notif.notify_clipboard_synced("text/uri-list", total_bytes as usize, &peer_alias).await;
            })
        }));

        let failure_tx = internal_event_tx.clone();
        transfer_server.set_failure_callback(Arc::new(move |f| {
            let _ = failure_tx.send(DaemonEvent::TransferFailed {
                session_id: f.session_id,
                peer_alias: f.peer_alias,
                error: f.error,
            });
        }));

        let auth_notif = notification_service.clone();
        transfer_server.set_auth_callback(Arc::new(move |peer_alias, file_count, total_bytes| {
            let notif = auth_notif.clone();
            Box::pin(async move {
                notif.request_transfer_authorization(&peer_alias, file_count, total_bytes).await
            })
        }));

        // 2. Initialize Wayland Clipboard Adapter & TTL Manager
        let (packet_tx, _packet_rx) = broadcast::channel(32);
        let clipboard_cache = ClipboardPayloadCache::default();

        let device_id = config.effective_alias().to_string();
        let clipboard_adapter = WaylandClipboardAdapter::new(
            device_id.clone(),
            clipboard_cache.clone(),
            packet_tx.clone(),
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let ttl_manager = ClipboardTtlManager::new(device_id, clipboard_cache.clone(), packet_tx.clone());
        ttl_manager.spawn_ttl_loop(shutdown_rx.clone());

        // Connect clipboard adapter and cache to TransferServer
        transfer_server.set_clipboard_adapter(clipboard_adapter.clone());
        transfer_server.set_clipboard_cache(clipboard_cache.clone());

        // Connect virtual webcam sink to TransferServer if loopback device is discovered
        if let Ok(Some(loopback_dev)) = archdrop_webcam::discover_loopback_device(None) {
            match archdrop_webcam::LinuxV4l2Sink::open(&loopback_dev.device_path) {
                Ok(v4l2_sink) => {
                    let sink = Arc::new(archdrop_webcam::WebcamSink::new(Box::new(v4l2_sink)));
                    transfer_server.set_webcam_sink(sink);
                    info!(
                        device = ?loopback_dev.device_path,
                        label = %loopback_dev.card_label,
                        "Virtual webcam sink connected to transfer server"
                    );
                }
                Err(e) => {
                    warn!(
                        device = ?loopback_dev.device_path,
                        "Failed to open virtual webcam device on startup: {e}"
                    );
                }
            }
        }

        let server_clip_tx = internal_event_tx.clone();
        let server_clip_notif = notification_service.clone();
        transfer_server.set_clipboard_callback(Arc::new(move |packet| {
            let tx = server_clip_tx.clone();
            let notif = server_clip_notif.clone();
            tokio::spawn(async move {
                match packet {
                    ClipboardPacket::Inline(p) => {
                        let byte_size = p.payload.size_bytes;
                        let origin = p.device_id.clone();
                        let mime = p.payload.mime_type.clone();
                        let _ = tx.send(DaemonEvent::ClipboardSynced {
                            mime_type: mime.clone(),
                            byte_size,
                            origin_device: origin.clone(),
                        });
                        notif.notify_clipboard_synced(&mime, byte_size, &origin).await;
                    }
                    ClipboardPacket::Descriptor(p) => {
                        let byte_size = p.descriptor.size_bytes;
                        let origin = p.device_id.clone();
                        let mime = p.descriptor.primary_mime.clone();
                        let _ = tx.send(DaemonEvent::ClipboardSynced {
                            mime_type: mime.clone(),
                            byte_size,
                            origin_device: origin.clone(),
                        });
                        notif.notify_clipboard_synced(&mime, byte_size, &origin).await;
                    }
                    ClipboardPacket::Clear(_) => {}
                }
            });
        }));

        // Initialize BLE GATT Server and Controller for Zero-Wi-Fi Clipboard Sync
        let (ble_clip_tx, mut ble_clip_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
        let ble_clipboard_payload = Arc::new(tokio::sync::RwLock::new(None));
        let gatt_state = GattServerState {
            clipboard_payload: ble_clipboard_payload.clone(),
            clipboard_write_tx: Some(ble_clip_tx.clone()),
            ..Default::default()
        };

        let ble_controller: Option<Arc<BleController>> = match BleController::init(BleConfig::default()).await {
            Ok(mut ctrl) => {
                if let Err(e) = ctrl.serve_gatt(gatt_state).await {
                    warn!("Failed to register local BLE GATT application: {e}");
                } else {
                    info!("Registered local Arch Drop GATT application with CHAR_CLIPBOARD_SYNC");
                }
                Some(Arc::new(ctrl))
            }
            Err(e) => {
                warn!("Bluetooth LE controller unavailable: {e}");
                None
            }
        };

        // Subscribe to incoming BLE companion clipboard notifications
        if let Some(ref ctrl) = ble_controller {
            let ctrl_sub = ctrl.clone();
            let clip_tx_sub = ble_clip_tx.clone();
            let mut sub_shutdown = shutdown_rx.clone();
            tokio::spawn(async move {
                let mut peer_rx = match ctrl_sub.scan_for_peers().await {
                    Ok(rx) => {
                        info!("Active background BLE discovery scanner initialized");
                        Some(rx)
                    }
                    Err(e) => {
                        debug!("Could not start background LE discovery scanner: {e}");
                        None
                    }
                };

                let mut active_subs: std::collections::HashMap<archdrop_wireless::Address, tokio::task::JoinHandle<()>> =
                    std::collections::HashMap::new();
                let mut ticker = tokio::time::interval(Duration::from_secs(5));
                loop {
                    tokio::select! {
                        Some(dev) = async {
                            match peer_rx.as_mut() {
                                Some(rx) => rx.recv().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            if dev.address == archdrop_wireless::Address::any() {
                                continue;
                            }
                            if dev.is_archdrop_service || dev.proximity_beacon.is_some() || dev.hotspot_status.is_some() {
                                if !active_subs.contains_key(&dev.address) {
                                    match ctrl_sub.subscribe_clipboard_notifications(dev.address, clip_tx_sub.clone()).await {
                                        Ok(handle) => {
                                            info!(peer = %dev.address, name = ?dev.name, "Subscribed to BLE companion GATT clipboard notifications");
                                            active_subs.insert(dev.address, handle);
                                        }
                                        Err(e) => {
                                            debug!(peer = %dev.address, "Could not subscribe to clipboard notifications on {}: {e}", dev.address);
                                        }
                                    }
                                }
                            }
                        }
                        _ = ticker.tick() => {
                            // Prune subscriptions where the notification stream ended (e.g. peer disconnect)
                            active_subs.retain(|addr, handle| {
                                if handle.is_finished() {
                                    debug!(peer = %addr, "BLE clipboard notification stream terminated; releasing for reconnection");
                                    false
                                } else {
                                    true
                                }
                            });
                        }
                        _ = sub_shutdown.changed() => {
                            if *sub_shutdown.borrow() {
                                for (_, handle) in active_subs {
                                    handle.abort();
                                }
                                break;
                            }
                        }
                    }
                }
            });
        }

        // Ingest incoming BLE GATT clipboard writes into Wayland selection & desktop notification
        let ble_ingest_adapter = clipboard_adapter.clone();
        let ble_ingest_notif = notification_service.clone();
        let ble_ingest_event_tx = internal_event_tx.clone();
        let ble_ingest_config = config.clone();
        let ble_ingest_identity = self.identity.clone();
        let ble_ingest_payload = ble_clipboard_payload.clone();
        let mut ble_shutdown = shutdown_rx.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    raw_opt = ble_clip_rx.recv() => {
                        let raw = match raw_opt {
                            Some(r) => r,
                            None => break,
                        };

                        let parsed = match BleClipboardPacket::parse(&raw) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!("Failed to parse incoming BLE GATT clipboard packet: {e}");
                                continue;
                            }
                        };

                        // 1. Authenticate the 4-byte auth token in constant time
                        let candidate_keys = candidate_paired_keys(&ble_ingest_config, Some(&ble_ingest_identity));
                        if !parsed.verify_against_keys(&candidate_keys) {
                            warn!("BLE GATT clipboard sync rejected: invalid HMAC auth token");
                            continue;
                        }

                        // 2. Validate SHA-256 deduplication to suppress echo loops
                        let hash = archdrop_clipboard::compute_sha256(&parsed.payload);
                        if ble_ingest_adapter.is_recent_paste(&hash) {
                            debug!(sha256 = %hash, "Echo suppressed on BLE GATT write: matches recent paste");
                            continue;
                        }
                        ble_ingest_adapter.set_last_pasted_hash(hash.clone());

                        // 3. Ingest into local Wayland clipboard selection (wl-copy)
                        let mime = parsed.mime_str().to_string();
                        let byte_size = parsed.payload.len();
                        if let Err(e) = ble_ingest_adapter.paste_remote_payload(&mime, &parsed.payload) {
                            warn!("Failed to paste BLE clipboard payload to Wayland: {e}");
                        } else {
                            *ble_ingest_payload.write().await = Some(raw.clone());
                            info!(mime = %mime, bytes = byte_size, "Ingested BLE GATT clipboard payload into Wayland");
                        }

                        // 4. Dispatch desktop notification and daemon event
                        let origin = "BLE Companion".to_string();
                        let _ = ble_ingest_event_tx.send(DaemonEvent::ClipboardSynced {
                            mime_type: mime.clone(),
                            byte_size,
                            origin_device: origin.clone(),
                        });
                        ble_ingest_notif.notify_clipboard_synced(&mime, byte_size, &origin).await;
                    }
                    _ = ble_shutdown.changed() => {
                        if *ble_shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // Wire clipboard listener for events, desktop notification, and network broadcasting
        let registered_peers = transfer_server.registered_peers();
        let broadcast_config = config.clone();
        let broadcast_identity = self.identity.clone();
        let broadcast_adapter = clipboard_adapter.clone();
        let ble_broadcast = ble_controller.clone();
        let ble_payload_broadcast = ble_clipboard_payload.clone();

        let mut clipboard_rx = packet_tx.subscribe();
        let clipboard_tx = internal_event_tx.clone();
        let clipboard_notif = notification_service.clone();
        let clipboard_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut shutdown = clipboard_shutdown;
            loop {
                tokio::select! {
                    res = clipboard_rx.recv() => {
                        match res {
                            Ok(packet) => {
                                // 1. Echo suppression: check if the packet's SHA256 matches last_pasted_sha256
                                let packet_hash = match &packet {
                                    ClipboardPacket::Inline(p) => Some(p.payload.sha256.clone()),
                                    ClipboardPacket::Descriptor(p) => Some(p.descriptor.sha256.clone()),
                                    ClipboardPacket::Clear(_) => None,
                                };

                                if let Some(ref h) = packet_hash {
                                    if broadcast_adapter.is_recent_paste(h) {
                                        debug!("Echo suppressed in broadcaster: matches recent remote paste");
                                        continue;
                                    }
                                    broadcast_adapter.set_last_pasted_hash(h.clone());
                                }

                                // 2. Notify internal daemon event and DBus desktop notification
                                match &packet {
                                    ClipboardPacket::Inline(p) => {
                                        let byte_size = p.payload.size_bytes;
                                        let origin = p.device_id.clone();
                                        let mime = p.payload.mime_type.clone();
                                        let _ = clipboard_tx.send(DaemonEvent::ClipboardSynced {
                                            mime_type: mime.clone(),
                                            byte_size,
                                            origin_device: origin.clone(),
                                        });
                                        clipboard_notif.notify_clipboard_synced(&mime, byte_size, &origin).await;
                                    }
                                    ClipboardPacket::Descriptor(p) => {
                                        let byte_size = p.descriptor.size_bytes;
                                        let origin = p.device_id.clone();
                                        let mime = p.descriptor.primary_mime.clone();
                                        let _ = clipboard_tx.send(DaemonEvent::ClipboardSynced {
                                            mime_type: mime.clone(),
                                            byte_size,
                                            origin_device: origin.clone(),
                                        });
                                        clipboard_notif.notify_clipboard_synced(&mime, byte_size, &origin).await;
                                    }
                                    ClipboardPacket::Clear(_) => {}
                                }

                                // 3. Zero-Wi-Fi BLE GATT Clipboard Sync
                                // If payload is text (<= 512 bytes), in addition to LAN HTTP broadcast,
                                // connect/write directly to nearby BLE companion device's CHAR_CLIPBOARD_SYNC characteristic.
                                if let Some(ref ble) = ble_broadcast {
                                    if let ClipboardPacket::Inline(ref p) = packet {
                                        if p.payload.size_bytes <= MAX_BLE_CLIPBOARD_PAYLOAD
                                            && (p.payload.mime_type.starts_with("text/") || p.payload.mime_type == "text/uri-list")
                                        {
                                            let mime_code = if p.payload.mime_type == "text/uri-list" {
                                                CLIPBOARD_MIME_URI_LIST
                                            } else {
                                                CLIPBOARD_MIME_TEXT_PLAIN
                                            };
                                            let paired_key = resolve_paired_ble_secret_from_config(&broadcast_config, None);
                                            if let Ok(ble_pkt) = BleClipboardPacket::new(&paired_key, mime_code, &p.payload.content) {
                                                *ble_payload_broadcast.write().await = Some(ble_pkt.to_bytes());
                                                let ble_clone = ble.clone();
                                                tokio::spawn(async move {
                                                    match ble_clone.broadcast_clipboard_sync_gatt(&ble_pkt).await {
                                                        Ok(count) => {
                                                            if count > 0 {
                                                                debug!("Synchronized clipboard text to {count} BLE companion(s) via GATT");
                                                            }
                                                        }
                                                        Err(e) => {
                                                            debug!("BLE GATT clipboard broadcast error: {e}");
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    }
                                }

                                // 4. Network Broadcast to all configured trusted paired peers over HTTP POST (/api/v2/clipboard)
                                // Per R1: broadcast captured clipboard packets (Inline / Descriptor)
                                match &packet {
                                    ClipboardPacket::Inline(_) | ClipboardPacket::Descriptor(_) => {}
                                    ClipboardPacket::Clear(_) => {
                                        debug!("Skipping network broadcast for compositor clear event");
                                        continue;
                                    }
                                }

                                let mut target_peers: std::collections::HashSet<(String, u16)> = std::collections::HashSet::new();

                                // WireGuard static peer IP
                                if let Some(ref ip) = broadcast_config.network.wireguard_peer_ip {
                                    target_peers.insert((ip.clone(), 53317));
                                }

                                // Configured network peers
                                for p in &broadcast_config.network.peers {
                                    if let Ok(addr) = p.parse::<std::net::SocketAddr>() {
                                        target_peers.insert((addr.ip().to_string(), addr.port()));
                                    } else if let Some((h, port_str)) = p.split_once(':') {
                                        if let Ok(port_val) = port_str.parse::<u16>() {
                                            target_peers.insert((h.to_string(), port_val));
                                        }
                                    } else {
                                        target_peers.insert((p.clone(), 53317));
                                    }
                                }

                                // Configured clipboard peers
                                for p in &broadcast_config.clipboard.peers {
                                    if let Ok(addr) = p.parse::<std::net::SocketAddr>() {
                                        target_peers.insert((addr.ip().to_string(), addr.port()));
                                    } else if let Some((h, port_str)) = p.split_once(':') {
                                        if let Ok(port_val) = port_str.parse::<u16>() {
                                            target_peers.insert((h.to_string(), port_val));
                                        }
                                    } else {
                                        target_peers.insert((p.clone(), 53317));
                                    }
                                }

                                // Registered peers from TransferServer
                                {
                                    let peers = registered_peers.read().await;
                                    for ep in peers.values() {
                                        let is_paired = match ep.fingerprint {
                                            Some(ref fp) => broadcast_config.is_peer_paired(fp),
                                            None => broadcast_config.security.paired_fingerprints.is_empty(),
                                        };
                                        if is_paired || broadcast_config.security.paired_fingerprints.is_empty() {
                                            target_peers.insert((ep.ip.clone(), ep.port));
                                        }
                                    }
                                }

                                // Broadcast to all resolved target peers
                                for (target_ip, target_port) in target_peers {
                                    if (target_ip == "127.0.0.1"
                                        || target_ip == "::1"
                                        || target_ip == "localhost"
                                        || target_ip == "0.0.0.0"
                                        || target_ip == "10.2.0.2")
                                        && target_port == port
                                    {
                                        continue;
                                    }
                                    let packet_clone = packet.clone();
                                    let client = archdrop_transfer::TransferClient::new();
                                    let local_fp = broadcast_identity.fingerprint.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = client.send_clipboard(&target_ip, target_port, false, None, Some(&local_fp), &packet_clone).await {
                                            debug!(target = %target_ip, port = target_port, "Failed to broadcast clipboard to peer: {e}");
                                        } else {
                                            info!(target = %target_ip, port = target_port, "Broadcasted clipboard to peer successfully");
                                        }
                                    });
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        if WaylandClipboardAdapter::is_wayland_available() {
            let download_base = format!("http://127.0.0.1:{}", port);
            if let Err(e) = clipboard_adapter.spawn_watcher(Some(download_base), shutdown_rx.clone()) {
                warn!("Could not start Wayland clipboard watcher: {}", e);
            } else {
                info!("Wayland unfocused clipboard monitoring active");
            }
        } else {
            info!("No Wayland session detected; clipboard watcher disabled");
        }

        // 3. Connect to userspace DERP relay if requested via --relay or config.mesh.enabled
        let mut derp_client_opt = None;
        if self.args.relay || config.mesh.enabled {
            let relay_url = self
                .args
                .relay_url
                .as_deref()
                .unwrap_or(&config.mesh.relay_url);

            let mesh_key_path = Config::default_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("mesh_key.pk8");

            match archdrop_mesh::crypto::MeshKeyPair::load_or_generate(&mesh_key_path) {
                Ok(mesh_key) => {
                    let mesh_key = Arc::new(mesh_key);
                    info!(
                        relay_url = %relay_url,
                        local_mesh_key = %mesh_key.public_key(),
                        "Connecting to userspace DERP relay"
                    );
                    match archdrop_mesh::client::DerpClient::connect(
                        archdrop_mesh::client::DerpClientConfig::new(relay_url, mesh_key),
                    )
                    .await
                    {
                        Ok(client) => {
                            info!("Connected to DERP relay successfully");
                            derp_client_opt = Some(client);
                        }
                        Err(e) => {
                            warn!("Could not connect to DERP relay (continuing LAN-only): {e}");
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to load or generate mesh key (continuing LAN-only): {e}");
                }
            }
        }

        // 4. Initialize IPC Server (UNIX domain socket)
        let (event_tx, _) = broadcast::channel::<DaemonStatusEvent>(64);
        let current_status = Arc::new(RwLock::new(DaemonStatusEvent::idle(
            config.security.paired_fingerprints.len(),
        )));

        let ipc_server = if let Some(ref custom_path) = self.args.socket_path {
            IpcServer::bind(custom_path, event_tx.clone(), shutdown_rx.clone(), current_status.clone())
                .context("Failed to bind custom IPC server socket")?
        } else {
            IpcServer::bind_default(event_tx.clone(), shutdown_rx.clone(), current_status.clone())
                .context("Failed to bind default IPC server socket")?
        };

        info!(socket = ?ipc_server.socket_path(), "IPC server initialized");
        let ipc_task = tokio::spawn(async move {
            if let Err(e) = ipc_server.run().await {
                error!("IPC server terminated with error: {e}");
            }
        });

        // 5. Spawn Transfer Server
        let derp_listener_opt = derp_client_opt.as_ref().map(|c| c.listener());
        let server_shutdown_rx = shutdown_rx.clone();
        let daemon_webcam_sink = transfer_server.webcam_sink();
        let daemon_is_audio_streaming = transfer_server.is_audio_streaming();
        let server_task = tokio::spawn(async move {
            if let Err(e) = transfer_server.run_with_mesh(derp_listener_opt, server_shutdown_rx).await {
                error!("TransferServer terminated with error: {}", e);
            }
        });

        // 6. Spawn State Aggregator Loop
        let mut daemon_state = DaemonState::new(
            config.security.paired_fingerprints.len(),
            port,
            derp_client_opt.is_some(),
        );
        let state_status = current_status.clone();
        let state_event_tx = event_tx.clone();
        let state_notif = notification_service.clone();
        let download_dir_notif = download_dir.clone();
        let mut state_shutdown = shutdown_rx.clone();

        let state_task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(250));
            loop {
                tokio::select! {
                    res = internal_event_rx.recv() => {
                        match res {
                            Ok(event) => {
                                let now = std::time::Instant::now();
                                match &event {
                                    DaemonEvent::TransferCompleted { file_name, total_bytes, peer_alias, .. } => {
                                        state_notif.notify_transfer_completed(file_name, *total_bytes, peer_alias, Some(&download_dir_notif)).await;
                                    }
                                    DaemonEvent::TransferFailed { peer_alias, error, .. } => {
                                        state_notif.notify_transfer_failed(error, peer_alias).await;
                                    }
                                    _ => {}
                                }
                                let _ = daemon_state.apply_event(&event, now);
                                let status_event = daemon_state.to_daemon_status_event(now);
                                *state_status.write().await = status_event.clone();
                                let _ = state_event_tx.send(status_event);
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = ticker.tick() => {
                        let now = std::time::Instant::now();
                        daemon_state.prune_transient(now);
                        let status_event = daemon_state.to_daemon_status_event(now);
                        let mut current = state_status.write().await;
                        if *current != status_event {
                            *current = status_event.clone();
                            drop(current);
                            let _ = state_event_tx.send(status_event);
                        }
                    }
                    _ = state_shutdown.changed() => {
                        if *state_shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        info!(
            port = port,
            tls = use_tls,
            relay = derp_client_opt.is_some(),
            "Arch Drop daemon running. Ready for incoming requests."
        );

        // 7. Wait for shutdown signal using pre-registered streams
        signals.wait().await?;

        info!("Shutdown signal received, initiating graceful drainage...");
        if let Some(sink) = daemon_webcam_sink.read().await.as_ref() {
            sink.stop();
        }
        if daemon_is_audio_streaming.load(std::sync::atomic::Ordering::SeqCst) {
            info!("Halting active audio streaming session on daemon shutdown");
            daemon_is_audio_streaming.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        let _ = shutdown_tx.send(true);

        // 8. Drain subsystem tasks with 5-second timeout
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                warn!("Drain timeout (5s) elapsed; forcing daemon exit");
            }
            res = async {
                let _ = server_task.await;
                let _ = ipc_task.await;
                let _ = state_task.await;
            } => {
                let () = res;
                info!("All daemon subsystems drained and terminated cleanly");
            }
        }

        info!("Arch Drop daemon terminated cleanly");
        Ok(())
    }

    pub async fn wait_for_shutdown_signal() -> Result<()> {
        let signals = ShutdownSignals::register()?;
        signals.wait().await
    }
}

pub async fn run_daemon(
    app_config: AppConfig,
    identity: CryptoIdentity,
    args: DaemonArgs,
) -> Result<()> {
    // Register signal streams immediately at the start of run_daemon before spawning subsystems
    // so there is zero window where a signal can be lost.
    let signals = ShutdownSignals::register()?;
    DaemonOrchestrator::new(app_config, identity, args)
        .run_with_signals(signals)
        .await
}
