//! Arch Drop Transfer: headless LocalSend v2 compatible transfer engine.
//!
//! Provides Hyper 1.11 HTTP/1.1 dual-stack server (:53317) and client with
//! mTLS certificate pinning and token authentication, supporting point-to-point
//! IP dialing over local LAN and WireGuard without multicast discovery.

pub mod client;
pub mod error;
pub mod server;
pub mod session;

pub use client::{TransferClient, TransferSummary};
pub use error::TransferError;
pub use server::{TransferProgressCallback, TransferProgressEvent, TransferServer};
pub use session::{FileTransferState, FileTransferStatus, SessionManager, TransferSession};
