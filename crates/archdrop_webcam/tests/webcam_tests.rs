use archdrop_webcam::device::discover_loopback_device;
use archdrop_webcam::error::{WebcamError, MODPROBE_DIAGNOSTIC};
use archdrop_webcam::mjpeg::{MjpegStreamParser, MAX_FRAME_SIZE};
use archdrop_webcam::mock::MockV4l2Sink;
use archdrop_webcam::sink::WebcamSink;
use archdrop_webcam::v4l2::{
    v4l2_capability, v4l2_format, v4l2_pix_format, V4l2Sink, V4L2_BUF_TYPE_VIDEO_OUTPUT,
    V4L2_PIX_FMT_JPEG, V4L2_PIX_FMT_MJPEG, VIDIOC_QUERYCAP, VIDIOC_STREAMOFF, VIDIOC_STREAMON,
    VIDIOC_S_FMT,
};
use bytes::Bytes;
use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn test_v4l2_struct_sizes_and_constants() {
    assert_eq!(std::mem::size_of::<v4l2_capability>(), 104);
    assert_eq!(std::mem::size_of::<v4l2_pix_format>(), 48);
    assert_eq!(std::mem::size_of::<v4l2_format>(), 208);

    assert_eq!(VIDIOC_QUERYCAP, 0x80685600);
    assert_eq!(VIDIOC_S_FMT, 0xc0d05605);
    assert_eq!(VIDIOC_STREAMON, 0x40045612);
    assert_eq!(VIDIOC_STREAMOFF, 0x40045613);

    assert_eq!(V4L2_BUF_TYPE_VIDEO_OUTPUT, 2);
    assert_eq!(V4L2_PIX_FMT_JPEG, 0x4745504a);
    assert_eq!(V4L2_PIX_FMT_MJPEG, 0x47504a4d);
}

#[test]
fn test_mjpeg_parser_standard_frame() {
    let mut parser = MjpegStreamParser::new("frame");

    let raw_frame = b"\xFF\xD8\x00\x00\xFF\xD9";
    let stream_data = format!(
        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nX-Timestamp-Ms: 1731816810000\r\n\r\n",
        raw_frame.len()
    );

    parser.push_chunk(stream_data.as_bytes());
    parser.push_chunk(raw_frame);
    parser.push_chunk(b"\r\n--frame--\r\n");

    let frame = parser.next_frame().expect("Parsing should succeed").expect("Should yield frame");
    assert_eq!(frame.data.as_ref(), raw_frame);
    assert_eq!(frame.timestamp_ms, Some(1731816810000));
    assert_eq!(frame.content_type.as_deref(), Some("image/jpeg"));

    let end = parser.next_frame().expect("Parsing end");
    assert!(end.is_none());
    assert!(parser.is_finished());
}

#[test]
fn test_mjpeg_parser_single_byte_fragmentation() {
    let mut parser = MjpegStreamParser::new("frame");

    let raw_frame = b"\xFF\xD8\xAA\xBB\xCC\xDD\xFF\xD9";
    let stream_data = format!(
        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        raw_frame.len()
    );

    let mut full_payload = stream_data.into_bytes();
    full_payload.extend_from_slice(raw_frame);
    full_payload.extend_from_slice(b"\r\n--frame--\r\n");

    let mut extracted_frame = None;
    for b in full_payload {
        parser.push_chunk(&[b]);
        if let Ok(Some(f)) = parser.next_frame() {
            extracted_frame = Some(f);
        }
    }
    let _ = parser.next_frame();

    assert!(extracted_frame.is_some());
    assert_eq!(extracted_frame.unwrap().data.as_ref(), raw_frame);
    assert!(parser.is_finished());
}

#[test]
fn test_mjpeg_parser_unix_lf_delimiters() {
    let mut parser = MjpegStreamParser::new("frame");

    let raw_frame = b"\xFF\xD8\x01\x02\xFF\xD9";
    let stream_data = format!(
        "--frame\nContent-Type: image/jpeg\nContent-Length: {}\n\n",
        raw_frame.len()
    );

    parser.push_chunk(stream_data.as_bytes());
    parser.push_chunk(raw_frame);
    parser.push_chunk(b"\n--frame--\n");

    let frame = parser.next_frame().expect("Parsing should succeed").expect("Should yield frame");
    assert_eq!(frame.data.as_ref(), raw_frame);
}

#[test]
fn test_mjpeg_parser_missing_content_length() {
    let mut parser = MjpegStreamParser::new("boundary123");

    let raw_frame = b"\xFF\xD8\x42\x43\xFF\xD9";
    // Omit Content-Length
    let stream_data = b"--boundary123\r\nContent-Type: image/jpeg\r\n\r\n";

    parser.push_chunk(stream_data);
    parser.push_chunk(raw_frame);
    parser.push_chunk(b"\r\n--boundary123--\r\n");

    let frame = parser.next_frame().expect("Parsing should succeed").expect("Should yield frame");
    assert_eq!(frame.data.as_ref(), raw_frame);
}

