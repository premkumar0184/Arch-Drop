//! Desktop audio streaming CLI subcommand handlers and diagnostics.

use anyhow::Result;
use archdrop_audio::capture::PipeWireCapture;
use archdrop_audio::error::PIPEWIRE_DIAGNOSTIC;
use archdrop_audio::sink::PipeWireVirtualSink;
use clap::{Args, Subcommand};

#[derive(Args, Debug, Clone)]
pub struct AudioArgs {
    #[command(subcommand)]
    pub sub: AudioSubcommands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum AudioSubcommands {
    /// Show PipeWire desktop audio capture and virtual sink subsystem diagnostics
    Status {
        /// Output status as JSON
        #[arg(long)]
        json: bool,
    },
}

pub async fn run_audio(args: AudioArgs) -> Result<()> {
    match args.sub {
        AudioSubcommands::Status { json } => handle_status(json),
    }
}

fn handle_status(json: bool) -> Result<()> {
    let pw_record_available = PipeWireCapture::is_system_available();
    let pw_loopback_available = PipeWireVirtualSink::is_available();
    let pipewire_ready = pw_record_available && pw_loopback_available;

    if json {
        let out = serde_json::json!({
            "pipewire_ready": pipewire_ready,
            "pw_record_available": pw_record_available,
            "pw_loopback_available": pw_loopback_available,
            "diagnostic": if pipewire_ready { None } else { Some(PIPEWIRE_DIAGNOSTIC) }
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Arch Drop Desktop Audio Streaming Status:");
        println!(
            "  PipeWire Capture (pw-record): {}",
            if pw_record_available { "Available" } else { "Not found" }
        );
        println!(
            "  Virtual Sink (pw-loopback):   {}",
            if pw_loopback_available { "Available" } else { "Not found" }
        );
        if pipewire_ready {
            println!("  Overall Status: Ready for low-latency desktop audio streaming");
        } else {
            println!("  Overall Status: PipeWire audio tools missing or inactive");
            println!();
            println!("To enable PipeWire audio capture and virtual sink:");
            println!("  {PIPEWIRE_DIAGNOSTIC}");
        }
    }

    Ok(())
}
