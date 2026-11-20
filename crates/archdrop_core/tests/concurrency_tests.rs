//! Concurrency and regression test suite for `archdrop_core`.
//!
//! Validates:
//! 1. 10-thread concurrent configuration persistence without PID collisions or partial reads.
//! 2. 10-thread concurrent cryptographic identity saves without empty/zero-byte read corruptions.
//! 3. Initial alias consistency and graceful hostname fallback.
//! 4. Peer fingerprint delimiter stripping (colons, dashes, whitespace) and agnostic pairing.

use archdrop_core::config::{AppConfig, Config};
use archdrop_core::crypto::{normalize_fingerprint, CryptoIdentity};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// ============================================================================
// 1. CONCURRENT CONFIG PERSISTENCE TESTS (10 THREADS)
// ============================================================================

/// Verifies that 10 concurrent threads updating and reading configuration via
/// `AppConfig::save_to_file` / `app_config.save()` and `AppConfig::load_from_file` / `Config::load_from_path`
/// encounter 0 collisions, 0 partial reads, and maintain strict 0o600 permissions.
#[test]
fn test_concurrent_multithreaded_config_writers_and_readers() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary test directory");
    let config_path = temp_dir.path().join("concurrent_config.toml");

    // Initialize with a valid default configuration
    let initial_config = Config::default();
    initial_config
        .save_to_path(&config_path)
        .expect("Initial config save must succeed");

    #[cfg(unix)]
    {
        let meta = std::fs::metadata(&config_path).expect("Metadata query failed");
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "Initial config must have 0o600 permissions"
        );
    }

    const TOTAL_THREADS: usize = 10;
    const WRITER_COUNT: usize = 5;
    const WRITER_ITERATIONS: usize = 30;

    let barrier = Arc::new(Barrier::new(TOTAL_THREADS));
    let stop_readers = Arc::new(AtomicBool::new(false));
    let write_errors = Arc::new(AtomicUsize::new(0));
    let read_errors = Arc::new(AtomicUsize::new(0));
    let read_successes = Arc::new(AtomicUsize::new(0));
    let error_messages = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::with_capacity(TOTAL_THREADS);

    // Spawn 5 Writer Threads
    for thread_id in 0..WRITER_COUNT {
        let path = config_path.clone();
        let b = barrier.clone();
        let w_err = write_errors.clone();
        let err_log = error_messages.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            for i in 0..WRITER_ITERATIONS {
                let mut app_config = match AppConfig::load_or_create(&path) {
                    Ok(ac) => ac,
                    Err(e) => {
                        w_err.fetch_add(1, Ordering::Relaxed);
                        err_log
                            .lock()
                            .unwrap()
                            .push(format!("Writer {thread_id} load error at iter {i}: {e:?}"));
                        continue;
                    }
                };

                // Mutate distinct fields
                app_config.config_mut().device.alias =
                    Some(format!("worker-thread-{thread_id}-iter-{i}"));
                app_config.config_mut().network.port = (40000 + thread_id * 100 + i) as u16;

                // Add distinct fingerprint
                let fp = format!(
                    "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552{:02X}",
                    (thread_id * 30 + i) % 256
                );
                app_config.config_mut().add_paired_fingerprint(fp);

                if let Err(e) = app_config.save() {
                    w_err.fetch_add(1, Ordering::Relaxed);
                    err_log
                        .lock()
                        .unwrap()
                        .push(format!("Writer {thread_id} save error at iter {i}: {e:?}"));
                }

                #[cfg(unix)]
                {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        assert_eq!(
                            meta.permissions().mode() & 0o777,
                            0o600,
                            "File mode must remain 0o600 during concurrent writes"
                        );
                    }
                }

                thread::yield_now();
            }
        }));
    }

    // Spawn 5 Reader Threads
    for thread_id in WRITER_COUNT..TOTAL_THREADS {
        let path = config_path.clone();
        let b = barrier.clone();
        let stop = stop_readers.clone();
        let r_err = read_errors.clone();
        let r_ok = read_successes.clone();
        let err_log = error_messages.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            while !stop.load(Ordering::Relaxed) {
                match Config::load_from_path(&path) {
                    Ok(cfg) => {
                        // Integrity checks: port must be non-zero, alias must not be blank
                        if cfg.network.port >= 40000 || cfg.network.port == 53317 {
                            r_ok.fetch_add(1, Ordering::Relaxed);
                        } else {
                            r_err.fetch_add(1, Ordering::Relaxed);
                            err_log.lock().unwrap().push(format!(
                                "Reader {thread_id} observed invalid port: {}",
                                cfg.network.port
                            ));
                        }
                    }
                    Err(e) => {
                        // Partial write or corrupt read!
                        r_err.fetch_add(1, Ordering::Relaxed);
                        err_log
                            .lock()
                            .unwrap()
                            .push(format!("Reader {thread_id} parse error: {e:?}"));
                    }
                }
                thread::yield_now();
            }
        }));
    }

    // Wait for writers to complete
    for h in handles.drain(..WRITER_COUNT) {
        h.join().expect("Writer thread panicked");
    }

    // Signal readers to stop and join them
    stop_readers.store(true, Ordering::Relaxed);
    for h in handles {
        h.join().expect("Reader thread panicked");
    }

    let total_w_errors = write_errors.load(Ordering::SeqCst);
    let total_r_errors = read_errors.load(Ordering::SeqCst);
    let total_r_success = read_successes.load(Ordering::SeqCst);
    let errors = error_messages.lock().unwrap();

    println!(
        "[CONFIG CONCURRENCY] Writes: 150, Write Errors: {total_w_errors}, Successful Reads: {total_r_success}, Read Errors: {total_r_errors}"
    );

    assert_eq!(
        total_w_errors, 0,
        "Concurrent writers collided on tmpfile or failed rename: {:?}",
        *errors
    );
    assert_eq!(
        total_r_errors, 0,
        "Concurrent readers observed partial/corrupted files: {:?}",
        *errors
    );
    assert!(
        total_r_success > 50,
        "Expected readers to complete significant reads, got: {total_r_success}"
    );

    // Final file verification
    let final_config = Config::load_from_path(&config_path).expect("Final config must be valid");
    assert!(
        final_config.network.port >= 40000,
        "Final port must reflect thread mutations"
    );

    #[cfg(unix)]
    {
        let meta = std::fs::metadata(&config_path).unwrap();
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "Final file permissions must be 0o600"
        );
    }
}

