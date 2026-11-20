use archdrop_core::crypto::{
    fingerprint_from_cert_der, fingerprint_from_cert_pem, fingerprint_matches,
    normalize_fingerprint, public_key_from_cert_der, validate_fingerprint, verify_cert_from_der,
    verify_cert_from_pem, CryptoIdentity,
};
use std::io::Cursor;
use x509_parser::pem::Pem;

// Upstream LocalSend test vectors from packages/core/src/crypto/cert.rs
static UPSTREAM_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAi9uDMYRn63SZtEPGRogZ\n\
Gdu5XXBAoQeMO60mycoinqLKDWyZdpMo+XWY3wYVhoAyxgzDOcPjIf+Uq1oEy/0K\n\
4WwfpbK8SCy851qgYkfMCT9D9mFvXwWoULJCUHFF7f947ArDE1nmuK1nNx2RodN2\n\
wJCXyzPjw0jn06bwGeg0EqfUC8wvW4FTZ6t1tErzmRqRdMUWuCJwsk1IMbDbFePh\n\
iK5jecOBG0RVVWLuw+TkuX8TUgrpIktH2+qEM1KdLyAMnL71hx2wMvE+lDKFKK9p\n\
37zXK8omjl+VgTC8ocjGeYDDsl43ZtW09V0pb7Vz2FM8b7BgM06kvJl48PIe5puY\n\
bQIDAQAB\n\
-----END PUBLIC KEY-----";

static UPSTREAM_CERT: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDGTCCAgGgAwIBAgIBATANBgkqhkiG9w0BAQsFADBQMRcwFQYDVQQDEw5Mb2Nh\n\
bFNlbmQgVXNlcjEJMAcGA1UEChMAMQkwBwYDVQQLEwAxCTAHBgNVBAcTADEJMAcG\n\
A1UECBMAMQkwBwYDVQQGEwAwHhcNMjUwMjA5MDAwMzE0WhcNMzUwMjA3MDAwMzE0\n\
WjBQMRcwFQYDVQQDEw5Mb2NhbFNlbmQgVXNlcjEJMAcGA1UEChMAMQkwBwYDVQQL\n\
EwAxCTAHBgNVBAcTADEJMAcGA1UECBMAMQkwBwYDVQQGEwAwggEiMA0GCSqGSIb3\n\
DQEBAQUAA4IBDwAwggEKAoIBAQCL24MxhGfrdJm0Q8ZGiBkZ27ldcEChB4w7rSbJ\n\
yiKeosoNbJl2kyj5dZjfBhWGgDLGDMM5w+Mh/5SrWgTL/QrhbB+lsrxILLznWqBi\n\
R8wJP0P2YW9fBahQskJQcUXt/3jsCsMTWea4rWc3HZGh03bAkJfLM+PDSOfTpvAZ\n\
6DQSp9QLzC9bgVNnq3W0SvOZGpF0xRa4InCyTUgxsNsV4+GIrmN5w4EbRFVVYu7D\n\
5OS5fxNSCukiS0fb6oQzUp0vIAycvvWHHbAy8T6UMoUor2nfvNcryiaOX5WBMLyh\n\
yMZ5gMOyXjdm1bT1XSlvtXPYUzxvsGAzTqS8mXjw8h7mm5htAgMBAAEwDQYJKoZI\n\
hvcNAQELBQADggEBABZ+I7D6wkeSrsi1NBLP2zoZ5oGh+INNcGTravfOQHs4Fbas\n\
/CysaUYjsD3fmaDh4MxgWEqAmWnnBiojfpGX2SGuFqRBKyT9DgitBt0L7Ezg1k3h\n\
bfSiFW4hXWp75grVO8xfML7ZcWMlhKrOsOMUGiy1qs3qsyJ3w7B2Tz78HhXGO5dd\n\
jyPmZarhixKO92UpEvKGxjO0E/3UUNUzxKTAAgFfhKpuwHUgIijM/EppZtA8OcSh\n\
fEztiV0xKfcPVx4d6dqRt/NMElK1Ivw2vUuxTymphZkkFOzht9m73/kyKaeFp8Ij\n\
VRus1zGVD8IVpIdPMyz01WJyS7M0fWaHXKWo+Bo=\n\
-----END CERTIFICATE-----";

#[test]
fn test_upstream_fingerprint_parity() {
    let (cert_pem, _) = Pem::read(Cursor::new(UPSTREAM_CERT.as_bytes())).unwrap();
    let fingerprint = fingerprint_from_cert_der(&cert_pem.contents);
    assert_eq!(
        fingerprint, "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA",
        "Fingerprint must match upstream LocalSend test vector exactly"
    );

    let pem_fp = fingerprint_from_cert_pem(UPSTREAM_CERT).unwrap();
    assert_eq!(pem_fp, fingerprint);
}

#[test]
fn test_upstream_cert_verification() {
    // Valid certificate and matching public key
    let result = verify_cert_from_pem(UPSTREAM_CERT, Some(UPSTREAM_PUBLIC_KEY));
    assert!(result.is_ok(), "Upstream cert should verify successfully");

    // Public key extraction from DER
    let (cert_pem, _) = Pem::read(Cursor::new(UPSTREAM_CERT.as_bytes())).unwrap();
    let extracted_pk = public_key_from_cert_der(&cert_pem.contents).unwrap();
    assert_eq!(
        extracted_pk.replace("\r\n", "\n").trim(),
        UPSTREAM_PUBLIC_KEY.trim()
    );
}

