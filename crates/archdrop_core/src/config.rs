//! Self-documenting TOML configuration loader for Arch Drop.
//!
//! Handles loading, default template generation, atomic persistence,
//! POSIX file permission hardening (0o600), and path expansion.

use crate::error::ConfigError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Default self-documenting TOML template written on initial run.
pub const DEFAULT_CONFIG_TOML: &str = r#"# ==============================================================================
# Arch Drop Configuration
# Path: ~/.config/archdrop/config.toml
#
# Automatically generated with safe production defaults.
# The archdropd daemon and CLI read this file for runtime configuration.
# ==============================================================================

[device]
# Human-readable alias advertised to peer devices on LAN and WireGuard.
# If omitted or left empty, the system hostname is automatically used.
alias = "Arch Drop Laptop"

# Device category: "desktop", "mobile", "headless", "server", "web".
device_type = "desktop"

# Hardware / OS model string reported during discovery and pairing.
device_model = "Arch Linux (Hyprland)"

[network]
# Listening port for the LocalSend v2 HTTP/1.1 transfer engine.
# Standard LocalSend port is 53317.
port = 53317

# Socket bind address. "0.0.0.0" binds dual-stack IPv4/IPv6 across all interfaces.
bind_address = "0.0.0.0"

# Enable UDP multicast beacon discovery (LocalSend compatibility).
# Defaults to false for headless point-to-point / WireGuard operation.
multicast_enabled = false

# Optional static IP of the remote WireGuard companion peer (e.g. "10.2.0.2").
# Set this to enable direct point-to-point transfers without supplying --to <IP>.
# wireguard_peer_ip = "10.2.0.2"

[security]
# Enforce mutual TLS (mTLS) with pinned X.509 certificates for all transfers.
require_encryption = true

# Automatically accept file transfer sessions from paired certificate fingerprints
# without interactive confirmation prompts.
auto_accept_paired = true

# Verify streaming SHA-256 digests against sender-provided checksums on all incoming files.
verify_checksums = true

# Optional shared security token / PIN for legacy or unpaired transfer authentication.
# pin = "1234"

# Set of trusted peer X.509 certificate SHA-256 fingerprints (uppercase hexadecimal).
# File transfers from peers in this list are auto-accepted if auto_accept_paired = true.
paired_fingerprints = []

[storage]
# Destination directory for completed incoming file transfers.
# Tilde ('~') expansion is supported.
download_dir = "~/Downloads"

# Directory for partial streaming chunks and temporary files during active transfers.
temp_dir = "/tmp/archdrop"

[clipboard]
# Enable native Wayland background clipboard synchronization.
enabled = true

# Maximum payload size in bytes for inline clipboard synchronization.
# Apple Continuity pattern: payloads <= 4096 bytes (4 KB) are transmitted inline
# with immediate paste; payloads > 4096 bytes transmit a metadata descriptor only.
inline_threshold_bytes = 4096

# Ephemeral clipboard cache Time-To-Live (TTL) in seconds.
# Remote clipboard entries expire and are zeroized after this duration.
ttl_seconds = 120

# Filter out sensitive clipboard offers containing password manager hints
# (e.g. x-kde-passwordManagerHint, application/x-secret from KeePassXC / Bitwarden).
filter_secret_hints = true

# Securely zeroize cached clipboard buffers and clear Wayland selections
# upon receiving systemd-logind screen lock signals.
clear_on_lock = true

[mesh]
# Enable userspace DERP relay and WAN mesh networking.
enabled = false

# Primary DERP relay endpoint URL over HTTPS:443 (WebSocket or TLS Upgrade).
relay_url = "wss://derp.archdrop.net:443/derp"

# Optional secondary/fallback DERP relays.
fallback_relays = []

# Timeout in milliseconds for direct IP connection attempts before falling back to DERP.
direct_dial_timeout_ms = 1500

# Periodic keepalive heartbeat in seconds to maintain NAT pinholes.
keepalive_interval_secs = 20

# Trusted remote peer Ed25519 mesh public keys (64-character uppercase hex strings).
paired_mesh_keys = []
"#;

/// Root application configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub device: DeviceConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub clipboard: ClipboardConfig,
    #[serde(default)]
    pub mesh: MeshConfig,
}

