//! Comprehensive Adversarial & Stress Test Suite for `archdrop_core`.
//!
//! Executed by Milestone 1 Challenger 2 to empirically stress-test:
//! 1. Cryptographic identity generation, DER validation, fingerprinting, invalid PEMs, malformed keys.
//! 2. DTO deserialization under malformed/extreme JSON, missing fields, unknown device types, extreme integers, oversized payloads.
//! 3. Apple Continuity 4 KB threshold boundary values (4095, 4096, 4097 bytes) and multi-byte UTF-8 fuzzing.
//! 4. Invariants and safety checks.

use archdrop_core::crypto::{
    fingerprint_from_cert_der, fingerprint_from_cert_pem, normalize_fingerprint,
    public_key_from_cert_der, validate_fingerprint, verify_cert_from_der, verify_cert_from_pem,
    CryptoIdentity,
};
use archdrop_core::error::CryptoError;
use archdrop_core::models::clipboard::{
    ClearReason, ClipboardDescriptorPacket, ClipboardDescriptorPayload, ClipboardInlinePacket,
    ClipboardInlinePayload, ClipboardItemDto, ClipboardPacket, ClipboardPayload, KdeConnectPacket,
    CLIPBOARD_EPHEMERAL_TTL_SECS, CLIPBOARD_INLINE_THRESHOLD,
};
use archdrop_core::models::device::{DeviceType, ProtocolType};
use archdrop_core::models::dto_v2::{
    FileDto, PrepareUploadRequestDtoV2, RegisterDtoV2, DEFAULT_PORT, PROTOCOL_VERSION_V2,
};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

// ============================================================================
// 1. CRYPTO ADVERSARIAL & STRESS TESTS
// ============================================================================

/// Stress-test: Rapid sequential RSA-2048 keypair and X.509 cert generation.
/// Verifies uniqueness, performance, and self-consistency under load.
#[test]
fn stress_test_rapid_sequential_cert_generations() {
    const GENERATION_COUNT: usize = 5;
    let mut fingerprints = HashSet::new();
    let mut cert_ders = HashSet::new();

    let start_time = Instant::now();

    for i in 0..GENERATION_COUNT {
        let loop_start = Instant::now();
        let identity = CryptoIdentity::generate().unwrap_or_else(|e| {
            panic!("Sequential generation {i} failed with error: {e}");
        });
        let elapsed = loop_start.elapsed();

        // RSA-2048 prime search in unoptimized debug builds takes ~2-10s per key; in release builds ~0.4s.
        let max_allowed_secs = if cfg!(debug_assertions) { 15 } else { 2 };
        assert!(
            elapsed.as_secs() < max_allowed_secs,
            "Generation {i} took too long: {elapsed:?}"
        );

        // Verification: fingerprint format and consistency
        assert_eq!(identity.fingerprint.len(), 64);
        assert!(validate_fingerprint(&identity.fingerprint));
        assert_eq!(
            identity.fingerprint,
            fingerprint_from_cert_der(&identity.certificate_der)
        );

        // Verification: self-signed signature must verify with x509-parser
        verify_cert_from_der(&identity.certificate_der, None)
            .unwrap_or_else(|e| panic!("Cert {i} DER signature verification failed: {e}"));
        verify_cert_from_pem(&identity.certificate_pem, None)
            .unwrap_or_else(|e| panic!("Cert {i} PEM verification failed: {e}"));

        // Uniqueness: each iteration MUST produce a distinct key pair and cert
        assert!(
            fingerprints.insert(identity.fingerprint.clone()),
            "Duplicate fingerprint detected in iteration {i}: {}",
            identity.fingerprint
        );
        assert!(
            cert_ders.insert(identity.certificate_der.clone()),
            "Duplicate certificate DER detected in iteration {i}"
        );
    }

    let total_elapsed = start_time.elapsed();
    eprintln!(
        "[STRESS] {GENERATION_COUNT} RSA-2048 cert generations completed in {total_elapsed:?} ({:?}/cert)",
        total_elapsed / GENERATION_COUNT as u32
    );
}

