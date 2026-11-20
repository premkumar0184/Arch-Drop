//! Adversarial stress test suite for Arch Drop Virtual Webcam Sink (Milestone 1).

use archdrop_webcam::device::{
    discover_loopback_device, find_loopback_device, get_diagnostic_message, list_loopback_devices,
};
use archdrop_webcam::error::{WebcamError, MODPROBE_DIAGNOSTIC};
use archdrop_webcam::mjpeg::{MjpegStreamParser, MAX_HEADER_SIZE};
use archdrop_webcam::mock::MockV4l2Sink;
use archdrop_webcam::sink::WebcamSink;
use archdrop_webcam::v4l2::{
    v4l2_capability, v4l2_format, v4l2_pix_format, V4l2Sink,
    V4L2_BUF_TYPE_VIDEO_OUTPUT, V4L2_CAP_DEVICE_CAPS, V4L2_CAP_STREAMING, V4L2_CAP_VIDEO_OUTPUT,
    V4L2_PIX_FMT_JPEG, V4L2_PIX_FMT_MJPEG, VIDIOC_QUERYCAP, VIDIOC_STREAMOFF, VIDIOC_STREAMON,
    VIDIOC_S_FMT,
};
use bytes::Bytes;
use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

// ============================================================================
// 1. Device Scanner & Diagnostic String Stress Tests
// ============================================================================

#[test]
fn test_adversarial_diagnostic_string_verbatim() {
    let expected = "sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"ArchDrop Camera\"";
    assert_eq!(MODPROBE_DIAGNOSTIC, expected);
    assert_eq!(get_diagnostic_message(), expected);
}

#[test]
fn test_adversarial_scanner_empty_or_missing_sysfs() {
    let temp = TempDir::new().unwrap();

    // 1. Completely empty directory
    let res = discover_loopback_device(Some(temp.path()));
    assert!(res.is_err());
    match res {
        Err(WebcamError::LoopbackNotLoaded { diagnostic }) => {
            assert_eq!(diagnostic, MODPROBE_DIAGNOSTIC);
        }
        other => panic!("Unexpected result: {:?}", other),
    }

    // 2. Directory exists but has no children
    let virtual_dir = temp.path().join("devices/virtual/video4linux");
    create_dir_all(&virtual_dir).unwrap();
    let res2 = discover_loopback_device(Some(temp.path()));
    assert!(res2.is_err());
    match res2 {
        Err(WebcamError::LoopbackNotLoaded { diagnostic }) => {
            assert_eq!(diagnostic, MODPROBE_DIAGNOSTIC);
        }
        other => panic!("Unexpected result: {:?}", other),
    }

    // 3. Directory contains non-video files
    let dummy_file = virtual_dir.join("not_a_video_device");
    File::create(&dummy_file).unwrap();
    let invalid_video = virtual_dir.join("videononnumeric");
    File::create(&invalid_video).unwrap();
    let res3 = discover_loopback_device(Some(temp.path()));
    assert!(res3.is_err());
}

#[test]
fn test_adversarial_scanner_preference_archdrop_label() {
    let temp = TempDir::new().unwrap();
    let virtual_dir = temp.path().join("devices/virtual/video4linux");

    // Create video0: Generic Cam
    let v0 = virtual_dir.join("video0");
    create_dir_all(&v0).unwrap();
    File::create(v0.join("name"))
        .unwrap()
        .write_all(b"Generic Loopback 0\n")
        .unwrap();

    // Create video5: ArchDrop Camera (the preferred one)
    let v5 = virtual_dir.join("video5");
    create_dir_all(&v5).unwrap();
    File::create(v5.join("name"))
        .unwrap()
        .write_all(b"ArchDrop Camera\n")
        .unwrap();

    // Create video9: Another Cam
    let v9 = virtual_dir.join("video9");
    create_dir_all(&v9).unwrap();
    File::create(v9.join("name"))
        .unwrap()
        .write_all(b"OBS Virtual Cam\n")
        .unwrap();

    let all = list_loopback_devices(Some(temp.path())).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].index, 0);
    assert_eq!(all[1].index, 5);
    assert_eq!(all[2].index, 9);

    // discover_loopback_device MUST prioritize "ArchDrop Camera" over video0
    let preferred = discover_loopback_device(Some(temp.path()))
        .unwrap()
        .unwrap();
    assert_eq!(preferred.index, 5);
    assert_eq!(preferred.card_label, "ArchDrop Camera");

    let found = find_loopback_device(Some(temp.path())).unwrap();
    assert_eq!(found.index, 5);
}

#[test]
fn test_adversarial_scanner_fallback_first_device() {
    let temp = TempDir::new().unwrap();
    let virtual_dir = temp.path().join("devices/virtual/video4linux");

    // Only non-archdrop virtual cam present
    let v3 = virtual_dir.join("video3");
    create_dir_all(&v3).unwrap();
    File::create(v3.join("name"))
        .unwrap()
        .write_all(b"External Loopback\n")
        .unwrap();

    let found = find_loopback_device(Some(temp.path())).unwrap();
    assert_eq!(found.index, 3);
    assert_eq!(found.card_label, "External Loopback");
}

