//! Virtual video loopback device discovery and diagnostics.

use crate::error::{WebcamError, MODPROBE_DIAGNOSTIC};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Discovered virtual loopback video device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopbackDevice {
    /// Filesystem device node path (e.g. `/dev/video2`).
    pub device_path: PathBuf,
    /// Sysfs path (e.g. `/sys/devices/virtual/video4linux/video2`).
    pub sysfs_path: PathBuf,
    /// Human-readable card label (e.g. `"ArchDrop Camera"`).
    pub card_label: String,
    /// Video device index (e.g. 2 for `/dev/video2`).
    pub index: u32,
}

/// Discovers the preferred or first available v4l2loopback virtual device.
///
/// If `sysfs_root` is None, defaults to `/sys`.
/// If no loopback device is found or v4l2loopback is not loaded, returns
/// `Err(WebcamError::LoopbackNotLoaded { diagnostic })`.
pub fn discover_loopback_device(
    sysfs_root: Option<&Path>,
) -> Result<Option<LoopbackDevice>, WebcamError> {
    let devices = list_loopback_devices(sysfs_root)?;
    if devices.is_empty() {
        return Err(WebcamError::LoopbackNotLoaded {
            diagnostic: MODPROBE_DIAGNOSTIC.to_string(),
        });
    }

    // Prefer device labeled "ArchDrop Camera" if present
    if let Some(preferred) = devices
        .iter()
        .find(|d| d.card_label.to_lowercase().contains("archdrop"))
    {
        debug!(device = ?preferred.device_path, label = %preferred.card_label, "Discovered preferred ArchDrop loopback device");
        return Ok(Some(preferred.clone()));
    }

    let first = devices[0].clone();
    debug!(device = ?first.device_path, label = %first.card_label, "Discovered fallback loopback device");
    Ok(Some(first))
}

/// Required loopback discovery: returns `LoopbackDevice` directly or fails with `WebcamError`.
pub fn find_loopback_device(sysfs_root: Option<&Path>) -> Result<LoopbackDevice, WebcamError> {
    discover_loopback_device(sysfs_root)?
        .ok_or_else(|| WebcamError::LoopbackNotLoaded {
            diagnostic: MODPROBE_DIAGNOSTIC.to_string(),
        })
}

/// Lists all virtual loopback video devices found under sysfs.
pub fn list_loopback_devices(sysfs_root: Option<&Path>) -> Result<Vec<LoopbackDevice>, WebcamError> {
    let env_root = std::env::var_os("ARCHDROP_SYSFS_ROOT").map(PathBuf::from);
    let root = sysfs_root.or(env_root.as_deref()).unwrap_or_else(|| Path::new("/sys"));
    let virtual_dir = root.join("devices/virtual/video4linux");
    let class_dir = root.join("class/video4linux");

    let mut devices = Vec::new();

    // 1. Check /sys/devices/virtual/video4linux/video*
    if virtual_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&virtual_dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let name_str = file_name.to_string_lossy();
                if name_str.starts_with("video") {
                    if let Some(dev) = parse_device_entry(&entry.path(), &name_str, root) {
                        devices.push(dev);
                    }
                }
            }
        }
    }

    // 2. Also inspect /sys/class/video4linux/video* for symlinks resolving to virtual devices
    if class_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&class_dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let name_str = file_name.to_string_lossy();
                if name_str.starts_with("video") {
                    let entry_path = entry.path();
                    let is_virtual = if let Ok(target) = std::fs::read_link(&entry_path) {
                        target.to_string_lossy().contains("devices/virtual")
                    } else if let Ok(canon) = entry_path.canonicalize() {
                        canon.to_string_lossy().contains("devices/virtual")
                    } else {
                        false
                    };

                    if is_virtual {
                        let sysfs_path = entry_path
                            .canonicalize()
                            .unwrap_or_else(|_| entry_path.clone());
                        if !devices.iter().any(|d| d.sysfs_path == sysfs_path) {
                            if let Some(dev) = parse_device_entry(&sysfs_path, &name_str, root) {
                                devices.push(dev);
                            }
                        }
                    }
                }
            }
        }
    }

    // If neither virtual_dir nor class_dir exist or no devices were discovered, check module presence
    if !virtual_dir.exists() && devices.is_empty() {
        return Err(WebcamError::LoopbackNotLoaded {
            diagnostic: MODPROBE_DIAGNOSTIC.to_string(),
        });
    }

    devices.sort_by_key(|d| d.index);
    Ok(devices)
}

fn parse_device_entry(sysfs_path: &Path, name_str: &str, sysfs_root: &Path) -> Option<LoopbackDevice> {
    let index_str = name_str.strip_prefix("video")?;
    let index: u32 = index_str.parse().ok()?;

    // Read card label from <sysfs_path>/name if available
    let name_file = sysfs_path.join("name");
    let card_label = if let Ok(content) = std::fs::read_to_string(&name_file) {
        content.trim().to_string()
    } else {
        format!("Virtual Video Device {index}")
    };

    // Determine /dev/videoX path
    let dev_video = if sysfs_root != Path::new("/sys") {
        let mock_dev = sysfs_root.join(format!("dev/video{index}"));
        if mock_dev.exists() {
            mock_dev
        } else {
            PathBuf::from(format!("/dev/video{index}"))
        }
    } else {
        PathBuf::from(format!("/dev/video{index}"))
    };

    Some(LoopbackDevice {
        device_path: dev_video,
        sysfs_path: sysfs_path.to_path_buf(),
        card_label,
        index,
    })
}

/// Returns diagnostic instructions when v4l2loopback is missing.
pub fn get_diagnostic_message() -> &'static str {
    MODPROBE_DIAGNOSTIC
}
