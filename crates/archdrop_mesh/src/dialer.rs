//! Transparent 3-tier dynamic transport resolver and failover multiplexer.
//!
//! Provides automated sequential failover across 3 connectivity tiers:
//! 1. Direct Local LAN (TCP probe to peer's LAN IP :53317, fail fast < 300ms on ECONNREFUSED, max timeout 1500ms).
//! 2. Zero-Wi-Fi Offline P2P (BLE wakeup 0x0D -> LOHS SoftAP gateway `192.168.43.1:53317`).
//! 3. Remote WAN Relay (DERP client connecting via HTTPS:443 WebSocket to DERP relay, piping to loopback).

use crate::client::DerpClient;
use crate::error::MeshError;
use crate::stream::DerpStream;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

/// Default timeout for direct TCP connection attempts (1500 ms).
pub const DEFAULT_DIRECT_TIMEOUT: Duration = Duration::from_millis(1500);

/// Default timeout for offline P2P connection attempts (25000 ms).
pub const DEFAULT_P2P_TIMEOUT: Duration = Duration::from_millis(25000);

/// Classification of the active transport route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TransportTier {
    /// Tier 1: Direct Local LAN (:53317).
    DirectLan,
    /// Tier 2: Zero-Wi-Fi Offline P2P via LOHS SoftAP (`192.168.43.1:53317`).
    OfflineP2p,
    /// Tier 3: Remote WAN Relay via userspace DERP client.
    WanRelay,
}

impl TransportTier {
    pub fn label(&self) -> &'static str {
        match self {
            TransportTier::DirectLan => "Direct LAN",
            TransportTier::OfflineP2p => "Peer-to-Peer",
            TransportTier::WanRelay => "Cloud Relay",
        }
    }
}

impl std::fmt::Display for TransportTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Unified bidirectional stream: direct TCP socket, P2P SoftAP socket, or relayed virtual DERP stream.
pub enum PeerStream {
    Direct(TcpStream),
    P2p(TcpStream),
    Relayed(Box<DerpStream>),
}

impl std::fmt::Debug for PeerStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerStream::Direct(s) => write!(f, "PeerStream::Direct({:?})", s.peer_addr()),
            PeerStream::P2p(s) => write!(f, "PeerStream::P2p({:?})", s.peer_addr()),
            PeerStream::Relayed(s) => write!(f, "PeerStream::Relayed(stream_id={})", s.stream_id()),
        }
    }
}

impl PeerStream {
    pub fn is_direct(&self) -> bool {
        matches!(self, PeerStream::Direct(_))
    }

    pub fn is_p2p(&self) -> bool {
        matches!(self, PeerStream::P2p(_))
    }

    pub fn is_relayed(&self) -> bool {
        matches!(self, PeerStream::Relayed(_))
    }

    pub fn tier(&self) -> TransportTier {
        match self {
            PeerStream::Direct(_) => TransportTier::DirectLan,
            PeerStream::P2p(_) => TransportTier::OfflineP2p,
            PeerStream::Relayed(_) => TransportTier::WanRelay,
        }
    }

    pub fn peer_addr(&self) -> io::Result<std::net::SocketAddr> {
        match self {
            PeerStream::Direct(s) => s.peer_addr(),
            PeerStream::P2p(s) => s.peer_addr(),
            PeerStream::Relayed(_) => Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "Relayed DERP stream does not have a physical socket address",
            )),
        }
    }
}

