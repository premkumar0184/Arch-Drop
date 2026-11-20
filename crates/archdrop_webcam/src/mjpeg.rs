//! Streaming multipart MJPEG parser state machine.
//!
//! Efficient zero-copy parser for `multipart/x-mixed-replace; boundary=frame`
//! HTTP streams, validating JPEG Start of Image (SOI) and End of Image (EOI)
//! markers and handling arbitrary chunk fragmentation.

use crate::error::WebcamError;
use bytes::{Buf, Bytes, BytesMut};
use tracing::debug;

pub const MAX_FRAME_SIZE: usize = 10 * 1024 * 1024; // 10 MB safety ceiling
pub const MAX_HEADER_SIZE: usize = 16 * 1024; // 16 KB

/// A parsed single JPEG frame with associated metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MjpegFrame {
    /// Raw JPEG binary frame bytes (starts with 0xFF 0xD8, ends with 0xFF 0xD9).
    pub data: Bytes,
    /// Frame capture timestamp in milliseconds if provided by the client.
    pub timestamp_ms: Option<u64>,
    /// Frame MIME content type (e.g. `image/jpeg`).
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    SeekingBoundary,
    ReadingHeaders,
    ReadingPayload {
        content_length: usize,
        timestamp_ms: Option<u64>,
    },
    ScanningUntilNextBoundary {
        timestamp_ms: Option<u64>,
    },
}

/// Zero-copy streaming multipart MJPEG parser.
pub struct MjpegStreamParser {
    boundary_bytes: Vec<u8>,
    buffer: BytesMut,
    state: ParserState,
    is_finished: bool,
    current_content_type: Option<String>,
}

impl MjpegStreamParser {
    /// Creates a new parser for the given multipart boundary string.
    pub fn new(boundary: impl Into<String>) -> Self {
        let b = boundary.into();
        let trimmed = b.trim();
        let normalized = if trimmed.starts_with("--") {
            trimmed.to_string()
        } else {
            format!("--{trimmed}")
        };

        Self {
            boundary_bytes: normalized.into_bytes(),
            buffer: BytesMut::with_capacity(64 * 1024),
            state: ParserState::SeekingBoundary,
            is_finished: false,
            current_content_type: None,
        }
    }

