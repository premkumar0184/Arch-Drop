//! Error definitions for the Arch Drop virtual webcam crate.

use std::path::PathBuf;
use thiserror::Error;

/// Standard kernel diagnostic command required when v4l2loopback is not loaded.
pub const MODPROBE_DIAGNOSTIC: &str =
    "sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"ArchDrop Camera\"";

#[derive(Error, Debug)]
pub enum WebcamError {
    #[error("v4l2loopback kernel module is not loaded: {diagnostic}")]
    LoopbackNotLoaded { diagnostic: String },

    #[error("No loopback video device found: {0}")]
    DeviceNotFound(String),

    #[error("Failed to open video device at {path:?}: {source}")]
    DeviceOpenFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("V4L2 ioctl {ioctl_name} failed (errno {errno}): {message}")]
    IoctlFailed {
        ioctl_name: &'static str,
        errno: i32,
        message: String,
    },

    #[error("Failed to write frame to video device: {source}")]
    WriteFailed {
        #[source]
        source: std::io::Error,
    },

    #[error("Malformed multipart stream: {0}")]
    MalformedStream(String),

    #[error("Malformed JPEG frame (missing SOI/EOI markers): {0}")]
    MalformedJpeg(String),

    #[error("Frame size {size} exceeds maximum allowable bound {max}")]
    FrameTooLarge { size: usize, max: usize },

    #[error("Unexpected EOF during stream parsing: {0}")]
    UnexpectedEof(String),

    #[error("Webcam stream is already active")]
    StreamAlreadyActive,

    #[error("Webcam stream is not active")]
    StreamNotActive,

    #[error("Internal watch channel closed")]
    ChannelClosed,

    #[error("Webcam error: {0}")]
    Other(String),
}
