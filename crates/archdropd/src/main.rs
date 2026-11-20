pub mod daemon;
pub mod events;
pub mod ipc;
pub mod notification;
pub mod webcam;
pub mod audio;

use anyhow::{Context, Result};
use archdrop_clipboard::{
    pack_clipboard_data, ClipboardPayloadCache, WaylandClipboardAdapter,
};
use ipc::IpcServer;
use archdrop_core::config::{AppConfig, Config};
use archdrop_core::crypto::{normalize_fingerprint, CryptoIdentity};
use archdrop_mesh::dialer::{FallbackDialer, P2pDialer};
use archdrop_transfer::{TransferClient, TransferSummary};
use archdrop_wireless::ble::{
    BleConfig, BleController, HotspotWakeup0x0D, BAND_2_4_GHZ, BAND_5_GHZ, DEFAULT_TRANSFER_PORT,
};
use archdrop_wireless::nm::{
    LohsConnectionConfig, LohsSessionCoordinator, NmStationController, SecurityType,
    DEFAULT_LOHS_IDLE_TIMEOUT,
};
use clap::{Args, Parser, Subcommand};
use daemon::{run_daemon, DaemonArgs};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(
    name = "archdropd",
    author = "Arch Drop Contributors",
    version = "0.1.0",
    about = "Arch Drop: Seamless personal device continuity daemon for Arch Linux & Hyprland"
)]
struct Cli {
    /// Custom configuration file path (defaults to ~/.config/archdrop/config.toml)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the background headless continuity daemon
    Daemon(DaemonArgs),

    /// Send file(s) directly to a remote peer via point-to-point IP or offline P2P wireless
    Send(SendArgs),

    /// Universal clipboard control and inspection
    Clipboard(ClipboardArgs),

    /// Wireless P2P management, scanning, and diagnostics (BLE & NetworkManager)
    Wireless(WirelessArgs),

    /// Display local daemon status, network bindings, and paired devices
    Status(StatusArgs),

    /// Manage trusted peer fingerprints
    Pair(PairArgs),

    /// Virtual webcam loopback management and diagnostics (v4l2loopback)
    Webcam(webcam::WebcamArgs),

    /// Desktop PipeWire audio capture and virtual sink diagnostics
    Audio(audio::AudioArgs),
}

#[derive(Args, Debug)]
struct SendArgs {
    /// Target peer IP address (e.g. 10.2.0.2 or 192.168.1.50)
    #[arg(short, long)]
    to: Option<String>,

    /// Target peer Ed25519 mesh public key (hex) for DERP relay WAN transfer
    #[arg(long)]
    mesh: Option<String>,

    /// Override DERP relay URL
    #[arg(long)]
    relay_url: Option<String>,

    /// Direct TCP connection attempt timeout in milliseconds before falling back to DERP (default: 1500)
    #[arg(long)]
    direct_timeout_ms: Option<u64>,

    /// Use automated P2P wireless discovery and LOHS connection
    #[arg(long)]
    p2p: bool,

    /// Target peer port (default: 53317)
    #[arg(short, long, default_value_t = 53317)]
    port: u16,

    /// Target SSID for direct P2P association
    #[arg(long)]
    ssid: Option<String>,

    /// Passphrase for direct P2P association
    #[arg(long)]
    passphrase: Option<String>,

    /// Wi-Fi interface to use (e.g. wlan0)
    #[arg(long)]
    iface: Option<String>,

    /// Use TLS encryption for transfer
    #[arg(long, default_value_t = false)]
    tls: bool,

    /// Path(s) to files to transfer
    #[arg(required = true)]
    files: Vec<PathBuf>,
}

#[derive(Args, Debug)]
struct WirelessArgs {
    #[command(subcommand)]
    sub: WirelessSubcommands,
}

#[derive(Subcommand, Debug)]
enum WirelessSubcommands {
    /// Scan for nearby Wi-Fi access points and/or BLE peers
    Scan {
        /// Target SSID filter
        #[arg(short, long)]
        ssid: Option<String>,

        /// Wi-Fi interface (e.g. wlan0)
        #[arg(long)]
        iface: Option<String>,

        /// Scan timeout in seconds (default: 5)
        #[arg(long, default_value_t = 5)]
        timeout: u64,
    },
    /// Test associating to a Local-Only Hotspot SoftAP
    Connect {
        /// SSID of the SoftAP
        #[arg(short, long)]
        ssid: String,

        /// Passphrase of the SoftAP
        #[arg(short, long)]
        passphrase: Option<String>,

        /// Wi-Fi interface (e.g. wlan0)
        #[arg(long)]
        iface: Option<String>,
    },
    /// Broadcast an Apple 0x0D hotspot wakeup trigger over BLE
    Wakeup {
        /// Use 5 GHz band preference
        #[arg(long, default_value_t = true)]
        five_ghz: bool,
    },
    /// Display wireless hardware & NetworkManager status
    Status {
        /// Wi-Fi interface (e.g. wlan0)
        #[arg(long)]
        iface: Option<String>,
    },
}

