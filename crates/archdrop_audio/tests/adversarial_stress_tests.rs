//! Adversarial Stress and Boundary Test Suite for archdrop_audio.
//!
//! Thoroughly stress-tests:
//! 1. Framing boundaries, corrupt headers, sequence wraps, random byte fuzzing, and extreme payloads.
//! 2. Opus codec with 64k, 96k, 128k bitrates, pathological signals, and PCM fallback fidelity.
//! 3. Memory zeroization guarantees on AudioBuffer and AudioWireFrame.
//! 4. Audio streaming session backpressure, channel saturation, and cancellation.

use archdrop_audio::capture::MockAudioCapture;
use archdrop_audio::codec::{AudioEncoder, OpusAudioEncoder, PcmPassthroughEncoder};
use archdrop_audio::error::AudioError;
use archdrop_audio::framing::{
    AudioBuffer, AudioCodec, AudioWireFrame, AudioWireHeader, DEFAULT_CHANNELS,
    DEFAULT_SAMPLE_RATE, FLAG_DISCONTINUITY, FLAG_NORMAL, HEADER_SIZE,
};
use archdrop_audio::session::AudioStreamSession;
use archdrop_audio::sink::MockVirtualSink;
use std::time::Duration;
use zeroize::{Zeroize, ZeroizeOnDrop};

// ============================================================================
// 1. FRAMING STRESS & BOUNDARY TESTS
// ============================================================================

#[test]
fn test_framing_header_size_boundaries() {
    // Exact header size (32 bytes) with 0 payload
    let valid_header = AudioWireHeader::new(
        AudioCodec::Opus,
        2,
        FLAG_NORMAL,
        48000,
        0,
        1700000000000,
        0,
    );
    let raw_header = valid_header.to_bytes();
    assert_eq!(raw_header.len(), HEADER_SIZE);

    // Parsing exact 32 bytes with payload_len=0 should succeed
    let (decoded, rem) = AudioWireHeader::parse(&raw_header).expect("Parse 32-byte header");
    assert_eq!(decoded, valid_header);
    assert!(rem.is_empty());

    // Buffer lengths from 0 to 31 bytes must all be rejected with AudioError::Framing
    for len in 0..HEADER_SIZE {
        let truncated = &raw_header[..len];
        let res = AudioWireHeader::parse(truncated);
        assert!(
            matches!(res, Err(AudioError::Framing(_))),
            "Header buffer of length {len} should have been rejected"
        );

        let frame_res = AudioWireFrame::decode(truncated);
        assert!(
            matches!(frame_res, Err(AudioError::Framing(_))),
            "Frame buffer of length {len} should have been rejected"
        );
    }
}

#[test]
fn test_framing_corrupt_magic_bytes() {
    let base_header = AudioWireHeader::new(
        AudioCodec::Opus,
        2,
        FLAG_NORMAL,
        48000,
        1,
        1700000000000,
        10,
    );
    let valid_bytes = base_header.to_bytes();

    let corrupt_magics: Vec<[u8; 4]> = vec![
        [0x00, 0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF, 0xFF],
        *b"adau", // Lowercase
        *b"ADAV", // 1 char off
        *b"UADA", // Reversed
        *b"ARCH", // Unrelated ASCII
        [0x41, 0x44, 0x41, 0x00],
    ];

    for bad_magic in corrupt_magics {
        let mut corrupted = valid_bytes;
        corrupted[0..4].copy_from_slice(&bad_magic);

        let header_res = AudioWireHeader::from_bytes(&corrupted);
        assert!(
            matches!(header_res, Err(AudioError::Framing(_))),
            "Corrupt magic {:?} must be rejected",
            bad_magic
        );

        let parse_res = AudioWireHeader::parse(&corrupted);
        assert!(
            matches!(parse_res, Err(AudioError::Framing(_))),
            "Corrupt magic {:?} must be rejected by parse",
            bad_magic
        );

        let frame_res = AudioWireFrame::decode(&corrupted);
        assert!(
            matches!(frame_res, Err(AudioError::Framing(_))),
            "Corrupt magic {:?} must be rejected by frame decode",
            bad_magic
        );
    }
}

