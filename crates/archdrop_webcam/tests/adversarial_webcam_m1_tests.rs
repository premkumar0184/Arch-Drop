use archdrop_webcam::error::WebcamError;
use archdrop_webcam::mjpeg::{MjpegStreamParser, MAX_FRAME_SIZE, MAX_HEADER_SIZE};

#[test]
fn test_adversarial_single_byte_streaming_all_boundaries() {
    let mut parser = MjpegStreamParser::new("test_bound");
    let frame1_data = b"\xFF\xD8\x01\x02\x03\x04\xFF\xD9";
    let frame2_data = b"\xFF\xD8\xDE\xAD\xBE\xEF\xFF\xD9";

    // Build raw bytes cleanly
    let mut raw_stream = Vec::new();
    raw_stream.extend_from_slice(
        format!(
            "--test_bound\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nX-Timestamp-Ms: 123456789\r\n\r\n",
            frame1_data.len()
        )
        .as_bytes(),
    );
    raw_stream.extend_from_slice(frame1_data);
    raw_stream.extend_from_slice(
        format!(
            "\r\n--test_bound\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nX-Timestamp-Ms: 123456799\r\n\r\n",
            frame2_data.len()
        )
        .as_bytes(),
    );
    raw_stream.extend_from_slice(frame2_data);
    raw_stream.extend_from_slice(b"\r\n--test_bound--\r\n");

    let mut frames = Vec::new();
    for &byte in &raw_stream {
        parser.push_chunk(&[byte]);
        while let Ok(Some(f)) = parser.next_frame() {
            frames.push(f);
        }
    }

    assert_eq!(frames.len(), 2, "Must extract exactly 2 frames from byte-by-byte feed");
    assert_eq!(frames[0].data.as_ref(), frame1_data);
    assert_eq!(frames[0].timestamp_ms, Some(123456789));
    assert_eq!(frames[1].data.as_ref(), frame2_data);
    assert_eq!(frames[1].timestamp_ms, Some(123456799));
    assert!(parser.is_finished());
}

#[test]
fn test_adversarial_chunk_split_across_markers() {
    let mut parser = MjpegStreamParser::new("boundary");
    let frame = b"\xFF\xD8\xCA\xFE\xBA\xBE\xFF\xD9";

    // Split stream right across boundary marker "--bound" | "ary\r\n"
    parser.push_chunk(b"--bound");
    assert_eq!(parser.next_frame().unwrap(), None);

    parser.push_chunk(b"ary\r\n");
    assert_eq!(parser.next_frame().unwrap(), None);

    // Split across headers and \r\n\r\n
    parser.push_chunk(b"Content-Length: ");
    assert_eq!(parser.next_frame().unwrap(), None);

    parser.push_chunk(format!("{}", frame.len()).as_bytes());
    parser.push_chunk(b"\r");
    assert_eq!(parser.next_frame().unwrap(), None);

    parser.push_chunk(b"\n\r\n");
    assert_eq!(parser.next_frame().unwrap(), None);

    // Split right between 0xFF and 0xD8 (SOI)
    parser.push_chunk(&[0xFF]);
    assert_eq!(parser.next_frame().unwrap(), None);

    parser.push_chunk(&[0xD8, 0xCA, 0xFE, 0xBA, 0xBE, 0xFF]);
    assert_eq!(parser.next_frame().unwrap(), None);

    // Final byte of EOI 0xD9
    parser.push_chunk(&[0xD9]);
    let parsed = parser.next_frame().expect("Parsing should succeed").expect("Must yield frame");
    assert_eq!(parsed.data.as_ref(), frame);

    // Terminating boundary split across chunks
    parser.push_chunk(b"\r\n--bound");
    assert_eq!(parser.next_frame().unwrap(), None);
    parser.push_chunk(b"ary--");
    let end = parser.next_frame().unwrap();
    assert_eq!(end, None);
    assert!(parser.is_finished());
}