/// Device identification and announcement settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceConfig {
    /// Human-readable device alias. If None or empty, falls back to hostname or "Arch Drop Device".
    #[serde(default)]
    pub alias: Option<String>,
    /// Device form factor (e.g., "desktop", "mobile", "headless", "server").
    #[serde(default = "default_device_type")]
    pub device_type: String,
    /// Hardware or OS model string reported during pairing.
    #[serde(default = "default_device_model")]
    pub device_model: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            alias: None,
            device_type: default_device_type(),
            device_model: default_device_model(),
        }
    }
}

fn default_device_type() -> String {
    "desktop".to_string()
}

fn default_device_model() -> String {
    "Arch Linux (Hyprland)".to_string()
}

/// Network binding and transfer protocol settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkConfig {
    /// LocalSend v2 HTTP transfer engine port (default: 53317).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Socket bind address (default: "0.0.0.0").
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// UDP multicast beacon discovery enabled (default: false for P2P/WireGuard).
    #[serde(default)]
    pub multicast_enabled: bool,
    /// Optional static peer IP for WireGuard companion (e.g., "10.2.0.2").
    #[serde(default)]
    pub wireguard_peer_ip: Option<String>,
    /// Configured peer endpoints or IPs (e.g. ["192.168.1.3:53317"]).
    #[serde(default)]
    pub peers: Vec<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            bind_address: default_bind_address(),
            multicast_enabled: false,
            wireguard_peer_ip: None,
            peers: Vec::new(),
        }
    }
}

fn default_port() -> u16 {
    53317
}

fn default_bind_address() -> String {
    "0.0.0.0".to_string()
}

/// Security, mTLS, and peer authentication settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityConfig {
    /// Require TLS encryption and certificate verification (default: true).
    #[serde(default = "default_true")]
    pub require_encryption: bool,
    /// Automatically accept transfers from paired fingerprints without user prompts (default: true).
    #[serde(default = "default_true")]
    pub auto_accept_paired: bool,
    /// Verify streaming SHA-256 checksums against sender manifest (default: true).
    #[serde(default = "default_true")]
    pub verify_checksums: bool,
    /// Optional transfer PIN / shared secret token (alias = "token").
    #[serde(alias = "token", default)]
    pub pin: Option<String>,
    /// Set of trusted peer certificate SHA-256 fingerprints in uppercase hex.
    #[serde(default)]
    pub paired_fingerprints: HashSet<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            require_encryption: true,
            auto_accept_paired: true,
            verify_checksums: true,
            pin: None,
            paired_fingerprints: HashSet::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Filesystem storage and download locations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageConfig {
    /// Directory for saving received files (alias = "download_directory", default: "~/Downloads").
    #[serde(alias = "download_directory", default = "default_download_dir")]
    pub download_dir: PathBuf,
    /// Directory for temporary streaming chunks (default: "/tmp/archdrop").
    #[serde(default = "default_temp_dir")]
    pub temp_dir: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            temp_dir: default_temp_dir(),
        }
    }
}

fn default_download_dir() -> PathBuf {
    PathBuf::from("~/Downloads")
}

fn default_temp_dir() -> PathBuf {
    PathBuf::from("/tmp/archdrop")
}

/// Native Wayland clipboard continuity and security settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardConfig {
    /// Enable Wayland clipboard synchronization (alias = "auto_sync", default: true).
    #[serde(alias = "auto_sync", default = "default_true")]
    pub enabled: bool,
    /// Maximum inline payload size in bytes (alias = "threshold_bytes", default: 4096).
    #[serde(alias = "threshold_bytes", default = "default_threshold_bytes")]
    pub inline_threshold_bytes: usize,
    /// Ephemeral clipboard TTL in seconds (default: 120).
    #[serde(default = "default_ttl_seconds")]
    pub ttl_seconds: u64,
    /// Drop offers containing password manager hints (default: true).
    #[serde(default = "default_true")]
    pub filter_secret_hints: bool,
    /// Clear clipboard on systemd-logind session lock (default: true).
    #[serde(default = "default_true")]
    pub clear_on_lock: bool,
    /// Configured peer endpoints for clipboard broadcast (e.g. ["192.168.1.3:53317"]).
    #[serde(default)]
    pub peers: Vec<String>,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            inline_threshold_bytes: default_threshold_bytes(),
            ttl_seconds: default_ttl_seconds(),
            filter_secret_hints: true,
            clear_on_lock: true,
            peers: Vec::new(),
        }
    }
}