#[test]
fn test_framing_invalid_codec_values() {
    let base_header = AudioWireHeader::new(
        AudioCodec::Opus,
        2,
        FLAG_NORMAL,
        48000,
        1,
        1700000000000,
        0,
    );
    let valid_bytes = base_header.to_bytes();

    // Valid: 0x01 (Opus), 0x02 (PCM)
    // Invalid: all other u8 values
    let invalid_codecs = [0x00, 0x03, 0x04, 0x10, 0x7F, 0x80, 0xFF];
    for bad_codec in invalid_codecs {
        let mut corrupted = valid_bytes;
        corrupted[4] = bad_codec;

        let res = AudioWireHeader::from_bytes(&corrupted);
        assert!(
            matches!(res, Err(AudioError::Framing(_))),
            "Codec 0x{:02X} must be rejected",
            bad_codec
        );
    }
}

#[test]
fn test_framing_sequence_wrap_around() {
    // Test sequences around u64::MAX to ensure 64-bit sequence wrapping works cleanly
    let wrap_sequences = [
        u64::MAX - 2,
        u64::MAX - 1,
        u64::MAX,
        0, // Wrapped
        1,
    ];

    for seq in wrap_sequences {
        let frame = AudioWireFrame::create(
            AudioCodec::Opus,
            DEFAULT_CHANNELS,
            FLAG_NORMAL,
            DEFAULT_SAMPLE_RATE,
            seq,
            1700000000000,
            vec![1, 2, 3, 4],
        );

        let encoded = frame.encode();
        let (decoded, consumed) = AudioWireFrame::decode(&encoded).expect("Decode sequence wrap frame");
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.header.sequence, seq);
    }
}

#[test]
fn test_framing_payload_extremes() {
    // 1. Zero-length payload
    let empty_frame = AudioWireFrame::create(
        AudioCodec::Opus,
        2,
        FLAG_NORMAL,
        48000,
        100,
        1700000000000,
        vec![],
    );
    assert_eq!(empty_frame.payload.len(), 0);
    assert_eq!(empty_frame.header.payload_len, 0);

    let empty_encoded = empty_frame.encode();
    assert_eq!(empty_encoded.len(), HEADER_SIZE);

    let (empty_decoded, consumed) = AudioWireFrame::decode(&empty_encoded).expect("Decode empty frame");
    assert_eq!(consumed, HEADER_SIZE);
    assert_eq!(empty_decoded.payload.len(), 0);
    assert!(empty_decoded.payload.is_empty());

    // 2. Exact standard PCM payload (3840 bytes)
    let pcm_payload = vec![0x33u8; 3840];
    let pcm_frame = AudioWireFrame::create(
        AudioCodec::Pcm,
        2,
        FLAG_NORMAL,
        48000,
        101,
        1700000000000,
        pcm_payload.clone(),
    );
    let pcm_encoded = pcm_frame.encode();
    assert_eq!(pcm_encoded.len(), HEADER_SIZE + 3840);
    let (pcm_decoded, consumed) = AudioWireFrame::decode(&pcm_encoded).expect("Decode PCM frame");
    assert_eq!(consumed, HEADER_SIZE + 3840);
    assert_eq!(pcm_decoded.payload.as_slice(), &pcm_payload[..]);

    // 3. Oversized payload (100,000 bytes)
    let large_payload = vec![0x7Au8; 100_000];
    let large_frame = AudioWireFrame::create(
        AudioCodec::Opus,
        2,
        FLAG_DISCONTINUITY,
        48000,
        102,
        1700000000000,
        large_payload.clone(),
    );
    let large_encoded = large_frame.encode();
    let (large_decoded, consumed) = AudioWireFrame::decode(&large_encoded).expect("Decode large frame");
    assert_eq!(consumed, HEADER_SIZE + 100_000);
    assert_eq!(large_decoded.payload.len(), 100_000);
    assert_eq!(large_decoded.header.flags, FLAG_DISCONTINUITY);

    // 4. Header claims u32::MAX payload but buffer is small -> must return Err, NOT allocate or OOM
    let mut bad_header = AudioWireHeader::new(
        AudioCodec::Opus,
        2,
        FLAG_NORMAL,
        48000,
        1,
        1700000000000,
        u32::MAX, // ~4.29 GB
    )
    .to_bytes()
    .to_vec();
    bad_header.extend_from_slice(&[0u8; 64]); // Only 64 bytes provided

    let decode_res = AudioWireFrame::decode(&bad_header);
    assert!(
        matches!(decode_res, Err(AudioError::Framing(_))),
        "Must safely reject u32::MAX payload when buffer is insufficient"
    );
}

