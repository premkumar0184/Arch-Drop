//! Strongly-typed domain errors for `archdrop_core`.

use std::path::PathBuf;
use thiserror::Error;

/// Root error type for `archdrop_core`.
#[derive(Debug, Error)]
pub enum ArchdropError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Cryptographic error: {0}")]
    Crypto(#[from] CryptoError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors occurring during configuration loading, parsing, and persistence.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to determine configuration directory ($XDG_CONFIG_HOME and $HOME are unset)")]
    MissingConfigDir,

    #[error("I/O error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse TOML configuration from '{path}': {source}")]
    ParseToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Failed to serialize configuration to TOML: {source}")]
    SerializeToml {
        #[source]
        source: toml::ser::Error,
    },

    #[error("Invalid configuration value for field '{field}': {message}")]
    Validation {
        field: &'static str,
        message: String,
    },
}

/// Errors occurring during cryptographic operations and certificate validation.
#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("RSA key error: {0}")]
    Rsa(String),

    #[error("rcgen certificate generation error: {0}")]
    Rcgen(#[from] rcgen::Error),

    #[error("X.509 certificate parsing error: {0}")]
    X509Parse(String),

    #[error("PEM parsing error: {0}")]
    Pem(String),

    #[error("Certificate validity expired or not yet valid")]
    ValidityExpired,

    #[error("Public key mismatch")]
    PublicKeyMismatch,

    #[error("Certificate signature verification error: {0}")]
    SignatureVerification(String),

    #[error("Missing identity component in PEM file")]
    MissingIdentityComponent,

    #[error("Invalid fingerprint: {0}")]
    InvalidFingerprint(String),
}
