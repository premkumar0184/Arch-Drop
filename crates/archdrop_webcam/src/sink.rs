//! WebcamSink actor with depth-1 watch channel for zero buffering and minimal latency.

use crate::error::WebcamError;
use crate::v4l2::V4l2Sink;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;
use tracing::{debug, warn};

/// Live performance statistics for the virtual webcam sink actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebcamSinkStats {
    pub frames_received: u64,
    pub frames_written: u64,
    pub frames_dropped: u64,
    pub fps: f64,
    pub is_streaming: bool,
    pub device_path: PathBuf,
}

/// Actor managing the asynchronous zero-buffering handoff from network ingestion
/// to the Linux V4L2 virtual camera device.
pub struct WebcamSink {
    watch_tx: watch::Sender<Option<Bytes>>,
    stop_tx: watch::Sender<bool>,
    frames_received: Arc<AtomicU64>,
    frames_written: Arc<AtomicU64>,
    is_active: Arc<AtomicBool>,
    device_path: PathBuf,
    start_time: Instant,
}

impl WebcamSink {
    /// Instantiates and spawns a new `WebcamSink` actor around a `V4l2Sink` implementation.
    pub fn new(mut sink: Box<dyn V4l2Sink>) -> Self {
        let (watch_tx, mut watch_rx) = watch::channel::<Option<Bytes>>(None);
        let (stop_tx, mut stop_rx) = watch::channel::<bool>(false);

        let frames_received = Arc::new(AtomicU64::new(0));
        let frames_written = Arc::new(AtomicU64::new(0));
        let is_active = Arc::new(AtomicBool::new(true));

        let frames_written_clone = frames_written.clone();
        let is_active_clone = is_active.clone();
        let device_path = sink.device_path().to_path_buf();
        let dev_path_clone = device_path.clone();

        // Spawn background task running on tokio runtime
        tokio::spawn(async move {
            debug!(device = ?dev_path_clone, "WebcamSink worker task started");

            loop {
                tokio::select! {
                    stop_res = stop_rx.changed() => {
                        if stop_res.is_err() || *stop_rx.borrow() {
                            debug!("WebcamSink stop signal received");
                            break;
                        }
                    }
                    frame_res = watch_rx.changed() => {
                        if frame_res.is_err() {
                            break;
                        }
                        let maybe_frame = watch_rx.borrow_and_update().clone();
                        if let Some(frame) = maybe_frame {
                            if let Err(e) = sink.write_frame(&frame) {
                                warn!(device = ?dev_path_clone, "Failed to write frame to V4L2 sink: {e}");
                            } else {
                                frames_written_clone.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }

            let _ = sink.stream_off();
            is_active_clone.store(false, Ordering::SeqCst);
            debug!(device = ?dev_path_clone, "WebcamSink worker task exited");
        });

        Self {
            watch_tx,
            stop_tx,
            frames_received,
            frames_written,
            is_active,
            device_path,
            start_time: Instant::now(),
        }
    }

    /// Pushes a new JPEG frame to the sink. If the previous frame was not yet written,
    /// it is immediately overwritten (dropped) without queue buffering.
    pub fn push_frame(&self, frame: Bytes) -> Result<(), WebcamError> {
        if !self.is_active.load(Ordering::SeqCst) {
            return Err(WebcamError::StreamNotActive);
        }

        self.frames_received.fetch_add(1, Ordering::Relaxed);
        self.watch_tx
            .send(Some(frame))
            .map_err(|_| WebcamError::ChannelClosed)?;

        Ok(())
    }

    /// Retrieves current operational metrics and frame counts.
    pub fn stats(&self) -> WebcamSinkStats {
        let received = self.frames_received.load(Ordering::Relaxed);
        let written = self.frames_written.load(Ordering::Relaxed);
        let dropped = received.saturating_sub(written);

        let elapsed_secs = self.start_time.elapsed().as_secs_f64();
        let fps = if elapsed_secs > 0.05 {
            written as f64 / elapsed_secs
        } else {
            0.0
        };

        WebcamSinkStats {
            frames_received: received,
            frames_written: written,
            frames_dropped: dropped,
            fps,
            is_streaming: self.is_active.load(Ordering::SeqCst),
            device_path: self.device_path.clone(),
        }
    }

    /// Signals the background sink task to stop and issues `VIDIOC_STREAMOFF`.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
        self.is_active.store(false, Ordering::SeqCst);
    }

    /// Returns whether the sink actor is active and capable of receiving frames.
    pub fn is_streaming(&self) -> bool {
        self.is_active.load(Ordering::SeqCst)
    }

    /// The associated device node path.
    pub fn device_path(&self) -> &Path {
        &self.device_path
    }
}

impl Drop for WebcamSink {
    fn drop(&mut self) {
        self.stop();
    }
}
