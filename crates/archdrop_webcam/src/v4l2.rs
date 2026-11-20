//! Linux V4L2 loopback virtual video device interface.
//!
//! Provides direct ioctl bindings and file operations for writing MJPEG
//! frames into v4l2loopback without dynamic GUI dependencies.

use crate::error::WebcamError;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

// Standard Linux V4L2 ioctl codes and constants
pub const VIDIOC_QUERYCAP: libc::c_ulong = 0x80685600;
pub const VIDIOC_S_FMT: libc::c_ulong = 0xc0d05605;
pub const VIDIOC_STREAMON: libc::c_ulong = 0x40045612;
pub const VIDIOC_STREAMOFF: libc::c_ulong = 0x40045613;
pub const VIDIOC_S_CTRL: libc::c_ulong = 0xc008561c;

pub const V4L2_CID_USER_BASE: u32 = 0x00980000;
pub const V4L2_CID_KEEP_FORMAT: u32 = V4L2_CID_USER_BASE | 0xf900;
pub const V4L2_CID_SUSTAIN_FRAMERATE: u32 = V4L2_CID_USER_BASE | 0xf901;

pub const BLANK_FRAME_JPEG: &[u8] = include_bytes!("../assets/blank_frame.jpg");

pub const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
pub const V4L2_FIELD_NONE: u32 = 1;

pub const V4L2_CAP_VIDEO_OUTPUT: u32 = 0x00000002;
pub const V4L2_CAP_STREAMING: u32 = 0x04000000;
pub const V4L2_CAP_DEVICE_CAPS: u32 = 0x80000000;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct v4l2_control {
    pub id: u32,
    pub value: i32,
}