// ============================================================================
// 2. CONCURRENT CRYPTO PERSISTENCE TESTS (10 THREADS)
// ============================================================================

/// Verifies that 10 concurrent threads saving and reading cryptographic identities
/// via `CryptoIdentity::save_to_file` and `CryptoIdentity::load_from_file`
/// encounter 0 empty/corrupted reads and enforce strict 0o600 permissions.
#[test]
fn test_concurrent_multithreaded_crypto_save_and_read() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary test directory");
    let key_path = temp_dir.path().join("identity.pem");

    // Pre-generate 2 distinct identities to avoid expensive RSA prime generation inside threads
    let id_a = CryptoIdentity::generate().expect("Failed to generate identity A");
    let id_b = CryptoIdentity::generate().expect("Failed to generate identity B");

    // Seed file with identity A
    id_a.save_to_file(&key_path)
        .expect("Initial identity save must succeed");

    #[cfg(unix)]
    {
        let meta = std::fs::metadata(&key_path).expect("Metadata query failed");
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "Initial identity.pem must have 0o600 permissions"
        );
    }

    const TOTAL_THREADS: usize = 10;
    const WRITER_COUNT: usize = 5;
    const WRITER_ITERATIONS: usize = 30;

    let barrier = Arc::new(Barrier::new(TOTAL_THREADS));
    let stop_readers = Arc::new(AtomicBool::new(false));
    let write_errors = Arc::new(AtomicUsize::new(0));
    let read_corruptions = Arc::new(AtomicUsize::new(0));
    let read_successes = Arc::new(AtomicUsize::new(0));
    let error_messages = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::with_capacity(TOTAL_THREADS);

    // Spawn 5 Writer Threads
    for thread_id in 0..WRITER_COUNT {
        let path = key_path.clone();
        let b = barrier.clone();
        let w_err = write_errors.clone();
        let err_log = error_messages.clone();
        let cand_a = id_a.clone();
        let cand_b = id_b.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            for i in 0..WRITER_ITERATIONS {
                let to_save = if (thread_id + i) % 2 == 0 {
                    &cand_a
                } else {
                    &cand_b
                };

                if let Err(e) = to_save.save_to_file(&path) {
                    w_err.fetch_add(1, Ordering::Relaxed);
                    err_log
                        .lock()
                        .unwrap()
                        .push(format!("Writer {thread_id} save error at iter {i}: {e:?}"));
                }

                #[cfg(unix)]
                {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        assert_eq!(
                            meta.permissions().mode() & 0o777,
                            0o600,
                            "File mode must remain 0o600 during concurrent crypto saves"
                        );
                    }
                }

                thread::sleep(Duration::from_millis(1));
                thread::yield_now();
            }
        }));
    }

    // Spawn 5 Reader Threads
    for thread_id in WRITER_COUNT..TOTAL_THREADS {
        let path = key_path.clone();
        let b = barrier.clone();
        let stop = stop_readers.clone();
        let r_corrupt = read_corruptions.clone();
        let r_ok = read_successes.clone();
        let err_log = error_messages.clone();
        let fp_a = id_a.fingerprint.clone();
        let fp_b = id_b.fingerprint.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            while !stop.load(Ordering::Relaxed) {
                match CryptoIdentity::load_from_file(&path) {
                    Ok(loaded) => {
                        // Integrity verification: loaded fingerprint must match id_a or id_b exactly
                        if loaded.fingerprint == fp_a || loaded.fingerprint == fp_b {
                            if !loaded.certificate_der.is_empty()
                                && !loaded.private_key_der.is_empty()
                            {
                                r_ok.fetch_add(1, Ordering::Relaxed);
                            } else {
                                r_corrupt.fetch_add(1, Ordering::Relaxed);
                                err_log.lock().unwrap().push(format!(
                                    "Reader {thread_id} found empty DER buffers in identity"
                                ));
                            }
                        } else {
                            r_corrupt.fetch_add(1, Ordering::Relaxed);
                            err_log.lock().unwrap().push(format!(
                                "Reader {thread_id} found unknown fingerprint: {}",
                                loaded.fingerprint
                            ));
                        }
                    }
                    Err(e) => {
                        // Truncated / empty read corruption!
                        r_corrupt.fetch_add(1, Ordering::Relaxed);
                        err_log
                            .lock()
                            .unwrap()
                            .push(format!("Reader {thread_id} parse error: {e:?}"));
                    }
                }
                thread::yield_now();
            }
        }));
    }

    // Wait for writers to complete
    for h in handles.drain(..WRITER_COUNT) {
        h.join().expect("Crypto writer thread panicked");
    }

    // Signal readers to stop and join them
    stop_readers.store(true, Ordering::Relaxed);
    for h in handles {
        h.join().expect("Crypto reader thread panicked");
    }

    let total_w_errors = write_errors.load(Ordering::SeqCst);
    let total_r_corrupt = read_corruptions.load(Ordering::SeqCst);
    let total_r_success = read_successes.load(Ordering::SeqCst);
    let errors = error_messages.lock().unwrap();

    println!(
        "[CRYPTO CONCURRENCY] Saves: 150, Save Errors: {total_w_errors}, Successful Reads: {total_r_success}, Corrupt Reads: {total_r_corrupt}"
    );

    assert_eq!(
        total_w_errors, 0,
        "Concurrent crypto saves failed: {:?}",
        *errors
    );
    assert_eq!(
        total_r_corrupt, 0,
        "Concurrent crypto readers observed zero-byte/truncated files: {:?}",
        *errors
    );
    assert!(
        total_r_success > 0,
        "Expected readers to complete significant reads, got: {total_r_success}"
    );

    // Final load verification
    let final_id =
        CryptoIdentity::load_from_file(&key_path).expect("Final identity must load cleanly");
    assert!(
        final_id.fingerprint == id_a.fingerprint || final_id.fingerprint == id_b.fingerprint,
        "Final fingerprint must match either seed identity"
    );

    #[cfg(unix)]
    {
        let meta = std::fs::metadata(&key_path).unwrap();
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "Final identity.pem permissions must be 0o600"
        );
    }
}

