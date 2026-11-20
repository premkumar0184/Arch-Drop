//! Integration test for Arch Drop Phase 4 Milestone 1:
//! Systemd User Service, Daemon Lifecycle, and UNIX Domain Socket IPC.

#[allow(unused_imports)]
#[path = "../crates/archdropd/src/events.rs"]
mod events;

#[allow(unused_imports)]
#[path = "../crates/archdropd/src/notification.rs"]
mod notification;

#[path = "../crates/archdropd/src/ipc.rs"]
mod ipc;

#[allow(dead_code)]
#[path = "../crates/archdropd/src/daemon.rs"]
mod daemon;

use ipc::{DaemonStatusEvent, IpcServer, SocketGuard};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, watch, RwLock};

// =========================================================================
// 1. UNIX Domain Socket Path Resolution & Permissions (0o600 / 0o700)
// =========================================================================

#[test]
fn test_default_socket_path_resolution() {
    let path = IpcServer::default_socket_path();
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap();
    assert_eq!(filename, "archdropd.sock");

    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime.trim().is_empty() {
            assert!(
                path.starts_with(&runtime),
                "Default path should start with XDG_RUNTIME_DIR: {:?}",
                path
            );
        }
    }
}

#[tokio::test]
async fn test_socket_binding_and_permissions_0o600() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_dir = tmp.path().join("archdrop_runtime");
    let sock_path = sock_dir.join("archdropd.sock");

    let (event_tx, _) = broadcast::channel(16);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(0)));

    let server = IpcServer::bind(&sock_path, event_tx, shutdown_rx, status)
        .expect("Failed to bind IPC server");

    assert_eq!(server.socket_path(), sock_path.as_path());
    assert!(sock_path.exists(), "Socket file must exist on disk");

    // Verify parent directory has 0o700 permissions
    let dir_meta = std::fs::metadata(&sock_dir).expect("Failed to read dir metadata");
    let dir_mode = dir_meta.permissions().mode() & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "Parent runtime directory must have 0o700 permissions, got {:o}",
        dir_mode
    );

    // Verify socket file has 0o600 permissions
    let sock_meta = std::fs::metadata(&sock_path).expect("Failed to read socket metadata");
    let sock_mode = sock_meta.permissions().mode() & 0o777;
    assert_eq!(
        sock_mode, 0o600,
        "Socket file must have 0o600 permissions, got {:o}",
        sock_mode
    );
}

// =========================================================================
// 2. RAII SocketGuard Drop Cleanup
// =========================================================================

#[tokio::test]
async fn test_raii_socket_guard_cleanup_on_drop() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("guard_test.sock");

    let (event_tx, _) = broadcast::channel(16);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(0)));

    {
        let server = IpcServer::bind(&sock_path, event_tx, shutdown_rx, status)
            .expect("Failed to bind IPC server");
        assert!(sock_path.exists(), "Socket file must exist while server is in scope");
        // server and its inner SocketGuard drop here
        let _ = server;
    }

    assert!(
        !sock_path.exists(),
        "Socket file must be unlinked automatically when SocketGuard is dropped"
    );

    // Standalone SocketGuard test
    let test_file = tmp.path().join("standalone_guard.test");
    std::fs::write(&test_file, b"test").expect("Failed to create test file");
    assert!(test_file.exists());
    {
        let _guard = SocketGuard::new(test_file.clone());
        assert_eq!(_guard.path(), test_file.as_path());
    }
    assert!(
        !test_file.exists(),
        "Standalone SocketGuard must unlink target file on drop"
    );
}

// =========================================================================
// 3. Stale Socket Recovery & Active Daemon Conflict
// =========================================================================

#[tokio::test]
async fn test_stale_socket_recovery_when_unresponsive() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("stale.sock");

    // Create a dead socket / stale dummy file to simulate an ungraceful crash (kill -9)
    std::fs::write(&sock_path, b"stale data").expect("Failed to write dummy stale socket file");
    assert!(sock_path.exists());

    let (event_tx, _) = broadcast::channel(16);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(0)));

    // Bind should detect the unresponsive socket, safely unlink it, and succeed
    let server = IpcServer::bind(&sock_path, event_tx, shutdown_rx, status);
    assert!(
        server.is_ok(),
        "IpcServer::bind must recover from stale socket file: {:?}",
        server.err()
    );

    let server = server.unwrap();
    assert!(sock_path.exists());
    drop(server);
    assert!(!sock_path.exists());
}