    /// Appends a newly received chunk of stream bytes to the internal accumulator.
    pub fn push_chunk(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Attempts to extract the next completed JPEG frame from accumulated bytes.
    pub fn next_frame(&mut self) -> Result<Option<MjpegFrame>, WebcamError> {
        if self.is_finished {
            return Ok(None);
        }

        loop {
            match self.state {
                ParserState::SeekingBoundary => {
                    if let Some(pos) = find_subsequence(&self.buffer, &self.boundary_bytes) {
                        // Ensure we have at least 2 bytes following the boundary to distinguish
                        // the terminating delimiter "--" from a CRLF or LF header delimiter.
                        let after_boundary = pos + self.boundary_bytes.len();
                        if self.buffer.len() < after_boundary + 2 {
                            return Ok(None);
                        }

                        // Advance past any garbage preceding the boundary
                        self.buffer.advance(pos);
                        // Advance past boundary itself
                        self.buffer.advance(self.boundary_bytes.len());

                        // Check if boundary ends with "--" indicating end of multipart stream
                        if self.buffer.starts_with(b"--") {
                            self.buffer.advance(2);
                            self.is_finished = true;
                            debug!("Detected terminating multipart boundary delimiter");
                            return Ok(None);
                        }

                        // Consume optional newline following boundary delimiter
                        consume_newline(&mut self.buffer);
                        self.state = ParserState::ReadingHeaders;
                    } else {
                        // Keep only enough bytes to match boundary if split across chunks
                        if self.buffer.len() > self.boundary_bytes.len() + 4 {
                            let keep = self.boundary_bytes.len() + 4;
                            let discard = self.buffer.len() - keep;
                            self.buffer.advance(discard);
                        }
                        return Ok(None);
                    }
                }

                ParserState::ReadingHeaders => {
                    // Look for header terminator: \r\n\r\n or \n\n
                    if let Some((header_end, term_len)) = find_header_terminator(&self.buffer) {
                        let header_slice = &self.buffer[..header_end];
                        let header_str = String::from_utf8_lossy(header_slice);

                        let mut content_length: Option<usize> = None;
                        let mut timestamp_ms: Option<u64> = None;
                        let mut content_type: Option<String> = None;

                        for line in header_str.lines() {
                            let line = line.trim();
                            if let Some((k, v)) = line.split_once(':') {
                                let key = k.trim().to_lowercase();
                                let val = v.trim();
                                match key.as_str() {
                                    "content-length" => {
                                        if let Ok(len) = val.parse::<usize>() {
                                            content_length = Some(len);
                                        }
                                    }
                                    "x-timestamp-ms" | "x-timestamp" => {
                                        if let Ok(ts) = val.parse::<u64>() {
                                            timestamp_ms = Some(ts);
                                        }
                                    }
                                    "content-type" => {
                                        content_type = Some(val.to_string());
                                    }
                                    _ => {}
                                }
                            }
                        }

                        self.current_content_type = content_type;

                        // Advance past headers and terminator
                        self.buffer.advance(header_end + term_len);

                        if let Some(len) = content_length {
                            if len > MAX_FRAME_SIZE {
                                return Err(WebcamError::FrameTooLarge {
                                    size: len,
                                    max: MAX_FRAME_SIZE,
                                });
                            }
                            self.state = ParserState::ReadingPayload {
                                content_length: len,
                                timestamp_ms,
                            };
                        } else {
                            self.state = ParserState::ScanningUntilNextBoundary { timestamp_ms };
                        }
                    } else {
                        if self.buffer.len() > MAX_HEADER_SIZE {
                            return Err(WebcamError::MalformedStream(
                                "Multipart headers exceeded maximum allowable size of 16 KB".into(),
                            ));
                        }
                        return Ok(None);
                    }
                }

                ParserState::ReadingPayload {
                    content_length,
                    timestamp_ms,
                } => {
                    if self.buffer.len() < content_length {
                        // Need more data
                        return Ok(None);
                    }

                    // Zero-copy slice of the payload
                    let frame_data = self.buffer.split_to(content_length).freeze();

                    // Consume any trailing newline between payload and next boundary
                    consume_newline(&mut self.buffer);

                    validate_jpeg(&frame_data)?;

                    self.state = ParserState::SeekingBoundary;
                    return Ok(Some(MjpegFrame {
                        data: frame_data,
                        timestamp_ms,
                        content_type: self.current_content_type.take(),
                    }));
                }

                ParserState::ScanningUntilNextBoundary { timestamp_ms } => {
                    if let Some(pos) = find_subsequence(&self.buffer, &self.boundary_bytes) {
                        let mut frame_data = self.buffer.split_to(pos).freeze();

                        // Trim trailing CRLF or LF before the boundary
                        while frame_data.ends_with(b"\r") || frame_data.ends_with(b"\n") {
                            frame_data = frame_data.slice(0..frame_data.len() - 1);
                        }

                        if frame_data.len() > MAX_FRAME_SIZE {
                            return Err(WebcamError::FrameTooLarge {
                                size: frame_data.len(),
                                max: MAX_FRAME_SIZE,
                            });
                        }

                        validate_jpeg(&frame_data)?;

                        // Transition to SeekingBoundary to consume boundary in next loop
                        self.state = ParserState::SeekingBoundary;
                        return Ok(Some(MjpegFrame {
                            data: frame_data,
                            timestamp_ms,
                            content_type: self.current_content_type.take(),
                        }));
                    } else {
                        if self.buffer.len() > MAX_FRAME_SIZE {
                            return Err(WebcamError::FrameTooLarge {
                                size: self.buffer.len(),
                                max: MAX_FRAME_SIZE,
                            });
                        }
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Returns whether the terminating multipart boundary (`--<boundary>--`) has been encountered.
    pub fn is_finished(&self) -> bool {
        self.is_finished
    }
}

fn consume_newline(buf: &mut BytesMut) {
    if buf.starts_with(b"\r\n") {
        buf.advance(2);
    } else if buf.starts_with(b"\n") {
        buf.advance(1);
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn find_header_terminator(buf: &[u8]) -> Option<(usize, usize)> {
    if let Some(pos) = find_subsequence(buf, b"\r\n\r\n") {
        return Some((pos, 4));
    }
    if let Some(pos) = find_subsequence(buf, b"\n\n") {
        return Some((pos, 2));
    }
    None
}

/// Validates JPEG Start of Image (0xFF, 0xD8) and End of Image (0xFF, 0xD9) markers.
pub fn validate_jpeg(data: &[u8]) -> Result<(), WebcamError> {
    if data.len() < 4 {
        return Err(WebcamError::MalformedJpeg(format!(
            "Payload length {} is too small for a valid JPEG",
            data.len()
        )));
    }

    // SOI: 0xFF, 0xD8
    if data[0] != 0xFF || data[1] != 0xD8 {
        return Err(WebcamError::MalformedJpeg(format!(
            "Missing JPEG SOI marker [0xFF, 0xD8]; found [0x{:02X}, 0x{:02X}]",
            data[0], data[1]
        )));
    }

    // EOI: 0xFF, 0xD9 anywhere in the last 16 bytes (to tolerate trailing padding)
    let search_window = if data.len() > 16 {
        &data[data.len() - 16..]
    } else {
        data
    };

    let has_eoi = search_window
        .windows(2)
        .any(|w| w[0] == 0xFF && w[1] == 0xD9);

    if !has_eoi {
        return Err(WebcamError::MalformedJpeg(
            "Missing JPEG EOI marker [0xFF, 0xD9] at end of payload".into(),
        ));
    }

    Ok(())
}

/// Extracts width and height from a JPEG frame by parsing SOF markers without decoding pixel data.
pub fn parse_jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF || marker == 0x00 {
            i += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0xD9 {
            i += 2;
            continue;
        }
        // Start of Scan: entropy-coded scan begins here, header parsing concludes
        if marker == 0xDA {
            break;
        }
        let length = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if length < 2 || i + 2 + length > data.len() {
            break;
        }
        // SOF0 (0xC0), SOF1 (0xC1), SOF2 (0xC2), SOF3 (0xC3), SOF5-SOF7, SOF9-SOF11, SOF13-SOF15
        let is_sof = (marker >= 0xC0 && marker <= 0xC3)
            || (marker >= 0xC5 && marker <= 0xC7)
            || (marker >= 0xC9 && marker <= 0xCB)
            || (marker >= 0xCD && marker <= 0xCF);

        if is_sof && length >= 7 && i + 9 <= data.len() {
            let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            if width > 0 && height > 0 {
                return Some((width, height));
            }
        }
        i += 2 + length;
    }
    None
}

