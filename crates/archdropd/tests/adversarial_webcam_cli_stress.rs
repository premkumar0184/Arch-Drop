//! Adversarial CLI stress tests for `archdropd webcam` subcommands.

use archdropd::webcam::{run_webcam, WebcamArgs, WebcamSubcommands};
use archdrop_webcam::MODPROBE_DIAGNOSTIC;
use std::path::PathBuf;

#[tokio::test]
async fn test_adversarial_cli_status_text_and_json() {
    // 1. Status Text Mode
    let text_args = WebcamArgs {
        sub: WebcamSubcommands::Status { json: false },
    };
    let res = run_webcam(text_args).await;
    assert!(res.is_ok());

    // 2. Status JSON Mode
    let json_args = WebcamArgs {
        sub: WebcamSubcommands::Status { json: true },
    };
    let res_json = run_webcam(json_args).await;
    assert!(res_json.is_ok());
}

#[tokio::test]
async fn test_adversarial_cli_start_missing_device_error_handling() {
    let empty_dir = tempfile::tempdir().unwrap();
    std::env::set_var("ARCHDROP_SYSFS_ROOT", empty_dir.path());
    // Start without devices should fail with the modprobe diagnostic when loopback not loaded
    let start_args = WebcamArgs {
        sub: WebcamSubcommands::Start {
            device: None,
            width: 1280,
            height: 720,
        },
    };
    let res = run_webcam(start_args).await;
    std::env::remove_var("ARCHDROP_SYSFS_ROOT");
    assert!(res.is_err());
    let err_msg = format!("{:#}", res.unwrap_err());
    assert!(
        err_msg.contains(MODPROBE_DIAGNOSTIC),
        "Error message must contain modprobe diagnostic string: {err_msg}"
    );
}

#[tokio::test]
async fn test_adversarial_cli_start_invalid_paths() {
    // 1. Nonexistent path
    let args1 = WebcamArgs {
        sub: WebcamSubcommands::Start {
            device: Some(PathBuf::from("/tmp/nonexistent_device_node_archdrop_test")),
            width: 1280,
            height: 720,
        },
    };
    let res1 = run_webcam(args1).await;
    assert!(res1.is_err());
    let err1 = format!("{:#}", res1.unwrap_err());
    assert!(err1.contains("Failed to open V4L2 device"));

    // 2. Device path that is not a V4L2 device (/dev/null)
    let args2 = WebcamArgs {
        sub: WebcamSubcommands::Start {
            device: Some(PathBuf::from("/dev/null")),
            width: 1280,
            height: 720,
        },
    };
    let res2 = run_webcam(args2).await;
    assert!(res2.is_err());
    let err2 = format!("{:#}", res2.unwrap_err());
    assert!(
        err2.contains("VIDIOC_QUERYCAP") || err2.contains("Inappropriate ioctl"),
        "Expected VIDIOC_QUERYCAP ioctl failure on /dev/null, got: {err2}"
    );
}

#[tokio::test]
async fn test_adversarial_cli_stop_resilience() {
    // Stop must always succeed even when no device is actively streaming
    let stop_args = WebcamArgs {
        sub: WebcamSubcommands::Stop,
    };
    let res = run_webcam(stop_args).await;
    assert!(res.is_ok(), "Stop command should be resilient: {:?}", res);
}