#[test]
fn test_adversarial_corrupted_soi_and_eoi_variations() {
    // 1. Missing SOI
    let mut parser1 = MjpegStreamParser::new("frame");
    let bad_soi = b"\x00\x00\x11\x22\xFF\xD9";
    parser1.push_chunk(format!("--frame\r\nContent-Length: {}\r\n\r\n", bad_soi.len()).as_bytes());
    parser1.push_chunk(bad_soi);
    let err1 = parser1.next_frame().unwrap_err();
    assert!(matches!(err1, WebcamError::MalformedJpeg(_)));

    // 2. Missing EOI
    let mut parser2 = MjpegStreamParser::new("frame");
    let bad_eoi = b"\xFF\xD8\x11\x22\x00\x00";
    parser2.push_chunk(format!("--frame\r\nContent-Length: {}\r\n\r\n", bad_eoi.len()).as_bytes());
    parser2.push_chunk(bad_eoi);
    let err2 = parser2.next_frame().unwrap_err();
    assert!(matches!(err2, WebcamError::MalformedJpeg(_)));

    // 3. Payload too small (< 4 bytes)
    let mut parser3 = MjpegStreamParser::new("frame");
    let tiny = b"\xFF\xD8\xD9";
    parser3.push_chunk(format!("--frame\r\nContent-Length: {}\r\n\r\n", tiny.len()).as_bytes());
    parser3.push_chunk(tiny);
    let err3 = parser3.next_frame().unwrap_err();
    assert!(matches!(err3, WebcamError::MalformedJpeg(_)));

    // 4. Valid JPEG with trailing padding within 16-byte EOI search window
    let mut parser4 = MjpegStreamParser::new("frame");
    let mut padded = vec![0xFF, 0xD8, 0x01, 0x02, 0x03, 0x04, 0xFF, 0xD9];
    padded.extend_from_slice(&[0x00, 0x00, 0x00]); // 3 bytes padding
    parser4.push_chunk(format!("--frame\r\nContent-Length: {}\r\n\r\n", padded.len()).as_bytes());
    parser4.push_chunk(&padded);
    let ok_frame = parser4.next_frame().unwrap().expect("Should succeed with padded EOI");
    assert_eq!(ok_frame.data.len(), padded.len());
}

#[test]
fn test_adversarial_payload_exceeding_10mb() {
    // 1. Content-Length header declaring > 10MB
    let mut parser1 = MjpegStreamParser::new("frame");
    let oversized = MAX_FRAME_SIZE + 1;
    parser1.push_chunk(format!("--frame\r\nContent-Length: {oversized}\r\n\r\n").as_bytes());
    let err1 = parser1.next_frame().unwrap_err();
    assert!(matches!(err1, WebcamError::FrameTooLarge { size, max } if size == oversized && max == MAX_FRAME_SIZE));

    // 2. Scanning mode (no Content-Length): stream pumps > 10MB without boundary
    let mut parser2 = MjpegStreamParser::new("frame");
    parser2.push_chunk(b"--frame\r\nContent-Type: image/jpeg\r\n\r\n");
    // Feed in 1 MB chunks without boundary until > MAX_FRAME_SIZE
    let one_mb = vec![0x42u8; 1024 * 1024];
    let mut hit_error = false;
    for _ in 0..11 {
        parser2.push_chunk(&one_mb);
        if let Err(WebcamError::FrameTooLarge { .. }) = parser2.next_frame() {
            hit_error = true;
            break;
        }
    }
    assert!(hit_error, "Must reject streaming frame exceeding 10 MB without boundary");
}

#[test]
fn test_adversarial_headers_exceeding_16kb() {
    let mut parser = MjpegStreamParser::new("frame");
    parser.push_chunk(b"--frame\r\n");
    // Send 17 KB of dummy headers without \r\n\r\n terminator
    let giant_header = vec![b'A'; MAX_HEADER_SIZE + 1024];
    parser.push_chunk(&giant_header);
    let res = parser.next_frame();
    assert!(res.is_err(), "Must reject headers exceeding 16 KB");
    assert!(matches!(res.unwrap_err(), WebcamError::MalformedStream(_)));
}