fn default_threshold_bytes() -> usize {
    4096
}

fn default_ttl_seconds() -> u64 {
    120
}

/// Userspace DERP relay and WAN mesh networking configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshConfig {
    /// Enable userspace DERP relay and WAN mesh networking (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Primary DERP relay endpoint URL (default: "wss://derp.archdrop.net:443/derp").
    #[serde(default = "default_relay_url")]
    pub relay_url: String,
    /// Optional secondary/fallback DERP relays.
    #[serde(default)]
    pub fallback_relays: Vec<String>,
    /// Timeout in milliseconds for direct IP connection attempts before falling back to DERP (default: 1500).
    #[serde(default = "default_direct_dial_timeout_ms")]
    pub direct_dial_timeout_ms: u64,
    /// Periodic keepalive heartbeat in seconds to maintain NAT pinholes (default: 20).
    #[serde(default = "default_keepalive_interval_secs")]
    pub keepalive_interval_secs: u64,
    /// Trusted remote peer Ed25519 mesh public keys (64-character uppercase hex strings).
    #[serde(default)]
    pub paired_mesh_keys: Vec<String>,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relay_url: default_relay_url(),
            fallback_relays: Vec::new(),
            direct_dial_timeout_ms: default_direct_dial_timeout_ms(),
            keepalive_interval_secs: default_keepalive_interval_secs(),
            paired_mesh_keys: Vec::new(),
        }
    }
}

fn default_relay_url() -> String {
    "wss://derp.archdrop.net:443/derp".to_string()
}

fn default_direct_dial_timeout_ms() -> u64 {
    1500
}

fn default_keepalive_interval_secs() -> u64 {
    20
}

/// Expands a leading tilde ('~') into the user's home directory.
pub fn expand_tilde(path: &Path) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    path.to_path_buf()
}

