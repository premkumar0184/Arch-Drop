//! Mock implementation of `V4l2Sink` for automated testing.

use crate::error::WebcamError;
use crate::v4l2::V4l2Sink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// In-memory mock V4L2 device sink for hardware-independent unit testing.
#[derive(Clone)]
pub struct MockV4l2Sink {
    device_path: PathBuf,
    is_streaming: Arc<AtomicBool>,
    written_frames: Arc<Mutex<Vec<Vec<u8>>>>,
    format: Arc<Mutex<Option<(u32, u32, u32)>>>,
    stream_on_count: Arc<AtomicU64>,
    stream_off_count: Arc<AtomicU64>,
    should_fail_write: Arc<AtomicBool>,
}

impl Default for MockV4l2Sink {
    fn default() -> Self {
        Self::new(PathBuf::from("/dev/video-mock"))
    }
}

impl MockV4l2Sink {
    pub fn new(device_path: PathBuf) -> Self {
        Self {
            device_path,
            is_streaming: Arc::new(AtomicBool::new(false)),
            written_frames: Arc::new(Mutex::new(Vec::new())),
            format: Arc::new(Mutex::new(None)),
            stream_on_count: Arc::new(AtomicU64::new(0)),
            stream_off_count: Arc::new(AtomicU64::new(0)),
            should_fail_write: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn written_frames(&self) -> Vec<Vec<u8>> {
        self.written_frames.lock().unwrap().clone()
    }

    pub fn frame_count(&self) -> usize {
        self.written_frames.lock().unwrap().len()
    }

    pub fn configured_format(&self) -> Option<(u32, u32, u32)> {
        *self.format.lock().unwrap()
    }

    pub fn stream_on_count(&self) -> u64 {
        self.stream_on_count.load(Ordering::SeqCst)
    }

    pub fn stream_off_count(&self) -> u64 {
        self.stream_off_count.load(Ordering::SeqCst)
    }

    pub fn set_fail_writes(&self, fail: bool) {
        self.should_fail_write.store(fail, Ordering::SeqCst);
    }
}

impl V4l2Sink for MockV4l2Sink {
    fn set_format(&mut self, width: u32, height: u32, pixelformat: u32) -> Result<(), WebcamError> {
        let mut fmt = self.format.lock().unwrap();
        *fmt = Some((width, height, pixelformat));
        Ok(())
    }

    fn stream_on(&mut self) -> Result<(), WebcamError> {
        self.is_streaming.store(true, Ordering::SeqCst);
        self.stream_on_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), WebcamError> {
        if self.should_fail_write.load(Ordering::SeqCst) {
            return Err(WebcamError::WriteFailed {
                source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Mock write failure"),
            });
        }
        self.is_streaming.store(true, Ordering::SeqCst);
        let mut frames = self.written_frames.lock().unwrap();
        frames.push(frame.to_vec());
        Ok(())
    }

    fn stream_off(&mut self) -> Result<(), WebcamError> {
        self.is_streaming.store(false, Ordering::SeqCst);
        self.stream_off_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn is_streaming(&self) -> bool {
        self.is_streaming.load(Ordering::SeqCst)
    }

    fn device_path(&self) -> &Path {
        &self.device_path
    }
}
