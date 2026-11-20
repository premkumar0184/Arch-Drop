//! Error types and diagnostic messages for desktop audio capture and streaming.

use thiserror::Error;

/// Diagnostic hint displayed when PipeWire or WirePlumber is not detected or fails to connect.
pub const PIPEWIRE_DIAGNOSTIC: &str =
    "Ensure PipeWire and WirePlumber are running: systemctl --user status pipewire wireplumber";

/// Errors occurring within the audio capture, encoding, and streaming pipelines.
#[derive(Debug, Error)]
pub enum AudioError {
    #[error("PipeWire is unavailable: {0}")]
    PipeWireUnavailable(String),

    #[error("Audio device or node not found: {0}")]
    DeviceNotFound(String),

    #[error("Audio capture failed: {0}")]
    CaptureFailed(String),

    #[error("Virtual audio sink creation/teardown failed: {0}")]
    VirtualSinkFailed(String),

    #[error("Audio encoding failed: {0}")]
    EncodingFailed(String),

    #[error("Opus codec error: {0}")]
    Opus(String),

    #[error("Audio wire framing error: {0}")]
    Framing(String),

    #[error("Invalid PCM frame length: expected multiple of {expected} bytes, got {actual}")]
    InvalidPcmLength { expected: usize, actual: usize },

    #[error("Another audio stream session is already active (single-stream mutual exclusion)")]
    ConcurrentStreamActive,

    #[error("Audio stream closed or cancelled")]
    StreamClosed,

    #[error("IO error in audio pipeline: {0}")]
    Io(#[from] std::io::Error),
}

impl From<opus::Error> for AudioError {
    fn from(err: opus::Error) -> Self {
        AudioError::Opus(err.to_string())
    }
}
