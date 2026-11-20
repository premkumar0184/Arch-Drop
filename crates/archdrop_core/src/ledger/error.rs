use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("SQLite database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Payload hash verification failed for sequence {sequence}: expected {expected}, calculated {actual}")]
    PayloadHashMismatch {
        expected: String,
        actual: String,
        sequence: i64,
    },

    #[error("Sequence gap detected for origin {origin}: expected {expected}, received {received}")]
    GapDetected {
        origin: String,
        expected: i64,
        received: i64,
    },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("System time error: {0}")]
    Time(#[from] std::time::SystemTimeError),

    #[error("{0}")]
    Other(String),
}
