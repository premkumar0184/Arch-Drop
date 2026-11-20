//! Audio encoders for low-latency desktop audio streaming.
//!
//! Provides trait-driven abstractions for:
//! - `OpusAudioEncoder`: Low-latency Opus encoding (`opus::Application::LowDelay`, 20ms frames, 64–128 kbps).
//! - `PcmPassthroughEncoder`: Raw uncompressed 16-bit PCM fallback for zero CPU overhead.

use crate::error::AudioError;
use crate::framing::{AudioCodec, DEFAULT_CHANNELS, DEFAULT_FRAME_DURATION_MS, DEFAULT_SAMPLE_RATE};

/// Audio encoder trait for transforming PCM samples into wire packets.
pub trait AudioEncoder: Send {
    /// Return the active codec type.
    fn codec(&self) -> AudioCodec;

    /// Configured sample rate in Hz (e.g. 48000).
    fn sample_rate(&self) -> u32;

    /// Configured channel count (e.g. 2 for stereo).
    fn channels(&self) -> u8;

    /// Configured frame duration in milliseconds (e.g. 20 ms).
    fn frame_duration_ms(&self) -> u16;

    /// Encode an interleaved slice of signed 16-bit PCM samples (`[L0, R0, L1, R1, ...]`).
    fn encode(&mut self, pcm_samples: &[i16]) -> Result<Vec<u8>, AudioError>;

    /// Convenience helper to encode raw interleaved little-endian bytes (`s16le`).
    fn encode_bytes(&mut self, pcm_bytes: &[u8]) -> Result<Vec<u8>, AudioError> {
        if pcm_bytes.len() % 2 != 0 {
            return Err(AudioError::InvalidPcmLength {
                expected: 2,
                actual: pcm_bytes.len(),
            });
        }

        let samples: Vec<i16> = pcm_bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();

        self.encode(&samples)
    }

    /// Reset internal encoder state (e.g. on stream discontinuity).
    fn reset(&mut self) -> Result<(), AudioError>;
}

/// Low-latency Opus encoder implementation.
pub struct OpusAudioEncoder {
    encoder: opus::Encoder,
    sample_rate: u32,
    channels: u8,
    frame_duration_ms: u16,
    bitrate: u32,
}

impl OpusAudioEncoder {
    /// Create a new low-latency Opus encoder.
    ///
    /// # Arguments
    /// - `sample_rate`: Sample rate (standard is 48000).
    /// - `channels`: 1 for Mono, 2 for Stereo (standard is 2).
    /// - `bitrate`: Target bitrate in bps (standard range 64000..128000 bps).
    pub fn new(sample_rate: u32, channels: u8, bitrate: u32) -> Result<Self, AudioError> {
        let opus_channels = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            other => {
                return Err(AudioError::EncodingFailed(format!(
                    "Unsupported Opus channels: {other} (only 1 or 2 supported)"
                )))
            }
        };

        let mut encoder = opus::Encoder::new(sample_rate, opus_channels, opus::Application::LowDelay)
            .map_err(|e| AudioError::EncodingFailed(format!("Failed to create Opus encoder: {e}")))?;

        // Set target bitrate
        encoder
            .set_bitrate(opus::Bitrate::Bits(bitrate as i32))
            .map_err(|e| AudioError::EncodingFailed(format!("Failed to set Opus bitrate: {e}")))?;

        // Optimize for real-time streaming: disable FEC (Android c2.android.opus.decoder
        // cannot use inband FEC data, so it only wastes bitrate and alters frame structure
        // in ways that produce tonal noise artifacts on the receiver side).
        let _ = encoder.set_inband_fec(false);
        let _ = encoder.set_packet_loss_perc(0);

        Ok(Self {
            encoder,
            sample_rate,
            channels,
            frame_duration_ms: DEFAULT_FRAME_DURATION_MS,
            bitrate,
        })
    }

    /// Return the active bitrate in bps.
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }
}

