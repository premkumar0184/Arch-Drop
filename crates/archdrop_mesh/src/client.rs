//! DERP client session management, mutual authentication handshake,
//! 20s keepalive heartbeats, and virtual stream demultiplexing.

use crate::crypto::MeshKeyPair;
use crate::error::MeshError;
use crate::frame::DerpFrame;
use crate::stream::{
    decode_stream_payload, encode_stream_payload, DerpStream, FLAG_DATA, FLAG_FIN, FLAG_RST,
    FLAG_SYN,
};
use crate::transport::{connect_transport, FrameSink, FrameStream};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace, warn};

pub const DEFAULT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);

type StreamKey = ([u8; 32], u64);

/// Configuration parameters for `DerpClient`.
#[derive(Clone)]
pub struct DerpClientConfig {
    pub relay_url: String,
    pub key_pair: Arc<MeshKeyPair>,
    pub keepalive_interval: Duration,
    pub tls_config: Option<Arc<rustls::ClientConfig>>,
}

impl DerpClientConfig {
    pub fn new(relay_url: impl Into<String>, key_pair: Arc<MeshKeyPair>) -> Self {
        Self {
            relay_url: relay_url.into(),
            key_pair,
            keepalive_interval: DEFAULT_KEEPALIVE_INTERVAL,
            tls_config: None,
        }
    }
}

/// A userspace DERP client session connected to a DERP relay server.
pub struct DerpClient {
    local_key: [u8; 32],
    server_key: [u8; 32],
    outbound_tx: mpsc::Sender<DerpFrame>,
    accept_rx: Mutex<mpsc::Receiver<(DerpStream, [u8; 32])>>,
    active_streams: Arc<Mutex<HashMap<StreamKey, mpsc::Sender<Bytes>>>>,
    next_stream_id: AtomicU64,
    connected: Arc<AtomicBool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl DerpClient {
    /// Connects to a DERP relay at `relay_url`, performs the mutual Ed25519 authentication handshake,
    /// and initializes background keepalive and packet demuxing tasks.
    pub async fn connect(config: DerpClientConfig) -> Result<Arc<Self>, MeshError> {
        let (sink, source) =
            connect_transport(&config.relay_url, config.tls_config).await?;

        Self::connect_with_transport(
            sink,
            source,
            config.key_pair,
            config.keepalive_interval,
        )
        .await
    }

    /// Performs the DERP handshake and spawns client tasks over pre-established frame sink and stream.
    pub async fn connect_with_transport(
        mut sink: FrameSink,
        mut source: FrameStream,
        key_pair: Arc<MeshKeyPair>,
        keepalive_interval: Duration,
    ) -> Result<Arc<Self>, MeshError> {
        let local_key = key_pair.public_key().0;

        // 1. Handshake Phase 1: Receive ServerKey from relay
        let first_frame = source
            .next()
            .await
            .ok_or_else(|| MeshError::ConnectionClosed("Relay closed before ServerKey".into()))??;

        let server_key = match first_frame {
            DerpFrame::ServerKey { server_key, .. } => server_key,
            other => {
                return Err(MeshError::Protocol(format!(
                    "Expected ServerKey (0x01) as first frame, got {:?}",
                    other.frame_type()
                )))
            }
        };

        // 2. Handshake Phase 2: Sign challenge and send ClientInfo
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| MeshError::Crypto(format!("System time error: {e}")))?
            .as_secs();

        let signature = key_pair.sign_challenge(&server_key, now);
        let client_info = DerpFrame::ClientInfo {
            client_key: local_key,
            timestamp: now,
            signature,
        };
        sink.send(client_info).await?;

        // 3. Handshake Phase 3: Receive ServerInfo acceptance
        let server_info_frame = source
            .next()
            .await
            .ok_or_else(|| MeshError::ConnectionClosed("Relay closed before ServerInfo".into()))??;

        match server_info_frame {
            DerpFrame::ServerInfo { .. } => {
                debug!(
                    server = %hex::encode_upper(server_key),
                    client = %hex::encode_upper(local_key),
                    "DERP mutual authentication handshake succeeded"
                );
            }
            other => {
                return Err(MeshError::Protocol(format!(
                    "Expected ServerInfo (0x03), got {:?}",
                    other.frame_type()
                )))
            }
        }

        // 4. Session established: spawn writer, reader, and keepalive tasks
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<DerpFrame>(256);
        let (accept_tx, accept_rx) = mpsc::channel::<(DerpStream, [u8; 32])>(128);
        let active_streams: Arc<Mutex<HashMap<StreamKey, mpsc::Sender<Bytes>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let connected = Arc::new(AtomicBool::new(true));

        // Task A: Outbound frame writer
        let connected_writer = connected.clone();
        let writer_handle = tokio::spawn(async move {
            while let Some(frame) = outbound_rx.recv().await {
                if let Err(e) = sink.send(frame).await {
                    warn!("Failed to send frame to DERP relay: {e}");
                    connected_writer.store(false, Ordering::SeqCst);
                    break;
                }
            }
            let _ = sink.close().await;
        });

        // Task B: Keepalive heartbeat (default 20 seconds)
        let outbound_keepalive = outbound_tx.clone();
        let connected_keepalive = connected.clone();
        let keepalive_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(keepalive_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            while connected_keepalive.load(Ordering::SeqCst) {
                ticker.tick().await;
                if outbound_keepalive.send(DerpFrame::KeepAlive).await.is_err() {
                    break;
                }
                trace!("Transmitted periodic DERP KeepAlive heartbeat");
            }
        });