#[test]
fn test_framing_trailing_bytes_and_stream_concatenation() {
    // Concatenate 50 distinct frames into a single byte stream and stream-decode them
    let mut stream_buffer = Vec::new();
    let frame_count = 50;

    for i in 0..frame_count {
        let codec = if i % 2 == 0 {
            AudioCodec::Opus
        } else {
            AudioCodec::Pcm
        };
        let payload_size = 50 + (i * 10);
        let payload = vec![(i & 0xFF) as u8; payload_size];
        let flags = if i == 15 {
            FLAG_DISCONTINUITY
        } else {
            FLAG_NORMAL
        };

        let frame = AudioWireFrame::create(
            codec,
            DEFAULT_CHANNELS,
            flags,
            DEFAULT_SAMPLE_RATE,
            i as u64,
            1700000000000 + (i as u64 * 20),
            payload,
        );
        stream_buffer.extend_from_slice(&frame.encode());
    }

    // Now parse the stream iteratively using `consumed`
    let mut cursor = 0;
    let mut decoded_count = 0;

    while cursor < stream_buffer.len() {
        let slice = &stream_buffer[cursor..];
        let (frame, consumed) = AudioWireFrame::decode(slice).expect("Decode frame from stream");

        assert_eq!(frame.header.sequence, decoded_count as u64);
        let expected_codec = if decoded_count % 2 == 0 {
            AudioCodec::Opus
        } else {
            AudioCodec::Pcm
        };
        assert_eq!(frame.header.codec, expected_codec);
        if decoded_count == 15 {
            assert_eq!(frame.header.flags, FLAG_DISCONTINUITY);
        } else {
            assert_eq!(frame.header.flags, FLAG_NORMAL);
        }

        let expected_payload_size = 50 + (decoded_count * 10);
        assert_eq!(frame.payload.len(), expected_payload_size);
        assert_eq!(frame.payload[0], (decoded_count & 0xFF) as u8);

        cursor += consumed;
        decoded_count += 1;
    }

    assert_eq!(decoded_count, frame_count);
    assert_eq!(cursor, stream_buffer.len());
}

#[test]
fn test_framing_fuzz_random_byte_streams() {
    // Simple fast pseudo-random generator (xorshift64)
    let mut state: u64 = 0xDEADBEEFCAFE1234;
    let mut next_rand = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    // Stress test with 5,000 random slices of sizes 0..2048 bytes
    // INVARIANT: Frame decoding and header parsing must NEVER panic under arbitrary random inputs.
    for _ in 0..5_000 {
        let len = (next_rand() % 2048) as usize;
        let mut random_bytes = vec![0u8; len];
        for chunk in random_bytes.chunks_mut(8) {
            let r = next_rand().to_ne_bytes();
            let c_len = chunk.len();
            chunk.copy_from_slice(&r[..c_len]);
        }

        // Must safely return Ok or Err, never panic
        let _ = AudioWireFrame::decode(&random_bytes);
        let _ = AudioWireHeader::parse(&random_bytes);
        if random_bytes.len() >= HEADER_SIZE {
            let mut exact = [0u8; HEADER_SIZE];
            exact.copy_from_slice(&random_bytes[..HEADER_SIZE]);
            let _ = AudioWireHeader::from_bytes(&exact);
        }
    }
}

// ============================================================================
// 2. CODEC STRESS & PCM FALLBACK TESTS
// ============================================================================

