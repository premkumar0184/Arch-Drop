//! Audio capture backends for PipeWire and headless mock testing.

use crate::error::{AudioError, PIPEWIRE_DIAGNOSTIC};
use crate::framing::{DEFAULT_CHANNELS, DEFAULT_FRAME_DURATION_MS, DEFAULT_SAMPLE_RATE};
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};
use tracing::{debug, info};

/// Default monitor target for the host system default audio output.
pub const DEFAULT_SINK_TARGET: &str = "@DEFAULT_AUDIO_SINK@";

/// Number of PCM bytes in a standard 20ms 48kHz 16-bit stereo frame (3,840 bytes).
pub const FRAME_PCM_BYTES: usize =
    (DEFAULT_SAMPLE_RATE as usize * DEFAULT_FRAME_DURATION_MS as usize / 1000)
        * DEFAULT_CHANNELS as usize
        * std::mem::size_of::<i16>();

/// Audio capture backend abstraction.
#[async_trait]
pub trait AudioCaptureBackend: Send + Sync {
    /// Start capturing audio, returning a streaming reader handle.
    async fn start(&mut self) -> Result<Box<dyn AudioCaptureStream>, AudioError>;

    /// Target audio device/node name being captured.
    fn device_name(&self) -> &str;

    /// Check if the capture backend is available on the current host system.
    fn is_available(&self) -> bool;
}

/// Active audio capture stream yielding 20ms PCM frames.
#[async_trait]
pub trait AudioCaptureStream: Send + Sync {
    /// Read the next 20ms PCM frame (exactly 3840 bytes for 48kHz stereo s16le).
    ///
    /// Returns `Ok(Some(bytes))` with the frame, `Ok(None)` on clean EOF,
    /// or `Err(AudioError)` on capture failure.
    async fn read_pcm_frame(&mut self) -> Result<Option<Vec<u8>>, AudioError>;

    /// Stop capture stream and cleanly release underlying resources.
    async fn stop(&mut self) -> Result<(), AudioError>;
}

/// PipeWire audio capture backend using `pw-record`.
pub struct PipeWireCapture {
    target: String,
    sample_rate: u32,
    channels: u8,
    latency_ms: u16,
}

impl PipeWireCapture {
    /// Create a new PipeWire capture backend targeting a specific node or sink monitor.
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            latency_ms: DEFAULT_FRAME_DURATION_MS,
        }
    }

    /// Create capture backend targeting the default PipeWire sink monitor.
    pub fn default_sink() -> Self {
        Self::new(DEFAULT_SINK_TARGET)
    }

    /// Check if pw-record binary is available on the system.
    pub fn is_system_available() -> bool {
        std::path::Path::new("/usr/bin/pw-record").exists()
            || std::process::Command::new("which")
                .arg("pw-record")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    }
}

#[async_trait]
impl AudioCaptureBackend for PipeWireCapture {
    async fn start(&mut self) -> Result<Box<dyn AudioCaptureStream>, AudioError> {
        if !self.is_available() {
            return Err(AudioError::PipeWireUnavailable(PIPEWIRE_DIAGNOSTIC.into()));
        }

        info!(
            target = %self.target,
            rate = self.sample_rate,
            channels = self.channels,
            "Spawning pw-record capture process"
        );

        let mut cmd = Command::new("pw-record");
        cmd.arg("-P")
            .arg("stream.capture.sink=true")
            .arg("--target")
            .arg(&self.target)
            .arg("--raw")
            .arg("--rate")
            .arg(self.sample_rate.to_string())
            .arg("--channels")
            .arg(self.channels.to_string())
            .arg("--format")
            .arg("s16")
            .arg("--latency")
            .arg(format!("{}ms", self.latency_ms))
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        let mut child = cmd.spawn().map_err(|e| {
            AudioError::CaptureFailed(format!(
                "Failed to spawn pw-record for target '{}': {e}",
                self.target
            ))
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            AudioError::CaptureFailed("Failed to capture pw-record stdout pipe".into())
        })?;

        Ok(Box::new(PipeWireCaptureStream {
            child: Some(child),
            stdout,
            target: self.target.clone(),
        }))
    }

    fn device_name(&self) -> &str {
        &self.target
    }

    fn is_available(&self) -> bool {
        Self::is_system_available()
    }
}

/// Active stream handle for `pw-record`.
pub struct PipeWireCaptureStream {
    child: Option<Child>,
    stdout: ChildStdout,
    target: String,
}

