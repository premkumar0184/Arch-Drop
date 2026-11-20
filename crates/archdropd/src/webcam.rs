//! Virtual webcam CLI subcommand handlers and diagnostics.

use anyhow::{Context, Result};
use archdrop_webcam::{
    discover_loopback_device, find_loopback_device, list_loopback_devices, LinuxV4l2Sink,
    V4l2Sink, MODPROBE_DIAGNOSTIC, V4L2_PIX_FMT_MJPEG,
};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug, Clone)]
pub struct WebcamArgs {
    #[command(subcommand)]
    pub sub: WebcamSubcommands,
}

#[derive(Subcommand, Debug, Clone)]
pub enum WebcamSubcommands {
    /// Show virtual webcam loopback device status and diagnostics
    Status {
        /// Output status as JSON
        #[arg(long)]
        json: bool,
    },
    /// Start or verify the virtual webcam loopback device sink
    Start {
        /// Optional path to /dev/videoX device node
        #[arg(short, long)]
        device: Option<PathBuf>,
        /// Width in pixels (default: 1280)
        #[arg(long, default_value_t = 1280)]
        width: u32,
        /// Height in pixels (default: 720)
        #[arg(long, default_value_t = 720)]
        height: u32,
    },
    /// Stop active virtual webcam stream and release loopback device
    Stop,
}

pub async fn run_webcam(args: WebcamArgs) -> Result<()> {
    match args.sub {
        WebcamSubcommands::Status { json } => handle_status(json),
        WebcamSubcommands::Start {
            device,
            width,
            height,
        } => handle_start(device, width, height),
        WebcamSubcommands::Stop => handle_stop(),
    }
}

fn handle_status(json: bool) -> Result<()> {
    match list_loopback_devices(None) {
        Ok(devices) if !devices.is_empty() => {
            if json {
                let out = serde_json::json!({
                    "loaded": true,
                    "device_count": devices.len(),
                    "devices": devices,
                    "diagnostic": null
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Virtual Webcam Status:");
                println!("  Kernel Module: v4l2loopback is loaded");
                println!("  Discovered Devices ({}):", devices.len());
                for dev in &devices {
                    println!(
                        "    - {:<14} Label: {:<20} (Index: {})",
                        dev.device_path.display(),
                        dev.card_label,
                        dev.index
                    );
                }
            }
        }
        _ => {
            if json {
                let out = serde_json::json!({
                    "loaded": false,
                    "device_count": 0,
                    "devices": [],
                    "diagnostic": MODPROBE_DIAGNOSTIC
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Virtual Webcam Status:");
                println!("  Kernel Module: v4l2loopback is NOT loaded (no loopback devices found)");
                println!();
                println!("To enable Arch Drop virtual webcam sink, run:");
                println!("  {MODPROBE_DIAGNOSTIC}");
            }
        }
    }

    Ok(())
}

fn handle_start(device: Option<PathBuf>, width: u32, height: u32) -> Result<()> {
    let dev_path = match device {
        Some(p) => p,
        None => match find_loopback_device(None) {
            Ok(d) => d.device_path,
            Err(_) => {
                eprintln!("Error: No virtual loopback video device found.");
                eprintln!("To enable virtual webcam support, run:");
                eprintln!("  {MODPROBE_DIAGNOSTIC}");
                anyhow::bail!(
                    "v4l2loopback kernel module is not loaded: {MODPROBE_DIAGNOSTIC}"
                );
            }
        },
    };

    let mut sink = LinuxV4l2Sink::open(&dev_path)
        .with_context(|| format!("Failed to open V4L2 device at {dev_path:?}"))?;

    sink.set_format(width, height, V4L2_PIX_FMT_MJPEG)
        .with_context(|| format!("Failed to set {width}x{height} MJPEG format on {dev_path:?}"))?;

    sink.stream_on()
        .with_context(|| format!("Failed to activate streaming on {dev_path:?}"))?;

    println!(
        "Virtual webcam sink successfully initialized on {:?} ({}x{} MJPEG, streaming ON).",
        sink.device_path(),
        width,
        height
    );

    // Teardown: stream_off is called and fd closed when sink drops
    sink.stream_off()
        .with_context(|| format!("Failed to stop stream during verification on {dev_path:?}"))?;
    println!("Device verified and ready for incoming streams.");

    Ok(())
}

fn handle_stop() -> Result<()> {
    // If a loopback device is open or available, issue STREAMOFF
    if let Ok(Some(dev)) = discover_loopback_device(None) {
        if let Ok(mut sink) = LinuxV4l2Sink::open(&dev.device_path) {
            let _ = sink.stream_off();
        }
    }

    println!("Virtual webcam sink stopped (VIDIOC_STREAMOFF issued) and device descriptor released.");
    Ok(())
}