#[test]
fn test_opus_all_bitrates_roundtrip_fidelity() {
    // Standard target bitrates: 64k, 96k, 128k bps
    let bitrates = [64000, 96000, 128000];

    for &bitrate in &bitrates {
        let mut encoder = OpusAudioEncoder::new(DEFAULT_SAMPLE_RATE, DEFAULT_CHANNELS, bitrate)
            .expect("Init Opus encoder");
        assert_eq!(encoder.bitrate(), bitrate);
        assert_eq!(encoder.codec(), AudioCodec::Opus);
        assert_eq!(encoder.sample_rate(), 48000);
        assert_eq!(encoder.channels(), 2);
        assert_eq!(encoder.frame_duration_ms(), 20);

        let mut decoder = opus::Decoder::new(48000, opus::Channels::Stereo)
            .expect("Init Opus decoder for verification");

        // Generate 20 consecutive frames of 440 Hz stereo sine wave
        // 20 ms at 48000 Hz = 960 samples per channel = 1920 i16 elements per frame
        let samples_per_frame = 960;
        for frame_idx in 0..20 {
            let mut pcm = Vec::with_capacity(samples_per_frame * 2);
            for s in 0..samples_per_frame {
                let t = ((frame_idx * samples_per_frame + s) as f32) / 48000.0;
                let val = (f32::sin(2.0 * std::f32::consts::PI * 440.0 * t) * 16384.0) as i16;
                pcm.push(val); // L
                pcm.push(val); // R
            }

            let encoded = encoder.encode(&pcm).expect("Opus encode sine frame");
            assert!(
                !encoded.is_empty(),
                "Opus packet at {bitrate} bps must not be empty"
            );
            assert!(
                encoded.len() < 1275,
                "Opus packet exceeds maximum RFC 6716 length"
            );

            // Decode packet and verify sample count
            let mut decoded_pcm = vec![0i16; samples_per_frame * 2];
            let decoded_samples = decoder
                .decode(&encoded, &mut decoded_pcm, false)
                .expect("Opus decode frame");

            assert_eq!(
                decoded_samples, samples_per_frame,
                "Decoded samples must match 20ms frame"
            );

            // Verify non-zero energy in decoded audio (not blank silence)
            let max_amplitude = decoded_pcm.iter().map(|&x| x.abs()).max().unwrap_or(0);
            assert!(
                max_amplitude > 1000,
                "Decoded audio must retain audible energy for 440Hz sine wave (got {max_amplitude})"
            );
        }

        // Test encoder reset
        encoder.reset().expect("Reset Opus encoder state");
    }
}

#[test]
fn test_opus_pathological_inputs() {
    let mut encoder = OpusAudioEncoder::new(48000, 2, 96000).expect("Init Opus encoder");

    // 1. Digital silence (all zeros)
    let silence = vec![0i16; 1920];
    let encoded_silence = encoder.encode(&silence).expect("Encode silence");
    assert!(!encoded_silence.is_empty());

    // 2. Full-scale clipping / rail saturation (+32767 and -32768)
    let mut square_wave = Vec::with_capacity(1920);
    for i in 0..1920 {
        square_wave.push(if (i / 40) % 2 == 0 { 32767 } else { -32768 });
    }
    let encoded_square = encoder.encode(&square_wave).expect("Encode rail-saturated audio");
    assert!(!encoded_square.is_empty());

    // 3. Odd byte count passed to encode_bytes -> must return InvalidPcmLength
    let odd_bytes = vec![0u8; 1919];
    let odd_res = encoder.encode_bytes(&odd_bytes);
    assert!(
        matches!(odd_res, Err(AudioError::InvalidPcmLength { expected: 2, actual: 1919 })),
        "Odd byte length must fail with InvalidPcmLength"
    );

    // 4. Invalid sample count for Opus (e.g. 100 samples instead of 2.5/5/10/20/40/60 ms frame)
    let invalid_sample_count = vec![0i16; 100];
    let invalid_res = encoder.encode(&invalid_sample_count);
    assert!(
        matches!(invalid_res, Err(AudioError::EncodingFailed(_))),
        "Non-standard frame length must return EncodingFailed, not panic"
    );
}

