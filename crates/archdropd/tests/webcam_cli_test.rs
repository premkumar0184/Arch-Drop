use archdropd::webcam::{run_webcam, WebcamArgs, WebcamSubcommands};
use std::path::PathBuf;

#[tokio::test]
async fn test_webcam_status_cli_command() {
    // Test text status output
    let text_args = WebcamArgs {
        sub: WebcamSubcommands::Status { json: false },
    };
    let res = run_webcam(text_args).await;
    assert!(res.is_ok(), "webcam status command failed: {:?}", res);

    // Test JSON status output
    let json_args = WebcamArgs {
        sub: WebcamSubcommands::Status { json: true },
    };
    let res_json = run_webcam(json_args).await;
    assert!(res_json.is_ok(), "webcam status --json command failed: {:?}", res_json);
}

#[tokio::test]
async fn test_webcam_start_cli_nonexistent_device_handling() {
    let args = WebcamArgs {
        sub: WebcamSubcommands::Start {
            device: Some(PathBuf::from("/dev/video-nonexistent-archdrop-test")),
            width: 1280,
            height: 720,
        },
    };
    let res = run_webcam(args).await;
    assert!(res.is_err(), "Expected start with nonexistent device to fail");
    let err_msg = format!("{:#}", res.unwrap_err());
    assert!(
        err_msg.contains("Failed to open V4L2 device"),
        "Unexpected error message: {err_msg}"
    );
}

#[tokio::test]
async fn test_webcam_stop_cli_command() {
    let args = WebcamArgs {
        sub: WebcamSubcommands::Stop,
    };
    let res = run_webcam(args).await;
    assert!(res.is_ok(), "webcam stop command failed: {:?}", res);
}

#[test]
fn test_wireguard_safety_invariant() {
    let output = std::process::Command::new("ip")
        .args(["addr", "show", "dev", "tiktok"])
        .output();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("10.2.0.2/32"),
            "WireGuard tiktok interface must have 10.2.0.2/32 intact: {stdout}"
        );
    }
}