// FourCC pixel formats
pub const V4L2_PIX_FMT_JPEG: u32 = 0x4745504a; // 'JPEG'
pub const V4L2_PIX_FMT_MJPEG: u32 = 0x47504a4d; // 'MJPG'

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct v4l2_capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct v4l2_pix_format {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub priv_: u32,
    pub flags: u32,
    pub union_field: u32,
    pub quantization: u32,
    pub xfer_func: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union v4l2_format_fmt {
    pub pix: v4l2_pix_format,
    pub raw_data: [u8; 200],
    pub _align: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_format {
    pub type_: u32,
    pub fmt: v4l2_format_fmt,
}

/// Abstract V4L2 sink interface for writing video frames into a virtual webcam.
pub trait V4l2Sink: Send + Sync {
    /// Configure video frame format (width, height, and fourcc pixelformat).
    fn set_format(&mut self, width: u32, height: u32, pixelformat: u32) -> Result<(), WebcamError>;

    /// Turn on streaming on the V4L2 device.
    fn stream_on(&mut self) -> Result<(), WebcamError>;

    /// Write raw frame bytes (e.g. MJPEG) into the video device.
    fn write_frame(&mut self, frame: &[u8]) -> Result<(), WebcamError>;

    /// Turn off streaming on the V4L2 device.
    fn stream_off(&mut self) -> Result<(), WebcamError>;

    /// Returns whether the sink is currently streaming.
    fn is_streaming(&self) -> bool;

    /// Device filesystem path.
    fn device_path(&self) -> &Path;
}

/// Native Linux V4L2 Loopback sink with RAII file descriptor management.
pub struct LinuxV4l2Sink {
    fd: RawFd,
    device_path: PathBuf,
    is_streaming: bool,
    width: u32,
    height: u32,
    pixelformat: u32,
    last_failed_dim: Option<(u32, u32)>,
}

impl LinuxV4l2Sink {
    /// Open a V4L2 video output device (e.g. `/dev/video2`).
    pub fn open(device_path: &Path) -> Result<Self, WebcamError> {
        let c_path = CString::new(device_path.as_os_str().as_bytes()).map_err(|_| {
            WebcamError::DeviceOpenFailed {
                path: device_path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Device path contains null byte",
                ),
            }
        })?;

        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(WebcamError::DeviceOpenFailed {
                path: device_path.to_path_buf(),
                source: std::io::Error::last_os_error(),
            });
        }

        // Query capabilities
        let mut cap: v4l2_capability = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP, &mut cap) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(WebcamError::IoctlFailed {
                ioctl_name: "VIDIOC_QUERYCAP",
                errno: err.raw_os_error().unwrap_or(-1),
                message: err.to_string(),
            });
        }

        let caps = if (cap.capabilities & V4L2_CAP_DEVICE_CAPS) != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };

        if (caps & V4L2_CAP_VIDEO_OUTPUT) == 0 {
            warn!(
                device = ?device_path,
                caps = caps,
                "Device does not explicitly report V4L2_CAP_VIDEO_OUTPUT capability"
            );
        }

        debug!(
            device = ?device_path,
            driver = ?String::from_utf8_lossy(&cap.driver).trim_end_matches('\0'),
            card = ?String::from_utf8_lossy(&cap.card).trim_end_matches('\0'),
            "Opened Linux V4L2 video sink"
        );

        let mut sink = Self {
            fd,
            device_path: device_path.to_path_buf(),
            is_streaming: false,
            width: 0,
            height: 0,
            pixelformat: V4L2_PIX_FMT_MJPEG,
            last_failed_dim: None,
        };

        // Preconfigure device format to 1280x720 MJPEG
        if let Err(e) = sink.set_format(1280, 720, V4L2_PIX_FMT_MJPEG) {
            warn!(device = ?device_path, "Failed to set initial 1280x720 MJPEG format: {e}");
        }

        // Enable v4l2loopback keep_format and sustain_framerate controls so consumers (Chrome, Google Meet, OBS)
        // always identify the device as a ready video capture camera.
        let _ = sink.set_control(V4L2_CID_KEEP_FORMAT, 1);
        let _ = sink.set_control(V4L2_CID_SUSTAIN_FRAMERATE, 1);

        // Prime the loopback buffer with an initial blank holding frame so video capture starts immediately
        let _ = sink.write_frame(BLANK_FRAME_JPEG);

        Ok(sink)
    }

    /// Sets a V4L2 control (such as CID_KEEP_FORMAT or CID_SUSTAIN_FRAMERATE on v4l2loopback).
    pub fn set_control(&self, id: u32, value: i32) -> Result<(), WebcamError> {
        let mut ctrl = v4l2_control { id, value };
        let ret = unsafe { libc::ioctl(self.fd, VIDIOC_S_CTRL, &mut ctrl) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            debug!(
                device = ?self.device_path,
                id = id,
                value = value,
                "VIDIOC_S_CTRL returned: {err}"
            );
            return Err(WebcamError::IoctlFailed {
                ioctl_name: "VIDIOC_S_CTRL",
                errno: err.raw_os_error().unwrap_or(-1),
                message: err.to_string(),
            });
        }
        Ok(())
    }
}

impl V4l2Sink for LinuxV4l2Sink {
    fn set_format(&mut self, width: u32, height: u32, pixelformat: u32) -> Result<(), WebcamError> {
        let mut fmt: v4l2_format = unsafe { std::mem::zeroed() };
        fmt.type_ = V4L2_BUF_TYPE_VIDEO_OUTPUT;

        let pix = v4l2_pix_format {
            width,
            height,
            pixelformat,
            field: V4L2_FIELD_NONE,
            bytesperline: 0,
            sizeimage: width * height * 2,
            colorspace: 0,
            priv_: 0,
            flags: 0,
            union_field: 0,
            quantization: 0,
            xfer_func: 0,
        };
        fmt.fmt.pix = pix;

        let ret = unsafe { libc::ioctl(self.fd, VIDIOC_S_FMT, &mut fmt) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            return Err(WebcamError::IoctlFailed {
                ioctl_name: "VIDIOC_S_FMT",
                errno: err.raw_os_error().unwrap_or(-1),
                message: err.to_string(),
            });
        }

        self.width = width;
        self.height = height;
        self.pixelformat = pixelformat;

        debug!(
            device = ?self.device_path,
            width = width,
            height = height,
            "V4L2 format successfully configured"
        );
        Ok(())
    }