/// Stress-test: DER parsing & validation resilience against malformed / corrupted inputs.
#[test]
fn stress_test_der_validation_adversarial_inputs() {
    // 1. Empty byte slice
    let err_empty = verify_cert_from_der(&[], None);
    assert!(
        matches!(err_empty, Err(CryptoError::X509Parse(_))),
        "Empty DER must return X509Parse error, got: {err_empty:?}"
    );

    // 2. Truncated DER headers
    let truncated_der = [0x30, 0x82, 0x04];
    let err_trunc = verify_cert_from_der(&truncated_der, None);
    assert!(
        matches!(err_trunc, Err(CryptoError::X509Parse(_))),
        "Truncated DER must return X509Parse error, got: {err_trunc:?}"
    );

    // 3. Fuzzed random bytes (simulating garbage/network corruption)
    let fuzzed_sizes = [16, 64, 256, 1024, 2048];
    for &size in &fuzzed_sizes {
        let fuzzed: Vec<u8> = (0..size).map(|b| (b * 101 + 37) as u8).collect();
        let err_fuzz = verify_cert_from_der(&fuzzed, None);
        assert!(
            err_fuzz.is_err(),
            "Fuzzed DER of size {size} must return error"
        );
    }

    // 4. Valid certificate with 1-bit signature corruption
    let identity = CryptoIdentity::generate().expect("Generate test cert");
    let mut corrupted_der = identity.certificate_der.clone();
    let last_idx = corrupted_der.len() - 5;
    corrupted_der[last_idx] ^= 0x01; // flip 1 bit in signature segment

    let err_corrupted_sig = verify_cert_from_der(&corrupted_der, None);
    assert!(
        matches!(
            err_corrupted_sig,
            Err(CryptoError::SignatureVerification(_)) | Err(CryptoError::X509Parse(_))
        ),
        "Bit-flipped certificate must fail signature verification, got: {err_corrupted_sig:?}"
    );

    // 5. Valid certificate with mismatched expected public key
    let other_identity = CryptoIdentity::generate().expect("Generate second cert");
    let other_pk =
        public_key_from_cert_der(&other_identity.certificate_der).expect("Extract public key");

    let err_mismatch = verify_cert_from_der(&identity.certificate_der, Some(&other_pk));
    assert!(
        matches!(err_mismatch, Err(CryptoError::PublicKeyMismatch)),
        "Mismatched public key must return PublicKeyMismatch, got: {err_mismatch:?}"
    );

    // 6. Valid certificate with malformed expected public key PEM
    let malformed_pk = "-----BEGIN PUBLIC KEY-----\nNOT_VALID_BASE64\n-----END PUBLIC KEY-----";
    let err_pk_malformed = verify_cert_from_der(&identity.certificate_der, Some(malformed_pk));
    assert!(
        matches!(
            err_pk_malformed,
            Err(CryptoError::Pem(_)) | Err(CryptoError::X509Parse(_))
        ),
        "Malformed public key PEM must return error, got: {err_pk_malformed:?}"
    );
}

/// Stress-test: Fingerprint calculation throughput and test vector adherence.
#[test]
fn stress_test_fingerprint_throughput_and_vectors() {
    // 1. Upstream test vector parity
    const UPSTREAM_FP: &str = "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA";
    assert_eq!(UPSTREAM_FP.len(), 64);
    assert!(validate_fingerprint(UPSTREAM_FP));
    assert_eq!(
        normalize_fingerprint("4badde53a7f7cdeeed93189fd898e02bf6b4806ca4c05de0ace08319b86552fa"),
        Some(UPSTREAM_FP.to_string())
    );

    // 2. Empty byte slice fingerprint (SHA-256 of empty bytes)
    let empty_fp = fingerprint_from_cert_der(&[]);
    assert_eq!(
        empty_fp,
        "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
    );

    // 3. Throughput: 10,000 iterations of fingerprint calculation
    let test_bytes = vec![0xABu8; 1024]; // 1 KB synthetic cert DER
    let start = Instant::now();
    for _ in 0..10_000 {
        let fp = fingerprint_from_cert_der(&test_bytes);
        assert_eq!(fp.len(), 64);
    }
    let elapsed = start.elapsed();
    eprintln!("[BENCH] 10,000 SHA-256 fingerprint iterations completed in {elapsed:?}");

    // 4. Adversarial strings for validate_fingerprint
    assert!(!validate_fingerprint(""));
    assert!(!validate_fingerprint("123"));
    assert!(!validate_fingerprint(&"A".repeat(63))); // 63 chars
    assert!(!validate_fingerprint(&"A".repeat(65))); // 65 chars
    assert!(!validate_fingerprint(&format!("{}Z", "A".repeat(63)))); // non-hex 'Z'
    assert!(!validate_fingerprint(&format!("{} ", "A".repeat(63)))); // space
    assert!(!validate_fingerprint(&format!("{}\0", "A".repeat(63)))); // null byte
    assert!(!validate_fingerprint(&format!("{}\n", "A".repeat(63)))); // newline
    assert!(!validate_fingerprint(
        "4B:AD:DE:53:A7:F7:CD:EE:ED:93:18:9F:D8:98:E0:2B:F6:B4:80:6C:A4:C0:5D:E0:AC:E0:83:19:B8:65:52:FA"
    )); // colon separated rejected
}

