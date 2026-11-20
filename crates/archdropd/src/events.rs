//! Event aggregation, speed engine (EMA alpha=0.35), and Waybar JSON state machine.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::ipc::DaemonStatusEvent;

/// Dynamic JSON payload streamed to Waybar's custom module.
///
/// Complies with Waybar's `return-type: json` custom module specification:
/// - `text`: short string displayed directly on the bar.
/// - `tooltip`: multiline Pango markup string displayed on hover.
/// - `class`: dynamic CSS class names applied to `#custom-archdrop`.
/// - `percentage`: progress percentage (0-100) for progress bar styling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaybarPayload {
    pub text: String,
    pub tooltip: String,
    pub class: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<u8>,
}

/// Transfer direction for active sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Inbound,
    Outbound,
}

/// Internal daemon events broadcast across subsystems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonEvent {
    PeerDiscovered {
        alias: String,
        address: String,
        transport: String,
    },
    PeerConnected {
        alias: String,
        address: String,
        fingerprint: String,
    },
    PeerDisconnected {
        alias: String,
    },
    TransferStarted {
        session_id: String,
        peer_alias: String,
        direction: Direction,
        total_files: usize,
        total_bytes: u64,
    },
    TransferProgress {
        session_id: String,
        file_name: String,
        bytes_transferred: u64,
        total_bytes: u64,
        speed_bps: u64,
        percentage: u8,
    },
    TransferCompleted {
        session_id: String,
        peer_alias: String,
        file_name: String,
        file_count: usize,
        total_bytes: u64,
    },
    TransferFailed {
        session_id: String,
        peer_alias: String,
        error: String,
    },
    ClipboardSynced {
        mime_type: String,
        byte_size: usize,
        origin_device: String,
    },
}

/// Exponential Moving Average (EMA) speed calculator ($\alpha = 0.35$).
#[derive(Debug, Clone)]
pub struct SpeedEngine {
    alpha: f64,
    current_speed_bps: f64,
    last_update: Option<Instant>,
    last_bytes: u64,
}

impl Default for SpeedEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl SpeedEngine {
    pub fn new() -> Self {
        Self {
            alpha: 0.35,
            current_speed_bps: 0.0,
            last_update: None,
            last_bytes: 0,
        }
    }

    pub fn with_alpha(alpha: f64) -> Self {
        Self {
            alpha,
            current_speed_bps: 0.0,
            last_update: None,
            last_bytes: 0,
        }
    }

    pub fn reset(&mut self) {
        self.current_speed_bps = 0.0;
        self.last_update = None;
        self.last_bytes = 0;
    }

    /// Update EMA speed with new transferred bytes count at given timestamp.
    /// Returns the smoothed speed in bytes per second.
    pub fn update(&mut self, bytes_transferred: u64, now: Instant) -> u64 {
        if let Some(last_time) = self.last_update {
            let elapsed = now.duration_since(last_time).as_secs_f64();
            if elapsed > 0.0 {
                let delta_bytes = bytes_transferred.saturating_sub(self.last_bytes);
                let inst_speed = (delta_bytes as f64) / elapsed;
                if self.current_speed_bps == 0.0 {
                    self.current_speed_bps = inst_speed;
                } else {
                    self.current_speed_bps =
                        self.alpha * inst_speed + (1.0 - self.alpha) * self.current_speed_bps;
                }
                self.last_update = Some(now);
                self.last_bytes = bytes_transferred;
            }
        } else {
            self.last_update = Some(now);
            self.last_bytes = bytes_transferred;
        }
        self.current_speed_bps.round() as u64
    }

    pub fn current_speed_bps(&self) -> u64 {
        self.current_speed_bps.round() as u64
    }
}

/// Format bytes per second into human-readable network speed.
pub fn format_speed(bps: u64) -> String {
    let bytes = bps as f64;
    if bytes < 1024.0 {
        format!("{:.0} B/s", bytes)
    } else if bytes < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", bytes / 1024.0)
    } else if bytes < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bytes / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB/s", bytes / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format byte counts into human-readable storage units.
pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1024.0 {
        format!("{} B", bytes)
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", b / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Calculate estimated time of arrival (ETA) given transferred/total bytes and speed.
pub fn calculate_eta(
    bytes_transferred: u64,
    total_bytes: u64,
    speed_bps: u64,
) -> Option<Duration> {
    if total_bytes <= bytes_transferred || speed_bps == 0 {
        return None;
    }
    let remaining = total_bytes - bytes_transferred;
    let secs = (remaining as f64) / (speed_bps as f64);
    Some(Duration::from_secs(secs.ceil() as u64))
}

/// Format an ETA duration into compact string format.
pub fn format_eta(eta: Option<Duration>) -> String {
    match eta {
        Some(d) => {
            let total_secs = d.as_secs();
            if total_secs < 60 {
                format!("{}s", total_secs)
            } else if total_secs < 3600 {
                format!("{}m {}s", total_secs / 60, total_secs % 60)
            } else {
                format!("{}h {}m", total_secs / 3600, (total_secs % 3600) / 60)
            }
        }
        None => "--".to_string(),
    }
}

/// Calculate percentage bounded in 0..=100.
pub fn calculate_percentage(bytes_transferred: u64, total_bytes: u64) -> u8 {
    if total_bytes == 0 {
        0
    } else {
        let pct = (bytes_transferred as f64 * 100.0 / total_bytes as f64).round() as u64;
        std::cmp::min(100, pct) as u8
    }
}

/// Active in-flight transfer tracking.
#[derive(Debug, Clone)]
pub struct ActiveTransfer {
    pub session_id: String,
    pub peer_alias: String,
    pub direction: Direction,
    pub file_name: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub speed_bps: u64,
    pub percentage: u8,
}

/// Transient visual state displayed with an expiration timer.
#[derive(Debug, Clone)]
pub enum TransientVisual {
    Completed {
        file_name: String,
        total_bytes: u64,
        peer_alias: String,
        until: Instant,
    },
    Clipboard {
        mime_type: String,
        byte_size: usize,
        origin_device: String,
        until: Instant,
    },
    Error {
        peer_alias: String,
        error: String,
        until: Instant,
    },
}

/// Aggregated daemon state engine that produces WaybarPayloads.
#[derive(Debug, Clone)]
pub struct DaemonState {
    pub paired_peers: usize,
    pub active_peer_alias: Option<String>,
    pub port: u16,
    pub relay_connected: bool,
    pub active_transfer: Option<ActiveTransfer>,
    pub transient: Option<TransientVisual>,
    pub speed_engine: SpeedEngine,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new(0, 53317, false)
    }
}

impl DaemonState {
    pub fn new(paired_peers: usize, port: u16, relay_connected: bool) -> Self {
        Self {
            paired_peers,
            active_peer_alias: None,
            port,
            relay_connected,
            active_transfer: None,
            transient: None,
            speed_engine: SpeedEngine::new(),
        }
    }

    /// Process an incoming DaemonEvent and update state. Returns the resulting WaybarPayload.
    pub fn apply_event(&mut self, event: &DaemonEvent, now: Instant) -> WaybarPayload {
        match event {
            DaemonEvent::PeerDiscovered { .. } => {}
            DaemonEvent::PeerConnected { alias, .. } => {
                self.active_peer_alias = Some(alias.clone());
            }
            DaemonEvent::PeerDisconnected { alias } => {
                if self.active_peer_alias.as_deref() == Some(alias) {
                    self.active_peer_alias = None;
                }
            }
            DaemonEvent::TransferStarted {
                session_id,
                peer_alias,
                direction,
                total_bytes,
                ..
            } => {
                self.speed_engine.reset();
                self.transient = None;
                self.active_transfer = Some(ActiveTransfer {
                    session_id: session_id.clone(),
                    peer_alias: peer_alias.clone(),
                    direction: *direction,
                    file_name: "transfer".to_string(),
                    bytes_transferred: 0,
                    total_bytes: *total_bytes,
                    speed_bps: 0,
                    percentage: 0,
                });
            }
            DaemonEvent::TransferProgress {
                session_id,
                file_name,
                bytes_transferred,
                total_bytes,
                speed_bps,
                percentage,
            } => {
                let smoothed_speed = if *speed_bps > 0 {
                    *speed_bps
                } else {
                    self.speed_engine.update(*bytes_transferred, now)
                };

                let pct = if *percentage > 0 {
                    *percentage
                } else {
                    calculate_percentage(*bytes_transferred, *total_bytes)
                };

                if let Some(ref mut tx) = self.active_transfer {
                    if tx.session_id == *session_id {
                        tx.file_name = file_name.clone();
                        tx.bytes_transferred = *bytes_transferred;
                        tx.total_bytes = *total_bytes;
                        tx.speed_bps = smoothed_speed;
                        tx.percentage = pct;
                    } else {
                        // Reset speed engine and replace stale/previous active transfer
                        self.speed_engine.reset();
                        self.active_transfer = Some(ActiveTransfer {
                            session_id: session_id.clone(),
                            peer_alias: "Peer".to_string(),
                            direction: Direction::Inbound,
                            file_name: file_name.clone(),
                            bytes_transferred: *bytes_transferred,
                            total_bytes: *total_bytes,
                            speed_bps: smoothed_speed,
                            percentage: pct,
                        });
                    }
                } else {
                    self.active_transfer = Some(ActiveTransfer {
                        session_id: session_id.clone(),
                        peer_alias: "Peer".to_string(),
                        direction: Direction::Inbound,
                        file_name: file_name.clone(),
                        bytes_transferred: *bytes_transferred,
                        total_bytes: *total_bytes,
                        speed_bps: smoothed_speed,
                        percentage: pct,
                    });
                }
            }
            DaemonEvent::TransferCompleted {
                peer_alias,
                file_name,
                total_bytes,
                ..
            } => {
                self.active_transfer = None;
                self.speed_engine.reset();
                self.transient = Some(TransientVisual::Completed {
                    file_name: file_name.clone(),
                    total_bytes: *total_bytes,
                    peer_alias: peer_alias.clone(),
                    until: now + Duration::from_secs(5),
                });
            }
            DaemonEvent::TransferFailed {
                peer_alias,
                error,
                ..
            } => {
                self.active_transfer = None;
                self.speed_engine.reset();
                self.transient = Some(TransientVisual::Error {
                    peer_alias: peer_alias.clone(),
                    error: error.clone(),
                    until: now + Duration::from_secs(5),
                });
            }
            DaemonEvent::ClipboardSynced {
                mime_type,
                byte_size,
                origin_device,
            } => {
                self.transient = Some(TransientVisual::Clipboard {
                    mime_type: mime_type.clone(),
                    byte_size: *byte_size,
                    origin_device: origin_device.clone(),
                    until: now + Duration::from_secs(3),
                });
            }
        }

        self.prune_transient(now);
        self.to_waybar_payload(now)
    }