        // Task C: Inbound frame demultiplexer
        let active_streams_demux = active_streams.clone();
        let outbound_demux = outbound_tx.clone();
        let connected_demux = connected.clone();
        let demux_handle = tokio::spawn(async move {
            while let Some(frame_res) = source.next().await {
                match frame_res {
                    Ok(DerpFrame::RecvPacket { src_key, payload }) => {
                        match decode_stream_payload(payload) {
                            Ok((stream_id, flags, chunk_data)) => {
                                let key = (src_key, stream_id);
                                if flags & FLAG_SYN != 0 {
                                    // New incoming virtual stream
                                    let (inbound_tx, inbound_rx) = mpsc::channel::<Bytes>(128);
                                    let mut map = active_streams_demux.lock().await;
                                    map.insert(key, inbound_tx);
                                    drop(map);

                                    let stream = DerpStream::new(
                                        stream_id,
                                        local_key,
                                        src_key,
                                        outbound_demux.clone(),
                                        inbound_rx,
                                    );
                                    if accept_tx.send((stream, src_key)).await.is_err() {
                                        warn!("Accept queue full or closed, dropping incoming stream");
                                    }
                                } else if flags & FLAG_FIN != 0 {
                                    let mut map = active_streams_demux.lock().await;
                                    if let Some(tx) = map.remove(&key) {
                                        let _ = tx.send(Bytes::new()).await;
                                    }
                                } else if flags & FLAG_RST != 0 {
                                    let mut map = active_streams_demux.lock().await;
                                    map.remove(&key);
                                } else if flags & FLAG_DATA != 0 {
                                    let map = active_streams_demux.lock().await;
                                    if let Some(tx) = map.get(&key) {
                                        let _ = tx.send(chunk_data).await;
                                    } else {
                                        trace!(
                                            src = %hex::encode_upper(src_key),
                                            stream_id = stream_id,
                                            "Dropping data packet for unknown stream"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to decode virtual stream payload: {e}");
                            }
                        }
                    }
                    Ok(DerpFrame::Ping { nonce }) => {
                        let _ = outbound_demux.send(DerpFrame::Pong { nonce }).await;
                    }
                    Ok(DerpFrame::Pong { .. }) => {
                        trace!("Received Pong probe echo from DERP relay");
                    }
                    Ok(DerpFrame::KeepAlive) => {
                        trace!("Received KeepAlive heartbeat from DERP relay");
                    }
                    Ok(DerpFrame::PeerGone { peer_key }) => {
                        info!(peer = %hex::encode_upper(peer_key), "Peer disconnected from DERP relay");
                    }
                    Ok(DerpFrame::PeerPresent { peer_key }) => {
                        info!(peer = %hex::encode_upper(peer_key), "Peer connected to DERP relay");
                    }
                    Ok(other) => {
                        trace!("Ignoring unhandled frame: {:?}", other.frame_type());
                    }
                    Err(e) => {
                        warn!("Error reading from DERP relay stream: {e}");
                        connected_demux.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }
            connected_demux.store(false, Ordering::SeqCst);
        });

        Ok(Arc::new(Self {
            local_key,
            server_key,
            outbound_tx,
            accept_rx: Mutex::new(accept_rx),
            active_streams,
            next_stream_id: AtomicU64::new(1),
            connected,
            tasks: Mutex::new(vec![writer_handle, keepalive_handle, demux_handle]),
        }))
    }

    /// Local 32-byte Ed25519 public key.
    pub fn local_key(&self) -> &[u8; 32] {
        &self.local_key
    }

    /// Server 32-byte Ed25519 public key.
    pub fn server_key(&self) -> &[u8; 32] {
        &self.server_key
    }

    /// Returns whether the relay connection is currently active.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Opens a virtual bidirectional stream to a remote peer via the DERP relay.
    pub async fn open_stream(&self, target_peer_key: &[u8; 32]) -> Result<DerpStream, MeshError> {
        if !self.is_connected() {
            return Err(MeshError::ConnectionClosed(
                "DERP client is disconnected".into(),
            ));
        }

        let stream_id = self.next_stream_id.fetch_add(1, Ordering::SeqCst);
        let (inbound_tx, inbound_rx) = mpsc::channel::<Bytes>(128);

        let key = (*target_peer_key, stream_id);
        {
            let mut map = self.active_streams.lock().await;
            map.insert(key, inbound_tx);
        }

        // Transmit FLAG_SYN packet to initiate virtual stream with remote peer
        let payload = encode_stream_payload(stream_id, FLAG_SYN, &[]);
        let syn_frame = DerpFrame::SendPacket {
            dst_key: *target_peer_key,
            payload,
        };

        self.outbound_tx
            .send(syn_frame)
            .await
            .map_err(|_| MeshError::ConnectionClosed("Outbound frame channel closed".into()))?;

        trace!(
            target = %hex::encode_upper(target_peer_key),
            stream_id = stream_id,
            "Opened virtual stream over DERP relay"
        );

        Ok(DerpStream::new(
            stream_id,
            self.local_key,
            *target_peer_key,
            self.outbound_tx.clone(),
            inbound_rx,
        ))
    }

    /// Accepts an incoming virtual stream initiated by a remote peer.
    pub async fn accept_stream(&self) -> Result<(DerpStream, [u8; 32]), MeshError> {
        let mut rx = self.accept_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| MeshError::ConnectionClosed("DerpClient accept channel closed".into()))
    }

    /// Creates a listener adapter for receiving incoming streams.
    pub fn listener(self: &Arc<Self>) -> DerpListener {
        DerpListener {
            client: self.clone(),
        }
    }

    /// Closes the DERP client connection and terminates all background tasks.
    pub async fn close(&self) -> Result<(), MeshError> {
        self.connected.store(false, Ordering::SeqCst);
        let mut tasks = self.tasks.lock().await;
        for task in tasks.drain(..) {
            task.abort();
        }
        Ok(())
    }
}

/// Incoming virtual stream listener for `DerpClient`.
#[derive(Clone)]
pub struct DerpListener {
    client: Arc<DerpClient>,
}

impl DerpListener {
    /// Accepts the next incoming virtual stream and remote peer public key.
    pub async fn accept(&self) -> Result<(DerpStream, [u8; 32]), MeshError> {
        self.client.accept_stream().await
    }

    /// Underlying client reference.
    pub fn client(&self) -> &Arc<DerpClient> {
        &self.client
    }
}