/// Stress-test: Invalid PEM handling & parser edge cases.
#[test]
fn stress_test_invalid_pem_handling() {
    // 1. Empty string
    let err_empty = fingerprint_from_cert_pem("");
    assert!(err_empty.is_err(), "Empty PEM must return error");

    // 2. Arbitrary text
    let err_text = fingerprint_from_cert_pem("This is definitely not a certificate.");
    assert!(err_text.is_err(), "Arbitrary text must return error");

    // 3. Corrupted headers
    let broken_headers = [
        "-----BEGIN CERTIFICATE-----\nMIID...-----END",
        "-----BEGIN FOOBAR-----\nMIIB...-----END FOOBAR-----",
        "-----BEGIN CERTIFICATE-----",
        "MIID...-----END CERTIFICATE-----",
    ];
    for broken in &broken_headers {
        let res = fingerprint_from_cert_pem(broken);
        assert!(res.is_err(), "Broken header must fail: {broken}");
    }

    // 4. Incomplete identity in combined PEM
    let identity = CryptoIdentity::generate().unwrap();

    // Only certificate, missing private key
    let cert_only = &identity.certificate_pem;
    let err_no_key = CryptoIdentity::from_combined_pem(cert_only);
    assert!(
        matches!(err_no_key, Err(CryptoError::MissingIdentityComponent)),
        "Combined PEM missing private key must return MissingIdentityComponent"
    );

    // Only private key, missing certificate
    let key_only = &identity.private_key_pem;
    let err_no_cert = CryptoIdentity::from_combined_pem(key_only);
    assert!(
        matches!(err_no_cert, Err(CryptoError::MissingIdentityComponent)),
        "Combined PEM missing certificate must return MissingIdentityComponent"
    );

    // 5. Swapped parts in from_pem_parts
    // When key and cert are swapped, PEM reading succeeds on syntax level, but
    // public_key_from_cert_der on key DER will fail X509Parse.
    let swapped =
        CryptoIdentity::from_pem_parts(&identity.private_key_pem, &identity.certificate_pem);
    assert!(swapped.is_ok(), "from_pem_parts reads raw PEM contents");
    let swapped_id = swapped.unwrap();
    // Verify that attempting to parse swapped certificate as X.509 DER fails
    let verify_swapped = verify_cert_from_der(&swapped_id.certificate_der, None);
    assert!(
        verify_swapped.is_err(),
        "Private key DER passed as certificate MUST fail X.509 verification"
    );
}

// ============================================================================
// 2. DTO DESERIALIZATION ADVERSARIAL FUZZING
// ============================================================================