#[test]
fn test_mjpeg_parser_corrupted_soi() {
    let mut parser = MjpegStreamParser::new("frame");
    let corrupt_frame = b"\x00\x00\x00\x00\xFF\xD9"; // Missing 0xFF 0xD8
    let stream_data = format!(
        "--frame\r\nContent-Length: {}\r\n\r\n",
        corrupt_frame.len()
    );

    parser.push_chunk(stream_data.as_bytes());
    parser.push_chunk(corrupt_frame);

    let res = parser.next_frame();
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), WebcamError::MalformedJpeg(_)));
}

#[test]
fn test_mjpeg_parser_corrupted_eoi() {
    let mut parser = MjpegStreamParser::new("frame");
    let corrupt_frame = b"\xFF\xD8\x00\x00\x00\x00"; // Missing 0xFF 0xD9
    let stream_data = format!(
        "--frame\r\nContent-Length: {}\r\n\r\n",
        corrupt_frame.len()
    );

    parser.push_chunk(stream_data.as_bytes());
    parser.push_chunk(corrupt_frame);

    let res = parser.next_frame();
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), WebcamError::MalformedJpeg(_)));
}

#[test]
fn test_mjpeg_parser_frame_too_large() {
    let mut parser = MjpegStreamParser::new("frame");
    let giant_size = MAX_FRAME_SIZE + 1024;
    let stream_data = format!(
        "--frame\r\nContent-Length: {}\r\n\r\n",
        giant_size
    );

    parser.push_chunk(stream_data.as_bytes());
    let res = parser.next_frame();
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), WebcamError::FrameTooLarge { .. }));
}

#[test]
fn test_device_discovery_not_loaded_diagnostic() {
    let temp = TempDir::new().unwrap();
    let res = discover_loopback_device(Some(temp.path()));
    assert!(res.is_err());

    if let Err(WebcamError::LoopbackNotLoaded { diagnostic }) = res {
        assert_eq!(diagnostic, MODPROBE_DIAGNOSTIC);
        assert!(diagnostic.contains("v4l2loopback"));
        assert!(diagnostic.contains("exclusive_caps=1"));
        assert!(diagnostic.contains("ArchDrop Camera"));
    } else {
        panic!("Expected LoopbackNotLoaded error");
    }
}

#[test]
fn test_device_discovery_mock_sysfs() {
    let temp = TempDir::new().unwrap();
    let virtual_video2 = temp.path().join("devices/virtual/video4linux/video2");
    create_dir_all(&virtual_video2).unwrap();

    let mut name_file = File::create(virtual_video2.join("name")).unwrap();
    name_file.write_all(b"ArchDrop Camera\n").unwrap();

    let dev_dir = temp.path().join("dev");
    create_dir_all(&dev_dir).unwrap();
    let _ = File::create(dev_dir.join("video2")).unwrap();

    let dev_opt = discover_loopback_device(Some(temp.path())).unwrap();
    assert!(dev_opt.is_some());
    let dev = dev_opt.unwrap();
    assert_eq!(dev.index, 2);
    assert_eq!(dev.card_label, "ArchDrop Camera");
    assert!(dev.device_path.to_string_lossy().contains("video2"));
}

#[test]
fn test_mock_v4l2_sink_operations() {
    let mut mock = MockV4l2Sink::new(PathBuf::from("/dev/video-test"));

    assert!(!mock.is_streaming());
    mock.set_format(1280, 720, V4L2_PIX_FMT_MJPEG).unwrap();
    assert_eq!(mock.configured_format(), Some((1280, 720, V4L2_PIX_FMT_MJPEG)));

    mock.stream_on().unwrap();
    assert!(mock.is_streaming());
    assert_eq!(mock.stream_on_count(), 1);

    let dummy_frame = vec![0xFF, 0xD8, 0x11, 0x22, 0xFF, 0xD9];
    mock.write_frame(&dummy_frame).unwrap();
    assert_eq!(mock.frame_count(), 1);
    assert_eq!(mock.written_frames()[0], dummy_frame);

    mock.stream_off().unwrap();
    assert!(!mock.is_streaming());
    assert_eq!(mock.stream_off_count(), 1);
}

#[tokio::test]
async fn test_webcam_sink_actor_handoff_and_depth_1() {
    let mock = MockV4l2Sink::new(PathBuf::from("/dev/video-test"));
    let mock_clone = mock.clone();

    let sink = WebcamSink::new(Box::new(mock));
    assert!(sink.is_streaming());

    let frame1 = Bytes::from_static(b"\xFF\xD8\x01\xFF\xD9");
    let frame2 = Bytes::from_static(b"\xFF\xD8\x02\xFF\xD9");

    sink.push_frame(frame1).unwrap();
    sink.push_frame(frame2).unwrap();

    // Give writer task a few milliseconds to process
    tokio::time::sleep(Duration::from_millis(50)).await;

    let stats = sink.stats();
    assert_eq!(stats.frames_received, 2);
    assert!(stats.frames_written >= 1);

    sink.stop();
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert!(!sink.is_streaming());
    assert_eq!(mock_clone.stream_off_count(), 1);
}
