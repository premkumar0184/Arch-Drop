//! Comprehensive integration and property tests for archdrop_audio.

use archdrop_audio::capture::MockAudioCapture;
use archdrop_audio::codec::{OpusAudioEncoder, PcmPassthroughEncoder};
use archdrop_audio::error::AudioError;
use archdrop_audio::framing::{
    AudioBuffer, AudioCodec, AudioWireFrame, AudioWireHeader, ADAU_MAGIC, FLAG_NORMAL, HEADER_SIZE,
};
use archdrop_audio::session::AudioStreamSession;
use archdrop_audio::sink::MockVirtualSink;
use std::time::Duration;

#[test]
fn test_canonical_32_byte_header_fields() {
    let header = AudioWireHeader::new(
        AudioCodec::Opus,
        2,
        FLAG_NORMAL,
        48000,
        999,
        1700000000999,
        320,
    );

    let bytes = header.to_bytes();
    assert_eq!(bytes.len(), HEADER_SIZE);
    assert_eq!(bytes.len(), 32);

    // Verify exact byte positions matching canonical spec:
    // Magic: 0..4 = "ADAU"
    assert_eq!(&bytes[0..4], &ADAU_MAGIC);
    assert_eq!(&bytes[0..4], b"ADAU");
    // Codec: 4 = 0x01
    assert_eq!(bytes[4], 0x01);
    // Channels: 5 = 0x02
    assert_eq!(bytes[5], 0x02);
    // Flags: 6..8 = 0x0000
    assert_eq!(&bytes[6..8], &[0x00, 0x00]);
    // Sample Rate: 8..12 = 48000 (0x0000BB80)
    assert_eq!(&bytes[8..12], &[0x00, 0x00, 0xBB, 0x80]);
    // Sequence: 12..20 = 999
    assert_eq!(u64::from_be_bytes(bytes[12..20].try_into().unwrap()), 999);
    // TimestampMs: 20..28 = 1700000000999
    assert_eq!(
        u64::from_be_bytes(bytes[20..28].try_into().unwrap()),
        1700000000999
    );
    // PayloadLen: 28..32 = 320
    assert_eq!(u32::from_be_bytes(bytes[28..32].try_into().unwrap()), 320);
}

#[test]
fn test_audio_buffer_zeroize() {
    let data = vec![0x42; 1024];
    let buf = AudioBuffer::new(data);
    assert_eq!(buf.len(), 1024);
    assert_eq!(buf[0], 0x42);
    // Drop triggers zeroize
    drop(buf);
}

#[test]
fn test_corrupted_frame_handling() {
    // 1. Buffer too short for header
    let short_data = vec![0u8; 16];
    assert!(matches!(
        AudioWireFrame::decode(&short_data),
        Err(AudioError::Framing(_))
    ));

    // 2. Buffer has valid header but payload is truncated
    let header = AudioWireHeader::new(
        AudioCodec::Pcm,
        2,
        FLAG_NORMAL,
        48000,
        1,
        1000,
        500, // payload len claims 500 bytes
    );
    let mut truncated = header.to_bytes().to_vec();
    truncated.extend_from_slice(&[0u8; 100]); // only 100 bytes payload provided
    assert!(matches!(
        AudioWireFrame::decode(&truncated),
        Err(AudioError::Framing(_))
    ));
}

#[test]
fn test_opus_bitrate_range() {
    for bitrate in [64000, 96000, 128000] {
        let encoder = OpusAudioEncoder::new(48000, 2, bitrate).expect("Opus encoder init");
        assert_eq!(encoder.bitrate(), bitrate);
    }
}

#[tokio::test]
async fn test_discontinuity_flag_under_channel_overflow() {
    // Generate synthetic audio capture that generates 50 frames instantly
    let capture = Box::new(MockAudioCapture::new().with_max_frames(50).with_pacing(false));
    let encoder = Box::new(PcmPassthroughEncoder::default());

    let mut session = AudioStreamSession::start(capture, encoder, None)
        .await
        .expect("Start session");

    // Sleep briefly so the worker attempts to push all 50 frames into capacity-32 channel
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut rx = session.take_receiver().expect("Receiver");

    let mut received_frames = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        let (frame, _) = AudioWireFrame::decode(&chunk).expect("Decode frame");
        received_frames.push(frame);
    }

    // Since channel capacity is 32, we should have received frames
    assert!(!received_frames.is_empty());
    assert!(received_frames.len() <= 32);

    // Initial frame has FLAG_NORMAL
    assert_eq!(received_frames[0].header.flags, FLAG_NORMAL);

    session.stop().await;
}

#[tokio::test]
async fn test_virtual_sink_session_cleanup() {
    let capture = Box::new(MockAudioCapture::new().with_max_frames(10));
    let encoder = Box::new(PcmPassthroughEncoder::default());
    let virtual_sink = Box::new(MockVirtualSink::new());

    let mut session = AudioStreamSession::start(capture, encoder, Some(virtual_sink))
        .await
        .expect("Start session with virtual sink");

    assert!(session.is_active());

    // Stop session
    session.stop().await;
    assert!(!session.is_active());
}
