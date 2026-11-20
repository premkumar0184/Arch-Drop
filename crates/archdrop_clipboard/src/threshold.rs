use crate::error::ClipboardError;
use archdrop_core::models::clipboard::{
    ClipboardDescriptorPacket, ClipboardDescriptorPayload, ClipboardInlinePacket,
    ClipboardInlinePayload, ClipboardPacket, KdeConnectPacket, CLIPBOARD_EPHEMERAL_TTL_SECS,
    CLIPBOARD_INLINE_THRESHOLD,
};
pub use base64;
use base64::prelude::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct CachedPayload {
    pub payload_id: String,
    pub mime_type: String,
    pub data: Arc<[u8]>,
    pub token: String,
    pub sha256: String,
    pub created_at: std::time::Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpackedClipboard {
    InlineText {
        mime_type: String,
        text: String,
        sha256: String,
    },
    InlineBinary {
        mime_type: String,
        data: Vec<u8>,
        sha256: String,
    },
    Descriptor {
        payload_id: String,
        mime_type: String,
        size_bytes: usize,
        sha256: String,
        download_url: Option<String>,
        token: String,
    },
    Clear {
        reason: archdrop_core::models::clipboard::ClearReason,
    },
}

pub fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Packs raw clipboard bytes according to Apple Continuity 4 KB threshold rule.
///
/// Returns the wire packet and optionally a `CachedPayload` (if payload > 4 KB)
/// to be held in volatile memory for on-demand fetch.
pub fn pack_clipboard_data(
    device_id: &str,
    mime_type: &str,
    data: &[u8],
    download_url_base: Option<&str>,
) -> (ClipboardPacket, Option<CachedPayload>) {
    let sha256 = compute_sha256(data);
    let size_bytes = data.len();
    let timestamp = current_epoch_ms();

    if size_bytes <= CLIPBOARD_INLINE_THRESHOLD {
        // Inlined tier (<= 4 KB)
        let content_str = match std::str::from_utf8(data) {
            Ok(s) => s.to_string(),
            Err(_) => {
                format!("data:{};base64,{}", mime_type, BASE64_STANDARD.encode(data))
            }
        };

        let packet = ClipboardPacket::Inline(ClipboardInlinePacket {
            version: 2,
            device_id: device_id.to_string(),
            timestamp,
            ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
            payload: ClipboardInlinePayload {
                mime_type: mime_type.to_string(),
                content: content_str,
                sha256,
                size_bytes,
            },
        });

        (packet, None)
    } else {
        // Descriptor tier (> 4 KB)
        let payload_id = uuid::Uuid::new_v4().to_string();
        let token = uuid::Uuid::new_v4().to_string();

        let download_url = download_url_base.map(|base| {
            format!(
                "{}/api/v2/clipboard/payload/{}?token={}",
                base.trim_end_matches('/'),
                payload_id,
                token
            )
        });

        let packet = ClipboardPacket::Descriptor(ClipboardDescriptorPacket {
            version: 2,
            device_id: device_id.to_string(),
            timestamp,
            ttl_seconds: CLIPBOARD_EPHEMERAL_TTL_SECS,
            descriptor: ClipboardDescriptorPayload {
                payload_id: payload_id.clone(),
                primary_mime: mime_type.to_string(),
                available_mimes: vec![mime_type.to_string()],
                size_bytes,
                sha256: sha256.clone(),
                ephemeral_token: token.clone(),
                download_url,
            },
        });

        let cached = CachedPayload {
            payload_id,
            mime_type: mime_type.to_string(),
            data: Arc::from(data.to_vec().into_boxed_slice()),
            token,
            sha256,
            created_at: std::time::Instant::now(),
        };

        (packet, Some(cached))
    }
}

/// Unpacks a received `ClipboardPacket` into domain representation.
pub fn unpack_clipboard_packet(packet: &ClipboardPacket) -> Result<UnpackedClipboard, ClipboardError> {
    match packet {
        ClipboardPacket::Inline(p) => {
            if p.payload.content.starts_with("data:") && p.payload.content.contains(";base64,") {
                if let Some(b64_part) = p.payload.content.split(";base64,").nth(1) {
                    let clean_b64: String = b64_part.chars().filter(|c| !c.is_whitespace()).collect();
                    if let Ok(decoded) = BASE64_STANDARD.decode(&clean_b64) {
                        return Ok(UnpackedClipboard::InlineBinary {
                            mime_type: p.payload.mime_type.clone(),
                            data: decoded,
                            sha256: p.payload.sha256.clone(),
                        });
                    }
                }
            }

            let is_image_or_non_text = p.payload.mime_type.starts_with("image/")
                || (!p.payload.mime_type.starts_with("text/")
                    && p.payload.mime_type != "application/json"
                    && p.payload.mime_type != "application/xml");
            if is_image_or_non_text {
                let clean_b64: String = p.payload.content.chars().filter(|c| !c.is_whitespace()).collect();
                if !clean_b64.is_empty() {
                    if let Ok(decoded) = BASE64_STANDARD.decode(&clean_b64) {
                        return Ok(UnpackedClipboard::InlineBinary {
                            mime_type: p.payload.mime_type.clone(),
                            data: decoded,
                            sha256: p.payload.sha256.clone(),
                        });
                    }
                }
            }

            Ok(UnpackedClipboard::InlineText {
                mime_type: p.payload.mime_type.clone(),
                text: p.payload.content.clone(),
                sha256: p.payload.sha256.clone(),
            })
        }
        ClipboardPacket::Descriptor(p) => Ok(UnpackedClipboard::Descriptor {
            payload_id: p.descriptor.payload_id.clone(),
            mime_type: p.descriptor.primary_mime.clone(),
            size_bytes: p.descriptor.size_bytes,
            sha256: p.descriptor.sha256.clone(),
            download_url: p.descriptor.download_url.clone(),
            token: p.descriptor.ephemeral_token.clone(),
        }),
        ClipboardPacket::Clear(p) => Ok(UnpackedClipboard::Clear { reason: p.reason }),
    }
}

/// Converts a legacy KDE Connect clipboard packet (`kdeconnect.clipboard`) into a native inline packet.
pub fn from_kdeconnect_packet(device_id: &str, kde: &KdeConnectPacket) -> ClipboardPacket {
    let content = kde.body.content.as_bytes();
    let (packet, _) = pack_clipboard_data(device_id, "text/plain;charset=utf-8", content, None);
    packet
}

/// Converts a native inline packet into a legacy KDE Connect clipboard packet.
pub fn to_kdeconnect_packet(packet: &ClipboardPacket) -> Option<KdeConnectPacket> {
    match packet {
        ClipboardPacket::Inline(p) => Some(KdeConnectPacket::new_clipboard(p.payload.content.clone())),
        _ => None,
    }
}