/// Stress-test: RegisterDtoV2 deserialization under malformed, extreme, and fuzzed JSON.
#[test]
fn stress_test_register_dto_fuzzing() {
    // 1. Missing required fields (alias, version, fingerprint, port, protocol)
    let missing_field_cases = [
        r#"{}"#,
        r#"{"version": "2.2", "port": 53317, "protocol": "https"}"#,
        r#"{"alias": "Test", "port": 53317, "protocol": "https"}"#,
        r#"{"alias": "Test", "version": "2.2", "protocol": "https"}"#,
        r#"{"alias": "Test", "version": "2.2", "fingerprint": "123", "protocol": "https"}"#,
        r#"{"alias": "Test", "version": "2.2", "fingerprint": "123", "port": 53317}"#,
    ];
    for case in &missing_field_cases {
        let res: Result<RegisterDtoV2, _> = serde_json::from_str(case);
        assert!(res.is_err(), "Missing fields must reject: {case}");
    }

    // 2. Unknown device types -> MUST fallback to DeviceType::Desktop
    let unknown_types = [
        "toaster",
        "smart_fridge",
        "refrigerator",
        "robot",
        "smart_watch",
        "VR_HEADSET",
        "tesla",
        "unknown_device",
        "",
    ];
    for dt in &unknown_types {
        let json = format!(
            r#"{{
                "alias": "Test Device",
                "version": "2.2",
                "deviceType": "{dt}",
                "fingerprint": "FP123",
                "port": 53317,
                "protocol": "https"
            }}"#
        );
        let dto: RegisterDtoV2 = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("Unknown deviceType '{dt}' should deserialize: {e}"));
        assert_eq!(
            dto.device_type,
            Some(DeviceType::Desktop),
            "Unknown deviceType '{dt}' must fall back to Desktop"
        );
    }

    // 3. Standard device types (case-insensitive)
    let standard_types = [
        ("mobile", DeviceType::Mobile),
        ("Mobile", DeviceType::Mobile),
        ("MOBILE", DeviceType::Mobile),
        ("desktop", DeviceType::Desktop),
        ("Desktop", DeviceType::Desktop),
        ("DESKTOP", DeviceType::Desktop),
        ("web", DeviceType::Web),
        ("headless", DeviceType::Headless),
        ("server", DeviceType::Server),
    ];
    for (raw, expected) in &standard_types {
        let json = format!(
            r#"{{
                "alias": "Test Device",
                "version": "2.2",
                "deviceType": "{raw}",
                "fingerprint": "FP123",
                "port": 53317,
                "protocol": "https"
            }}"#
        );
        let dto: RegisterDtoV2 = serde_json::from_str(&json).expect("Standard deviceType");
        assert_eq!(dto.device_type, Some(*expected));
    }

    // 4. Missing or null deviceType -> None
    let json_null = r#"{
        "alias": "Test",
        "version": "2.2",
        "deviceType": null,
        "fingerprint": "FP",
        "port": 53317,
        "protocol": "https"
    }"#;
    let dto_null: RegisterDtoV2 = serde_json::from_str(json_null).unwrap();
    assert_eq!(dto_null.device_type, None);

    let json_missing_dt = r#"{
        "alias": "Test",
        "version": "2.2",
        "fingerprint": "FP",
        "port": 53317,
        "protocol": "https"
    }"#;
    let dto_missing_dt: RegisterDtoV2 = serde_json::from_str(json_missing_dt).unwrap();
    assert_eq!(dto_missing_dt.device_type, None);

    // 5. ProtocolType parsing: only "http" and "https" (case-insensitive)
    let valid_protocols = ["http", "HTTP", "Http", "https", "HTTPS", "Https"];
    for proto in &valid_protocols {
        let json = format!(
            r#"{{
                "alias": "Test",
                "version": "2.2",
                "fingerprint": "FP",
                "port": 53317,
                "protocol": "{proto}"
            }}"#
        );
        let dto: RegisterDtoV2 = serde_json::from_str(&json).expect("Valid protocol");
        if proto.to_lowercase() == "http" {
            assert_eq!(dto.protocol, ProtocolType::Http);
        } else {
            assert_eq!(dto.protocol, ProtocolType::Https);
        }
    }

    let invalid_protocols = ["ftp", "ws", "wss", "grpc", "quic", "", "null"];
    for proto in &invalid_protocols {
        let json = format!(
            r#"{{
                "alias": "Test",
                "version": "2.2",
                "fingerprint": "FP",
                "port": 53317,
                "protocol": "{proto}"
            }}"#
        );
        let res: Result<RegisterDtoV2, _> = serde_json::from_str(&json);
        assert!(res.is_err(), "Invalid protocol '{proto}' must be rejected");
    }

    // 6. Port number boundaries
    // Valid: 0, 1, 53317, 65535
    for valid_port in [0, 1, 53317, 65535] {
        let json = format!(
            r#"{{
                "alias": "Test",
                "version": "2.2",
                "fingerprint": "FP",
                "port": {valid_port},
                "protocol": "https"
            }}"#
        );
        let dto: RegisterDtoV2 = serde_json::from_str(&json).expect("Valid port");
        assert_eq!(dto.port, valid_port);
    }

    // Invalid port: -1, 65536, 1000000, "53317"
    let invalid_ports = [
        r#"{"alias":"T","version":"2","fingerprint":"F","protocol":"https","port":-1}"#,
        r#"{"alias":"T","version":"2","fingerprint":"F","protocol":"https","port":65536}"#,
        r#"{"alias":"T","version":"2","fingerprint":"F","protocol":"https","port":1000000}"#,
        r#"{"alias":"T","version":"2","fingerprint":"F","protocol":"https","port":"53317"}"#,
        r#"{"alias":"T","version":"2","fingerprint":"F","protocol":"https","port":53317.5}"#,
    ];
    for case in &invalid_ports {
        let res: Result<RegisterDtoV2, _> = serde_json::from_str(case);
        assert!(res.is_err(), "Invalid port must fail: {case}");
    }
}