#[tokio::test]
async fn test_active_socket_conflict_rejection() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("active.sock");

    let (event_tx1, _) = broadcast::channel(16);
    let (_shutdown_tx1, shutdown_rx1) = watch::channel(false);
    let status1 = Arc::new(RwLock::new(DaemonStatusEvent::idle(0)));

    let server1 = IpcServer::bind(&sock_path, event_tx1, shutdown_rx1, status1)
        .expect("First bind must succeed");

    // Spawn server1 so it can answer connect probes
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let _ = ready_tx.send(());
        let _ = server1.run().await;
    });
    ready_rx.await.expect("Server 1 task spawned");

    // Second bind attempt on the active socket must fail
    let (event_tx2, _) = broadcast::channel(16);
    let (_shutdown_tx2, shutdown_rx2) = watch::channel(false);
    let status2 = Arc::new(RwLock::new(DaemonStatusEvent::idle(0)));

    let bind_res = IpcServer::bind(&sock_path, event_tx2, shutdown_rx2, status2);
    assert!(
        bind_res.is_err(),
        "Binding to an actively responding socket must fail"
    );

    let err_msg = bind_res.unwrap_err().to_string();
    assert!(
        err_msg.contains("already running"),
        "Error message must indicate daemon is already running: {err_msg}"
    );

    server_task.abort();
}

// =========================================================================
// 4. Client Communication, Commands (Ping/Status) & Event Streaming
// =========================================================================

#[tokio::test]
async fn test_ipc_communication_commands_and_event_streaming() {
    let tmp = TempDir::new().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("ipc_comm.sock");

    let (event_tx, _) = broadcast::channel(32);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(DaemonStatusEvent::idle(2)));

    let server = IpcServer::bind(&sock_path, event_tx.clone(), shutdown_rx, status.clone())
        .expect("Bind failed");

    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });

    // Connect client
    tokio::time::sleep(Duration::from_millis(50)).await;
    let stream = UnixStream::connect(&sock_path)
        .await
        .expect("Client failed to connect to IPC socket");

    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // 1. Initial status snapshot sent automatically on connect
    let initial_line = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected initial status snapshot line");
    let initial_event: DaemonStatusEvent =
        serde_json::from_str(&initial_line).expect("Failed to parse initial snapshot JSON");
    assert_eq!(initial_event.state, "idle");
    assert_eq!(initial_event.active_peers, 2);

    // 2. Send 'ping' command -> expect pong response
    writer
        .write_all(b"ping\n")
        .await
        .expect("Failed to write ping");
    let pong_line = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected pong line");
    let pong_val: serde_json::Value =
        serde_json::from_str(&pong_line).expect("Failed to parse pong JSON");
    assert_eq!(pong_val["type"], "pong");

    // 3. Send 'status' command -> expect current status JSON
    writer
        .write_all(b"status\n")
        .await
        .expect("Failed to write status query");
    let status_line = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected status query response line");
    let status_event: DaemonStatusEvent =
        serde_json::from_str(&status_line).expect("Failed to parse status response JSON");
    assert_eq!(status_event.state, "idle");

    // 4. Stream a transfer event broadcast from daemon
    let transfer_event = DaemonStatusEvent::transferring("test_video.mp4", 55.4, 25_000_000);
    event_tx
        .send(transfer_event.clone())
        .expect("Failed to broadcast transfer event");

    let streamed_line = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected streamed transfer event line");
    let streamed_event: DaemonStatusEvent =
        serde_json::from_str(&streamed_line).expect("Failed to parse streamed event JSON");
    assert_eq!(streamed_event.state, "transferring");
    assert_eq!(streamed_event.percentage, Some(55.4));
    assert_eq!(streamed_event.current_transfer.as_deref(), Some("test_video.mp4"));

    // 5. Stream a paired event broadcast from daemon
    let paired_event = DaemonStatusEvent::paired("Pixel 9 Pro", 1);
    event_tx
        .send(paired_event.clone())
        .expect("Failed to broadcast paired event");

    let paired_line = lines
        .next_line()
        .await
        .expect("Read error")
        .expect("Expected streamed paired event line");
    let received_paired: DaemonStatusEvent =
        serde_json::from_str(&paired_line).expect("Failed to parse streamed paired JSON");
    assert_eq!(received_paired.state, "paired");
    assert!(received_paired.text.contains("Pixel 9 Pro"));

    // Shutdown server
    let _ = shutdown_tx.send(true);
    let _ = server_task.await;
}