#[test]
fn test_opus_channel_configuration_validation() {
    // Mono (1) and Stereo (2) are supported
    assert!(OpusAudioEncoder::new(48000, 1, 64000).is_ok());
    assert!(OpusAudioEncoder::new(48000, 2, 64000).is_ok());

    // 0 channels or >2 channels must return EncodingFailed
    assert!(matches!(
        OpusAudioEncoder::new(48000, 0, 64000),
        Err(AudioError::EncodingFailed(_))
    ));
    assert!(matches!(
        OpusAudioEncoder::new(48000, 3, 64000),
        Err(AudioError::EncodingFailed(_))
    ));
    assert!(matches!(
        OpusAudioEncoder::new(48000, 6, 64000),
        Err(AudioError::EncodingFailed(_))
    ));
}

#[test]
fn test_pcm_passthrough_fallback_exact_fidelity() {
    let mut encoder = PcmPassthroughEncoder::default();
    assert_eq!(encoder.codec(), AudioCodec::Pcm);
    assert_eq!(encoder.sample_rate(), 48000);
    assert_eq!(encoder.channels(), 2);

    // Test with full signed 16-bit dynamic range values
    let test_samples: Vec<i16> = vec![
        0, 1, -1, 32767, -32768, 12345, -12345, 255, -256, 16384, -16384,
    ];

    let encoded = encoder.encode(&test_samples).expect("Encode PCM samples");
    assert_eq!(encoded.len(), test_samples.len() * 2);

    // Verify bit-exact s16le reconstruction
    for (i, &sample) in test_samples.iter().enumerate() {
        let reconstructed = i16::from_le_bytes([encoded[i * 2], encoded[i * 2 + 1]]);
        assert_eq!(reconstructed, sample, "Sample mismatch at index {i}");
    }

    // Verify encode_bytes handles valid s16le byte slice directly
    let encoded_from_bytes = encoder.encode_bytes(&encoded).expect("Encode PCM bytes");
    assert_eq!(encoded_from_bytes, encoded);

    // Odd byte count must fail
    let odd_bytes = vec![0u8; 15];
    assert!(matches!(
        encoder.encode_bytes(&odd_bytes),
        Err(AudioError::InvalidPcmLength { expected: 2, actual: 15 })
    ));

    // Zero-length input
    let empty_res = encoder.encode(&[]).expect("Encode empty PCM");
    assert!(empty_res.is_empty());
}

// ============================================================================
// 3. MEMORY ZEROIZATION GUARANTEES
// ============================================================================

#[test]
fn test_audio_buffer_zeroize_trait_and_contents() {
    // 1. Static assertion: AudioBuffer implements Zeroize and ZeroizeOnDrop
    fn assert_zeroize_traits<T: Zeroize + ZeroizeOnDrop>() {}
    assert_zeroize_traits::<AudioBuffer>();

    // 2. Direct zeroize() execution zeroes all bytes
    let secret_pattern = vec![0xA5u8; 2048];
    let mut buffer = AudioBuffer::new(secret_pattern);
    let ptr = buffer.as_ptr();
    let original_len = buffer.len();
    assert_eq!(original_len, 2048);
    assert!(buffer.iter().all(|&b| b == 0xA5));

    // Invoke zeroize() directly
    buffer.zeroize();
    // In zeroize crate, Vec<Z>::zeroize() zeroes every element in as_mut_slice() and then calls clear()
    assert_eq!(buffer.len(), 0);
    assert!(buffer.is_empty());

    // Because the underlying Vec is cleared while still alive (not yet deallocated),
    // the memory at `ptr` for original_len is guaranteed to have been wiped with zeroes:
    let underlying_memory = unsafe { std::slice::from_raw_parts(ptr, original_len) };
    assert!(
        underlying_memory.iter().all(|&b| b == 0x00),
        "Underlying memory allocation must be strictly zeroed"
    );
}

#[test]
fn test_audio_buffer_into_inner_transfers_cleanly() {
    let original_data = vec![0x5Cu8; 512];
    let buffer = AudioBuffer::new(original_data.clone());

    // into_inner() takes ownership without zeroing the returned vector
    let extracted = buffer.into_inner();
    assert_eq!(extracted, original_data);
}