#[derive(Args, Debug)]
struct ClipboardArgs {
    #[command(subcommand)]
    sub: ClipboardSubcommands,
}

#[derive(Subcommand, Debug)]
enum ClipboardSubcommands {
    /// Push text directly to a remote peer's clipboard
    Push {
        /// Target peer IP
        #[arg(short, long)]
        to: String,

        /// Target port
        #[arg(short, long, default_value_t = 53317)]
        port: u16,

        /// Text payload to send
        text: String,
    },
    /// Clear local Wayland clipboard
    Clear,
}

#[derive(Args, Debug)]
struct PairArgs {
    #[command(subcommand)]
    sub: PairSubcommands,
}

#[derive(Subcommand, Debug)]
enum PairSubcommands {
    /// Add a trusted peer fingerprint
    Add {
        /// Hexadecimal SHA-256 certificate fingerprint
        fingerprint: String,
    },
    /// Remove a trusted peer fingerprint
    Remove {
        /// Hexadecimal SHA-256 certificate fingerprint
        fingerprint: String,
    },
    /// List all currently paired peer fingerprints
    List,
}

#[derive(Args, Debug, Clone, Default)]
pub struct StatusArgs {
    /// Follow and stream continuous status updates in Waybar JSON format
    #[arg(short, long)]
    pub follow: bool,

    /// Output current status snapshot as raw JSON
    #[arg(long)]
    pub json: bool,

    /// Custom UNIX domain socket path to connect to
    #[arg(long)]
    pub socket_path: Option<PathBuf>,
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("archdropd=info,archdrop_transfer=info,archdrop_clipboard=info,archdrop_webcam=info"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false).compact())
        .with(filter)
        .init();
}

fn load_app_config(custom_path: Option<PathBuf>) -> Result<AppConfig> {
    if let Some(path) = custom_path {
        AppConfig::load_or_create(&path).context("Failed to load config from custom path")
    } else {
        AppConfig::load_default().context("Failed to load or create default config")
    }
}

fn load_or_create_crypto_identity() -> Result<CryptoIdentity> {
    let base_dir = Config::default_dir().context("Failed to resolve config directory")?;
    let key_path = base_dir.join("identity.pem");

    if key_path.exists() {
        CryptoIdentity::load_from_file(&key_path).context("Failed to load crypto identity from disk")
    } else {
        info!("No existing cryptographic identity found, generating new RSA-2048 identity");
        let identity = CryptoIdentity::generate().context("Failed to generate RSA-2048 keypair")?;
        identity
            .save_to_file(&key_path)
            .context("Failed to save newly generated crypto identity")?;
        Ok(identity)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();

    let cli = Cli::parse();
    let app_config = load_app_config(cli.config)?;
    let identity = load_or_create_crypto_identity()?;

    match cli.command {
        Commands::Daemon(args) => run_daemon(app_config, identity, args).await,
        Commands::Send(args) => run_send(app_config, identity, args).await,
        Commands::Clipboard(args) => run_clipboard(app_config, identity, args).await,
        Commands::Wireless(args) => run_wireless(args).await,
        Commands::Status(args) => run_status(app_config, identity, args).await,
        Commands::Pair(args) => run_pair(app_config, args),
        Commands::Webcam(args) => webcam::run_webcam(args).await,
        Commands::Audio(args) => audio::run_audio(args).await,
    }
}

/// Concrete Linux Desktop P2P Dialer integrating BlueZ BLE GATT and NetworkManager.
pub struct LinuxDesktopP2pDialer {
    app_config: AppConfig,
    iface: Option<String>,
    port: u16,
}

impl LinuxDesktopP2pDialer {
    pub fn new(app_config: AppConfig, iface: Option<String>, port: u16) -> Self {
        Self {
            app_config,
            iface,
            port,
        }
    }
}

impl P2pDialer for LinuxDesktopP2pDialer {
    fn dial_p2p<'a>(
        &'a self,
        target_peer: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<tokio::net::TcpStream, archdrop_mesh::error::MeshError>> + Send + 'a>> {
        Box::pin(async move {
            info!("Tier 2 P2P: Initiating BLE wakeup & LOHS SoftAP credential negotiation");

            // 1. BLE 0x0D Wakeup and GATT credentials fetch
            let creds = try_ble_wakeup_and_credentials(&self.app_config, target_peer)
                .await
                .map_err(|e| archdrop_mesh::error::MeshError::ConnectionClosed(format!("BLE wakeup error: {e}")))?;

            // 2. Connect via NetworkManager D-Bus
            let nm = archdrop_wireless::nm::NmStationController::new()
                .await
                .map_err(|e| archdrop_mesh::error::MeshError::ConnectionClosed(format!("NetworkManager unavailable: {e}")))?;

            let wifi_dev = nm
                .find_wifi_device(self.iface.as_deref())
                .await
                .map_err(|e| archdrop_mesh::error::MeshError::ConnectionClosed(format!("No managed Wi-Fi device: {e}")))?;

            let lohs_config = archdrop_wireless::nm::LohsConnectionConfig::new(
                creds.ssid,
                creds.passphrase,
                archdrop_wireless::nm::SecurityType::Wpa2Psk,
            );

            let session = nm
                .connect_lohs(&wifi_dev, &lohs_config, Duration::from_secs(15))
                .await
                .map_err(|e| archdrop_mesh::error::MeshError::ConnectionClosed(format!("LOHS association failed: {e}")))?;

            let (coordinator, _watchdog) = archdrop_wireless::nm::LohsSessionCoordinator::spawn(
                session,
                archdrop_wireless::nm::DEFAULT_LOHS_IDLE_TIMEOUT,
            );

            let gateway_ip = coordinator.gateway_ip();
            let gateway_addr = format!("{}:{}", gateway_ip, self.port);

            info!(
                gateway = %gateway_ip,
                "Tier 2 P2P: Associated to SoftAP. Connecting TCP socket to gateway..."
            );

            let stream = tokio::net::TcpStream::connect(&gateway_addr)
                .await
                .map_err(|e| archdrop_mesh::error::MeshError::ConnectionClosed(format!("TCP connect to gateway {gateway_addr} failed: {e}")))?;

            coordinator.record_activity();
            Ok(stream)
        })
    }
}

