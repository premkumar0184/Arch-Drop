//! Arch Drop Linux Virtual Webcam Sink Crate.
//!
//! Provides zero-GUI V4L2 loopback writing, streaming MJPEG parsing,
//! depth-1 zero-buffering actor sinks, and sysfs diagnostics.

pub mod device;
pub mod error;
pub mod mjpeg;
pub mod mock;
pub mod sink;
pub mod v4l2;

pub use archdrop_core;

pub use device::{
    discover_loopback_device, find_loopback_device, get_diagnostic_message, list_loopback_devices,
    LoopbackDevice,
};
pub use error::{WebcamError, MODPROBE_DIAGNOSTIC};
pub use mjpeg::{parse_jpeg_dimensions, validate_jpeg, MjpegFrame, MjpegStreamParser, MAX_FRAME_SIZE};
pub use mock::MockV4l2Sink;
pub use sink::{WebcamSink, WebcamSinkStats};
pub use v4l2::{
    v4l2_capability, v4l2_format, v4l2_pix_format, LinuxV4l2Sink, V4l2Sink,
    V4L2_BUF_TYPE_VIDEO_OUTPUT, V4L2_PIX_FMT_JPEG, V4L2_PIX_FMT_MJPEG, VIDIOC_QUERYCAP,
    VIDIOC_STREAMOFF, VIDIOC_STREAMON, VIDIOC_S_FMT,
};
