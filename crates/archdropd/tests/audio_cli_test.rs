use archdropd::audio::{run_audio, AudioArgs, AudioSubcommands};

#[tokio::test]
async fn test_audio_status_cli_command() {
    // Test text status output
    let text_args = AudioArgs {
        sub: AudioSubcommands::Status { json: false },
    };
    let res = run_audio(text_args).await;
    assert!(res.is_ok(), "audio status command failed: {:?}", res);

    // Test JSON status output
    let json_args = AudioArgs {
        sub: AudioSubcommands::Status { json: true },
    };
    let res_json = run_audio(json_args).await;
    assert!(res_json.is_ok(), "audio status --json command failed: {:?}", res_json);
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
