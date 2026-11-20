//! Arch Drop Clipboard: native Wayland clipboard adapter (`wl-clipboard-rs`)
//! implementing Apple Continuity reference patterns (4 KB threshold rule,
//! 120-second ephemeral TTL, and session lock zeroization).

pub mod adapter;
pub mod cache;
pub mod error;
pub mod threshold;
pub mod ttl;

pub use adapter::WaylandClipboardAdapter;
pub use cache::ClipboardPayloadCache;
pub use error::ClipboardError;
pub use threshold::{
    compute_sha256, from_kdeconnect_packet, pack_clipboard_data, to_kdeconnect_packet,
    unpack_clipboard_packet, CachedPayload, UnpackedClipboard,
};
pub use ttl::ClipboardTtlManager;
