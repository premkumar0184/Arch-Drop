//! Domain data models and wire serialization schemas for Arch Drop.
//!
//! Submodules:
//! - [`dto_v2`]: Exact LocalSend v2.2 wire protocol DTOs and custom serde serializers.
//! - [`clipboard`]: Universal clipboard models enforcing the 4 KB threshold rule and KDE Connect compatibility.
//! - [`device`]: Local device identity and remote peer state representations.

pub mod clipboard;
pub mod device;
pub mod dto_v2;

// Re-export LocalSend v2 DTOs
pub use dto_v2::{
    device_type_v2, protocol_type_v2, CancelQueryParams, DeviceType, FileDto, FileMetadata,
    InfoResponseDtoV2, PrepareUploadRequestDtoV2, PrepareUploadResponseDtoV2, ProtocolType,
    RegisterDtoV2, RegisterResponseDtoV2, UploadQueryParams, DEFAULT_PORT, PROTOCOL_VERSION_V2,
};

// Re-export Clipboard data structures and constants
pub use clipboard::{
    ClearReason, ClipboardClearPacket, ClipboardDescriptorPacket, ClipboardDescriptorPayload,
    ClipboardInlinePacket, ClipboardInlinePayload, ClipboardItemDto, ClipboardPacket,
    ClipboardPayload, ClipboardType, KdeConnectClipboard, KdeConnectPacket, SessionLockEvent,
    CLIPBOARD_EPHEMERAL_TTL_SECS, CLIPBOARD_INLINE_THRESHOLD,
};

// Re-export Device models and status enums
pub use device::{Device, PairingState, PeerDevice, TransferDirection, TransferStatus};
