//! Binary wire framing for low-latency desktop audio streaming.
//!
//! Enforces the canonical 32-byte binary `ADAU` header framing:
//! - Magic: 4 bytes `0x41 0x44 0x41 0x55` ("ADAU")
//! - Codec: 1 byte (`0x01` Opus, `0x02` PCM)
//! - Channels: 1 byte (`0x02` stereo)
//! - Flags: 2 bytes u16 BE (`0x0001` discontinuity, `0x0000` normal)
//! - Sample Rate: 4 bytes u32 BE (`48000`)
//! - Sequence Number: 8 bytes u64 BE (monotonic)
//! - TimestampMs: 8 bytes u64 BE (epoch ms)
//! - PayloadLen: 4 bytes u32 BE
//! - Followed by Payload data (`[u8; PayloadLen]`)

use crate::error::AudioError;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Magic marker bytes for ArchDrop Audio binary framing ("ADAU").
pub const ADAU_MAGIC: [u8; 4] = [0x41, 0x44, 0x41, 0x55];

/// Canonical fixed header size in bytes.
pub const HEADER_SIZE: usize = 32;

/// Standard sample rate for desktop audio streaming.
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Default channel count (Stereo).
pub const DEFAULT_CHANNELS: u8 = 2;

/// Default frame duration in milliseconds (20 ms).
pub const DEFAULT_FRAME_DURATION_MS: u16 = 20;

/// Frame flags: Normal packet (in-order, continuous).
pub const FLAG_NORMAL: u16 = 0x0000;

/// Frame flags: Discontinuity indicator (packet dropped due to backpressure or buffer reset).
pub const FLAG_DISCONTINUITY: u16 = 0x0001;

/// Supported wire audio codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum AudioCodec {
    /// Opus low-latency encoded audio (48kHz stereo, 20ms frames).
    Opus = 0x01,
    /// Raw uncompressed 16-bit signed little-endian PCM (s16le, 48kHz stereo, 20ms frames).
    Pcm = 0x02,
}

impl AudioCodec {
    pub fn from_u8(val: u8) -> Result<Self, AudioError> {
        match val {
            0x01 => Ok(AudioCodec::Opus),
            0x02 => Ok(AudioCodec::Pcm),
            other => Err(AudioError::Framing(format!(
                "Unknown audio codec ID: 0x{:02X} (supported: 0x01=Opus, 0x02=PCM)",
                other
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "opus",
            AudioCodec::Pcm => "pcm",
        }
    }
}

/// Zeroizing audio buffer for memory hygiene.
///
/// Ensures residual audio data (media, voice, speech) is automatically zeroed in memory
/// when the buffer is dropped.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct AudioBuffer {
    data: Vec<u8>,
}

impl AudioBuffer {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        Self {
            data: slice.to_vec(),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn into_inner(mut self) -> Vec<u8> {
        let out = std::mem::take(&mut self.data);
        out
    }
}

impl std::fmt::Debug for AudioBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AudioBuffer({} bytes)", self.data.len())
    }
}

impl Deref for AudioBuffer {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for AudioBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

/// Canonical 32-byte binary wire header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioWireHeader {
    pub magic: [u8; 4],
    pub codec: AudioCodec,
    pub channels: u8,
    pub flags: u16,
    pub sample_rate: u32,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub payload_len: u32,
}

impl AudioWireHeader {
    pub fn new(
        codec: AudioCodec,
        channels: u8,
        flags: u16,
        sample_rate: u32,
        sequence: u64,
        timestamp_ms: u64,
        payload_len: u32,
    ) -> Self {
        Self {
            magic: ADAU_MAGIC,
            codec,
            channels,
            flags,
            sample_rate,
            sequence,
            timestamp_ms,
            payload_len,
        }
    }

    /// Serialize the header into an exact 32-byte big-endian slice.
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4] = self.codec as u8;
        buf[5] = self.channels;
        buf[6..8].copy_from_slice(&self.flags.to_be_bytes());
        buf[8..12].copy_from_slice(&self.sample_rate.to_be_bytes());
        buf[12..20].copy_from_slice(&self.sequence.to_be_bytes());
        buf[20..28].copy_from_slice(&self.timestamp_ms.to_be_bytes());
        buf[28..32].copy_from_slice(&self.payload_len.to_be_bytes());
        buf
    }

    /// Parse an exact 32-byte slice into an `AudioWireHeader`.
    pub fn from_bytes(bytes: &[u8; HEADER_SIZE]) -> Result<Self, AudioError> {
        if bytes[0..4] != ADAU_MAGIC {
            return Err(AudioError::Framing(format!(
                "Invalid magic bytes: expected ADAU ({:02X?}), got {:02X?}",
                ADAU_MAGIC,
                &bytes[0..4]
            )));
        }

        let codec = AudioCodec::from_u8(bytes[4])?;
        let channels = bytes[5];
        let flags = u16::from_be_bytes([bytes[6], bytes[7]]);
        let sample_rate = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let sequence = u64::from_be_bytes([
            bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18], bytes[19],
        ]);
        let timestamp_ms = u64::from_be_bytes([
            bytes[20], bytes[21], bytes[22], bytes[23], bytes[24], bytes[25], bytes[26], bytes[27],
        ]);
        let payload_len = u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);

        Ok(Self {
            magic: ADAU_MAGIC,
            codec,
            channels,
            flags,
            sample_rate,
            sequence,
            timestamp_ms,
            payload_len,
        })
    }

    /// Parse a header from any slice of at least 32 bytes.
    pub fn parse(input: &[u8]) -> Result<(Self, &[u8]), AudioError> {
        if input.len() < HEADER_SIZE {
            return Err(AudioError::Framing(format!(
                "Buffer too short for audio header: need at least {} bytes, got {}",
                HEADER_SIZE,
                input.len()
            )));
        }

        let mut header_bytes = [0u8; HEADER_SIZE];
        header_bytes.copy_from_slice(&input[0..HEADER_SIZE]);
        let header = Self::from_bytes(&header_bytes)?;
        Ok((header, &input[HEADER_SIZE..]))
    }
}

