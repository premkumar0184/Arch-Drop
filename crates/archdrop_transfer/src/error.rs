use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Network error: {0}")]
    Network(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("File not found in session: {0}")]
    FileNotFound(String),

    #[error("Invalid or expired session token")]
    InvalidToken,

    #[error("Integrity checksum mismatch: expected {expected}, actual {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("File size mismatch: expected {expected} bytes, received {actual} bytes")]
    SizeMismatch { expected: u64, actual: u64 },

    #[error("Transfer rejected by receiver: {0}")]
    Rejected(String),

    #[error("TLS configuration error: {0}")]
    Tls(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for TransferError {
    fn from(err: serde_json::Error) -> Self {
        TransferError::Serialization(err.to_string())
    }
}