impl AsyncRead for PeerStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            PeerStream::Direct(s) => Pin::new(s).poll_read(cx, buf),
            PeerStream::P2p(s) => Pin::new(s).poll_read(cx, buf),
            PeerStream::Relayed(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for PeerStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            PeerStream::Direct(s) => Pin::new(s).poll_write(cx, buf),
            PeerStream::P2p(s) => Pin::new(s).poll_write(cx, buf),
            PeerStream::Relayed(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            PeerStream::Direct(s) => Pin::new(s).poll_flush(cx),
            PeerStream::P2p(s) => Pin::new(s).poll_flush(cx),
            PeerStream::Relayed(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            PeerStream::Direct(s) => Pin::new(s).poll_shutdown(cx),
            PeerStream::P2p(s) => Pin::new(s).poll_shutdown(cx),
            PeerStream::Relayed(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Pluggable provider for Tier 2 offline P2P dialing (e.g. BLE wakeup + LOHS SoftAP).
pub trait P2pDialer: Send + Sync {
    /// Attempts to establish a P2P connection to the companion device.
    ///
    /// For example: transmits BLE 0x0D wakeup, negotiates credentials over GATT,
    /// associates via NetworkManager, and connects to the gateway TCP port (:53317).
    fn dial_p2p<'a>(
        &'a self,
        target_peer: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<TcpStream, MeshError>> + Send + 'a>>;
}

/// Dynamic 3-tier transport resolver selecting the optimal route:
/// 1. Direct Local LAN (TCP probe to peer's LAN IP :53317, fail fast < 300ms on ECONNREFUSED, max timeout 1500ms)
/// 2. Zero-Wi-Fi Offline P2P (BLE wakeup 0x0D -> LOHS SoftAP gateway `192.168.43.1:53317`)
/// 3. Remote WAN Relay (DERP client connecting via HTTPS:443 WebSocket to DERP relay, piping to loopback)
pub struct TransportResolver {
    derp_client: Option<Arc<DerpClient>>,
    p2p_dialer: Option<Arc<dyn P2pDialer>>,
    direct_timeout: Duration,
    p2p_timeout: Duration,
}

impl TransportResolver {
    /// Creates a new `TransportResolver` with optional active `DerpClient`.
    pub fn new(derp_client: Option<Arc<DerpClient>>) -> Self {
        Self {
            derp_client,
            p2p_dialer: None,
            direct_timeout: DEFAULT_DIRECT_TIMEOUT,
            p2p_timeout: DEFAULT_P2P_TIMEOUT,
        }
    }

    /// Configures the Tier 2 offline P2P dialer provider.
    pub fn with_p2p_dialer(mut self, dialer: Arc<dyn P2pDialer>) -> Self {
        self.p2p_dialer = Some(dialer);
        self
    }

    /// Overrides the timeout used for direct TCP connection attempts (default 1500ms).
    pub fn with_direct_timeout(mut self, timeout: Duration) -> Self {
        self.direct_timeout = timeout;
        self
    }

    /// Overrides the timeout used for offline P2P connection attempts (default 5000ms).
    pub fn with_p2p_timeout(mut self, timeout: Duration) -> Self {
        self.p2p_timeout = timeout;
        self
    }

    /// Returns a reference to the optional `DerpClient`.
    pub fn derp_client(&self) -> Option<&Arc<DerpClient>> {
        self.derp_client.as_ref()
    }

    /// Returns a reference to the optional `P2pDialer`.
    pub fn p2p_dialer(&self) -> Option<&Arc<dyn P2pDialer>> {
        self.p2p_dialer.as_ref()
    }

    /// Returns the direct connection timeout.
    pub fn direct_timeout(&self) -> Duration {
        self.direct_timeout
    }

    /// Returns the P2P connection timeout.
    pub fn p2p_timeout(&self) -> Duration {
        self.p2p_timeout
    }

    /// Resolves the optimal transport path across 3 sequential tiers:
    ///
    /// - Tier 1: Direct Local LAN TCP probe to `direct_addr`.
    ///   - Succeeds in < 50ms if active.
    ///   - Fails fast in < 300ms on `ECONNREFUSED` / `ENETUNREACH`.
    ///   - Times out at `direct_timeout` (1500ms) on blackhole IPs.
    /// - Tier 2: Zero-Wi-Fi Offline P2P via BLE 0x0D wakeup -> LOHS SoftAP -> `192.168.43.1:53317`.
    /// - Tier 3: Remote WAN Relay via userspace DERP client over HTTPS:443 WebSocket.
    pub async fn resolve_and_dial(
        &self,
        direct_addr: Option<&str>,
        target_peer: Option<&str>,
        target_peer_key: Option<&[u8; 32]>,
    ) -> Result<PeerStream, MeshError> {
        self.resolve_and_dial_with_p2p_override(
            direct_addr,
            target_peer,
            target_peer_key,
            self.p2p_dialer.as_deref(),
        )
        .await
    }

    /// Resolves and dials with an optional transient P2pDialer override.
    pub async fn resolve_and_dial_with_p2p_override(
        &self,
        direct_addr: Option<&str>,
        target_peer: Option<&str>,
        target_peer_key: Option<&[u8; 32]>,
        p2p_override: Option<&dyn P2pDialer>,
    ) -> Result<PeerStream, MeshError> {
        let start = std::time::Instant::now();

        // 1. Tier 1: Direct Local LAN
        if let Some(addr) = direct_addr {
            debug!(
                tier = "Tier 1: Direct LAN",
                direct_addr = addr,
                timeout_ms = self.direct_timeout.as_millis(),
                "Attempting direct TCP connection"
            );
            match tokio::time::timeout(self.direct_timeout, TcpStream::connect(addr)).await {
                Ok(Ok(stream)) => {
                    info!(
                        tier = "Tier 1: Direct LAN",
                        direct_addr = addr,
                        elapsed_ms = start.elapsed().as_millis(),
                        "Direct TCP connection established successfully"
                    );
                    return Ok(PeerStream::Direct(stream));
                }
                Ok(Err(e)) => {
                    warn!(
                        tier = "Tier 1: Direct LAN",
                        direct_addr = addr,
                        elapsed_ms = start.elapsed().as_millis(),
                        error = %e,
                        "Direct TCP connection refused/failed (<300ms SLA); advancing to Tier 2"
                    );
                }
                Err(_) => {
                    warn!(
                        tier = "Tier 1: Direct LAN",
                        direct_addr = addr,
                        elapsed_ms = start.elapsed().as_millis(),
                        "Direct TCP connection timed out (> {:?}); advancing to Tier 2",
                        self.direct_timeout
                    );
                }
            }
        }

        // 2. Tier 2: Zero-Wi-Fi Offline P2P (BLE wakeup 0x0D -> LOHS SoftAP)
        let effective_p2p = p2p_override.or_else(|| self.p2p_dialer.as_deref());
        if let Some(p2p) = effective_p2p {
            info!(
                tier = "Tier 2: Offline P2P",
                peer = ?target_peer,
                elapsed_ms = start.elapsed().as_millis(),
                "Attempting offline P2P connection via BLE wakeup & LOHS SoftAP"
            );
            match tokio::time::timeout(self.p2p_timeout, p2p.dial_p2p(target_peer)).await {
                Ok(Ok(stream)) => {
                    info!(
                        tier = "Tier 2: Offline P2P",
                        elapsed_ms = start.elapsed().as_millis(),
                        "Offline P2P connection established successfully via LOHS gateway"
                    );
                    return Ok(PeerStream::P2p(stream));
                }
                Ok(Err(e)) => {
                    warn!(
                        tier = "Tier 2: Offline P2P",
                        elapsed_ms = start.elapsed().as_millis(),
                        error = %e,
                        "Offline P2P connection failed; advancing to Tier 3 WAN relay"
                    );
                }
                Err(_) => {
                    warn!(
                        tier = "Tier 2: Offline P2P",
                        elapsed_ms = start.elapsed().as_millis(),
                        "Offline P2P connection timed out; advancing to Tier 3 WAN relay"
                    );
                }
            }
        }

        // 3. Tier 3: Remote WAN DERP Relay
        if let (Some(derp), Some(peer_key)) = (&self.derp_client, target_peer_key) {
            info!(
                tier = "Tier 3: WAN Relay",
                peer = %hex::encode_upper(peer_key),
                elapsed_ms = start.elapsed().as_millis(),
                "Opening virtual stream via DERP relay"
            );
            let derp_stream = derp.open_stream(peer_key).await?;
            let total_elapsed = start.elapsed();
            info!(
                tier = "Tier 3: WAN Relay",
                peer = %hex::encode_upper(peer_key),
                total_elapsed_ms = total_elapsed.as_millis(),
                "DERP relay stream opened successfully via Tier 3 WAN fallback"
            );
            return Ok(PeerStream::Relayed(Box::new(derp_stream)));
        }

        Err(MeshError::PeerUnreachable(format!(
            "All 3 transport tiers failed (direct_addr: {:?}, p2p_available: {}, derp_available: {}, peer_key_available: {})",
            direct_addr,
            effective_p2p.is_some(),
            self.derp_client.is_some(),
            target_peer_key.is_some()
        )))
    }
}

/// Fallback dialer coordinating direct TCP, offline P2P, and DERP relay fallback.
///
/// Preserves full backward compatibility with the original 2-tier `FallbackDialer` API
/// while delegating internally to `TransportResolver`.
pub struct FallbackDialer {
    resolver: TransportResolver,
}

impl FallbackDialer {
    /// Creates a new `FallbackDialer` with optional active `DerpClient`.
    pub fn new(derp_client: Option<Arc<DerpClient>>) -> Self {
        Self {
            resolver: TransportResolver::new(derp_client),
        }
    }

    /// Configures the Tier 2 offline P2P dialer provider.
    pub fn with_p2p_dialer(mut self, dialer: Arc<dyn P2pDialer>) -> Self {
        self.resolver = self.resolver.with_p2p_dialer(dialer);
        self
    }

    /// Overrides the timeout used for direct TCP connection attempts.
    pub fn with_direct_timeout(mut self, timeout: Duration) -> Self {
        self.resolver = self.resolver.with_direct_timeout(timeout);
        self
    }

    /// Overrides the timeout used for offline P2P connection attempts.
    pub fn with_p2p_timeout(mut self, timeout: Duration) -> Self {
        self.resolver = self.resolver.with_p2p_timeout(timeout);
        self
    }

    /// Returns a reference to the internal `TransportResolver`.
    pub fn resolver(&self) -> &TransportResolver {
        &self.resolver
    }

    /// Attempts direct TCP connection to `direct_addr` with configured timeout,
    /// falling back through Tier 2 P2P (if configured) and Tier 3 DERP relay.
    pub async fn dial_with_fallback(
        &self,
        direct_addr: Option<&str>,
        target_peer_key: Option<&[u8; 32]>,
    ) -> Result<PeerStream, MeshError> {
        self.resolver
            .resolve_and_dial(direct_addr, None, target_peer_key)
            .await
    }

    /// Complete 3-tier dialer taking direct address, peer identifier, and mesh public key.
    pub async fn dial_with_p2p_fallback(
        &self,
        direct_addr: Option<&str>,
        target_peer: Option<&str>,
        target_peer_key: Option<&[u8; 32]>,
    ) -> Result<PeerStream, MeshError> {
        self.resolver
            .resolve_and_dial(direct_addr, target_peer, target_peer_key)
            .await
    }

    /// 3-tier sequential dialer taking an optional external `P2pDialer` reference.
    pub async fn dial_3tier(
        &self,
        direct_addr: Option<&str>,
        p2p_dialer: Option<&dyn P2pDialer>,
        target_peer_key: Option<&[u8; 32]>,
    ) -> Result<PeerStream, MeshError> {
        self.resolver
            .resolve_and_dial_with_p2p_override(direct_addr, None, target_peer_key, p2p_dialer)
            .await
    }
}