async fn run_send(
    app_config: AppConfig,
    identity: CryptoIdentity,
    args: SendArgs,
) -> Result<()> {
    if args.ssid.is_some() || args.passphrase.is_some() {
        return run_send_p2p(app_config, identity, args).await;
    }

    // 1. Resolve target peer hints
    let target_direct_ip = args.to.clone();
    let target_mesh_key_hex = args.mesh.clone().or_else(|| {
        app_config.config().mesh.paired_mesh_keys.first().cloned()
    });

    let target_mesh_key = target_mesh_key_hex
        .as_deref()
        .and_then(|h| archdrop_mesh::crypto::MeshPublicKey::from_hex(h).ok());

    let target_peer_id = target_direct_ip.as_deref().or(target_mesh_key_hex.as_deref());

    if target_direct_ip.is_none() && target_mesh_key.is_none() && !args.p2p {
        anyhow::bail!(
            "Destination required: specify either --to <IP> for direct transfer or --p2p (or --mesh <PEER_KEY> for WAN DERP transfer)"
        );
    }

    // 2. Connect DERP client if mesh is enabled or mesh key is provided
    let mut derp_client_opt = None;
    if target_mesh_key.is_some() || app_config.config().mesh.enabled {
        let relay_url = args
            .relay_url
            .as_deref()
            .unwrap_or(&app_config.config().mesh.relay_url);

        let mesh_key_path = Config::default_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("mesh_key.pk8");

        if let Ok(mesh_key) = archdrop_mesh::crypto::MeshKeyPair::load_or_generate(&mesh_key_path) {
            let config = archdrop_mesh::client::DerpClientConfig::new(relay_url, Arc::new(mesh_key));
            if let Ok(client) = archdrop_mesh::client::DerpClient::connect(config).await {
                derp_client_opt = Some(client);
            }
        }
    }

    // 3. Build Tier 2 P2P dialer when --p2p is requested or when no direct IP is provided
    let p2p_dialer = if args.p2p || target_direct_ip.is_none() {
        Some(Arc::new(LinuxDesktopP2pDialer::new(
            app_config.clone(),
            args.iface.clone(),
            args.port,
        )))
    } else {
        None
    };

    // 4. Construct FallbackDialer with configured tiers
    let direct_timeout = args
        .direct_timeout_ms
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(app_config.config().mesh.direct_dial_timeout_ms));

    let mut dialer = FallbackDialer::new(derp_client_opt)
        .with_direct_timeout(direct_timeout)
        .with_p2p_timeout(Duration::from_secs(25));

    if let Some(p2p) = p2p_dialer {
        dialer = dialer.with_p2p_dialer(p2p);
    }

    // 5. Execute transfer using send_files_with_p2p_dialer
    let client = TransferClient::new();
    let direct_target = target_direct_ip.as_deref().map(|ip| (ip, args.port));
    let target_key_ref = target_mesh_key.as_ref().map(|k| &k.0);

    info!("Starting automated 3-tier transfer (Tier 1: LAN -> Tier 2: P2P -> Tier 3: DERP)");
    let summary = client
        .send_files_with_p2p_dialer(
            &dialer,
            direct_target,
            target_peer_id,
            target_key_ref,
            args.tls,
            &args.files,
            app_config.config(),
            &identity,
        )
        .await
        .context("Automated 3-tier file transfer failed")?;

    print_transfer_summary(&summary);
    Ok(())
}