impl Config {
    /// Returns the standard default configuration directory (`~/.config/archdrop`).
    pub fn default_dir() -> Result<PathBuf, ConfigError> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return Ok(PathBuf::from(trimmed).join("archdrop"));
            }
        }
        if let Some(cfg) = dirs::config_dir() {
            return Ok(cfg.join("archdrop"));
        }
        if let Some(home) = dirs::home_dir() {
            return Ok(home.join(".config").join("archdrop"));
        }
        Err(ConfigError::MissingConfigDir)
    }

    /// Returns the standard default configuration file path (`~/.config/archdrop/config.toml`).
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        Ok(Self::default_dir()?.join("config.toml"))
    }

    /// Loads the configuration from the specified path or creates the default file if missing.
    ///
    /// If `custom_path` is `None`, resolves to `~/.config/archdrop/config.toml`.
    /// On creation, writes the fully documented default template with POSIX mode `0o600`.
    pub fn load_or_create(custom_path: Option<&Path>) -> Result<Self, ConfigError> {
        let path = match custom_path {
            Some(p) => p.to_path_buf(),
            None => Self::default_path()?,
        };

        if !path.exists() {
            if let Some(parent) = path.parent() {
                ensure_dir_permissions(parent)?;
            }
            write_atomic_secure(&path, DEFAULT_CONFIG_TOML)?;
            return Self::load_from_path(&path);
        }

        Self::load_from_path(&path)
    }

    /// Loads and parses configuration from an existing file path.
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        toml::from_str(&content).map_err(|source| ConfigError::ParseToml {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Loads configuration from an existing file path (alias for load_from_path).
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        Self::load_from_path(path.as_ref())
    }

    /// Parses configuration from a TOML string (useful for testing and in-memory overrides).
    pub fn load_from_str(toml_str: &str) -> Result<Self, ConfigError> {
        toml::from_str(toml_str).map_err(|source| ConfigError::ParseToml {
            path: PathBuf::from("<memory>"),
            source,
        })
    }

    /// Serializes and writes configuration atomically to the specified path with mode `0o600`.
    pub fn save_to_path(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            ensure_dir_permissions(parent)?;
        }
        let serialized =
            toml::to_string_pretty(self).map_err(|source| ConfigError::SerializeToml { source })?;
        write_atomic_secure(path, &serialized)
    }

    /// Saves configuration to an existing file path (alias for save_to_path).
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        self.save_to_path(path.as_ref())
    }

    /// Saves the current configuration to the default path (`~/.config/archdrop/config.toml`).
    pub fn save_default(&self) -> Result<(), ConfigError> {
        let path = Self::default_path()?;
        self.save_to_path(&path)
    }

    /// Returns the effective device alias, falling back to `/etc/hostname`, `$HOSTNAME`, or `$USER`.
    pub fn effective_alias(&self) -> String {
        if let Some(ref a) = self.device.alias {
            let trimmed = a.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Ok(hostname) = std::fs::read_to_string("/etc/hostname") {
            let trimmed = hostname.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Ok(hostname) = std::env::var("HOSTNAME") {
            let trimmed = hostname.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Ok(user) = std::env::var("USER") {
            let trimmed = user.trim();
            if !trimmed.is_empty() {
                return format!("{trimmed}'s Laptop");
            }
        }
        "Arch Drop Device".to_string()
    }

    /// Checks if a peer fingerprint is trusted (delimiter-agnostic and case-insensitive).
    pub fn is_peer_paired(&self, fingerprint: &str) -> bool {
        let normalized = match crate::crypto::normalize_fingerprint(fingerprint) {
            Some(norm) => norm,
            None => fingerprint.trim().to_ascii_uppercase(),
        };
        self.security.paired_fingerprints.iter().any(|fp| {
            let stored_norm =
                crate::crypto::normalize_fingerprint(fp).unwrap_or_else(|| fp.clone());
            stored_norm.eq_ignore_ascii_case(&normalized)
        })
    }

    /// Adds a peer fingerprint in normalized uppercase format. Returns true if newly inserted.
    pub fn add_paired_fingerprint(&mut self, fingerprint: String) -> bool {
        let normalized = crate::crypto::normalize_fingerprint(&fingerprint)
            .unwrap_or_else(|| fingerprint.trim().to_ascii_uppercase());
        self.security.paired_fingerprints.insert(normalized)
    }

    /// Removes a peer fingerprint (delimiter-agnostic and case-insensitive). Returns true if it was present.
    pub fn remove_paired_fingerprint(&mut self, fingerprint: &str) -> bool {
        let normalized = match crate::crypto::normalize_fingerprint(fingerprint) {
            Some(norm) => norm,
            None => fingerprint.trim().to_ascii_uppercase(),
        };
        let to_remove: Vec<String> = self
            .security
            .paired_fingerprints
            .iter()
            .filter(|fp| {
                let stored_norm =
                    crate::crypto::normalize_fingerprint(fp).unwrap_or_else(|| (*fp).clone());
                stored_norm.eq_ignore_ascii_case(&normalized)
            })
            .cloned()
            .collect();
        let had_items = !to_remove.is_empty();
        for item in to_remove {
            self.security.paired_fingerprints.remove(&item);
        }
        had_items
    }

    /// Checks if a peer Ed25519 mesh public key is trusted (case-insensitive hex).
    pub fn is_mesh_key_paired(&self, key_hex: &str) -> bool {
        let normalized = key_hex.trim().to_ascii_uppercase();
        self.mesh
            .paired_mesh_keys
            .iter()
            .any(|k| k.trim().to_ascii_uppercase() == normalized)
    }

    /// Adds a peer Ed25519 mesh public key in uppercase hex format. Returns true if newly inserted.
    pub fn add_paired_mesh_key(&mut self, key_hex: String) -> bool {
        let normalized = key_hex.trim().to_ascii_uppercase();
        if !self.is_mesh_key_paired(&normalized) {
            self.mesh.paired_mesh_keys.push(normalized);
            true
        } else {
            false
        }
    }

    /// Returns the download directory with any leading `~` expanded.
    pub fn resolved_download_dir(&self) -> PathBuf {
        expand_tilde(&self.storage.download_dir)
    }

    /// Returns the temporary directory with any leading `~` expanded.
    pub fn resolved_temp_dir(&self) -> PathBuf {
        expand_tilde(&self.storage.temp_dir)
    }
}

/// Application configuration manager binding an active `Config` to its storage path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    config: Config,
    path: PathBuf,
}

impl AppConfig {
    /// Loads or creates configuration at the specified path.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        let config = Config::load_or_create(Some(path))?;
        Ok(Self {
            config,
            path: path.to_path_buf(),
        })
    }

    /// Loads configuration from an existing file path.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let p = path.as_ref();
        let config = Config::load_from_path(p)?;
        Ok(Self {
            config,
            path: p.to_path_buf(),
        })
    }

    /// Saves configuration to the specified file path, updating the internal path.
    pub fn save_to_file<P: AsRef<Path>>(&mut self, path: P) -> Result<(), ConfigError> {
        let p = path.as_ref();
        self.config.save_to_path(p)?;
        self.path = p.to_path_buf();
        Ok(())
    }

    /// Loads or creates configuration at the standard system path.
    pub fn load_default() -> Result<Self, ConfigError> {
        let path = Config::default_path()?;
        let config = Config::load_or_create(Some(&path))?;
        Ok(Self { config, path })
    }

    /// Returns an immutable reference to the underlying `Config`.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a mutable reference to the underlying `Config`.
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    /// Returns the configuration file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Saves the current configuration to its bound file path.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.config.save_to_path(&self.path)
    }
}