// ============================================================================
// 3. INITIAL ALIAS & HOSTNAME FALLBACK TESTS
// ============================================================================

/// Tests that `effective_alias()` resolves gracefully across all fallback scenarios,
/// never panicking or returning an empty string.
#[test]
fn test_effective_alias_graceful_fallbacks() {
    let mut conf = Config::default();

    // 1. Explicit configured alias
    conf.device.alias = Some("Custom Studio Laptop".to_string());
    assert_eq!(conf.effective_alias(), "Custom Studio Laptop");

    // 2. Whitespace-only alias must be trimmed and fallback
    conf.device.alias = Some("   \t  \n ".to_string());
    let fallback_ws = conf.effective_alias();
    assert!(
        !fallback_ws.trim().is_empty(),
        "Whitespace alias must fall back to hostname/user/device"
    );

    // 3. None alias must fall back gracefully
    conf.device.alias = None;
    let fallback_none = conf.effective_alias();
    assert!(
        !fallback_none.trim().is_empty(),
        "None alias must fall back to hostname/user/device"
    );

    // 4. In all circumstances, alias is non-empty
    assert!(
        fallback_none == fallback_ws,
        "None and whitespace-only aliases must resolve to the identical fallback"
    );
}

/// Tests that `AppConfig::load_or_create` produces an effective alias that is
/// identical to the effective alias produced by subsequent `load_from_path` calls.
#[test]
fn test_load_or_create_alias_consistency() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("initial_archdrop_config.toml");

    // Initial creation
    let app_config_initial =
        AppConfig::load_or_create(&config_path).expect("Initial load_or_create must succeed");
    let initial_alias = app_config_initial.config().effective_alias();

    // Subsequent reload
    let app_config_reloaded =
        Config::load_from_path(&config_path).expect("Reload from path must succeed");
    let reloaded_alias = app_config_reloaded.effective_alias();

    assert_eq!(
        initial_alias, reloaded_alias,
        "Initial load_or_create and subsequent load_from_path must resolve the identical effective alias"
    );
}

