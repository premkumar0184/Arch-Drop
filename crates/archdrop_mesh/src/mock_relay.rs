//! Mock DERP relay server for high-fidelity testing and validation.
//!
//! Listens on a local TCP socket, conducts real Ed25519 mutual handshake authentication,
//! routes `SendPacket` / `RecvPacket` frames between connected peers, and notifies of
//! peer presence and departure.

use crate::crypto::{MeshKeyPair, MeshPublicKey};
use crate::error::MeshError;
use crate::frame::{DerpCodec, DerpFrame};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tracing::{debug, info, trace, warn};

/// In-memory / localhost mock DERP relay server.
pub struct MockDerpRelay {
    server_key: Arc<MeshKeyPair>,
    local_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    server_task: JoinHandle<()>,
}

impl MockDerpRelay {
    /// Starts a new mock DERP relay on `127.0.0.1:0` (ephemeral port).
    pub async fn start() -> Result<Self, MeshError> {
        Self::bind("127.0.0.1:0").await
    }

    /// Starts a new mock DERP relay bound to `bind_addr`.
    pub async fn bind(bind_addr: &str) -> Result<Self, MeshError> {
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(MeshError::Io)?;
        let local_addr = listener.local_addr().map_err(MeshError::Io)?;
        let server_key = Arc::new(MeshKeyPair::generate()?);

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let peers: Arc<Mutex<HashMap<[u8; 32], mpsc::Sender<DerpFrame>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let srv_key_clone = server_key.clone();
        let peers_clone = peers.clone();

        let server_task = tokio::spawn(async move {
            info!(addr = %local_addr, "Mock DERP relay started");

            loop {
                tokio::select! {
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, remote_addr)) => {
                                let peers = peers_clone.clone();
                                let srv_key = srv_key_clone.clone();

                                tokio::spawn(async move {
                                    if let Err(e) = Self::handle_client(stream, srv_key, peers).await {
                                        trace!("Client {} disconnected: {e}", remote_addr);
                                    }
                                });
                            }
                            Err(e) => {
                                warn!("Accept error on mock DERP relay: {e}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            debug!("Mock DERP relay shutting down");
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            server_key,
            local_addr,
            shutdown_tx,
            server_task,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn relay_url(&self) -> String {
        format!("tcp://{}", self.local_addr)
    }

    pub fn server_public_key(&self) -> [u8; 32] {
        self.server_key.public_key().0
    }

    /// Client connection handler conducting handshake and frame routing.
    async fn handle_client(
        stream: tokio::net::TcpStream,
        server_key: Arc<MeshKeyPair>,
        peers: Arc<Mutex<HashMap<[u8; 32], mpsc::Sender<DerpFrame>>>>,
    ) -> Result<(), MeshError> {
        let mut framed = Framed::new(stream, DerpCodec::new());

        // 1. Send ServerKey
        let srv_pubkey = server_key.public_key().0;
        let challenge = Bytes::from_static(b"ARCHDROP-CHALLENGE-SALT");
        framed
            .send(DerpFrame::ServerKey {
                server_key: srv_pubkey,
                challenge,
            })
            .await?;

        // 2. Await ClientInfo
        let client_info = framed
            .next()
            .await
            .ok_or_else(|| MeshError::ConnectionClosed("Client disconnected before ClientInfo".into()))??;

        let (client_key, timestamp, signature) = match client_info {
            DerpFrame::ClientInfo {
                client_key,
                timestamp,
                signature,
            } => (client_key, timestamp, signature),
            other => {
                return Err(MeshError::Protocol(format!(
                    "Expected ClientInfo (0x02), got {:?}",
                    other.frame_type()
                )))
            }
        };

        // 3. Verify signature
        MeshKeyPair::verify_challenge(
            &MeshPublicKey(client_key),
            &srv_pubkey,
            timestamp,
            &signature,
            120,
        )?;

        // 4. Send ServerInfo
        let info_data = Bytes::from_static(b"{\"version\":1,\"status\":\"ok\"}");
        framed.send(DerpFrame::ServerInfo { data: info_data }).await?;

        // 5. Register peer and notify others
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<DerpFrame>(128);
        {
            let mut map = peers.lock().await;
            for (peer_key, tx) in map.iter() {
                let _ = tx
                    .send(DerpFrame::PeerPresent {
                        peer_key: client_key,
                    })
                    .await;
                let _ = outbound_tx
                    .send(DerpFrame::PeerPresent {
                        peer_key: *peer_key,
                    })
                    .await;
            }
            map.insert(client_key, outbound_tx.clone());
        }

        debug!(client = %hex::encode_upper(client_key), "Registered peer on mock DERP relay");

        // 6. Split and loop: forward inbound and outbound
        let (mut sink, mut source) = framed.split();

        let writer_task = tokio::spawn(async move {
            while let Some(frame) = outbound_rx.recv().await {
                if sink.send(frame).await.is_err() {
                    break;
                }
            }
        });

        let outbound_tx_self = outbound_tx.clone();
        while let Some(frame_res) = source.next().await {
            match frame_res {
                Ok(DerpFrame::SendPacket { dst_key, payload }) => {
                    let map = peers.lock().await;
                    if let Some(dst_tx) = map.get(&dst_key) {
                        let recv_frame = DerpFrame::RecvPacket {
                            src_key: client_key,
                            payload,
                        };
                        let _ = dst_tx.send(recv_frame).await;
                    } else {
                        trace!(
                            dst = %hex::encode_upper(dst_key),
                            "Dropping packet for unreachable peer on relay"
                        );
                    }
                }
                Ok(DerpFrame::KeepAlive) => {
                    trace!("KeepAlive from peer");
                }
                Ok(DerpFrame::Ping { nonce }) => {
                    let _ = outbound_tx_self.send(DerpFrame::Pong { nonce }).await;
                }
                Ok(_) => {}
                Err(e) => {
                    debug!("Peer {} read error: {e}", hex::encode_upper(client_key));
                    break;
                }
            }
        }

        // 7. Unregister and notify others
        writer_task.abort();
        {
            let mut map = peers.lock().await;
            map.remove(&client_key);
            for tx in map.values() {
                let _ = tx
                    .send(DerpFrame::PeerGone {
                        peer_key: client_key,
                    })
                    .await;
            }
        }

        info!(client = %hex::encode_upper(client_key), "Peer unregistered from mock DERP relay");
        Ok(())
    }

    /// Stops the mock relay.
    pub async fn stop(self) {
        let _ = self.shutdown_tx.send(true);
        self.server_task.abort();
    }
}