#[test]
fn test_adversarial_delimiter_matrix_crlf_lf_mixed() {
    // Pure UNIX LF with no Content-Length
    let mut parser1 = MjpegStreamParser::new("frame");
    let frame1 = b"\xFF\xD8\xAA\xBB\xFF\xD9";
    parser1.push_chunk(b"--frame\nContent-Type: image/jpeg\n\n");
    parser1.push_chunk(frame1);
    parser1.push_chunk(b"\n--frame--\n");

    let f1 = parser1.next_frame().unwrap().expect("UNIX LF without Content-Length must succeed");
    assert_eq!(f1.data.as_ref(), frame1);
    let end1 = parser1.next_frame().unwrap();
    assert_eq!(end1, None);
    assert!(parser1.is_finished());

    // Mixed CRLF / LF
    let mut parser2 = MjpegStreamParser::new("delim");
    let frame2 = b"\xFF\xD8\x00\xFF\xD9";
    // \r\n after boundary, \n after headers, \r\n\r\n terminator
    parser2.push_chunk(b"--delim\r\nContent-Type: image/jpeg\nX-Custom: test\r\n\r\n");
    parser2.push_chunk(frame2);
    parser2.push_chunk(b"\r\n--delim--\n");

    let f2 = parser2.next_frame().unwrap().expect("Mixed CRLF/LF must parse");
    assert_eq!(f2.data.as_ref(), frame2);
    let end2 = parser2.next_frame().unwrap();
    assert_eq!(end2, None);
    assert!(parser2.is_finished());
}

#[test]
fn test_adversarial_garbage_preamble_and_recovery() {
    let mut parser = MjpegStreamParser::new("myboundary");
    let frame = b"\xFF\xD8\x12\x34\xFF\xD9";

    // 64 KB of garbage bytes before the boundary
    let garbage = vec![0xEEu8; 64 * 1024];
    parser.push_chunk(&garbage);
    assert_eq!(parser.next_frame().unwrap(), None);

    // Now send legitimate frame
    parser.push_chunk(format!("--myboundary\r\nContent-Length: {}\r\n\r\n", frame.len()).as_bytes());
    parser.push_chunk(frame);
    parser.push_chunk(b"\r\n--myboundary--\r\n");

    let extracted = parser.next_frame().unwrap().expect("Must recover after garbage preamble");
    assert_eq!(extracted.data.as_ref(), frame);
    let end = parser.next_frame().unwrap();
    assert_eq!(end, None);
    assert!(parser.is_finished());
}

#[test]
fn test_adversarial_multiple_frames_in_single_chunk() {
    let mut parser = MjpegStreamParser::new("burst");
    let mut chunk = Vec::new();

    let expected_frames = vec![
        vec![0xFF, 0xD8, 0x01, 0xFF, 0xD9],
        vec![0xFF, 0xD8, 0x02, 0xFF, 0xD9],
        vec![0xFF, 0xD8, 0x03, 0xFF, 0xD9],
        vec![0xFF, 0xD8, 0x04, 0xFF, 0xD9],
    ];

    for f in &expected_frames {
        chunk.extend_from_slice(
            format!("--burst\r\nContent-Length: {}\r\n\r\n", f.len()).as_bytes(),
        );
        chunk.extend_from_slice(f);
        chunk.extend_from_slice(b"\r\n");
    }
    chunk.extend_from_slice(b"--burst--\r\n");

    parser.push_chunk(&chunk);

    let mut parsed = Vec::new();
    while let Ok(Some(frame)) = parser.next_frame() {
        parsed.push(frame.data.to_vec());
    }

    assert_eq!(parsed.len(), 4, "Must extract all 4 burst frames from single chunk");
    assert_eq!(parsed, expected_frames);
    assert!(parser.is_finished());
}

#[test]
fn test_adversarial_empty_chunks_and_idempotence() {
    let mut parser = MjpegStreamParser::new("idle");
    for _ in 0..100 {
        parser.push_chunk(b"");
        assert_eq!(parser.next_frame().unwrap(), None);
    }
}
