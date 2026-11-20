use crate::cache::ClipboardPayloadCache;
use crate::error::ClipboardError;
use crate::threshold::{compute_sha256, pack_clipboard_data};
use archdrop_core::models::clipboard::{ClearReason, ClipboardClearPacket, ClipboardPacket};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use wl_clipboard_rs::copy::{
    clear as copy_clear, copy as copy_data, ClipboardType as CopyClipboardType, MimeType, Options,
    Seat as CopySeat, Source,
};
use wl_clipboard_rs::paste::Seat as PasteSeat;
use wl_clipboard_rs::watch::{
    ClipboardEvent, ClipboardType as WatchClipboardType, Watcher,
};

#[derive(Clone)]
pub struct WaylandClipboardAdapter {
    device_id: String,
    cache: ClipboardPayloadCache,
    packet_tx: broadcast::Sender<ClipboardPacket>,
    last_pasted_sha256: Arc<Mutex<Option<String>>>,
    remote_cleared: Arc<AtomicBool>,
    is_running: Arc<AtomicBool>,
}

impl WaylandClipboardAdapter {
    pub fn new(
        device_id: String,
        cache: ClipboardPayloadCache,
        packet_tx: broadcast::Sender<ClipboardPacket>,
    ) -> Self {
        Self {
            device_id,
            cache,
            packet_tx,
            last_pasted_sha256: Arc::new(Mutex::new(None)),
            remote_cleared: Arc::new(AtomicBool::new(false)),
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Checks if a Wayland display session is present in the environment.
    pub fn is_wayland_available() -> bool {
        std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("WAYLAND_SOCKET").is_ok()
    }

    /// Copies remote clipboard content into the local Wayland compositor selection,
    /// recording the checksum to suppress infinite echo loops.
    pub fn paste_remote_payload(&self, mime_type: &str, data: &[u8]) -> Result<(), ClipboardError> {
        let hash = compute_sha256(data);
        {
            let mut lock = self.last_pasted_sha256.lock().unwrap();
            *lock = Some(hash.clone());
        }

        debug!(
            mime = %mime_type,
            size = data.len(),
            sha256 = %hash,
            "Pasting remote clipboard payload to native Wayland compositor"
        );

        let opts = Options::new();
        let source = Source::Bytes(data.to_vec().into());
        let mime = MimeType::Specific(mime_type.to_string());

        let res = copy_data(opts, source, mime);
        if res.is_err() {
            let child = std::process::Command::new("wl-copy")
                .arg("--type")
                .arg(mime_type)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn();
            if let Ok(mut c) = child {
                if let Some(mut stdin) = c.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(data);
                }
                if let Ok(status) = c.wait() {
                    if status.success() {
                        return Ok(());
                    }
                }
            }
        }
        res.map_err(|e| ClipboardError::Wayland(e.to_string()))
    }

    /// Convenience wrapper to copy text into the native Wayland selection.
    pub fn paste_remote_text(&self, text: &str) -> Result<(), ClipboardError> {
        self.paste_remote_payload("text/plain;charset=utf-8", text.as_bytes())
    }

    /// Returns the SHA-256 hash of the most recently pasted remote content, if any.
    pub fn get_last_pasted_hash(&self) -> Option<String> {
        self.last_pasted_sha256.lock().unwrap().clone()
    }

    /// Manually sets the last pasted hash for echo suppression.
    pub fn set_last_pasted_hash(&self, hash: String) {
        let mut lock = self.last_pasted_sha256.lock().unwrap();
        *lock = Some(hash);
    }

    /// Checks if a hash matches the most recently pasted content (case-insensitive).
    pub fn is_recent_paste(&self, hash: &str) -> bool {
        let lock = self.last_pasted_sha256.lock().unwrap();
        match lock.as_deref() {
            Some(last) => last.eq_ignore_ascii_case(hash),
            None => false,
        }
    }

    /// Returns a reference to the shared last_pasted_sha256 mutex.
    pub fn last_pasted_sha256(&self) -> Arc<Mutex<Option<String>>> {
        self.last_pasted_sha256.clone()
    }

    /// Clears the local Wayland compositor clipboard.
    pub fn clear_local_clipboard(&self) -> Result<(), ClipboardError> {
        info!("Clearing local Wayland selection");
        self.remote_cleared.store(true, Ordering::SeqCst);
        let res = copy_clear(CopyClipboardType::Regular, CopySeat::All);
        if res.is_err() {
            if let Ok(output) = std::process::Command::new("wl-copy").arg("--clear").output() {
                if output.status.success() {
                    return Ok(());
                }
            }
        }
        res.map_err(|e| ClipboardError::Wayland(e.to_string()))
    }

    /// Spawns the unfocused Wayland selection monitor thread using `wl-clipboard-rs` watcher.
    pub fn spawn_watcher(
        &self,
        download_url_base: Option<String>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), ClipboardError> {
        if !Self::is_wayland_available() {
            warn!("WAYLAND_DISPLAY not found; clipboard monitoring will remain inactive");
            return Err(ClipboardError::WaylandNotAvailable(
                "WAYLAND_DISPLAY environment variable not set".to_string(),
            ));
        }

        let device_id = self.device_id.clone();
        let cache = self.cache.clone();
        let packet_tx = self.packet_tx.clone();
        let last_pasted = self.last_pasted_sha256.clone();
        let remote_cleared = self.remote_cleared.clone();
        let is_running = self.is_running.clone();

        is_running.store(true, Ordering::SeqCst);

        let rt_handle = tokio::runtime::Handle::try_current().ok();

        std::thread::Builder::new()
            .name("archdrop-wayland-clip".to_string())
            .spawn(move || {
                info!("Starting native Wayland unfocused clipboard watcher thread");
                let mut watcher = match Watcher::new(WatchClipboardType::Regular, PasteSeat::Unspecified) {
                    Ok(w) => w,
                    Err(e) => {
                        error!("Failed to initialize Wayland clipboard Watcher: {}", e);
                        is_running.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                while is_running.load(Ordering::Relaxed) {
                    if *shutdown_rx.borrow() {
                        info!("Wayland clipboard watcher stopping due to shutdown signal");
                        break;
                    }

                    match watcher.next_event() {
                        Ok(Some(ClipboardEvent::Changed {
                            clipboard: _,
                            mime_types,
                            mut offer,
                        })) => {
                            // Find preferred MIME type (text or first available)
                            let selected_mime = mime_types
                                .iter()
                                .find(|m| m.starts_with("text/plain"))
                                .or_else(|| mime_types.first())
                                .cloned()
                                .unwrap_or_else(|| "text/plain;charset=utf-8".to_string());

                            let mut reader = match offer.receive(&selected_mime) {
                                Ok(r) => r,
                                Err(e) => {
                                    debug!("Failed to receive offer stream: {}", e);
                                    continue;
                                }
                            };

                            let mut buffer = Vec::new();
                            if let Err(e) = reader.read_to_end(&mut buffer) {
                                debug!("Failed to read clipboard stream: {}", e);
                                continue;
                            }

                            if buffer.is_empty() {
                                continue;
                            }

                            let hash = compute_sha256(&buffer);

                            // Echo suppression check
                            {
                                let lock = last_pasted.lock().unwrap();
                                if let Some(ref last_hash) = *lock {
                                    if last_hash.eq_ignore_ascii_case(&hash) {
                                        debug!("Echo suppressed: selection matches last remote paste");
                                        continue;
                                    }
                                }
                            }

                            info!(
                                mime = %selected_mime,
                                bytes = buffer.len(),
                                sha256 = %hash,
                                "Captured local Wayland clipboard selection"
                            );

                            // Apply 4 KB threshold rule
                            let (packet, cached) = pack_clipboard_data(
                                &device_id,
                                &selected_mime,
                                &buffer,
                                download_url_base.as_deref(),
                            );

                            if let Some(cached_payload) = cached {
                                if let Some(ref handle) = rt_handle {
                                    let c = cache.clone();
                                    handle.spawn(async move {
                                        c.insert(cached_payload).await;
                                    });
                                }
                            }

                            let _ = packet_tx.send(packet);
                        }
                        Ok(Some(ClipboardEvent::Cleared { clipboard: _ })) => {
                            if remote_cleared.swap(false, Ordering::SeqCst) {
                                debug!("Echo suppressed: local clipboard was cleared by remote peer");
                                continue;
                            }
                            info!("Wayland clipboard cleared by user or compositor");
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);

                            let clear_packet = ClipboardPacket::Clear(ClipboardClearPacket {
                                version: 2,
                                device_id: device_id.clone(),
                                timestamp: now_ms,
                                reason: ClearReason::UserAction,
                            });
                            let _ = packet_tx.send(clear_packet);
                        }
                        Ok(None) => {
                            // No event, short yield
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(e) => {
                            debug!("Watcher error: {}", e);
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                }

                is_running.store(false, Ordering::SeqCst);
                info!("Wayland clipboard watcher thread terminated");
            })
            .map_err(ClipboardError::Io)?;

        Ok(())
    }
}