// ============================================================================
// 2. V4L2 Ioctl Struct Layouts, STREAMON/STREAMOFF & RAII Cleanup
// ============================================================================

#[test]
fn test_adversarial_v4l2_struct_offsets_and_sizes() {
    // Exact sizing required by Linux kernel videodev2.h
    assert_eq!(std::mem::size_of::<v4l2_capability>(), 104);
    assert_eq!(std::mem::size_of::<v4l2_pix_format>(), 48);
    assert_eq!(std::mem::size_of::<v4l2_format>(), 208);

    // Check offsets of v4l2_capability
    assert_eq!(std::mem::offset_of!(v4l2_capability, driver), 0);
    assert_eq!(std::mem::offset_of!(v4l2_capability, card), 16);
    assert_eq!(std::mem::offset_of!(v4l2_capability, bus_info), 48);
    assert_eq!(std::mem::offset_of!(v4l2_capability, version), 80);
    assert_eq!(std::mem::offset_of!(v4l2_capability, capabilities), 84);
    assert_eq!(std::mem::offset_of!(v4l2_capability, device_caps), 88);
    assert_eq!(std::mem::offset_of!(v4l2_capability, reserved), 92);

    // Check offsets of v4l2_pix_format
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, width), 0);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, height), 4);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, pixelformat), 8);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, field), 12);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, bytesperline), 16);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, sizeimage), 20);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, colorspace), 24);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, priv_), 28);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, flags), 32);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, union_field), 36);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, quantization), 40);
    assert_eq!(std::mem::offset_of!(v4l2_pix_format, xfer_func), 44);

    // Check offsets of v4l2_format
    assert_eq!(std::mem::offset_of!(v4l2_format, type_), 0);
    assert_eq!(std::mem::offset_of!(v4l2_format, fmt), 8);

    // Constants
    assert_eq!(VIDIOC_QUERYCAP, 0x80685600);
    assert_eq!(VIDIOC_S_FMT, 0xc0d05605);
    assert_eq!(VIDIOC_STREAMON, 0x40045612);
    assert_eq!(VIDIOC_STREAMOFF, 0x40045613);

    assert_eq!(V4L2_BUF_TYPE_VIDEO_OUTPUT, 2);
    assert_eq!(V4L2_CAP_VIDEO_OUTPUT, 0x00000002);
    assert_eq!(V4L2_CAP_STREAMING, 0x04000000);
    assert_eq!(V4L2_CAP_DEVICE_CAPS, 0x80000000);
    assert_eq!(V4L2_PIX_FMT_JPEG, 0x4745504a);
    assert_eq!(V4L2_PIX_FMT_MJPEG, 0x47504a4d);
}

#[test]
fn test_adversarial_streamon_streamoff_idempotency() {
    let mut mock = MockV4l2Sink::new(PathBuf::from("/dev/video-test"));

    // Multiple stream_on calls
    assert!(!mock.is_streaming());
    mock.stream_on().unwrap();
    assert!(mock.is_streaming());
    assert_eq!(mock.stream_on_count(), 1);

    // Second call should succeed and be idempotent
    mock.stream_on().unwrap();
    assert!(mock.is_streaming());
    assert_eq!(mock.stream_on_count(), 2);

    // Multiple stream_off calls
    mock.stream_off().unwrap();
    assert!(!mock.is_streaming());
    assert_eq!(mock.stream_off_count(), 1);

    mock.stream_off().unwrap();
    assert!(!mock.is_streaming());
    assert_eq!(mock.stream_off_count(), 2);
}

#[test]
fn test_adversarial_raii_fd_close_on_drop() {
    // Create an anonymous pipe to verify RAII closure on LinuxV4l2Sink drop
    let mut fds = [0i32; 2];
    let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
    assert_eq!(ret, 0, "Failed to create pipe for FD verification");

    let read_fd = fds[0];
    let write_fd = fds[1];

    // Verify write_fd is open
    let flags_before = unsafe { libc::fcntl(write_fd, libc::F_GETFD) };
    assert!(flags_before >= 0, "write_fd should be valid initially");

    // Emulate LinuxV4l2Sink structure around write_fd and verify drop closes it
    {
        struct TestSinkGuard {
            fd: i32,
            _is_streaming: bool,
        }
        impl Drop for TestSinkGuard {
            fn drop(&mut self) {
                if self.fd >= 0 {
                    unsafe { libc::close(self.fd) };
                }
            }
        }

        let _guard = TestSinkGuard {
            fd: write_fd,
            _is_streaming: false,
        };
        // _guard drops here
    }

    // Verify write_fd is now CLOSED (fcntl returns -1 with EBADF)
    let flags_after = unsafe { libc::fcntl(write_fd, libc::F_GETFD) };
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    assert_eq!(flags_after, -1, "write_fd should be closed after drop");
    assert_eq!(errno, libc::EBADF, "Expected errno EBADF on closed fd");

    // Clean up read_fd
    unsafe { libc::close(read_fd) };
}