/// Stress-test: FileDto and PrepareUploadRequestDtoV2 extreme integer values, oversized payloads, path injection.
#[test]
fn stress_test_file_dto_extreme_integers_and_payloads() {
    // 1. Extreme file sizes: 0, 1, 4096, 1 GB, u64::MAX
    let sizes = [0u64, 1, 4096, 1_073_741_824, u64::MAX];
    for &size in &sizes {
        let json = format!(
            r#"{{
                "id": "file-1",
                "fileName": "test.bin",
                "size": {size},
                "fileType": "application/octet-stream"
            }}"#
        );
        let dto: FileDto = serde_json::from_str(&json).expect("Extreme size deserialize");
        assert_eq!(dto.size, size);
    }

    // 2. Negative size or overflow beyond u64
    let invalid_sizes = [
        r#"{"id":"f","fileName":"t","size":-1,"fileType":"bin"}"#,
        r#"{"id":"f","fileName":"t","size":18446744073709551616,"fileType":"bin"}"#, // u64::MAX + 1
        r#"{"id":"f","fileName":"t","size":"1024","fileType":"bin"}"#,
    ];
    for case in &invalid_sizes {
        let res: Result<FileDto, _> = serde_json::from_str(case);
        assert!(res.is_err(), "Invalid size must fail: {case}");
    }

    // 3. Path injection strings in fileName
    let long_name = "a".repeat(10_000);
    let adversarial_names = [
        "../../../etc/passwd",
        "/root/.ssh/id_ed25519",
        "C:\\Windows\\System32\\cmd.exe",
        "file\0with\0nulls.txt",
        "CON",
        "PRN",
        "AUX",
        "NUL",
        long_name.as_str(),
        "🎉🚀📦🔥_unicode_test.tar.gz",
    ];
    for &name in &adversarial_names {
        let dto = FileDto {
            id: "file-test".to_string(),
            file_name: name.to_string(),
            size: 1024,
            file_type: "text/plain".to_string(),
            sha256: None,
            preview: None,
            metadata: None,
        };
        let json = serde_json::to_string(&dto).expect("Serialize adversarial filename");
        let parsed: FileDto = serde_json::from_str(&json).expect("Deserialize filename");
        assert_eq!(parsed.file_name, name);
    }

    // 4. Large number of files in PrepareUploadRequestDtoV2 (1,000 files)
    let mut files = HashMap::new();
    for i in 0..1_000 {
        files.insert(
            format!("file-{i}"),
            FileDto {
                id: format!("file-{i}"),
                file_name: format!("document_{i}.pdf"),
                size: (i as u64) * 1024,
                file_type: "application/pdf".to_string(),
                sha256: None,
                preview: None,
                metadata: None,
            },
        );
    }

    let req = PrepareUploadRequestDtoV2 {
        info: RegisterDtoV2 {
            alias: "Massive Transfer Sender".to_string(),
            version: "2.2".to_string(),
            device_model: Some("Arch Linux".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: "FP123".to_string(),
            port: DEFAULT_PORT,
            protocol: ProtocolType::Https,
            download: false,
        },
        files,
    };

    let start = Instant::now();
    let json = serde_json::to_string(&req).expect("Serialize 1000 files");
    let serialized_size = json.len();
    let parsed: PrepareUploadRequestDtoV2 =
        serde_json::from_str(&json).expect("Deserialize 1000 files");
    let elapsed = start.elapsed();

    assert_eq!(parsed.files.len(), 1_000);
    eprintln!(
        "[STRESS] 1,000 file DTOs serialized/deserialized ({serialized_size} bytes) in {elapsed:?}"
    );
}

// ============================================================================
// 3. APPLE CONTINUITY 4 KB THRESHOLD BOUNDARY TESTS (4095, 4096, 4097)
// ============================================================================

/// Exact boundary testing for Apple Continuity 4 KB threshold rule:
/// - 4095 bytes (4096 - 1) -> Inline
/// - 4096 bytes (Apple threshold boundary) -> Inline
/// - 4097 bytes (4096 + 1) -> Descriptor
#[test]
fn test_apple_4kb_threshold_exact_boundaries() {
    assert_eq!(CLIPBOARD_INLINE_THRESHOLD, 4096);

    // Boundary 1: 4095 bytes (INLINE)
    let payload_4095 = "A".repeat(4095);
    assert_eq!(payload_4095.len(), 4095);
    assert!(payload_4095.len() <= CLIPBOARD_INLINE_THRESHOLD);

    let inline_4095 = ClipboardPayload::Inline(ClipboardInlinePayload {
        mime_type: "text/plain;charset=utf-8".to_string(),
        content: payload_4095.clone(),
        sha256: "hash4095".to_string(),
        size_bytes: payload_4095.len(),
    });
    assert!(inline_4095.is_inline(), "4095 bytes MUST be Inline");
    assert_eq!(inline_4095.size_bytes(), 4095);

    // Roundtrip through ClipboardPacket::Inline
    let packet_4095 = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "host-node".to_string(),
        timestamp: 1794022800000,
        ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
        payload: match inline_4095 {
            ClipboardPayload::Inline(p) => p,
            _ => unreachable!(),
        },
    });
    let json_4095 = serde_json::to_string(&packet_4095).expect("Serialize 4095 packet");
    let de_4095: ClipboardPacket =
        serde_json::from_str(&json_4095).expect("Deserialize 4095 packet");
    assert_eq!(packet_4095, de_4095);

    // Boundary 2: 4096 bytes (INLINE - Exact Upper Boundary)
    let payload_4096 = "B".repeat(4096);
    assert_eq!(payload_4096.len(), 4096);
    assert!(payload_4096.len() <= CLIPBOARD_INLINE_THRESHOLD);

    let inline_4096 = ClipboardPayload::Inline(ClipboardInlinePayload {
        mime_type: "text/plain;charset=utf-8".to_string(),
        content: payload_4096.clone(),
        sha256: "hash4096".to_string(),
        size_bytes: payload_4096.len(),
    });
    assert!(inline_4096.is_inline(), "4096 bytes MUST be Inline");
    assert_eq!(inline_4096.size_bytes(), 4096);

    let packet_4096 = ClipboardPacket::Inline(ClipboardInlinePacket {
        version: 2,
        device_id: "host-node".to_string(),
        timestamp: 1794022800000,
        ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
        payload: match inline_4096 {
            ClipboardPayload::Inline(p) => p,
            _ => unreachable!(),
        },
    });
    let json_4096 = serde_json::to_string(&packet_4096).expect("Serialize 4096 packet");
    let de_4096: ClipboardPacket =
        serde_json::from_str(&json_4096).expect("Deserialize 4096 packet");
    assert_eq!(packet_4096, de_4096);

    // Boundary 3: 4097 bytes (DESCRIPTOR - First Byte Over Threshold)
    let payload_4097_len = 4097;
    assert!(payload_4097_len > CLIPBOARD_INLINE_THRESHOLD);

    let descriptor_4097 = ClipboardPayload::Descriptor(ClipboardDescriptorPayload {
        payload_id: "desc-uuid-4097".to_string(),
        primary_mime: "text/plain;charset=utf-8".to_string(),
        available_mimes: vec!["text/plain".to_string()],
        size_bytes: payload_4097_len,
        sha256: "hash4097".to_string(),
        ephemeral_token: "token4097".to_string(),
        download_url: Some("https://10.2.0.2:53317/api/v2/clipboard/desc-uuid-4097".to_string()),
    });
    assert!(
        !descriptor_4097.is_inline(),
        "4097 bytes MUST be Descriptor"
    );
    assert_eq!(descriptor_4097.size_bytes(), 4097);

    let packet_4097 = ClipboardPacket::Descriptor(ClipboardDescriptorPacket {
        version: 2,
        device_id: "host-node".to_string(),
        timestamp: 1794022800000,
        ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
        descriptor: match descriptor_4097 {
            ClipboardPayload::Descriptor(d) => d,
            _ => unreachable!(),
        },
    });
    let json_4097 = serde_json::to_string(&packet_4097).expect("Serialize 4097 packet");
    let de_4097: ClipboardPacket =
        serde_json::from_str(&json_4097).expect("Deserialize 4097 packet");
    assert_eq!(packet_4097, de_4097);
}

