use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Wayland error: {0}")]
    Wayland(String),

    #[error("Wayland compositor not available: {0}")]
    WaylandNotAvailable(String),

    #[error("Clipboard data is invalid UTF-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("Payload not found in volatile cache: {0}")]
    PayloadNotFound(String),

    #[error("Invalid or expired download token")]
    InvalidToken,

    #[error("Clipboard item expired (120-second TTL)")]
    Expired,

    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for ClipboardError {
    fn from(err: serde_json::Error) -> Self {
        ClipboardError::Serialization(err.to_string())
    }
}
