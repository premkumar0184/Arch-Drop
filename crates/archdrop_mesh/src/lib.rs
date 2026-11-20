//! Arch Drop Mesh: Userspace DERP Relay & WAN NAT Traversal.
//!
//! Provides:
//! - Binary DERP frame codec with 10 frame types (`frame::DerpFrame`)
//! - Ed25519 peer public-key authentication via Ring (`crypto::MeshKeyPair`)
//! - Virtual bidirectional stream multiplexing (`stream::DerpStream`)
//! - Userspace DERP relay client with 20s keepalive heartbeats (`client::DerpClient`)
//! - Transparent tiered fallback dialer (`dialer::FallbackDialer`)
//! - In-memory/embedded mock DERP relay for testing (`mock_relay::MockDerpRelay`)

pub mod client;
pub mod crypto;
pub mod dialer;
pub mod error;
pub mod frame;
pub mod mock_relay;
pub mod stream;
pub mod sync;
pub mod transport;

pub use client::{DerpClient, DerpClientConfig, DerpListener, DEFAULT_KEEPALIVE_INTERVAL};
pub use crypto::{
    compute_hmac_sha256, derive_session_keys, derive_stream_keys, derive_stream_keys_from_secret,
    edwards_priv_to_x25519_priv, edwards_to_montgomery_pub, EphemeralX25519, MeshKeyPair,
    MeshPublicKey, NoiseCipherState, NoiseSession, CHALLENGE_TAG, MAX_HANDSHAKE_SKEW_SECS,
    TAG_LEN,
};
pub use dialer::{
    FallbackDialer, P2pDialer, PeerStream, TransportResolver, TransportTier,
    DEFAULT_DIRECT_TIMEOUT, DEFAULT_P2P_TIMEOUT,
};
pub use error::MeshError;
pub use frame::{DerpCodec, DerpFrame, FrameType, MAX_FRAME_SIZE};
pub use mock_relay::MockDerpRelay;
pub use stream::{
    decode_stream_payload, encode_stream_payload, DerpStream, FLAG_ACK, FLAG_DATA, FLAG_FIN,
    FLAG_RST, FLAG_SYN,
};
pub use sync::{ReconciliationEngine, SyncAck, SyncBatch, SyncStats, VectorExchange, WireEvent};
pub use transport::{connect_transport, create_default_tls_config, create_insecure_tls_config};