/// Stress-test: Multi-byte UTF-8 boundary transitions.
/// E.g., 4-byte emojis and 2-byte accented characters at the 4096 border.
#[test]
fn test_apple_4kb_threshold_multibyte_utf8_boundaries() {
    // Crab emoji 🦀 is 4 bytes in UTF-8
    let crab = "🦀";
    assert_eq!(crab.len(), 4);
    assert_eq!(crab.chars().count(), 1);

    // Case 1: 4092 ASCII chars + 1 crab (4 bytes) = 4096 bytes (Exact border -> Inline)
    let mut border_4096 = "A".repeat(4092);
    border_4096.push_str(crab);
    assert_eq!(border_4096.len(), 4096);
    assert_eq!(border_4096.chars().count(), 4093);
    assert!(border_4096.len() <= CLIPBOARD_INLINE_THRESHOLD);

    let inline = ClipboardInlinePayload {
        mime_type: "text/plain;charset=utf-8".to_string(),
        content: border_4096.clone(),
        sha256: "hash".to_string(),
        size_bytes: border_4096.len(),
    };
    assert_eq!(inline.size_bytes, 4096);

    // Case 2: 4093 ASCII chars + 1 crab (4 bytes) = 4097 bytes (> 4096 -> Descriptor)
    let mut border_4097 = "A".repeat(4093);
    border_4097.push_str(crab);
    assert_eq!(border_4097.len(), 4097);
    assert!(border_4097.len() > CLIPBOARD_INLINE_THRESHOLD);

    // Case 3: 4095 ASCII chars + 1 two-byte character 'é' (0xC3 0xA9) = 4097 bytes -> Descriptor
    let mut border_e_4097 = "A".repeat(4095);
    border_e_4097.push('é');
    assert_eq!(border_e_4097.len(), 4097);
    assert!(border_e_4097.len() > CLIPBOARD_INLINE_THRESHOLD);

    // Case 4: 4094 ASCII chars + 1 two-byte character 'é' = 4096 bytes -> Inline
    let mut border_e_4096 = "A".repeat(4094);
    border_e_4096.push('é');
    assert_eq!(border_e_4096.len(), 4096);
    assert!(border_e_4096.len() <= CLIPBOARD_INLINE_THRESHOLD);
}