    fn stream_on(&mut self) -> Result<(), WebcamError> {
        if self.is_streaming {
            return Ok(());
        }

        // On Linux V4L2 with direct write() file I/O, streaming is started automatically
        // by the kernel driver via start_fileio(). VIDIOC_STREAMON without REQBUFS returns EINVAL.
        let mut type_ = V4L2_BUF_TYPE_VIDEO_OUTPUT;
        let ret = unsafe { libc::ioctl(self.fd, VIDIOC_STREAMON, &mut type_) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EINVAL) {
                return Err(WebcamError::IoctlFailed {
                    ioctl_name: "VIDIOC_STREAMON",
                    errno: err.raw_os_error().unwrap_or(-1),
                    message: err.to_string(),
                });
            }
        }

        self.is_streaming = true;
        info!(device = ?self.device_path, "V4L2 virtual camera streaming activated");
        Ok(())
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), WebcamError> {
        if let Some((w, h)) = crate::mjpeg::parse_jpeg_dimensions(frame) {
            if (self.width != w || self.height != h) && w > 0 && h > 0 {
                if self.last_failed_dim != Some((w, h)) {
                    match self.set_format(w, h, self.pixelformat) {
                        Ok(()) => {
                            self.last_failed_dim = None;
                            info!(
                                device = ?self.device_path,
                                width = w,
                                height = h,
                                "V4L2 format updated for incoming stream dimensions"
                            );
                        }
                        Err(e) => {
                            self.last_failed_dim = Some((w, h));
                            warn!(
                                device = ?self.device_path,
                                stream_width = w,
                                stream_height = h,
                                device_width = self.width,
                                device_height = self.height,
                                "Cannot change V4L2 device format while consumer has buffers mapped: {e}"
                            );
                        }
                    }
                }
            }
        }

        let written = unsafe {
            libc::write(
                self.fd,
                frame.as_ptr() as *const libc::c_void,
                frame.len(),
            )
        };

        if written < 0 {
            let err = std::io::Error::last_os_error();
            return Err(WebcamError::WriteFailed { source: err });
        }

        if !self.is_streaming {
            self.is_streaming = true;
            info!(device = ?self.device_path, "V4L2 virtual camera streaming active (frame written)");
        }

        Ok(())
    }

    fn stream_off(&mut self) -> Result<(), WebcamError> {
        if !self.is_streaming {
            return Ok(());
        }

        let mut type_ = V4L2_BUF_TYPE_VIDEO_OUTPUT;
        let ret = unsafe { libc::ioctl(self.fd, VIDIOC_STREAMOFF, &mut type_) };
        self.is_streaming = false;

        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EINVAL) {
                warn!(
                    device = ?self.device_path,
                    errno = err.raw_os_error(),
                    "VIDIOC_STREAMOFF ioctl returned error during teardown: {err}"
                );
                return Err(WebcamError::IoctlFailed {
                    ioctl_name: "VIDIOC_STREAMOFF",
                    errno: err.raw_os_error().unwrap_or(-1),
                    message: err.to_string(),
                });
            }
        }

        info!(device = ?self.device_path, "V4L2 virtual camera streaming stopped");
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    fn device_path(&self) -> &Path {
        &self.device_path
    }
}

impl Drop for LinuxV4l2Sink {
    fn drop(&mut self) {
        if self.is_streaming {
            let _ = self.stream_off();
        }
        if self.fd >= 0 {
            unsafe {
                libc::close(self.fd);
            }
            debug!(device = ?self.device_path, "Closed Linux V4L2 device file descriptor");
        }
    }
}
