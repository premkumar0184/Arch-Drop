//! Arch Drop Desktop Linux Audio Capture & Virtual Sink Engine.
//!
//! Provides low-latency audio capture via PipeWire monitor sinks or virtual loopback devices,
//! Opus and raw PCM encoding, 32-byte canonical `ADAU` binary wire framing,
//! bounded async streaming, and zeroizing memory buffers.

pub mod capture;
pub mod codec;
pub mod error;
pub mod framing;
pub mod session;
pub mod sink;

pub use archdrop_core;

pub use capture::{
    AudioCaptureBackend, AudioCaptureStream, MockAudioCapture, MockAudioCaptureStream,
    PipeWireCapture, PipeWireCaptureStream, DEFAULT_SINK_TARGET, FRAME_PCM_BYTES,
};
pub use codec::{AudioEncoder, OpusAudioEncoder, PcmPassthroughEncoder};
pub use error::{AudioError, PIPEWIRE_DIAGNOSTIC};
pub use framing::{
    AudioBuffer, AudioCodec, AudioWireFrame, AudioWireHeader, ADAU_MAGIC, DEFAULT_CHANNELS,
    DEFAULT_FRAME_DURATION_MS, DEFAULT_SAMPLE_RATE, FLAG_DISCONTINUITY, FLAG_NORMAL, HEADER_SIZE,
};
pub use session::{AudioSessionConfig, AudioStreamSession, CHANNEL_CAPACITY};
pub use sink::{
    MockVirtualSink, PipeWireVirtualSink, VirtualSinkBackend, DEFAULT_VIRTUAL_SINK_DESC,
    DEFAULT_VIRTUAL_SINK_NAME,
};