#[async_trait]
impl AudioCaptureStream for PipeWireCaptureStream {
    async fn read_pcm_frame(&mut self) -> Result<Option<Vec<u8>>, AudioError> {
        let mut buf = vec![0u8; FRAME_PCM_BYTES];
        match self.stdout.read_exact(&mut buf).await {
            Ok(_) => Ok(Some(buf)),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                debug!("pw-record reached EOF");
                Ok(None)
            }
            Err(e) => Err(AudioError::CaptureFailed(format!(
                "Error reading from pw-record: {e}"
            ))),
        }
    }

    async fn stop(&mut self) -> Result<(), AudioError> {
        if let Some(mut child) = self.child.take() {
            info!(target = %self.target, "Terminating pw-record process");
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        Ok(())
    }
}

impl Drop for PipeWireCaptureStream {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            debug!(target = %self.target, "Killing pw-record process in RAII Drop");
            let _ = child.start_kill();
        }
    }
}

/// Synthetic mock audio capture backend for CI / testing without PipeWire.
#[derive(Clone)]
pub struct MockAudioCapture {
    frequency: f32,
    sample_rate: u32,
    channels: u8,
    frame_duration_ms: u16,
    max_frames: Option<usize>,
    paced: bool,
}

impl MockAudioCapture {
    pub fn new() -> Self {
        Self {
            frequency: 440.0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            frame_duration_ms: DEFAULT_FRAME_DURATION_MS,
            max_frames: None,
            paced: false,
        }
    }

    pub fn with_max_frames(mut self, max: usize) -> Self {
        self.max_frames = Some(max);
        self
    }

    pub fn with_pacing(mut self, paced: bool) -> Self {
        self.paced = paced;
        self
    }

    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.frequency = freq;
        self
    }
}

impl Default for MockAudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AudioCaptureBackend for MockAudioCapture {
    async fn start(&mut self) -> Result<Box<dyn AudioCaptureStream>, AudioError> {
        Ok(Box::new(MockAudioCaptureStream {
            frequency: self.frequency,
            sample_rate: self.sample_rate,
            channels: self.channels,
            frame_duration_ms: self.frame_duration_ms,
            max_frames: self.max_frames,
            paced: self.paced,
            frames_generated: 0,
            sample_counter: 0,
            stopped: false,
        }))
    }

    fn device_name(&self) -> &str {
        "MockAudioCaptureDevice"
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// Synthetic audio stream generator.
pub struct MockAudioCaptureStream {
    frequency: f32,
    sample_rate: u32,
    channels: u8,
    frame_duration_ms: u16,
    max_frames: Option<usize>,
    paced: bool,
    frames_generated: usize,
    sample_counter: u64,
    stopped: bool,
}

#[async_trait]
impl AudioCaptureStream for MockAudioCaptureStream {
    async fn read_pcm_frame(&mut self) -> Result<Option<Vec<u8>>, AudioError> {
        if self.stopped {
            return Ok(None);
        }

        if let Some(max) = self.max_frames {
            if self.frames_generated >= max {
                return Ok(None);
            }
        }

        if self.paced {
            tokio::time::sleep(Duration::from_millis(self.frame_duration_ms as u64)).await;
        } else {
            tokio::task::yield_now().await;
        }

        let samples_per_channel =
            (self.sample_rate as usize * self.frame_duration_ms as usize) / 1000;
        let mut frame_bytes = Vec::with_capacity(samples_per_channel * self.channels as usize * 2);

        for _ in 0..samples_per_channel {
            let t = self.sample_counter as f32 / self.sample_rate as f32;
            let val =
                (f32::sin(2.0 * std::f32::consts::PI * self.frequency * t) * 16000.0) as i16;
            let bytes = val.to_le_bytes();

            for _ in 0..self.channels {
                frame_bytes.extend_from_slice(&bytes);
            }

            self.sample_counter = self.sample_counter.wrapping_add(1);
        }

        self.frames_generated += 1;
        Ok(Some(frame_bytes))
    }

    async fn stop(&mut self) -> Result<(), AudioError> {
        self.stopped = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_audio_capture_exact_frame_size() {
        let mut backend = MockAudioCapture::new().with_max_frames(3);
        let mut stream = backend.start().await.expect("Start mock stream");

        for i in 0..3 {
            let frame = stream.read_pcm_frame().await.expect("Read frame");
            assert!(frame.is_some());
            let bytes = frame.unwrap();
            assert_eq!(bytes.len(), FRAME_PCM_BYTES, "Frame {} size mismatch", i);
        }

        // 4th read should be EOF
        let eof = stream.read_pcm_frame().await.expect("Read frame at EOF");
        assert!(eof.is_none());
    }

    #[tokio::test]
    async fn test_mock_audio_capture_stop() {
        let mut backend = MockAudioCapture::new();
        let mut stream = backend.start().await.unwrap();

        let frame = stream.read_pcm_frame().await.unwrap();
        assert!(frame.is_some());

        stream.stop().await.unwrap();
        let frame_after_stop = stream.read_pcm_frame().await.unwrap();
        assert!(frame_after_stop.is_none());
    }
}