/// Ensures the directory exists with safe permissions (`0o700` on Unix).
fn ensure_dir_permissions(dir: &Path) -> Result<(), ConfigError> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    if !dir.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            builder.mode(0o700);
            builder.create(dir).map_err(|source| ConfigError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(dir).map_err(|source| ConfigError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
    }
    Ok(())
}

/// RAII guard ensuring temporary files are unlinked if an operation fails or panics.
struct TempFileGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> TempFileGuard<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<'a> Drop for TempFileGuard<'a> {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// Writes content atomically via a temporary file with mode `0o600` on Unix.
///
/// Guaranteed to prevent race conditions and collisions during intra-process concurrent
/// writes across multiple threads, as well as multi-process concurrent access.
fn write_atomic_secure(target_path: &Path, content: &str) -> Result<(), ConfigError> {
    let file_name = target_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config");
    let unique_nonce = uuid::Uuid::new_v4().simple();
    let tmp_file_name = format!(".{}.tmp.{}.{}", file_name, std::process::id(), unique_nonce);
    let tmp_path = target_path.with_file_name(tmp_file_name);

    let mut guard = TempFileGuard::new(&tmp_path);

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);

        let mut file = options.open(&tmp_path).map_err(|source| ConfigError::Io {
            path: tmp_path.clone(),
            source,
        })?;