impl AudioEncoder for OpusAudioEncoder {
    fn codec(&self) -> AudioCodec {
        AudioCodec::Opus
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn frame_duration_ms(&self) -> u16 {
        self.frame_duration_ms
    }

    fn encode(&mut self, pcm_samples: &[i16]) -> Result<Vec<u8>, AudioError> {
        // Expected samples for 20 ms at 48000 Hz stereo = 48000 * 0.02 * 2 = 1920 samples
        let mut out_buf = vec![0u8; 4000];
        let bytes_written = self
            .encoder
            .encode(pcm_samples, &mut out_buf)
            .map_err(|e| AudioError::EncodingFailed(format!("Opus encoding failed: {e}")))?;

        out_buf.truncate(bytes_written);
        Ok(out_buf)
    }

    fn reset(&mut self) -> Result<(), AudioError> {
        let _ = self.encoder.reset_state();
        Ok(())
    }
}

/// Raw uncompressed 16-bit PCM passthrough encoder.
pub struct PcmPassthroughEncoder {
    sample_rate: u32,
    channels: u8,
    frame_duration_ms: u16,
}

impl PcmPassthroughEncoder {
    pub fn new(sample_rate: u32, channels: u8) -> Self {
        Self {
            sample_rate,
            channels,
            frame_duration_ms: DEFAULT_FRAME_DURATION_MS,
        }
    }

    pub fn default_48k_stereo() -> Self {
        Self::new(DEFAULT_SAMPLE_RATE, DEFAULT_CHANNELS)
    }
}

impl Default for PcmPassthroughEncoder {
    fn default() -> Self {
        Self::default_48k_stereo()
    }
}

impl AudioEncoder for PcmPassthroughEncoder {
    fn codec(&self) -> AudioCodec {
        AudioCodec::Pcm
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn frame_duration_ms(&self) -> u16 {
        self.frame_duration_ms
    }

    fn encode(&mut self, pcm_samples: &[i16]) -> Result<Vec<u8>, AudioError> {
        let mut out = Vec::with_capacity(pcm_samples.len() * 2);
        for &sample in pcm_samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(out)
    }

    fn encode_bytes(&mut self, pcm_bytes: &[u8]) -> Result<Vec<u8>, AudioError> {
        if pcm_bytes.len() % 2 != 0 {
            return Err(AudioError::InvalidPcmLength {
                expected: 2,
                actual: pcm_bytes.len(),
            });
        }
        Ok(pcm_bytes.to_vec())
    }

    fn reset(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcm_passthrough_encoder() {
        let mut encoder = PcmPassthroughEncoder::default();
        assert_eq!(encoder.codec(), AudioCodec::Pcm);
        assert_eq!(encoder.sample_rate(), 48000);
        assert_eq!(encoder.channels(), 2);

        let input_samples: Vec<i16> = vec![0, 1000, -1000, 32767, -32768];
        let encoded = encoder.encode(&input_samples).expect("Encode PCM");

        assert_eq!(encoded.len(), 10);
        // Verify little endian byte reconstruction
        assert_eq!(i16::from_le_bytes([encoded[0], encoded[1]]), 0);
        assert_eq!(i16::from_le_bytes([encoded[2], encoded[3]]), 1000);
        assert_eq!(i16::from_le_bytes([encoded[4], encoded[5]]), -1000);
        assert_eq!(i16::from_le_bytes([encoded[6], encoded[7]]), 32767);
        assert_eq!(i16::from_le_bytes([encoded[8], encoded[9]]), -32768);
    }

    #[test]
    fn test_opus_roundtrip_encode_decode() {
        let mut encoder = OpusAudioEncoder::new(48000, 2, 96000).expect("Create Opus encoder");
        assert_eq!(encoder.codec(), AudioCodec::Opus);
        assert_eq!(encoder.bitrate(), 96000);

        // Generate 20ms of 480Hz stereo sine wave (960 stereo samples = 1920 i16)
        let num_samples = 960;
        let mut pcm: Vec<i16> = Vec::with_capacity(num_samples * 2);
        for i in 0..num_samples {
            let t = i as f32 / 48000.0;
            let val = (f32::sin(2.0 * std::f32::consts::PI * 480.0 * t) * 16000.0) as i16;
            pcm.push(val); // Left
            pcm.push(val); // Right
        }

        let encoded = encoder.encode(&pcm).expect("Opus encode");
        // Opus at 96kbps for 20ms produces ~240 bytes (should be between 50 and 600 bytes)
        assert!(encoded.len() > 50 && encoded.len() < 600);

        // Decode using Opus decoder to verify low-delay validity
        let mut decoder = opus::Decoder::new(48000, opus::Channels::Stereo).expect("Create Opus decoder");
        let mut decoded_pcm = vec![0i16; 1920];
        let samples_decoded = decoder
            .decode(&encoded, &mut decoded_pcm, false)
            .expect("Opus decode");

        assert_eq!(samples_decoded, 960);
    }
}
