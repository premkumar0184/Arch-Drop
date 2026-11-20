use thiserror::Error;

#[derive(Debug, Error)]
pub enum MeshError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Frame encoding/decoding error: {0}")]
    Frame(String),

    #[error("Protocol violation: {0}")]
    Protocol(String),

    #[error("Cryptographic error: {0}")]
    Crypto(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Peer not found or unreachable: {0}")]
    PeerUnreachable(String),

    #[error("Connection closed or rejected: {0}")]
    ConnectionClosed(String),

    #[error("Connection timeout: {0}")]
    Timeout(String),

    #[error("Stream reset or aborted: {0}")]
    StreamReset(String),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("Configuration error: {0}")]
    Config(String),
}