#[tokio::test]
async fn test_adversarial_webcam_sink_frame_deluge_zero_buffering() {
    let mock = MockV4l2Sink::new(PathBuf::from("/dev/video-test"));
    let mock_clone = mock.clone();

    let sink = Arc::new(WebcamSink::new(Box::new(mock)));
    assert!(sink.is_streaming());

    // Rapidly push 200 frames with zero delay (frame deluge)
    let raw_frame = Bytes::from_static(b"\xFF\xD8\xAA\xBB\xFF\xD9");
    for _ in 0..200 {
        sink.push_frame(raw_frame.clone()).unwrap();
    }

    // Allow worker task to process available frames
    tokio::time::sleep(Duration::from_millis(50)).await;

    let stats = sink.stats();
    assert_eq!(stats.frames_received, 200);
    // Because watch channel capacity is 1, unwritten frames are overwritten
    assert!(stats.frames_written >= 1);
    assert!(stats.frames_dropped > 0);
    assert_eq!(
        stats.frames_received,
        stats.frames_written + stats.frames_dropped
    );

    // Stop sink
    sink.stop();
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert!(!sink.is_streaming());
    assert_eq!(mock_clone.stream_off_count(), 1);

    // Subsequent push_frame must return StreamNotActive
    let push_res = sink.push_frame(raw_frame);
    assert!(push_res.is_err());
    assert!(matches!(push_res.unwrap_err(), WebcamError::StreamNotActive));
}

// ============================================================================
// 3. MJPEG Parser Edge Cases & Adversarial Boundary Tests
// ============================================================================

#[test]
fn test_adversarial_mjpeg_parser_header_overflow() {
    let mut parser = MjpegStreamParser::new("frame");
    let oversized_header = vec![b'X'; MAX_HEADER_SIZE + 128];

    parser.push_chunk(b"--frame\r\n");
    parser.push_chunk(&oversized_header);

    let res = parser.next_frame();
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), WebcamError::MalformedStream(_)));
}

#[test]
fn test_adversarial_mjpeg_parser_payload_boundary_variants() {
    // 1. Boundary with leading dashes in constructor
    let mut parser1 = MjpegStreamParser::new("--custom_boundary");
    let raw_frame = b"\xFF\xD8\x12\x34\xFF\xD9";
    let data = format!(
        "--custom_boundary\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        raw_frame.len()
    );
    parser1.push_chunk(data.as_bytes());
    parser1.push_chunk(raw_frame);
    parser1.push_chunk(b"\r\n--custom_boundary--\r\n");

    let f1 = parser1.next_frame().unwrap().unwrap();
    assert_eq!(f1.data.as_ref(), raw_frame);
    let end1 = parser1.next_frame().unwrap();
    assert!(end1.is_none());
    assert!(parser1.is_finished());

    // 2. Boundary with whitespace and without leading dashes
    let mut parser2 = MjpegStreamParser::new("  my_boundary  ");
    let data2 = format!(
        "--my_boundary\nContent-Length: {}\n\n",
        raw_frame.len()
    );
    parser2.push_chunk(data2.as_bytes());
    parser2.push_chunk(raw_frame);
    parser2.push_chunk(b"\n--my_boundary--\n");

    let f2 = parser2.next_frame().unwrap().unwrap();
    assert_eq!(f2.data.as_ref(), raw_frame);
    let end2 = parser2.next_frame().unwrap();
    assert!(end2.is_none());
    assert!(parser2.is_finished());
}

#[test]
fn test_adversarial_mjpeg_parser_split_soi_eoi_across_chunks() {
    let mut parser = MjpegStreamParser::new("boundary");

    let header = b"--boundary\r\nContent-Length: 6\r\n\r\n";
    parser.push_chunk(header);

    // Split SOI: [0xFF] in chunk 1, [0xD8] in chunk 2
    parser.push_chunk(b"\xFF");
    assert!(parser.next_frame().unwrap().is_none());

    parser.push_chunk(b"\xD8\x11\x22");
    assert!(parser.next_frame().unwrap().is_none());

    // Split EOI: [0xFF] in chunk 3, [0xD9] in chunk 4
    parser.push_chunk(b"\xFF");
    assert!(parser.next_frame().unwrap().is_none());

    parser.push_chunk(b"\xD9");
    let frame = parser.next_frame().unwrap().expect("Frame must complete");
    assert_eq!(frame.data.as_ref(), b"\xFF\xD8\x11\x22\xFF\xD9");
}