// =========================================================================
// 5. Systemd User Service Unit Verification (systemd-analyze)
// =========================================================================

#[test]
fn test_systemd_user_service_unit_file_directives_and_verification() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let unit_path = manifest_dir.join("../../systemd/user/archdropd.service");
    assert!(unit_path.exists(), "Unit file must exist at {:?}", unit_path);

    let content = std::fs::read_to_string(&unit_path)
        .expect("Failed to read archdropd.service");

    // Verify key directives
    assert!(content.contains("Description=Arch Drop Headless Daemon"));
    assert!(content.contains("PartOf=graphical-session.target"));
    assert!(content.contains("After=graphical-session.target network.target"));
    assert!(content.contains("Restart=on-failure"));
    assert!(content.contains("RestartSec=3"));
    assert!(content.contains("RuntimeDirectory=archdrop"));
    assert!(content.contains("RuntimeDirectoryMode=0700"));
    assert!(content.contains("ProtectSystem=strict"));
    assert!(content.contains("ProtectHome=read-only"));
    assert!(content.contains("ReadWritePaths=-%h/Downloads -%h/.config/archdrop %t/archdrop"));
    assert!(content.contains("PrivateTmp=true"));
    assert!(content.contains("RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK"));
    assert!(content.contains("MemoryDenyWriteExecute=true"));
    assert!(content.contains("LockPersonality=true"));
    assert!(content.contains("SystemCallArchitectures=native"));
    assert!(content.contains("WantedBy=graphical-session.target"));

    // Verify with systemd-analyze --user verify
    // Ensure target binary path exists so systemd-analyze does not report "command not found"
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/prem".to_string());
    let cargo_bin = PathBuf::from(&home).join(".cargo").join("bin");
    let archdropd_bin = cargo_bin.join("archdropd");
    let created_dummy = if !archdropd_bin.exists() {
        let _ = std::fs::create_dir_all(&cargo_bin);
        let _ = std::fs::write(&archdropd_bin, b"#!/bin/sh\nexit 0\n");
        let _ = std::fs::set_permissions(&archdropd_bin, std::fs::Permissions::from_mode(0o755));
        true
    } else {
        false
    };

    let output = Command::new("systemd-analyze")
        .args(["--user", "verify", unit_path.to_str().unwrap()])
        .output();

    if created_dummy {
        let _ = std::fs::remove_file(&archdropd_bin);
    }

    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "systemd-analyze --user verify failed with exit code {:?}\nstdout: {}\nstderr: {}",
                out.status.code(),
                stdout,
                stderr
            );
        }
        Err(e) => {
            println!("systemd-analyze not present on host or failed to run: {e}");
        }
    }
}

// =========================================================================
// 6. Host WireGuard Invariant Audit
// =========================================================================

#[test]
fn test_wireguard_safety_invariant_untouched() {
    let output = Command::new("ip")
        .args(["addr", "show", "tiktok"])
        .output()
        .expect("Failed to execute ip addr");

    assert!(output.status.success(), "ip addr show tiktok failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("10.2.0.2/32"),
        "WireGuard interface tiktok must maintain 10.2.0.2/32. Output:\n{}",
        stdout
    );

    let rule_output = Command::new("ip")
        .args(["rule", "show"])
        .output()
        .expect("Failed to execute ip rule");

    assert!(rule_output.status.success(), "ip rule show failed");
    let rule_stdout = String::from_utf8_lossy(&rule_output.stdout);
    assert!(
        rule_stdout.contains("51820"),
        "Routing table rule 51820 must remain configured. Output:\n{}",
        rule_stdout
    );
}

// =========================================================================
// 7. Lifecycle Scripts Exit Code & Option Validation
// =========================================================================