// ============================================================================
// 4. PEER FINGERPRINT DELIMITER NORMALIZATION TESTS
// ============================================================================

/// Tests that fingerprints with colons, dashes, spaces, and lowercase hex
/// are cleanly normalized to the canonical 64-character uppercase hex format.
#[test]
fn test_fingerprint_normalization_delimiters() {
    let canonical = "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA";
    let canonical_lower = "4badde53a7f7cdeeed93189fd898e02bf6b4806ca4c05de0ace08319b86552fa";

    // 1. Plain 64-character hex strings
    assert_eq!(
        normalize_fingerprint(canonical),
        Some(canonical.to_string())
    );
    assert_eq!(
        normalize_fingerprint(canonical_lower),
        Some(canonical.to_string())
    );

    // 2. Colon-delimited format (OpenSSL / Wireshark style: 95 chars)
    let colon_delimited =
        "4B:AD:DE:53:A7:F7:CD:EE:ED:93:18:9F:D8:98:E0:2B:F6:B4:80:6C:A4:C0:5D:E0:AC:E0:83:19:B8:65:52:FA";
    let colon_lower =
        "4b:ad:de:53:a7:f7:cd:ee:ed:93:18:9f:d8:98:e0:2b:f6:b4:80:6c:a4:c0:5d:e0:ac:e0:83:19:b8:65:52:fa";
    assert_eq!(
        normalize_fingerprint(colon_delimited),
        Some(canonical.to_string()),
        "Colon-delimited uppercase must normalize to canonical"
    );
    assert_eq!(
        normalize_fingerprint(colon_lower),
        Some(canonical.to_string()),
        "Colon-delimited lowercase must normalize to canonical"
    );

    // 3. Dash-delimited format (Windows / WireGuard style)
    let dash_delimited =
        "4B-AD-DE-53-A7-F7-CD-EE-ED-93-18-9F-D8-98-E0-2B-F6-B4-80-6C-A4-C0-5D-E0-AC-E0-83-19-B8-65-52-FA";
    assert_eq!(
        normalize_fingerprint(dash_delimited),
        Some(canonical.to_string()),
        "Dash-delimited fingerprint must normalize to canonical"
    );

    // 4. Space-delimited format
    let space_delimited =
        "4B AD DE 53 A7 F7 CD EE ED 93 18 9F D8 98 E0 2B F6 B4 80 6C A4 C0 5D E0 AC E0 83 19 B8 65 52 FA";
    assert_eq!(
        normalize_fingerprint(space_delimited),
        Some(canonical.to_string()),
        "Space-delimited fingerprint must normalize to canonical"
    );

    // 5. Surrounding and internal mixed whitespace
    let mixed_whitespace =
        "  4B:AD:DE:53:A7:F7:CD:EE:ED:93:18:9F:D8:98:E0:2B:F6:B4:80:6C:A4:C0:5D:E0:AC:E0:83:19:B8:65:52:FA \n\t";
    assert_eq!(
        normalize_fingerprint(mixed_whitespace),
        Some(canonical.to_string())
    );

    // 6. Invalid fingerprints
    // Too short (63 chars)
    assert_eq!(normalize_fingerprint(&canonical[..63]), None);
    // Too long (65 chars)
    let too_long = format!("{canonical}A");
    assert_eq!(normalize_fingerprint(&too_long), None);
    // Non-hexadecimal characters
    let non_hex = "4B:AD:DE:53:A7:F7:CD:EE:ED:93:18:9F:D8:98:E0:2B:F6:B4:80:6C:A4:C0:5D:E0:AC:E0:83:19:B8:65:52:ZZ";
    assert_eq!(normalize_fingerprint(non_hex), None);
}