#[allow(dead_code, clippy::too_many_arguments)]
async fn run_send_fallback(
    app_config: AppConfig,
    identity: CryptoIdentity,
    direct_to: Option<&str>,
    target_mesh_hex: &str,
    override_relay_url: Option<&str>,
    port: u16,
    tls: bool,
    direct_timeout_ms: Option<u64>,
    files: Vec<PathBuf>,
) -> Result<()> {
    let target_key = archdrop_mesh::crypto::MeshPublicKey::from_hex(target_mesh_hex)
        .context("Invalid target peer mesh public key hex")?;

    let relay_url = override_relay_url
        .unwrap_or(&app_config.config().mesh.relay_url);

    let mesh_key_path = Config::default_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("mesh_key.pk8");

    let mesh_key = Arc::new(
        archdrop_mesh::crypto::MeshKeyPair::load_or_generate(&mesh_key_path)
            .context("Failed to load or generate local mesh key")?,
    );

    info!(
        target_mesh = %target_key,
        relay_url = %relay_url,
        direct_to = ?direct_to,
        "Connecting to DERP relay for tiered fallback transfer"
    );

    let derp_client = archdrop_mesh::client::DerpClient::connect(
        archdrop_mesh::client::DerpClientConfig::new(relay_url, mesh_key),
    )
    .await
    .context("Failed to connect to DERP relay")?;

    let timeout_duration = direct_timeout_ms
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(app_config.config().mesh.direct_dial_timeout_ms));

    let dialer = archdrop_mesh::dialer::FallbackDialer::new(Some(derp_client))
        .with_direct_timeout(timeout_duration);

    let client = TransferClient::new();
    let direct_target = direct_to.map(|to| (to, port));

    let summary = client
        .send_files_with_dialer(
            &dialer,
            direct_target,
            Some(&target_key.0),
            tls,
            &files,
            app_config.config(),
            &identity,
        )
        .await
        .context("File transfer via FallbackDialer failed")?;

    print_transfer_summary(&summary);
    Ok(())
}

#[allow(dead_code)]
async fn run_send_direct(
    app_config: AppConfig,
    identity: CryptoIdentity,
    to: &str,
    port: u16,
    tls: bool,
    files: Vec<PathBuf>,
) -> Result<()> {
    let client = TransferClient::new();
    info!(
        target = %to,
        port = port,
        files_count = files.len(),
        "Initiating direct point-to-point file transfer"
    );

    let summary = client
        .send_files(
            to,
            port,
            tls,
            &files,
            app_config.config(),
            &identity,
        )
        .await
        .context("File transfer failed")?;

    print_transfer_summary(&summary);
    Ok(())
}

async fn run_send_p2p(
    app_config: AppConfig,
    identity: CryptoIdentity,
    args: SendArgs,
) -> Result<()> {
    info!("Initiating offline P2P wireless transfer flow");

    // 1. Determine LOHS SSID and passphrase
    let (target_ssid, target_passphrase) = if let Some(ssid) = args.ssid {
        (ssid, args.passphrase)
    } else {
        info!("Step 1/3: Attempting BLE wakeup and hotspot discovery...");
        let target_peer = args.mesh.as_deref().or(args.to.as_deref());
        match try_ble_wakeup_and_credentials(&app_config, target_peer).await {
            Ok(creds) => {
                info!("Obtained LOHS credentials via BLE for SSID: {}", creds.ssid);
                (creds.ssid, Some(creds.passphrase))
            }
            Err(e) => {
                warn!(
                    "BLE discovery unavailable or peer not found: {}. Scanning Wi-Fi for Arch Drop hotspots...",
                    e
                );
                let nm = NmStationController::new()
                    .await
                    .context("NetworkManager D-Bus connection failed")?;
                let dev = nm
                    .find_wifi_device(args.iface.as_deref())
                    .await
                    .context("No managed Wi-Fi device found")?;
                let aps = nm
                    .scan_access_points(&dev, None, Duration::from_secs(5))
                    .await
                    .context("Wi-Fi scan failed")?;
                let candidate = aps.into_iter().find(|ap| {
                    ap.ssid.starts_with("ArchDrop_")
                        || ap.ssid.starts_with("AndroidShare_")
                        || ap.ssid.starts_with("DIRECT-")
                });
                if let Some(ap) = candidate {
                    info!("Discovered prospective LOHS SoftAP: {}", ap.ssid);
                    (ap.ssid, args.passphrase)
                } else {
                    anyhow::bail!(
                        "No target SSID provided (--ssid) and no compatible Arch Drop SoftAP discovered via BLE or Wi-Fi"
                    );
                }
            }
        }
    };

    // 2. Associate to LOHS SoftAP via NetworkManager with volatile profile
    info!(
        ssid = %target_ssid,
        "Step 2/3: Establishing volatile station association via NetworkManager..."
    );
    let nm = NmStationController::new()
        .await
        .context("Failed to connect to NetworkManager system D-Bus")?;
    let wifi_dev = nm
        .find_wifi_device(args.iface.as_deref())
        .await
        .context("Failed to locate managed 802.11 Wi-Fi device")?;

    let (security, psk) = match target_passphrase {
        Some(ref pass) => (SecurityType::Wpa2Psk, pass.clone()),
        None => (SecurityType::Open, String::new()),
    };

    let lohs_config = LohsConnectionConfig::new(target_ssid.clone(), psk, security);

    let session = nm
        .connect_lohs(&wifi_dev, &lohs_config, Duration::from_secs(30))
        .await
        .context("Failed to establish LOHS station association")?;

    let (coordinator, _watchdog) =
        LohsSessionCoordinator::spawn(session, DEFAULT_LOHS_IDLE_TIMEOUT);

    let gateway_ip = coordinator.gateway_ip();
    info!(
        gateway = %gateway_ip,
        local = %coordinator.local_ip(),
        "Step 3/3: Associated to LOHS. Dialing LocalSend v2 receiver on gateway..."
    );

    coordinator.record_activity();

    // 3. Dial LocalSend transfer on resolved gateway IP
    let client = TransferClient::new();
    let transfer_res = client
        .send_files(
            &gateway_ip.to_string(),
            args.port,
            args.tls,
            &args.files,
            app_config.config(),
            &identity,
        )
        .await;

    coordinator.record_activity();

    // 4. Deactivate LOHS connection cleanly
    info!("Deactivating transient LOHS connection profile...");
    let _ = coordinator.deactivate().await;

    match transfer_res {
        Ok(summary) => {
            print_transfer_summary(&summary);
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!(
            "P2P file transfer to gateway {} failed: {:#}",
            gateway_ip,
            e
        )),
    }
}