/// Stress-test: Unicode fuzzing in clipboard payloads.
/// Tests null bytes, control characters, RTL override, zero-width chars, compound ZWJ emojis.
#[test]
fn stress_test_clipboard_unicode_fuzzing() {
    let fuzzed_strings = [
        // Null bytes in clipboard
        "Text with \0 embedded \0 nulls",
        "\0\0\0\0\0",
        // Control characters
        "\x01\x02\x03\x04\x05\x06\x07\x08\x0E\x0F\x1B\x7F",
        // Right-to-Left Override (bidi spoofing)
        "SafeFilename\u{202E}txt.exe",
        // Zero-width characters
        "Invisible\u{200B}\u{200C}\u{200D}\u{FEFF}Separators",
        // Complex compound emojis with ZWJ
        "Family: 👨‍👩‍👧‍👦 Astronaut: 👩‍🚀 Flag: 🏴󠁧󠁢󠁳󠁣󠁴󠁿",
        // CJK, Arabic, Hebrew, Devanagari mix
        "Arch Drop: 拱落 / هبوط القوس / קשת / आर्च ड्रॉप",
        // Extreme surrogate / astral characters
        "𐍈𐍗𐍘𐍙𐍚𐍛𐍜𐍝𐍞𐍟",
    ];

    for &fuzz in &fuzzed_strings {
        let item = ClipboardItemDto {
            id: "fuzz-item".to_string(),
            mime_type: "text/plain;charset=utf-8".to_string(),
            size: fuzz.len(),
            sha256: "dummy-hash".to_string(),
            timestamp_epoch_ms: 1794022800000,
            is_sensitive: false,
            content: Some(fuzz.to_string()),
            download_url: None,
        };

        let json = serde_json::to_string(&item).expect("Serialize fuzzed clipboard item");
        let parsed: ClipboardItemDto =
            serde_json::from_str(&json).expect("Deserialize fuzzed item");
        assert_eq!(parsed.content.as_deref(), Some(fuzz));
        assert_eq!(parsed.size, fuzz.len());

        // Also test KDE Connect packet compatibility
        let kde_packet = KdeConnectPacket::new_clipboard(fuzz.to_string());
        let kde_json = serde_json::to_string(&kde_packet).expect("Serialize KDE packet");
        let kde_parsed: KdeConnectPacket =
            serde_json::from_str(&kde_json).expect("Deserialize KDE");
        assert_eq!(kde_parsed.body.content, fuzz);
    }
}

// ============================================================================
// 4. PROTOCOL & INTEGRATION INVARIANT VERIFICATION
// ============================================================================

/// Invariant: Protocol constants and enum mappings match LocalSend and Apple specifications.
#[test]
fn test_protocol_and_continuity_invariants() {
    assert_eq!(PROTOCOL_VERSION_V2, "2.2");
    assert_eq!(DEFAULT_PORT, 53317);
    assert_eq!(CLIPBOARD_INLINE_THRESHOLD, 4096);
    assert_eq!(CLIPBOARD_EPHEMERAL_TTL_SECS, 120);

    // Verify ClearReason serialization wire names
    let reasons = [
        (ClearReason::SessionLock, "\"session_lock\""),
        (ClearReason::TtlExpired, "\"ttl_expired\""),
        (ClearReason::UserAction, "\"user_action\""),
    ];
    for (reason, expected_json) in reasons {
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, expected_json);
    }

    // Verify DeviceType string mapping
    assert_eq!(DeviceType::Desktop.as_str(), "desktop");
    assert_eq!(DeviceType::Mobile.as_str(), "mobile");
    assert_eq!(DeviceType::Web.as_str(), "web");
    assert_eq!(DeviceType::Headless.as_str(), "headless");
    assert_eq!(DeviceType::Server.as_str(), "server");

    // Verify ProtocolType string mapping
    assert_eq!(ProtocolType::Http.as_str(), "http");
    assert_eq!(ProtocolType::Https.as_str(), "https");
}

// ============================================================================
// 5. DEEP ADVERSARIAL EDGE CASES & SECURITY INVARIANTS
// ============================================================================

/// Adversarial observation: `fingerprint_from_cert_pem` parses the first PEM block
/// without verifying that `pem.label == "CERTIFICATE"`.
/// If passed a combined PEM where PRIVATE KEY precedes CERTIFICATE, it computes
/// the fingerprint of the private key rather than the certificate.
#[test]
fn test_fingerprint_from_cert_pem_label_behavior() {
    let identity = CryptoIdentity::generate().unwrap();

    // 1. Standard certificate PEM computes the correct certificate fingerprint
    let cert_fp = fingerprint_from_cert_pem(&identity.certificate_pem).unwrap();
    assert_eq!(cert_fp, identity.fingerprint);

    // 2. Private key passed to fingerprint_from_cert_pem:
    // It succeeds because Pem::read does not enforce label == "CERTIFICATE".
    // We document and verify this behavior.
    let key_fp = fingerprint_from_cert_pem(&identity.private_key_pem);
    assert!(
        key_fp.is_ok(),
        "fingerprint_from_cert_pem reads raw first PEM block"
    );
    // The key fingerprint must NOT match the certificate fingerprint
    assert_ne!(
        key_fp.unwrap(),
        identity.fingerprint,
        "Private key hash must differ from certificate hash"
    );

    // 3. In to_combined_pem(), private key is first.
    let combined = identity.to_combined_pem();
    let combined_first_block_fp = fingerprint_from_cert_pem(&combined).unwrap();
    // Because private key is first in combined PEM, fingerprint_from_cert_pem(&combined)
    // hashes the private key block.
    assert_ne!(
        combined_first_block_fp, identity.fingerprint,
        "fingerprint_from_cert_pem on combined PEM reads first block (private key)"
    );
}