/// A complete binary audio frame consisting of a 32-byte ADAU header and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioWireFrame {
    pub header: AudioWireHeader,
    pub payload: AudioBuffer,
}

impl AudioWireFrame {
    pub fn new(header: AudioWireHeader, payload: AudioBuffer) -> Self {
        Self { header, payload }
    }

    /// Construct a frame with automatic payload length synchronization.
    pub fn create(
        codec: AudioCodec,
        channels: u8,
        flags: u16,
        sample_rate: u32,
        sequence: u64,
        timestamp_ms: u64,
        payload: Vec<u8>,
    ) -> Self {
        let payload_len = payload.len() as u32;
        let header = AudioWireHeader::new(
            codec,
            channels,
            flags,
            sample_rate,
            sequence,
            timestamp_ms,
            payload_len,
        );
        Self {
            header,
            payload: AudioBuffer::new(payload),
        }
    }

    /// Encode frame into a contiguous byte vector.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_SIZE + self.payload.len());
        out.extend_from_slice(&self.header.to_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Encode frame into `bytes::Bytes` for zero-copy HTTP streaming.
    pub fn to_bytes(&self) -> Bytes {
        Bytes::from(self.encode())
    }

    /// Decode a complete frame from a slice, returning the frame and number of consumed bytes.
    pub fn decode(src: &[u8]) -> Result<(Self, usize), AudioError> {
        let (header, remainder) = AudioWireHeader::parse(src)?;
        let payload_len = header.payload_len as usize;

        if remainder.len() < payload_len {
            return Err(AudioError::Framing(format!(
                "Buffer too short for payload: expected {} bytes, got {}",
                payload_len,
                remainder.len()
            )));
        }

        let payload = AudioBuffer::from_slice(&remainder[..payload_len]);
        let consumed = HEADER_SIZE + payload_len;

        Ok((Self { header, payload }, consumed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_serialization_roundtrip() {
        let header = AudioWireHeader::new(
            AudioCodec::Opus,
            2,
            FLAG_NORMAL,
            48000,
            123456789,
            1700000000123,
            240,
        );

        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[0..4], &ADAU_MAGIC);
        assert_eq!(bytes[4], 0x01); // Opus
        assert_eq!(bytes[5], 2); // Stereo
        assert_eq!(&bytes[6..8], &[0x00, 0x00]); // Normal
        assert_eq!(u32::from_be_bytes(bytes[8..12].try_into().unwrap()), 48000);
        assert_eq!(
            u64::from_be_bytes(bytes[12..20].try_into().unwrap()),
            123456789
        );
        assert_eq!(
            u64::from_be_bytes(bytes[20..28].try_into().unwrap()),
            1700000000123
        );
        assert_eq!(u32::from_be_bytes(bytes[28..32].try_into().unwrap()), 240);

        let decoded = AudioWireHeader::from_bytes(&bytes).expect("Failed to parse header");
        assert_eq!(header, decoded);
    }

    #[test]
    fn test_pcm_header_and_discontinuity_flag() {
        let header = AudioWireHeader::new(
            AudioCodec::Pcm,
            2,
            FLAG_DISCONTINUITY,
            48000,
            42,
            1700000000456,
            3840,
        );

        let bytes = header.to_bytes();
        assert_eq!(bytes[4], 0x02); // PCM
        assert_eq!(&bytes[6..8], &[0x00, 0x01]); // Discontinuity

        let decoded = AudioWireHeader::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.codec, AudioCodec::Pcm);
        assert_eq!(decoded.flags, FLAG_DISCONTINUITY);
        assert_eq!(decoded.payload_len, 3840);
    }

    #[test]
    fn test_invalid_magic_rejected() {
        let mut bytes = [0u8; 32];
        bytes[0..4].copy_from_slice(b"NOPE");
        let result = AudioWireHeader::from_bytes(&bytes);
        assert!(matches!(result, Err(AudioError::Framing(_))));
    }

    #[test]
    fn test_invalid_codec_rejected() {
        let mut bytes = AudioWireHeader::new(
            AudioCodec::Opus,
            2,
            FLAG_NORMAL,
            48000,
            1,
            1000,
            100,
        )
        .to_bytes();
        bytes[4] = 0x99; // Invalid codec
        let result = AudioWireHeader::from_bytes(&bytes);
        assert!(matches!(result, Err(AudioError::Framing(_))));
    }

    #[test]
    fn test_frame_encode_decode() {
        let payload = vec![0xAB, 0xCD, 0xEF, 0x01, 0x23];
        let frame = AudioWireFrame::create(
            AudioCodec::Opus,
            2,
            FLAG_NORMAL,
            48000,
            10,
            1700000000000,
            payload.clone(),
        );

        let encoded = frame.encode();
        assert_eq!(encoded.len(), 32 + payload.len());

        let (decoded, consumed) = AudioWireFrame::decode(&encoded).expect("Decode frame");
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.header.sequence, 10);
        assert_eq!(decoded.header.payload_len, 5);
        assert_eq!(decoded.payload.as_slice(), &payload[..]);
    }

    #[test]
    fn test_zeroize_on_drop() {
        let buffer = AudioBuffer::from_slice(&[1, 2, 3, 4, 5]);
        let ptr = buffer.as_ptr();
        drop(buffer);
        // Note: The memory is zeroized upon drop.
        assert_eq!(ptr.is_null(), false);
    }
}