/// Domain separation tag for Arch Drop BLE pairing keys
pub const BLE_KEY_DERIVATION_SALT: &[u8] = b"ARCHDROP-BLE-KEY-DERIVATION-V1";

/// Standard RFC 2104 HMAC-SHA256 implementation using `sha2::Sha256`.
pub fn compute_hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let mut hasher = Sha256::new();
        hasher.update(key);
        let hash = hasher.finalize();
        k_block[..32].copy_from_slice(&hash);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }

    let mut o_key_pad = [0x5cu8; 64];
    let mut i_key_pad = [0x36u8; 64];
    for i in 0..64 {
        o_key_pad[i] ^= k_block[i];
        i_key_pad[i] ^= k_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(&i_key_pad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&o_key_pad);
    outer.update(&inner_hash);
    let outer_hash = outer.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_hash);
    out
}

/// Derives a deterministic 32-byte symmetric paired key from a peer identity hex string or secret.
pub fn derive_paired_key(peer_identity: &str) -> [u8; 32] {
    let cleaned: String = peer_identity
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    let raw_bytes = if !cleaned.is_empty() && cleaned.len() % 2 == 0 {
        hex::decode(&cleaned).unwrap_or_else(|_| peer_identity.as_bytes().to_vec())
    } else {
        peer_identity.as_bytes().to_vec()
    };

    compute_hmac_sha256(BLE_KEY_DERIVATION_SALT, &raw_bytes)
}

/// Dynamically resolves the paired BLE secret from Config or falls back to default.
pub fn resolve_paired_ble_secret_from_config(cfg: &Config, target_peer: Option<&str>) -> Vec<u8> {
    if let Some(target) = target_peer {
        let cleaned = target.replace([':', ' ', '-'], "");
        if cfg.is_peer_paired(&cleaned) || cfg.is_mesh_key_paired(&cleaned) {
            return derive_paired_key(&cleaned).to_vec();
        }
    }
    if let Some(mesh_key) = cfg.mesh.paired_mesh_keys.first() {
        return derive_paired_key(mesh_key).to_vec();
    }
    if let Some(fp) = cfg.security.paired_fingerprints.iter().next() {
        return derive_paired_key(fp).to_vec();
    }
    if let Some(ref pin) = cfg.security.pin {
        return derive_paired_key(pin).to_vec();
    }
    b"ARCHDROP_SECRET".to_vec()
}

/// Dynamically resolves the paired BLE secret from AppConfig or falls back to default.
pub fn resolve_paired_ble_secret(app_config: &AppConfig, target_peer: Option<&str>) -> Vec<u8> {
    resolve_paired_ble_secret_from_config(app_config.config(), target_peer)
}

