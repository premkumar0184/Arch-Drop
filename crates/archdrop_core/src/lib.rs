//! Arch Drop Core: shared data models, persistent configuration loader,
//! X.509 self-signed cryptographic identity engine, and wire schemas.

pub mod config;
pub mod crypto;
pub mod error;
pub mod ledger;
pub mod models;

// Public re-exports for flat ergonomics
pub use config::{
    expand_tilde, AppConfig, ClipboardConfig, Config, DeviceConfig, NetworkConfig, SecurityConfig,
    StorageConfig, DEFAULT_CONFIG_TOML,
};
pub use crypto::{
    fingerprint_from_cert_der, fingerprint_from_cert_pem, normalize_fingerprint,
    public_key_from_cert_der, validate_fingerprint, verify_cert_from_der, verify_cert_from_pem,
    CryptoIdentity,
};
pub use error::{ArchdropError, ConfigError, CryptoError};
pub use ledger::{
    arbitrate_clipboard, arbitrate_lww, AckStatus, Event, EventKey, EventLedger, LedgerError,
    OutboxEntry, SyncSummary, VectorClockEntry,
};

/// Common prelude re-exporting key types and traits for dependent crates.
pub mod prelude {
    pub use crate::config::{AppConfig, Config};
    pub use crate::crypto::{
        fingerprint_from_cert_der, normalize_fingerprint, validate_fingerprint, CryptoIdentity,
    };
    pub use crate::error::{ArchdropError, ConfigError, CryptoError};
    pub use crate::ledger::{
        arbitrate_clipboard, arbitrate_lww, AckStatus, Event, EventKey, EventLedger, LedgerError,
        OutboxEntry, SyncSummary, VectorClockEntry,
    };
    pub use crate::models::clipboard::*;
    pub use crate::models::device::*;
    pub use crate::models::dto_v2::*;
}

