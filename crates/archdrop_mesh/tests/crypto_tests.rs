use archdrop_mesh::crypto::{MeshKeyPair, MeshPublicKey, MAX_HANDSHAKE_SKEW_SECS};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn test_mesh_key_generation_and_properties() {
    let keypair = MeshKeyPair::generate().expect("Key generation must succeed");
    let pubkey = keypair.public_key();

    assert_eq!(pubkey.as_bytes().len(), 32);

    let hex_str = pubkey.to_hex();
    assert_eq!(hex_str.len(), 64);
    assert!(hex_str.chars().all(|c| c.is_ascii_hexdigit()));

    let parsed = MeshPublicKey::from_hex(&hex_str).expect("Parsing hex must succeed");
    assert_eq!(parsed, pubkey);

    // Formatter checks
    assert_eq!(format!("{pubkey}"), hex_str);
    assert!(format!("{pubkey:?}").contains(&hex_str));

    // Serde roundtrip
    let json = serde_json::to_string(&pubkey).expect("Serialization must succeed");
    let deser: MeshPublicKey = serde_json::from_str(&json).expect("Deserialization must succeed");
    assert_eq!(deser, pubkey);
}

#[test]
fn test_sign_and_verify_arbitrary_message() {
    let keypair = MeshKeyPair::generate().unwrap();
    let pubkey = keypair.public_key();

    let msg = b"Important Arch Drop mesh control message";
    let sig = keypair.sign(msg);

    assert_eq!(sig.len(), 64);

    // Valid verification
    pubkey
        .verify(msg, &sig)
        .expect("Valid signature must verify");

    // Tampered message
    let tampered_msg = b"Tampered Arch Drop mesh control message";
    assert!(
        pubkey.verify(tampered_msg, &sig).is_err(),
        "Tampered message must fail verification"
    );

    // Tampered signature
    let mut tampered_sig = sig;
    tampered_sig[0] ^= 0xff;
    assert!(
        pubkey.verify(msg, &tampered_sig).is_err(),
        "Tampered signature must fail verification"
    );

    // Wrong public key
    let other_keypair = MeshKeyPair::generate().unwrap();
    assert!(
        other_keypair.public_key().verify(msg, &sig).is_err(),
        "Wrong public key must fail verification"
    );
}

#[test]
fn test_challenge_protocol_authentication() {
    let keypair = MeshKeyPair::generate().unwrap();
    let pubkey = keypair.public_key();
    let server_key = [0x55u8; 32];

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let sig = keypair.sign_challenge(&server_key, now);

    // Normal verification within skew window
    MeshKeyPair::verify_challenge(&pubkey, &server_key, now, &sig, MAX_HANDSHAKE_SKEW_SECS)
        .expect("Challenge verification must succeed");

    // Mismatched server key
    let wrong_server_key = [0x66u8; 32];
    assert!(
        MeshKeyPair::verify_challenge(
            &pubkey,
            &wrong_server_key,
            now,
            &sig,
            MAX_HANDSHAKE_SKEW_SECS
        )
        .is_err(),
        "Mismatched server key must fail challenge verification"
    );
}

#[test]
fn test_challenge_timestamp_skew() {
    let keypair = MeshKeyPair::generate().unwrap();
    let pubkey = keypair.public_key();
    let server_key = [0x77u8; 32];

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Expired timestamp (120 seconds in past)
    let expired_ts = now - 120;
    let sig_expired = keypair.sign_challenge(&server_key, expired_ts);
    let err_past = MeshKeyPair::verify_challenge(
        &pubkey,
        &server_key,
        expired_ts,
        &sig_expired,
        MAX_HANDSHAKE_SKEW_SECS,
    )
    .unwrap_err();
    assert!(
        err_past.to_string().contains("skew"),
        "Past skew should be rejected: {err_past}"
    );

    // Future timestamp (120 seconds in future)
    let future_ts = now + 120;
    let sig_future = keypair.sign_challenge(&server_key, future_ts);
    let err_future = MeshKeyPair::verify_challenge(
        &pubkey,
        &server_key,
        future_ts,
        &sig_future,
        MAX_HANDSHAKE_SKEW_SECS,
    )
    .unwrap_err();
    assert!(
        err_future.to_string().contains("skew"),
        "Future skew should be rejected: {err_future}"
    );
}

#[test]
fn test_keypair_pkcs8_roundtrip() {
    let keypair = MeshKeyPair::generate().unwrap();
    let pubkey = keypair.public_key();
    let pkcs8 = keypair.pkcs8_bytes();

    let reloaded = MeshKeyPair::from_pkcs8(pkcs8).expect("PKCS#8 reload must succeed");
    assert_eq!(reloaded.public_key(), pubkey);

    let msg = b"Testing PKCS#8 serialization roundtrip";
    let sig1 = keypair.sign(msg);
    let sig2 = reloaded.sign(msg);

    assert_eq!(sig1, sig2, "Deterministic signatures must match");
    pubkey.verify(msg, &sig2).unwrap();
}

#[test]
fn test_keypair_file_persistence_mode_0o600() {
    let temp_dir = tempfile::tempdir().unwrap();
    let key_path = temp_dir.path().join("secure_keys").join("mesh_key.pk8");

    let keypair = MeshKeyPair::generate().unwrap();
    keypair.save_to_file(&key_path).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&key_path).unwrap();
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "Private key file must have 0o600 permissions"
        );
    }

    let loaded = MeshKeyPair::load_from_file(&key_path).unwrap();
    assert_eq!(loaded.public_key(), keypair.public_key());

    // load_or_generate when already present
    let reloaded = MeshKeyPair::load_or_generate(&key_path).unwrap();
    assert_eq!(reloaded.public_key(), keypair.public_key());
}