#[test]
fn test_identity_generation_and_self_consistency() {
    let id = CryptoIdentity::generate().expect("Identity generation must succeed");

    assert!(id.private_key_pem.contains("BEGIN PRIVATE KEY"));
    assert!(id.certificate_pem.contains("BEGIN CERTIFICATE"));
    assert!(!id.certificate_der.is_empty());
    assert!(!id.private_key_der.is_empty());

    // Validate fingerprint format
    assert_eq!(id.fingerprint.len(), 64);
    assert!(validate_fingerprint(&id.fingerprint));
    assert_eq!(id.fingerprint, id.fingerprint.to_uppercase());

    // Verify self-signed certificate signature
    verify_cert_from_der(&id.certificate_der, None)
        .expect("Generated self-signed certificate must verify its own signature");
    verify_cert_from_pem(&id.certificate_pem, None).expect("Generated certificate PEM must verify");

    // Fingerprint recomputation match
    assert_eq!(
        id.fingerprint,
        fingerprint_from_cert_der(&id.certificate_der)
    );
}

#[test]
fn test_identity_file_persistence_and_permissions() {
    let temp_dir = tempfile::tempdir().unwrap();
    let id_path = temp_dir.path().join("sub_dir").join("identity.pem");

    let original = CryptoIdentity::generate().unwrap();
    original
        .save_to_file(&id_path)
        .expect("Save to file must succeed");

    // Check POSIX permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&id_path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "identity.pem must have 0o600 permissions");
    }

    // Load back from file
    let loaded = CryptoIdentity::load_from_file(&id_path).expect("Load from file must succeed");
    assert_eq!(loaded.fingerprint, original.fingerprint);
    assert_eq!(loaded.certificate_der, original.certificate_der);
    assert_eq!(loaded.private_key_der, original.private_key_der);

    // load_or_generate idempotent behavior
    let loaded_again = CryptoIdentity::load_or_generate(&id_path).unwrap();
    assert_eq!(loaded_again.fingerprint, original.fingerprint);
}

#[test]
fn test_fingerprint_validation_and_normalization() {
    let valid_upper = "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA";
    let valid_lower = "4badde53a7f7cdeeed93189fd898e02bf6b4806ca4c05de0ace08319b86552fa";

    assert!(validate_fingerprint(valid_upper));
    assert!(validate_fingerprint(valid_lower));
    assert_eq!(
        normalize_fingerprint(valid_lower),
        Some(valid_upper.to_string())
    );

    // Invalid length (63 chars)
    assert!(!validate_fingerprint(&valid_upper[..63]));
    assert_eq!(normalize_fingerprint(&valid_upper[..63]), None);

    // Invalid characters (contains 'Z')
    let invalid_chars = "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FZ";
    assert!(!validate_fingerprint(invalid_chars));
    assert_eq!(normalize_fingerprint(invalid_chars), None);

    // Empty string
    assert!(!validate_fingerprint(""));
    assert_eq!(normalize_fingerprint(""), None);
}

#[test]
fn test_fingerprint_matches_delimiter_agnostic() {
    let canonical = "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA";
    let colon_lower = "4b:ad:de:53:a7:f7:cd:ee:ed:93:18:9f:d8:98:e0:2b:f6:b4:80:6c:a4:c0:5d:e0:ac:e0:83:19:b8:65:52:fa";
    let dash_upper = "4B-AD-DE-53-A7-F7-CD-EE-ED-93-18-9F-D8-98-E0-2B-F6-B4-80-6C-A4-C0-5D-E0-AC-E0-83-19-B8-65-52-FA";
    let space_mixed = " 4B AD DE 53 A7 F7 CD EE ED 93 18 9F D8 98 E0 2B F6 B4 80 6C A4 C0 5D E0 AC E0 83 19 B8 65 52 FA \n";

    assert!(fingerprint_matches(canonical, colon_lower));
    assert!(fingerprint_matches(colon_lower, dash_upper));
    assert!(fingerprint_matches(dash_upper, space_mixed));
    assert!(!fingerprint_matches(canonical, "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF"));
}

#[test]
fn test_crypto_identity_atomic_concurrent_read_write() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    let temp_dir = tempfile::tempdir().unwrap();
    let id_path = temp_dir.path().join("concurrent_identity.pem");

    let initial = CryptoIdentity::generate().unwrap();
    initial.save_to_file(&id_path).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let read_corruptions = Arc::new(AtomicUsize::new(0));
    let read_successes = Arc::new(AtomicUsize::new(0));

    // Spawn 4 concurrent reader threads
    let mut readers = vec![];
    for _ in 0..4 {
        let p = id_path.clone();
        let s = stop.clone();
        let ok = read_successes.clone();
        let err = read_corruptions.clone();
        readers.push(thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                match CryptoIdentity::load_from_file(&p) {
                    Ok(id) => {
                        if !id.fingerprint.is_empty() {
                            ok.fetch_add(1, Ordering::Relaxed);
                        } else {
                            err.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        // Truncated, 0-byte, or partial read caught
                        err.fetch_add(1, Ordering::Relaxed);
                    }
                }
                thread::yield_now();
            }
        }));
    }

    // Spawn 1 writer thread repeatedly persisting updated identities
    let p_writer = id_path.clone();
    let s_writer = stop.clone();
    let writer_id = initial.clone();
    let writer = thread::spawn(move || {
        for _ in 0..200 {
            writer_id
                .save_to_file(&p_writer)
                .expect("Save must succeed");
            thread::sleep(Duration::from_millis(1));
        }
        s_writer.store(true, Ordering::Relaxed);
    });

    writer.join().unwrap();
    for r in readers {
        r.join().unwrap();
    }

    let corruptions = read_corruptions.load(Ordering::Relaxed);
    let successes = read_successes.load(Ordering::Relaxed);
    assert_eq!(
        corruptions, 0,
        "Atomic replacement must yield 0 corrupted/empty reads (observed {corruptions} corruptions across {successes} reads)"
    );
    assert!(successes > 0, "Readers must successfully execute reads");
}