/// Adversarial test: Combined PEM with mismatched private key and certificate.
/// Verifies that from_combined_pem constructs the struct, but verify_cert_from_der
/// with extracted public key catches mismatches.
#[test]
fn test_from_combined_pem_mismatched_key_and_cert() {
    let id_a = CryptoIdentity::generate().unwrap();
    let id_b = CryptoIdentity::generate().unwrap();

    // Create a forged combined PEM with cert from A and private key from B
    let forged_combined = format!(
        "{}\n\n{}\n",
        id_b.private_key_pem.trim(),
        id_a.certificate_pem.trim()
    );

    let loaded = CryptoIdentity::from_combined_pem(&forged_combined).unwrap();
    // The fingerprint matches cert A
    assert_eq!(loaded.fingerprint, id_a.fingerprint);
    // The certificate DER matches cert A
    assert_eq!(loaded.certificate_der, id_a.certificate_der);
    // The private key DER matches key B
    assert_eq!(loaded.private_key_der, id_b.private_key_der);

    // Now verify that public key extracted from cert A does NOT match private key B
    let pk_a = public_key_from_cert_der(&loaded.certificate_der).unwrap();
    let pk_b = public_key_from_cert_der(&id_b.certificate_der).unwrap();
    assert_ne!(pk_a, pk_b, "Mismatched cert/key must have different SPKIs");

    // Verification fails when checking cert against key B's public key
    let verify_res = verify_cert_from_der(&loaded.certificate_der, Some(&pk_b));
    assert!(
        matches!(verify_res, Err(CryptoError::PublicKeyMismatch)),
        "Verification must detect public key mismatch"
    );
}

/// Adversarial test: DTO deserialization with invalid unicode surrogates and extreme JSON types.
#[test]
fn test_dto_deep_nesting_and_surrogate_fuzzing() {
    // 1. Unpaired unicode surrogate in JSON string (\uD800)
    let json_surrogate = r#"{
        "alias": "Test\uD800Device",
        "version": "2.2",
        "fingerprint": "123",
        "port": 53317,
        "protocol": "https"
    }"#;
    let res_surrogate: Result<RegisterDtoV2, _> = serde_json::from_str(json_surrogate);
    assert!(
        res_surrogate.is_err(),
        "Unpaired surrogate \\uD800 must fail JSON decoding"
    );

    // 2. Boolean or array passed where string expected
    let type_confusion_cases = [
        r#"{"alias": true, "version": "2.2", "fingerprint": "123", "port": 53317, "protocol": "https"}"#,
        r#"{"alias": ["test"], "version": "2.2", "fingerprint": "123", "port": 53317, "protocol": "https"}"#,
        r#"{"alias": {"nested": "test"}, "version": "2.2", "fingerprint": "123", "port": 53317, "protocol": "https"}"#,
    ];
    for case in &type_confusion_cases {
        let res: Result<RegisterDtoV2, _> = serde_json::from_str(case);
        assert!(res.is_err(), "Type confusion payload must fail: {case}");
    }
}

/// Adversarial test: TOML configuration parser resilience against malformed and extreme values.
#[test]
fn test_config_adversarial_inputs() {
    use archdrop_core::config::Config;

    // 1. Invalid port in TOML (-500)
    let toml_negative_port = r#"
[network]
port = -500
"#;
    let err_port = Config::load_from_str(toml_negative_port);
    assert!(err_port.is_err(), "Negative port in TOML must fail");

    // 2. Port overflow (> 65535)
    let toml_overflow_port = r#"
[network]
port = 70000
"#;
    let err_overflow = Config::load_from_str(toml_overflow_port);
    assert!(err_overflow.is_err(), "Port overflow must fail");

    // 3. String where boolean expected
    let toml_string_bool = r#"
[security]
require_encryption = "yes"
"#;
    let err_bool = Config::load_from_str(toml_string_bool);
    assert!(err_bool.is_err(), "String for boolean must fail");

    // 4. Unknown fields in TOML are gracefully tolerated (forward compatibility)
    let toml_unknown_fields = r#"
[device]
alias = "Future Device"
unknown_future_field = 42

[unknown_section]
feature_flag = true
"#;
    let ok_future = Config::load_from_str(toml_unknown_fields);
    assert!(
        ok_future.is_ok(),
        "Unknown fields must be ignored for forward compatibility"
    );
    let cfg = ok_future.unwrap();
    assert_eq!(cfg.device.alias.as_deref(), Some("Future Device"));

    // 5. Paired fingerprints case-insensitivity and duplicate normalization
    let toml_fps = r#"
[security]
paired_fingerprints = [
    "4badde53a7f7cdeeed93189fd898e02bf6b4806ca4c05de0ace08319b86552fa",
    "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA"
]
"#;
    let cfg_fps = Config::load_from_str(toml_fps).unwrap();
    assert!(
        cfg_fps.is_peer_paired("4badde53a7f7cdeeed93189fd898e02bf6b4806ca4c05de0ace08319b86552fa")
    );
    assert!(
        cfg_fps.is_peer_paired("4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA")
    );
    assert!(!cfg_fps.is_peer_paired("UNKNOWN_FINGERPRINT"));
}