#[test]
fn test_lifecycle_scripts_exit_code_on_invalid_arguments() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir.join("../..");

    // 1. install-service.sh --invalid-flag -> exit 1
    let out = Command::new("bash")
        .arg(project_root.join("scripts/install-service.sh"))
        .arg("--invalid-flag")
        .output()
        .expect("Failed to run install-service.sh");
    assert_eq!(
        out.status.code(),
        Some(1),
        "install-service.sh with invalid flag must exit with code 1"
    );

    // 2. uninstall-service.sh --bogus -> exit 1
    let out = Command::new("bash")
        .arg(project_root.join("scripts/uninstall-service.sh"))
        .arg("--bogus")
        .output()
        .expect("Failed to run uninstall-service.sh");
    assert_eq!(
        out.status.code(),
        Some(1),
        "uninstall-service.sh with invalid flag must exit with code 1"
    );

    // 3. manage-service.sh invalid-cmd -> exit 1
    let out = Command::new("bash")
        .arg(project_root.join("scripts/manage-service.sh"))
        .arg("invalid-cmd")
        .output()
        .expect("Failed to run manage-service.sh");
    assert_eq!(
        out.status.code(),
        Some(1),
        "manage-service.sh with invalid command must exit with code 1"
    );

    // 4. manage-service.sh (no args) -> exit 1
    let out = Command::new("bash")
        .arg(project_root.join("scripts/manage-service.sh"))
        .output()
        .expect("Failed to run manage-service.sh");
    assert_eq!(
        out.status.code(),
        Some(1),
        "manage-service.sh with no args must exit with code 1"
    );

    // 5. manage-service.sh logs --invalid -> exit 1
    let out = Command::new("bash")
        .arg(project_root.join("scripts/manage-service.sh"))
        .args(["logs", "--invalid"])
        .output()
        .expect("Failed to run manage-service.sh logs");
    assert_eq!(
        out.status.code(),
        Some(1),
        "manage-service.sh logs with invalid flag must exit with code 1"
    );
}

#[test]
fn test_lifecycle_scripts_exit_code_on_help() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir.join("../..");

    for script in ["install-service.sh", "uninstall-service.sh", "manage-service.sh"] {
        let out = Command::new("bash")
            .arg(project_root.join("scripts").join(script))
            .arg("--help")
            .output()
            .unwrap_or_else(|e| panic!("Failed to run {script}: {e}"));
        assert_eq!(
            out.status.code(),
            Some(0),
            "{script} --help must exit with code 0"
        );
    }
}

#[test]
fn test_manage_service_logs_no_follow() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest_dir.join("../../scripts/manage-service.sh");

    let out = Command::new("bash")
        .arg(script)
        .args(["logs", "--no-follow"])
        .output()
        .expect("Failed to run manage-service.sh logs --no-follow");
    assert_eq!(
        out.status.code(),
        Some(0),
        "manage-service.sh logs --no-follow must exit with code 0 without blocking"
    );
}

// =========================================================================
// 8. PKGBUILD Directory Resolution
// =========================================================================

#[test]
fn test_pkgbuild_directory_resolution() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pkgbuild_dir = manifest_dir.join("../../packaging/arch");
    let pkgbuild_path = pkgbuild_dir.join("PKGBUILD");
    assert!(pkgbuild_path.exists(), "PKGBUILD must exist");

    let content = std::fs::read_to_string(&pkgbuild_path).expect("Failed to read PKGBUILD");
    assert!(
        content.contains(r#"cd "${startdir}/../..""#),
        "PKGBUILD must resolve root via startdir/../.."
    );
    assert!(
        !content.contains(r#"cd "${startdir:-../..}""#),
        "PKGBUILD must not use startdir:-../.. parameter expansion trap"
    );

    // Verify navigating two levels up from packaging/arch resolves to workspace root
    let root = pkgbuild_dir.join("../..").canonicalize().expect("Canonicalize root");
    assert!(root.join("Cargo.toml").exists(), "Project root must have Cargo.toml");
    assert!(root.join("systemd/user/archdropd.service").exists(), "Project root must have systemd unit template");
    assert!(root.join("README.md").exists(), "Project root must have README.md");
}

// =========================================================================
// 9. Early Signal Registration Invariant
// =========================================================================

#[tokio::test]
async fn test_early_signal_registration_succeeds() {
    let signals = daemon::ShutdownSignals::register();
    assert!(
        signals.is_ok(),
        "ShutdownSignals::register must successfully bind SIGTERM, SIGINT, SIGHUP: {:?}",
        signals.err()
    );
}