async fn try_ble_wakeup_and_credentials(
    app_config: &AppConfig,
    target_peer: Option<&str>,
) -> Result<archdrop_wireless::HotspotCredentials> {
    let paired_key = resolve_paired_ble_secret(app_config, target_peer);
    let ble_config = BleConfig::default();
    let mut ble = BleController::init(ble_config)
        .await
        .map_err(|e| anyhow::anyhow!("Bluetooth init error: {e}"))?;

    let wakeup = HotspotWakeup0x0D::new(
        &paired_key,
        BAND_5_GHZ,
        DEFAULT_TRANSFER_PORT,
    );
    let _ = ble.send_hotspot_wakeup(wakeup.clone()).await;

    let mut peer_rx = ble
        .scan_for_peers()
        .await
        .map_err(|e| anyhow::anyhow!("BLE discovery error: {e}"))?;

    let mut peer_addr = None;
    let scan_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(12);

    while tokio::time::Instant::now() < scan_deadline {
        tokio::select! {
            res = tokio::time::timeout_at(scan_deadline, peer_rx.recv()) => {
                match res {
                    Ok(Some(dev)) => {
                        let matches_name = dev.name.as_deref().map(|n| {
                            let lower = n.to_lowercase();
                            lower.contains("archdrop") || lower.contains("moto") || lower.contains("android")
                        }).unwrap_or(false);

                        if dev.is_archdrop_service
                            || dev.hotspot_status.is_some()
                            || dev.proximity_beacon.is_some()
                            || matches_name
                        {
                            peer_addr = Some(dev.address);
                            break;
                        }
                    }
                    _ => break,
                }
            }
        }
    }

    if let Some(addr) = peer_addr {
        tracing::info!("Found candidate BLE peer at {addr}, triggering wakeup and fetching credentials");
        let _ = ble.send_wakeup_command_gatt(addr, &wakeup).await;

        let mut last_err = None;
        for attempt in 1..=20 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            match ble.fetch_hotspot_credentials(addr).await {
                Ok(creds) => {
                    tracing::info!("Successfully fetched LOHS credentials for SSID '{}' from {addr}", creds.ssid);
                    return Ok(creds);
                }
                Err(e) => {
                    tracing::debug!("Attempt {attempt}/20 fetching LOHS credentials from {addr} failed: {e}");
                    last_err = Some(e);
                }
            }
        }
        anyhow::bail!("Failed to fetch credentials from {addr} after retries: {:?}", last_err);
    } else {
        anyhow::bail!("No Arch Drop peer responded over BLE (timed out after 12s)");
    }
}

fn print_transfer_summary(summary: &TransferSummary) {
    println!("\n=== Transfer Complete ===");
    println!("Target Peer : {}", summary.target_alias);
    println!("Session ID  : {}", summary.session_id);
    println!("Files Sent  : {}", summary.files_sent);
    println!(
        "Total Size  : {:.2} MB ({} bytes)",
        summary.total_bytes as f64 / (1024.0 * 1024.0),
        summary.total_bytes
    );
}

async fn run_wireless(args: WirelessArgs) -> Result<()> {
    match args.sub {
        WirelessSubcommands::Scan {
            ssid,
            iface,
            timeout,
        } => {
            let nm = NmStationController::new()
                .await
                .context("NetworkManager D-Bus service is unavailable")?;
            let dev = nm
                .find_wifi_device(iface.as_deref())
                .await
                .context("No managed Wi-Fi device found")?;
            println!(
                "Scanning Wi-Fi access points on device (timeout: {}s)...",
                timeout
            );
            let aps = nm
                .scan_access_points(&dev, ssid.as_deref(), Duration::from_secs(timeout))
                .await?;
            println!("\nDiscovered Access Points ({}):", aps.len());
            for ap in aps {
                println!(
                    "  SSID: {:<24} BSSID: {:<18} Freq: {:<5} MHz  Signal: {:<3}%  Sec: {:?}",
                    if ap.ssid.is_empty() {
                        "<Hidden>"
                    } else {
                        &ap.ssid
                    },
                    ap.bssid,
                    ap.frequency_mhz,
                    ap.strength,
                    ap.security
                );
            }
        }
        WirelessSubcommands::Connect {
            ssid,
            passphrase,
            iface,
        } => {
            let nm = NmStationController::new()
                .await
                .context("NetworkManager D-Bus service is unavailable")?;
            let dev = nm
                .find_wifi_device(iface.as_deref())
                .await
                .context("No managed Wi-Fi device found")?;
            let (security, psk) = match passphrase {
                Some(ref pass) => (SecurityType::Wpa2Psk, pass.clone()),
                None => (SecurityType::Open, String::new()),
            };
            let config = LohsConnectionConfig::new(ssid.clone(), psk, security);
            println!("Associating to LOHS SoftAP '{}'...", ssid);
            let mut session = nm.connect_lohs(&dev, &config, Duration::from_secs(30)).await?;
            println!("Successfully associated!");
            println!("  Local IP   : {}", session.local_ip());
            println!("  Gateway IP : {}", session.gateway_ip());
            println!("  Active Path: {}", session.active_path().as_str());
            println!("Deactivating connection...");
            session.deactivate().await?;
            println!("Deactivated cleanly.");
        }
        WirelessSubcommands::Wakeup { five_ghz } => {
            let config = AppConfig::load_default().map(|c| c.config().clone()).unwrap_or_default();
            let paired_key = resolve_paired_ble_secret_from_config(&config, None);
            let band = if five_ghz {
                BAND_5_GHZ
            } else {
                BAND_2_4_GHZ
            };
            let wakeup = HotspotWakeup0x0D::new(
                &paired_key,
                band,
                DEFAULT_TRANSFER_PORT,
            );
            println!("Initializing Bluetooth adapter for 0x0D wakeup advertisement...");
            let ble_config = BleConfig::default();
            let mut ble = BleController::init(ble_config)
                .await
                .map_err(|e| anyhow::anyhow!("Bluetooth error: {e}"))?;
            ble.send_hotspot_wakeup(wakeup)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to broadcast wakeup advertisement: {e}"))?;
            println!("Apple 0x0D Hotspot Wakeup advertisement broadcast active. Waiting 5s...");
            tokio::time::sleep(Duration::from_secs(5)).await;
            ble.stop_advertising();
            println!("Wakeup advertisement stopped.");
        }
        WirelessSubcommands::Status { iface } => {
            println!("=== Wireless & NetworkManager Status ===");
            match NmStationController::new().await {
                Ok(nm) => {
                    println!("NetworkManager: CONNECTED (zbus 5.0)");
                    match nm.find_wifi_device(iface.as_deref()).await {
                        Ok(dev) => println!("Primary Wi-Fi Device: {}", dev.as_str()),
                        Err(e) => println!("Wi-Fi Device Error: {e}"),
                    }
                }
                Err(e) => println!("NetworkManager: UNAVAILABLE ({e})"),
            }
            match BleController::init(BleConfig::default()).await {
                Ok(ble) => {
                    println!("BlueZ Bluetooth: CONNECTED");
                    println!("Adapter Name  : {}", ble.adapter().name());
                }
                Err(e) => println!("BlueZ Bluetooth: UNAVAILABLE ({e})"),
            }
        }
    }
    Ok(())
}

