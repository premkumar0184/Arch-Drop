//! Virtual audio sink management for PipeWire and headless mock testing.
//!
//! Supports automatic creation and RAII teardown of "ArchDrop Virtual Audio Output"
//! sink nodes via PipeWire loopback (`pw-loopback`) ensuring 0 orphan audio loops.

use crate::error::{AudioError, PIPEWIRE_DIAGNOSTIC};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tracing::{debug, info};

/// Canonical name for the ArchDrop virtual audio sink.
pub const DEFAULT_VIRTUAL_SINK_NAME: &str = "ArchDrop_Virtual_Audio_Output";

/// Canonical human-readable description for pavucontrol / audio settings.
pub const DEFAULT_VIRTUAL_SINK_DESC: &str = "ArchDrop Virtual Audio Output";

/// Virtual sink backend abstraction.
#[async_trait]
pub trait VirtualSinkBackend: Send + Sync {
    /// Create the virtual sink node and return its target capture node name in PipeWire.
    async fn create_sink(&mut self, sink_name: &str, description: &str) -> Result<String, AudioError>;

    /// Destroy the virtual sink node and release PipeWire resources.
    async fn destroy_sink(&mut self) -> Result<(), AudioError>;

    /// Current virtual sink name if active.
    fn sink_name(&self) -> Option<&str>;

    /// Whether the virtual sink is currently active.
    fn is_active(&self) -> bool;
}

/// PipeWire virtual sink backend utilizing `pw-loopback`.
pub struct PipeWireVirtualSink {
    child: Option<Child>,
    sink_name: Option<String>,
}

impl PipeWireVirtualSink {
    pub fn new() -> Self {
        Self {
            child: None,
            sink_name: None,
        }
    }

    /// Check if PipeWire tools are available on host.
    pub fn is_available() -> bool {
        std::path::Path::new("/usr/bin/pw-loopback").exists()
            || std::process::Command::new("which")
                .arg("pw-loopback")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    }
}

impl Default for PipeWireVirtualSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VirtualSinkBackend for PipeWireVirtualSink {
    async fn create_sink(&mut self, sink_name: &str, description: &str) -> Result<String, AudioError> {
        if self.child.is_some() {
            let _ = self.destroy_sink().await;
        }

        if !Self::is_available() {
            return Err(AudioError::PipeWireUnavailable(PIPEWIRE_DIAGNOSTIC.into()));
        }

        info!(
            name = %sink_name,
            desc = %description,
            "Creating PipeWire virtual sink via pw-loopback"
        );

        let capture_props = format!("media.class=Audio/Sink node.description=\"{}\"", description);

        let mut cmd = Command::new("pw-loopback");
        cmd.arg("--name")
            .arg(sink_name)
            .arg("--capture-props")
            .arg(&capture_props)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            AudioError::VirtualSinkFailed(format!(
                "Failed to spawn pw-loopback for sink '{}': {e}",
                sink_name
            ))
        })?;

        self.child = Some(child);
        self.sink_name = Some(sink_name.to_string());

        // Target capture name in PipeWire is "input.<sink_name>"
        let target_capture_node = format!("input.{}", sink_name);

        // Small grace period for PipeWire graph to register the node
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        Ok(target_capture_node)
    }

    async fn destroy_sink(&mut self) -> Result<(), AudioError> {
        if let Some(mut child) = self.child.take() {
            if let Some(name) = self.sink_name.take() {
                info!(name = %name, "Tearing down PipeWire virtual sink");
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        self.sink_name = None;
        Ok(())
    }

    fn sink_name(&self) -> Option<&str> {
        self.sink_name.as_deref()
    }

    fn is_active(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for PipeWireVirtualSink {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            debug!("Killing pw-loopback in RAII Drop");
            let _ = child.start_kill();
        }
    }
}

/// Mock virtual audio sink for CI / headless testing.
pub struct MockVirtualSink {
    active_name: Option<String>,
}

impl MockVirtualSink {
    pub fn new() -> Self {
        Self { active_name: None }
    }
}

impl Default for MockVirtualSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VirtualSinkBackend for MockVirtualSink {
    async fn create_sink(&mut self, sink_name: &str, _description: &str) -> Result<String, AudioError> {
        self.active_name = Some(sink_name.to_string());
        Ok(format!("input.{}", sink_name))
    }

    async fn destroy_sink(&mut self) -> Result<(), AudioError> {
        self.active_name = None;
        Ok(())
    }

    fn sink_name(&self) -> Option<&str> {
        self.active_name.as_deref()
    }

    fn is_active(&self) -> bool {
        self.active_name.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_virtual_sink_lifecycle() {
        let mut sink = MockVirtualSink::new();
        assert!(!sink.is_active());
        assert_eq!(sink.sink_name(), None);

        let node = sink
            .create_sink("TestSink", "Test Sink Description")
            .await
            .expect("Create sink");
        assert_eq!(node, "input.TestSink");
        assert!(sink.is_active());
        assert_eq!(sink.sink_name(), Some("TestSink"));

        sink.destroy_sink().await.expect("Destroy sink");
        assert!(!sink.is_active());
        assert_eq!(sink.sink_name(), None);
    }
}