    /// Prune expired transient states.
    pub fn prune_transient(&mut self, now: Instant) {
        if let Some(ref t) = self.transient {
            let expired = match t {
                TransientVisual::Completed { until, .. } => now >= *until,
                TransientVisual::Clipboard { until, .. } => now >= *until,
                TransientVisual::Error { until, .. } => now >= *until,
            };
            if expired {
                self.transient = None;
            }
        }
    }

    /// Render current state as a WaybarPayload.
    pub fn to_waybar_payload(&self, now: Instant) -> WaybarPayload {
        // 1. Active Transfer State (highest precedence)
        if let Some(ref tx) = self.active_transfer {
            let speed_str = format_speed(tx.speed_bps);
            let transferred_str = format_size(tx.bytes_transferred);
            let total_str = format_size(tx.total_bytes);
            let eta = calculate_eta(tx.bytes_transferred, tx.total_bytes, tx.speed_bps);
            let eta_str = format_eta(eta);

            match tx.direction {
                Direction::Inbound => {
                    let text = format!("󰇚 {} ({}%)", speed_str, tx.percentage);
                    let tooltip = format!(
                        "<b>󰇚 Receiving</b>: {}\n<b>From</b>: {}\n<b>Progress</b>: {} / {} ({}%)\n<b>Speed</b>: {} | <b>ETA</b>: {}",
                        tx.file_name, tx.peer_alias, transferred_str, total_str, tx.percentage, speed_str, eta_str
                    );
                    return WaybarPayload {
                        text,
                        tooltip,
                        class: vec!["transferring".to_string(), "inbound".to_string()],
                        percentage: Some(tx.percentage),
                    };
                }
                Direction::Outbound => {
                    let text = format!("󰕒 {} ({}%)", speed_str, tx.percentage);
                    let tooltip = format!(
                        "<b>󰕒 Sending</b>: {}\n<b>To</b>: {}\n<b>Progress</b>: {} / {} ({}%)\n<b>Speed</b>: {} | <b>ETA</b>: {}",
                        tx.file_name, tx.peer_alias, transferred_str, total_str, tx.percentage, speed_str, eta_str
                    );
                    return WaybarPayload {
                        text,
                        tooltip,
                        class: vec!["transferring".to_string(), "outbound".to_string()],
                        percentage: Some(tx.percentage),
                    };
                }
            }
        }

        // 2. Transient Visual State (Completed / Clipboard / Error)
        if let Some(ref t) = self.transient {
            let expired = match t {
                TransientVisual::Completed { until, .. } => now >= *until,
                TransientVisual::Clipboard { until, .. } => now >= *until,
                TransientVisual::Error { until, .. } => now >= *until,
            };
            if !expired {
                match t {
                    TransientVisual::Completed {
                        file_name,
                        total_bytes,
                        peer_alias,
                        ..
                    } => {
                        let size_str = format_size(*total_bytes);
                        let text = format!("󰄬 Received ({size_str})");
                        let tooltip = format!(
                            "<b>󰄬 Transfer Complete!</b>\n<b>File</b>: {file_name} ({size_str})\n<b>From</b>: {peer_alias}"
                        );
                        return WaybarPayload {
                            text,
                            tooltip,
                            class: vec!["completed".to_string()],
                            percentage: Some(100),
                        };
                    }
                    TransientVisual::Clipboard {
                        mime_type,
                        byte_size,
                        origin_device,
                        ..
                    } => {
                        let size_str = format_size(*byte_size as u64);
                        let text = format!("󰅌 Clip: {size_str}");
                        let tooltip = format!(
                            "<b>󰅌 Universal Clipboard Synced</b>\n<b>MIME</b>: {mime_type}\n<b>Size</b>: {size_str}\n<b>Origin</b>: {origin_device}"
                        );
                        return WaybarPayload {
                            text,
                            tooltip,
                            class: vec!["clipboard".to_string(), "paired".to_string()],
                            percentage: None,
                        };
                    }
                    TransientVisual::Error {
                        peer_alias,
                        error,
                        ..
                    } => {
                        let text = "󰅚 Transfer Failed".to_string();
                        let tooltip = format!(
                            "<b>󰅚 Transfer Error</b>: {error}\n<b>Peer</b>: {peer_alias}"
                        );
                        return WaybarPayload {
                            text,
                            tooltip,
                            class: vec!["error".to_string()],
                            percentage: None,
                        };
                    }
                }
            }
        }

        // 3. Paired / Connected Peer State
        if let Some(ref alias) = self.active_peer_alias {
            let text = format!("󰄡 {alias}");
            let tooltip = format!(
                "<b>Arch Drop</b>: Online\n<b>Active Peer</b>: {alias}\n<b>Port</b>: :{}\n<b>Clipboard Sync</b>: Active",
                self.port
            );
            return WaybarPayload {
                text,
                tooltip,
                class: vec!["paired".to_string()],
                percentage: None,
            };
        }

        if self.paired_peers > 0 {
            let text = "󰄡 ArchDrop".to_string();
            let tooltip = format!(
                "<b>Arch Drop</b>: Ready\n<b>Status</b>: Listening (:{})\n<b>Paired Peers</b>: {}\n<b>Relay</b>: {}",
                self.port,
                self.paired_peers,
                if self.relay_connected { "Connected" } else { "Disconnected" }
            );
            return WaybarPayload {
                text,
                tooltip,
                class: vec!["paired".to_string()],
                percentage: None,
            };
        }

        // 4. Default Idle State
        let text = "󰄡 ArchDrop".to_string();
        let tooltip = format!(
            "<b>Arch Drop</b>: Ready\n<b>Status</b>: Listening (:{})\n<b>Paired Peers</b>: 0\n<b>Relay</b>: {}",
            self.port,
            if self.relay_connected { "Connected" } else { "Disconnected" }
        );
        WaybarPayload {
            text,
            tooltip,
            class: vec!["idle".to_string()],
            percentage: None,
        }
    }

    /// Convert into DaemonStatusEvent for M1 compatibility.
    pub fn to_daemon_status_event(&self, now: Instant) -> DaemonStatusEvent {
        let payload = self.to_waybar_payload(now);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let state = if self.active_transfer.is_some() {
            "transferring".to_string()
        } else if let Some(ref t) = self.transient {
            match t {
                TransientVisual::Completed { until, .. } if now < *until => "completed".to_string(),
                TransientVisual::Error { until, .. } if now < *until => "error".to_string(),
                _ => {
                    if self.paired_peers > 0 || self.active_peer_alias.is_some() {
                        "paired".to_string()
                    } else {
                        "idle".to_string()
                    }
                }
            }
        } else if self.paired_peers > 0 || self.active_peer_alias.is_some() {
            "paired".to_string()
        } else {
            "idle".to_string()
        };

        let current_transfer = self.active_transfer.as_ref().map(|t| t.file_name.clone());

        DaemonStatusEvent {
            timestamp,
            text: payload.text,
            tooltip: payload.tooltip,
            class: payload.class,
            percentage: payload.percentage.map(|p| p as f64),
            state,
            active_peers: self.paired_peers,
            current_transfer,
        }
    }
}