async fn run_clipboard(
    app_config: AppConfig,
    _identity: CryptoIdentity,
    args: ClipboardArgs,
) -> Result<()> {
    match args.sub {
        ClipboardSubcommands::Push { to, port, text } => {
            let (packet, _) = pack_clipboard_data(
                &app_config.config().effective_alias(),
                "text/plain;charset=utf-8",
                text.as_bytes(),
                None,
            );
            println!(
                "Prepared clipboard packet: {} bytes for target {}:{}",
                text.len(),
                to,
                port
            );
            let client = archdrop_transfer::TransferClient::new();
            let local_fp = _identity.fingerprint.clone();
            match client
                .send_clipboard(&to, port, false, None, Some(&local_fp), &packet)
                .await
            {
                Ok(_) => {
                    println!("Successfully pushed clipboard text to {}:{}", to, port);
                }
                Err(e) => {
                    eprintln!("Failed to push clipboard text to {}:{}: {}", to, port, e);
                }
            }
        }
        ClipboardSubcommands::Clear => {
            let adapter = WaylandClipboardAdapter::new(
                app_config.config().effective_alias().to_string(),
                ClipboardPayloadCache::default(),
                tokio::sync::broadcast::channel(1).0,
            );
            adapter.clear_local_clipboard().context("Failed to clear local Wayland clipboard")?;
            println!("Local Wayland clipboard selection cleared.");
        }
    }
    Ok(())
}