        // Explicitly enforce 0o600 on the file descriptor to override any restrictive umask
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));

        file.write_all(content.as_bytes())
            .map_err(|source| ConfigError::Io {
                path: tmp_path.clone(),
                source,
            })?;

        file.sync_all().map_err(|source| ConfigError::Io {
            path: tmp_path.clone(),
            source,
        })?;

        // Explicitly drop file handle before renaming
        drop(file);
    }

    #[cfg(not(unix))]
    {
        use std::io::Write;

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);

        let mut file = options.open(&tmp_path).map_err(|source| ConfigError::Io {
            path: tmp_path.clone(),
            source,
        })?;

        file.write_all(content.as_bytes())
            .map_err(|source| ConfigError::Io {
                path: tmp_path.clone(),
                source,
            })?;

        file.sync_all().map_err(|source| ConfigError::Io {
            path: tmp_path.clone(),
            source,
        })?;

        drop(file);
    }

    std::fs::rename(&tmp_path, target_path).map_err(|source| ConfigError::Io {
        path: target_path.to_path_buf(),
        source,
    })?;

    // Successfully renamed to target_path; disarm guard so tempfile is not removed
    guard.disarm();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_toml_uses_complete_defaults() {
        let conf = Config::load_from_str("").expect("Empty TOML should load with full defaults");
        assert_eq!(conf.network.port, 53317);
        assert_eq!(conf.network.bind_address, "0.0.0.0");
        assert!(!conf.network.multicast_enabled);
        assert!(conf.security.require_encryption);
        assert!(conf.security.auto_accept_paired);
        assert!(conf.security.verify_checksums);
        assert_eq!(conf.clipboard.inline_threshold_bytes, 4096);
        assert_eq!(conf.clipboard.ttl_seconds, 120);
        assert!(conf.clipboard.enabled);
        assert!(conf.clipboard.filter_secret_hints);
        assert!(conf.clipboard.clear_on_lock);
    }

    #[test]
    fn test_default_template_roundtrip() {
        let conf = Config::load_from_str(DEFAULT_CONFIG_TOML)
            .expect("Default template TOML must parse cleanly");
        assert_eq!(conf.network.port, 53317);
        assert_eq!(conf.device.alias.as_deref(), Some("Arch Drop Laptop"));
        assert_eq!(conf.clipboard.inline_threshold_bytes, 4096);
        assert_eq!(conf.clipboard.ttl_seconds, 120);
    }

    #[test]
    fn test_field_aliases() {
        let toml_data = r#"
[storage]
download_directory = "/custom/downloads"

[clipboard]
threshold_bytes = 8192
auto_sync = false

[security]
token = "shared_secret_pin"
"#;
        let conf = Config::load_from_str(toml_data).expect("Aliases should parse cleanly");
        assert_eq!(
            conf.storage.download_dir,
            PathBuf::from("/custom/downloads")
        );
        assert_eq!(conf.clipboard.inline_threshold_bytes, 8192);
        assert!(!conf.clipboard.enabled);
        assert_eq!(conf.security.pin.as_deref(), Some("shared_secret_pin"));
    }

    #[test]
    fn test_paired_fingerprints_case_insensitivity() {
        let mut conf = Config::default();
        let fp = "E7:89:12:AB:CD:EF:34:56".to_string();
        assert!(conf.add_paired_fingerprint(fp.clone()));
        assert!(!conf.add_paired_fingerprint(fp.clone())); // Already present

        assert!(conf.is_peer_paired("e7:89:12:ab:cd:ef:34:56"));
        assert!(conf.is_peer_paired("E7:89:12:AB:CD:EF:34:56"));
        assert!(!conf.is_peer_paired("00:00:00:00:00:00:00:00"));

        assert!(conf.remove_paired_fingerprint("e7:89:12:ab:cd:ef:34:56"));
        assert!(!conf.is_peer_paired("E7:89:12:AB:CD:EF:34:56"));
    }

    #[test]
    fn test_load_or_create_atomic_file() {
        let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
        let config_path = temp_dir.path().join("sub_dir").join("config.toml");

        // First call creates the file with default template
        let conf = Config::load_or_create(Some(&config_path)).expect("Creation should succeed");
        assert_eq!(conf.network.port, 53317);
        assert!(config_path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(&config_path).unwrap();
            let mode = metadata.permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "File permissions must be 0o600");

            let parent_meta = std::fs::metadata(config_path.parent().unwrap()).unwrap();
            assert_eq!(
                parent_meta.permissions().mode() & 0o777,
                0o700,
                "Dir permissions must be 0o700"
            );
        }

        // Second call reads existing file
        let conf_read = Config::load_or_create(Some(&config_path)).expect("Read should succeed");
        assert_eq!(conf_read.network.port, 53317);
    }

    #[test]
    fn test_expand_tilde() {
        let path_with_tilde = PathBuf::from("~/Documents/ArchDrop");
        let expanded = expand_tilde(&path_with_tilde);
        assert!(!expanded.starts_with("~"));
        if let Some(home) = dirs::home_dir() {
            assert!(expanded.starts_with(home));
        }

        let regular_path = PathBuf::from("/etc/archdrop");
        assert_eq!(expand_tilde(&regular_path), regular_path);
    }

    #[test]
    fn test_effective_alias_resolution() {
        let mut conf = Config::default();
        conf.device.alias = Some("My Custom Workstation".to_string());
        assert_eq!(conf.effective_alias(), "My Custom Workstation");

        conf.device.alias = None;
        let auto_alias = conf.effective_alias();
        assert!(!auto_alias.trim().is_empty());
    }

    #[test]
    fn test_app_config_wrapper() {
        let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
        let config_path = temp_dir.path().join("archdrop").join("config.toml");

        let mut app_config = AppConfig::load_or_create(&config_path).expect("AppConfig creation");
        assert_eq!(app_config.config().network.port, 53317);
        assert_eq!(app_config.path(), &config_path);

        app_config.config_mut().network.port = 54000;
        app_config.save().expect("Save AppConfig");

        let reloaded = AppConfig::load_or_create(&config_path).expect("Reload AppConfig");
        assert_eq!(reloaded.config().network.port, 54000);
    }
}
