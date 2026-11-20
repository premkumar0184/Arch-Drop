//! Audio streaming session coordinator managing capture, encoding, framing, and RAII cleanup.

use crate::capture::AudioCaptureBackend;
use crate::codec::AudioEncoder;
use crate::error::AudioError;
use crate::framing::{
    AudioCodec, AudioWireFrame, FLAG_DISCONTINUITY, FLAG_NORMAL,
};
use crate::sink::VirtualSinkBackend;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Capacity of the bounded async channel for audio frames (32 frames * 20ms = 640ms buffer).
pub const CHANNEL_CAPACITY: usize = 32;

/// Configuration for an audio streaming session.
#[derive(Debug, Clone)]
pub struct AudioSessionConfig {
    pub codec: AudioCodec,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub use_virtual_sink: bool,
    pub sink_name: String,
    pub sink_description: String,
}

impl Default for AudioSessionConfig {
    fn default() -> Self {
        Self {
            codec: AudioCodec::Opus,
            bitrate: 96000,
            sample_rate: 48000,
            channels: 2,
            use_virtual_sink: false,
            sink_name: crate::sink::DEFAULT_VIRTUAL_SINK_NAME.to_string(),
            sink_description: crate::sink::DEFAULT_VIRTUAL_SINK_DESC.to_string(),
        }
    }
}

/// Active audio streaming session.
pub struct AudioStreamSession {
    receiver: Option<mpsc::Receiver<Bytes>>,
    shutdown_tx: watch::Sender<bool>,
    worker_handle: Option<JoinHandle<()>>,
    is_active: Arc<AtomicBool>,
}

impl AudioStreamSession {
    /// Start a new streaming session with the specified capture backend, encoder, and optional virtual sink.
    pub async fn start(
        mut capture_backend: Box<dyn AudioCaptureBackend>,
        mut encoder: Box<dyn AudioEncoder>,
        mut virtual_sink: Option<Box<dyn VirtualSinkBackend>>,
    ) -> Result<Self, AudioError> {
        let codec = encoder.codec();
        let channels = encoder.channels();
        let sample_rate = encoder.sample_rate();

        // 1. If virtual sink is requested, create it first
        if let Some(sink) = virtual_sink.as_mut() {
            let target_node = sink
                .create_sink(
                    crate::sink::DEFAULT_VIRTUAL_SINK_NAME,
                    crate::sink::DEFAULT_VIRTUAL_SINK_DESC,
                )
                .await?;
            info!(target_node = %target_node, "Created virtual sink for audio session");
        }

        // 2. Start audio capture stream
        let mut capture_stream = capture_backend.start().await?;

        // 3. Create bounded channel and shutdown signaling
        let (tx, rx) = mpsc::channel::<Bytes>(CHANNEL_CAPACITY);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let is_active = Arc::new(AtomicBool::new(true));
        let is_active_clone = is_active.clone();

        // 4. Spawn capture + encoding worker task
        let worker_handle = tokio::spawn(async move {
            let mut sequence: u64 = 0;
            let mut discontinuity = false;

            debug!("Audio streaming worker loop started");

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            debug!("Audio streaming worker received shutdown signal");
                            break;
                        }
                    }
                    pcm_res = capture_stream.read_pcm_frame() => {
                        match pcm_res {
                            Ok(Some(pcm_bytes)) => {
                                match encoder.encode_bytes(&pcm_bytes) {
                                    Ok(encoded_payload) => {
                                        let timestamp_ms = SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis() as u64;

                                        let flags = if discontinuity {
                                            FLAG_DISCONTINUITY
                                        } else {
                                            FLAG_NORMAL
                                        };

                                        let frame = AudioWireFrame::create(
                                            codec,
                                            channels,
                                            flags,
                                            sample_rate,
                                            sequence,
                                            timestamp_ms,
                                            encoded_payload,
                                        );

                                        let frame_bytes = frame.to_bytes();

                                        match tx.try_send(frame_bytes) {
                                            Ok(_) => {
                                                sequence = sequence.wrapping_add(1);
                                                discontinuity = false;
                                            }
                                            Err(mpsc::error::TrySendError::Full(_)) => {
                                                warn!(
                                                    seq = sequence,
                                                    "Audio channel full (backpressure); dropping frame and setting discontinuity flag"
                                                );
                                                sequence = sequence.wrapping_add(1);
                                                discontinuity = true;
                                                tokio::task::yield_now().await;
                                            }
                                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                                debug!("Audio receiver disconnected (channel closed); stopping worker");
                                                break;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Audio encoding failed: {e}");
                                        discontinuity = true;
                                    }
                                }
                            }
                            Ok(None) => {
                                debug!("Audio capture reached EOF; ending stream");
                                break;
                            }
                            Err(e) => {
                                error!("Audio capture stream error: {e}");
                                break;
                            }
                        }
                    }
                }
            }

            // Cleanup: stop capture stream and unbind virtual sink
            let _ = capture_stream.stop().await;
            if let Some(mut sink) = virtual_sink.take() {
                let _ = sink.destroy_sink().await;
            }
            is_active_clone.store(false, Ordering::SeqCst);
            debug!("Audio streaming worker terminated cleanly");
        });

        Ok(Self {
            receiver: Some(rx),
            shutdown_tx,
            worker_handle: Some(worker_handle),
            is_active,
        })
    }

    /// Extract the streaming receiver for piping into the HTTP response body.
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<Bytes>> {
        self.receiver.take()
    }

    /// Whether the audio session is currently actively streaming.
    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::SeqCst)
    }

    /// Explicitly terminate the session and await worker cleanup.
    pub async fn stop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.await;
        }
        self.is_active.store(false, Ordering::SeqCst);
    }
}

impl Drop for AudioStreamSession {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::MockAudioCapture;
    use crate::codec::PcmPassthroughEncoder;
    use crate::sink::MockVirtualSink;

    #[tokio::test]
    async fn test_session_lifecycle_with_mock() {
        let capture = Box::new(MockAudioCapture::new().with_max_frames(5));
        let encoder = Box::new(PcmPassthroughEncoder::default());
        let virtual_sink = Some(Box::new(MockVirtualSink::new()) as Box<dyn VirtualSinkBackend>);

        let mut session = AudioStreamSession::start(capture, encoder, virtual_sink)
            .await
            .expect("Start session");

        let mut rx = session.take_receiver().expect("Take receiver");

        let mut count = 0;
        while let Some(chunk) = rx.recv().await {
            let (frame, consumed) = AudioWireFrame::decode(&chunk).expect("Decode frame");
            assert_eq!(consumed, chunk.len());
            assert_eq!(frame.header.sequence, count as u64);
            assert_eq!(frame.header.codec, AudioCodec::Pcm);
            assert_eq!(frame.header.channels, 2);
            assert_eq!(frame.payload.len(), 3840);
            count += 1;
        }

        assert_eq!(count, 5);
        session.stop().await;
        assert!(!session.is_active());
    }
}