async fn run_status(
    app_config: AppConfig,
    identity: CryptoIdentity,
    args: StatusArgs,
) -> Result<()> {
    if args.follow {
        let socket_path = args
            .socket_path
            .unwrap_or_else(IpcServer::default_socket_path);

        if !socket_path.exists() {
            anyhow::bail!(
                "archdropd daemon is not running (socket {:?} does not exist). Start the daemon first.",
                socket_path
            );
        }

        let stream = tokio::net::UnixStream::connect(&socket_path)
            .await
            .with_context(|| format!("Failed to connect to archdropd IPC socket at {:?}", socket_path))?;

        let (reader, mut writer) = stream.into_split();
        use std::io::Write;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        writer
            .write_all(b"status --follow\n")
            .await
            .context("Failed to write follow command to IPC socket")?;

        let mut lines = BufReader::new(reader).lines();
        let mut stdout = std::io::stdout();
        while let Some(line) = lines.next_line().await? {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                if let Err(e) = writeln!(stdout, "{trimmed}") {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                    return Err(e.into());
                }
                if let Err(e) = stdout.flush() {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                    return Err(e.into());
                }
            }
        }
        return Ok(());
    }

    if args.json {
        let socket_path = args
            .socket_path
            .unwrap_or_else(IpcServer::default_socket_path);

        if socket_path.exists() {
            if let Ok(stream) = tokio::net::UnixStream::connect(&socket_path).await {
                let (reader, mut writer) = stream.into_split();
                use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                let _ = writer.write_all(b"status\n").await;
                let mut lines = BufReader::new(reader).lines();
                if let Ok(Some(line)) = lines.next_line().await {
                    println!("{line}");
                    return Ok(());
                }
            }
        }

        let cfg = app_config.config();
        let snapshot = serde_json::json!({
            "alias": cfg.effective_alias(),
            "fingerprint": identity.fingerprint,
            "port": cfg.network.port,
            "bind": cfg.network.bind_address,
            "download_dir": cfg.storage.download_dir,
            "paired_count": cfg.security.paired_fingerprints.len(),
            "wayland_available": WaylandClipboardAdapter::is_wayland_available(),
        });
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }

    let cfg = app_config.config();
    println!("==================================================");
    println!("            ARCH DROP DAEMON STATUS               ");
    println!("==================================================");
    println!("Device Alias       : {}", cfg.effective_alias());
    println!("Device Model       : Arch Linux (Hyprland)");
    println!("SHA-256 Fingerprint: {}", identity.fingerprint);
    println!("Transfer Port      : {}", cfg.network.port);
    println!("Bind Address       : {}", cfg.network.bind_address);
    println!("Download Directory : {}", cfg.storage.download_dir.display());
    println!("Encryption Required: {}", cfg.security.require_encryption);
    println!("Auto-Accept Paired : {}", cfg.security.auto_accept_paired);
    println!("Paired Peers Count : {}", cfg.security.paired_fingerprints.len());
    for (idx, fp) in cfg.security.paired_fingerprints.iter().enumerate() {
        println!("  [{}] {}", idx + 1, fp);
    }
    println!("Wayland Active     : {}", WaylandClipboardAdapter::is_wayland_available());
    println!("==================================================");
    Ok(())
}

fn run_pair(mut app_config: AppConfig, args: PairArgs) -> Result<()> {
    match args.sub {
        PairSubcommands::Add { fingerprint } => {
            let normalized = normalize_fingerprint(&fingerprint)
                .context("Invalid SHA-256 fingerprint format. Must be 64 hexadecimal characters.")?;
            app_config.config_mut().add_paired_fingerprint(normalized.clone());
            app_config.save().context("Failed to save updated configuration")?;
            println!("Successfully paired peer fingerprint: {}", normalized);
        }
        PairSubcommands::Remove { fingerprint } => {
            let normalized = normalize_fingerprint(&fingerprint)
                .context("Invalid SHA-256 fingerprint format.")?;
            app_config.config_mut().remove_paired_fingerprint(&normalized);
            app_config.save().context("Failed to save updated configuration")?;
            println!("Removed peer fingerprint: {}", normalized);
        }
        PairSubcommands::List => {
            println!("Paired Fingerprints ({}):", app_config.config().security.paired_fingerprints.len());
            for fp in &app_config.config().security.paired_fingerprints {
                println!("  - {}", fp);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_paired_key_deterministic() {
        let key1 = derive_paired_key("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        let key2 = derive_paired_key("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        assert_eq!(key1, key2);
        assert_ne!(key1, [0u8; 32]);
    }

    #[test]
    fn test_derive_paired_key_different_inputs() {
        let key1 = derive_paired_key("aaaa");
        let key2 = derive_paired_key("bbbb");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_resolve_paired_ble_secret_fallback() {
        let config = Config::default();
        let secret = resolve_paired_ble_secret_from_config(&config, None);
        assert_eq!(secret, b"ARCHDROP_SECRET".to_vec());
    }

    #[test]
    fn test_resolve_paired_ble_secret_with_paired_fingerprint() {
        let mut config = Config::default();
        let fp = "AABBCCDDEEFF00112233445566778899AABBCCDDEEFF00112233445566778899";
        config.add_paired_fingerprint(fp.to_string());
        let secret = resolve_paired_ble_secret_from_config(&config, None);
        let expected = derive_paired_key(fp);
        assert_eq!(secret, expected.to_vec());
    }

    #[test]
    fn test_candidate_paired_keys_includes_all_sources() {
        let mut config = Config::default();
        let fp = "11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff";
        let mesh_key = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
        config.add_paired_fingerprint(fp.to_string());
        config.mesh.paired_mesh_keys.push(mesh_key.to_string());
        config.security.pin = Some("9876".to_string());

        let id = CryptoIdentity::generate().unwrap();
        let keys = daemon::candidate_paired_keys(&config, Some(&id));

        assert!(keys.contains(&derive_paired_key(&id.fingerprint).to_vec()));
        assert!(keys.contains(&derive_paired_key(fp).to_vec()));
        assert!(keys.contains(&derive_paired_key(mesh_key).to_vec()));
        assert!(keys.contains(&derive_paired_key("9876").to_vec()));
        assert!(keys.contains(&b"ARCHDROP_SECRET".to_vec()));
    }
}