#[test]
fn test_audio_wire_frame_payload_zeroization() {
    let payload_bytes = vec![0x7Eu8; 1024];
    let mut frame = AudioWireFrame::create(
        AudioCodec::Opus,
        2,
        FLAG_NORMAL,
        48000,
        1,
        1700000000000,
        payload_bytes,
    );

    let ptr = frame.payload.as_ptr();
    let original_len = frame.payload.len();
    assert_eq!(original_len, 1024);
    assert!(frame.payload.iter().all(|&b| b == 0x7E));

    // Zeroize payload buffer
    frame.payload.zeroize();
    assert_eq!(frame.payload.len(), 0);
    let underlying_memory = unsafe { std::slice::from_raw_parts(ptr, original_len) };
    assert!(
        underlying_memory.iter().all(|&b| b == 0x00),
        "Frame payload memory must be completely wiped"
    );
}

// ============================================================================
// 4. STREAMING SESSION BACKPRESSURE & DISCONTINUITY
// ============================================================================

#[tokio::test]
async fn test_audio_session_saturation_and_discontinuity() {
    // Generate 100 frames instantly without pacing (mock capture)
    let total_frames = 100;
    let capture = Box::new(
        MockAudioCapture::new()
            .with_max_frames(total_frames)
            .with_pacing(false),
    );
    let encoder = Box::new(PcmPassthroughEncoder::default());
    let virtual_sink = Some(Box::new(MockVirtualSink::new()) as _);

    let mut session = AudioStreamSession::start(capture, encoder, virtual_sink)
        .await
        .expect("Start session");

    // Allow worker to saturate the 32-capacity channel and drop surplus frames
    tokio::time::sleep(Duration::from_millis(80)).await;

    let mut rx = session.take_receiver().expect("Receiver");
    let mut drained_frames = Vec::new();

    while let Ok(chunk) = rx.try_recv() {
        let (frame, _) = AudioWireFrame::decode(&chunk).expect("Decode chunk");
        drained_frames.push(frame);
    }

    // Verify channel bounded capacity was enforced
    assert!(
        !drained_frames.is_empty(),
        "Must have received buffered frames"
    );
    assert!(
        drained_frames.len() <= 32,
        "Channel buffer must not exceed capacity 32, got {}",
        drained_frames.len()
    );

    // Initial frame must be normal
    assert_eq!(drained_frames[0].header.flags, FLAG_NORMAL);

    session.stop().await;
    assert!(!session.is_active());
}

#[tokio::test]
async fn test_audio_session_discontinuity_flag_recovery() {
    // Generate 1000 frames so stream remains active during test
    let capture = Box::new(
        MockAudioCapture::new()
            .with_max_frames(1000)
            .with_pacing(false),
    );
    let encoder = Box::new(PcmPassthroughEncoder::default());

    let mut session = AudioStreamSession::start(capture, encoder, None)
        .await
        .expect("Start session");

    let mut rx = session.take_receiver().expect("Receiver");

    // Allow worker to fill channel (capacity 32) and drop several frames
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Drain initial 32 frames from the saturated channel
    let mut initial_frames = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        let (frame, _) = AudioWireFrame::decode(&chunk).expect("Decode chunk");
        initial_frames.push(frame);
    }
    assert_eq!(initial_frames.len(), 32);
    // All initial frames must have normal flags
    for frame in &initial_frames {
        assert_eq!(frame.header.flags, FLAG_NORMAL);
    }

    // Now await the next frame that enters the channel after the drops
    // This recovery frame MUST have FLAG_DISCONTINUITY set!
    let mut found_discontinuity = false;
    for _ in 0..20 {
        if let Some(chunk) = rx.recv().await {
            let (frame, _) = AudioWireFrame::decode(&chunk).expect("Decode chunk");
            if (frame.header.flags & FLAG_DISCONTINUITY) != 0 {
                found_discontinuity = true;
                break;
            }
        }
    }

    assert!(
        found_discontinuity,
        "A frame admitted after backpressure drops must carry FLAG_DISCONTINUITY"
    );

    session.stop().await;
}