/// Tests that `Config::is_peer_paired`, `Config::add_paired_fingerprint`, and
/// `Config::remove_paired_fingerprint` operate in a delimiter-agnostic and case-insensitive manner.
#[test]
fn test_config_is_peer_paired_delimiter_agnostic() {
    let mut cfg = Config::default();
    let colon_fp =
        "4B:AD:DE:53:A7:F7:CD:EE:ED:93:18:9F:D8:98:E0:2B:F6:B4:80:6C:A4:C0:5D:E0:AC:E0:83:19:B8:65:52:FA";
    let plain_lower = "4badde53a7f7cdeeed93189fd898e02bf6b4806ca4c05de0ace08319b86552fa";
    let dash_fp =
        "4b-ad-de-53-a7-f7-cd-ee-ed-93-18-9f-d8-98-e0-2b-f6-b4-80-6c-a4-c0-5d-e0-ac-e0-83-19-b8-65-52-fa";
    let space_fp =
        "4B AD DE 53 A7 F7 CD EE ED 93 18 9F D8 98 E0 2B F6 B4 80 6C A4 C0 5D E0 AC E0 83 19 B8 65 52 FA";

    // Add in colon format
    assert!(
        cfg.add_paired_fingerprint(colon_fp.to_string()),
        "Adding colon-delimited fingerprint must succeed"
    );

    // Duplicate check with lowercase plain
    assert!(
        !cfg.add_paired_fingerprint(plain_lower.to_string()),
        "Adding duplicate fingerprint in lowercase plain format must return false"
    );

    // Agnostic query lookups
    assert!(
        cfg.is_peer_paired(colon_fp),
        "Lookup with colon format must match"
    );
    assert!(
        cfg.is_peer_paired(plain_lower),
        "Lookup with plain lowercase must match"
    );
    assert!(
        cfg.is_peer_paired(dash_fp),
        "Lookup with dash format must match"
    );
    assert!(
        cfg.is_peer_paired(space_fp),
        "Lookup with space format must match"
    );
    assert!(
        !cfg.is_peer_paired("00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF"),
        "Unpaired fingerprint must return false"
    );

    // Removal using dash-delimited representation
    assert!(
        cfg.remove_paired_fingerprint(dash_fp),
        "Removing via dash-delimited format must succeed"
    );

    // Verified removed across all representations
    assert!(!cfg.is_peer_paired(colon_fp));
    assert!(!cfg.is_peer_paired(plain_lower));
}
