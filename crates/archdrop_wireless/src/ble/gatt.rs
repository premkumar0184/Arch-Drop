//! GATT service definitions, UUIDs, and credential exchange serialization for Arch Drop.
//!
//! Provides the primary Bluetooth Low Energy GATT communication channel between Arch Linux and Android
//! for triggering Local-Only Hotspot (LOHS) activation and exchanging WPA2/WPA3 credentials.

use bluer::gatt::local::{Application, Characteristic, CharacteristicWrite, CharacteristicWriteMethod, Service};
use futures::FutureExt;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::ble::continuity::hotspot_0x0d::{HotspotStatus0x0E, HotspotWakeup0x0D};
use crate::ble::types::HotspotCredentials;
use crate::error::WirelessError;

/// Primary Arch Drop GATT Service UUID.
pub const ARCHDROP_SERVICE_UUID: Uuid = uuid::uuid!("a4c80001-7d1a-4c91-9e28-3e2840000001");

/// Characteristic 1: Wakeup & Hotspot Trigger Command (Write).
pub const CHAR_WAKEUP_COMMAND_UUID: Uuid = uuid::uuid!("a4c80002-7d1a-4c91-9e28-3e2840000001");

/// Characteristic 2: Local-Only Hotspot (LOHS) Wi-Fi Credentials (Read / Notify).
pub const CHAR_LOHS_CREDENTIALS_UUID: Uuid = uuid::uuid!("a4c80003-7d1a-4c91-9e28-3e2840000001");

/// Characteristic 3: Companion Device Telemetry & Radio State (Read / Notify).
pub const CHAR_DEVICE_TELEMETRY_UUID: Uuid = uuid::uuid!("a4c80004-7d1a-4c91-9e28-3e2840000001");

/// Characteristic 4: Zero-Wi-Fi Clipboard Synchronization (Write / WriteWithoutResponse / Read / Notify).
pub const CHAR_CLIPBOARD_SYNC_UUID: Uuid = uuid::uuid!("a4c80005-7d1a-4c91-9e28-3e2840000001");

/// Standard MIME type identifier: text/plain;charset=utf-8.
pub const CLIPBOARD_MIME_TEXT_PLAIN: u8 = 0x01;
/// Standard MIME type identifier: text/uri-list.
pub const CLIPBOARD_MIME_URI_LIST: u8 = 0x02;

/// Maximum payload length for BLE GATT clipboard synchronization (512 bytes).
pub const MAX_BLE_CLIPBOARD_PAYLOAD: usize = 512;

/// Salt used to derive paired symmetric keys for BLE HMAC authentication.
pub const BLE_KEY_DERIVATION_SALT: &[u8] = b"ARCHDROP-BLE-KEY-DERIVATION-V1";

/// Derives a deterministic 32-byte symmetric paired key from a peer identity hex string or secret.
pub fn derive_paired_key(peer_identity: &str) -> [u8; 32] {
    let cleaned: String = peer_identity
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    let raw_bytes = if !cleaned.is_empty() && cleaned.len() % 2 == 0 {
        hex::decode(&cleaned).unwrap_or_else(|_| peer_identity.as_bytes().to_vec())
    } else {
        peer_identity.as_bytes().to_vec()
    };

    let s_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, BLE_KEY_DERIVATION_SALT);
    let tag = ring::hmac::sign(&s_key, &raw_bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

/// Compact binary representation of a Zero-Wi-Fi BLE GATT clipboard packet.
///
/// Framing format:
/// - Offset 0x00..0x04 (4B): 4-byte HMAC auth token (using rotating epoch paired key).
/// - Offset 0x04..0x05 (1B): MIME type identifier (`0x01` text/plain, `0x02` text/uri-list).
/// - Offset 0x05..0x07 (2B): Big-endian payload length (supporting payloads up to 512 bytes).
/// - Offset 0x07..      (NB): UTF-8 text payload bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleClipboardPacket {
    /// 4-byte HMAC auth token preventing forgeries.
    pub auth_token: [u8; 4],
    /// MIME type code (0x01 for text/plain, 0x02 for text/uri-list).
    pub mime_type: u8,
    /// UTF-8 text payload bytes.
    pub payload: Vec<u8>,
}

impl BleClipboardPacket {
    pub const MIME_TEXT_PLAIN: u8 = CLIPBOARD_MIME_TEXT_PLAIN;
    pub const MIME_URI_LIST: u8 = CLIPBOARD_MIME_URI_LIST;
    pub const MAX_PAYLOAD_LEN: usize = MAX_BLE_CLIPBOARD_PAYLOAD;

    /// Constructs a new clipboard packet signed with the rotating epoch paired key.
    pub fn new(paired_key: &[u8], mime_type: u8, text: &str) -> Result<Self, WirelessError> {
        let payload = text.as_bytes().to_vec();
        if payload.len() > Self::MAX_PAYLOAD_LEN {
            return Err(WirelessError::InvalidFrame {
                reason: format!(
                    "Clipboard payload size {} exceeds BLE GATT limit of {} bytes",
                    payload.len(),
                    Self::MAX_PAYLOAD_LEN
                ),
            });
        }
        let epoch_minute = HotspotWakeup0x0D::current_epoch_minute();
        let auth_token = HotspotWakeup0x0D::compute_auth_token(paired_key, epoch_minute);
        Ok(Self {
            auth_token,
            mime_type,
            payload,
        })
    }

    /// Constructs a packet directly from parts.
    pub fn from_parts(auth_token: [u8; 4], mime_type: u8, payload: Vec<u8>) -> Self {
        Self {
            auth_token,
            mime_type,
            payload,
        }
    }

    /// Serializes the packet into compact framing bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(7 + self.payload.len());
        buf.extend_from_slice(&self.auth_token);
        buf.push(self.mime_type);
        buf.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Parses a clipboard packet from compact wire framing.
    pub fn parse(slice: &[u8]) -> Result<Self, WirelessError> {
        if slice.len() < 7 {
            return Err(WirelessError::FrameTruncated {
                expected: 7,
                actual: slice.len(),
            });
        }
        let mut auth_token = [0u8; 4];
        auth_token.copy_from_slice(&slice[0..4]);
        let mime_type = slice[4];
        let payload_len = u16::from_be_bytes([slice[5], slice[6]]) as usize;
        if payload_len > Self::MAX_PAYLOAD_LEN {
            return Err(WirelessError::InvalidFrame {
                reason: format!("Payload length {payload_len} exceeds maximum 512 bytes"),
            });
        }
        if slice.len() < 7 + payload_len {
            return Err(WirelessError::FrameTruncated {
                expected: 7 + payload_len,
                actual: slice.len(),
            });
        }
        let payload = slice[7..7 + payload_len].to_vec();
        Ok(Self {
            auth_token,
            mime_type,
            payload,
        })
    }

    /// Verifies the 4-byte auth token in constant time with +/- 2 minute clock skew tolerance.
    pub fn verify_auth(&self, paired_key: &[u8]) -> bool {
        let current_minute = HotspotWakeup0x0D::current_epoch_minute();
        for offset in [0i64, -1, 1, -2, 2] {
            let test_minute = (current_minute as i64 + offset).max(0) as u64;
            if self.verify_at_epoch(paired_key, test_minute) {
                return true;
            }
        }
        false
    }

    /// Verifies the 4-byte auth token at an exact epoch minute in constant time.
    pub fn verify_at_epoch(&self, paired_key: &[u8], epoch_minute: u64) -> bool {
        let expected = HotspotWakeup0x0D::compute_auth_token(paired_key, epoch_minute);
        crate::ble::continuity::hotspot_0x0d::constant_time_eq_4(&self.auth_token, &expected)
    }

    /// Verifies the token against a candidate list of paired keys in constant time.
    pub fn verify_against_keys(&self, paired_keys: &[Vec<u8>]) -> bool {
        for key in paired_keys {
            if self.verify_auth(key) {
                return true;
            }
        }
        false
    }

    /// Returns the UTF-8 text payload slice if valid.
    pub fn text(&self) -> Result<&str, WirelessError> {
        std::str::from_utf8(&self.payload).map_err(|e| WirelessError::InvalidFrame {
            reason: format!("Clipboard payload is not valid UTF-8: {e}"),
        })
    }

    /// Returns the MIME type as a static string.
    pub fn mime_str(&self) -> &'static str {
        match self.mime_type {
            Self::MIME_URI_LIST => "text/uri-list",
            _ => "text/plain;charset=utf-8",
        }
    }
}

/// Encodes a wakeup command into bytes for writing to CHAR_WAKEUP_COMMAND.
pub fn encode_wakeup_command(wakeup: &HotspotWakeup0x0D) -> Vec<u8> {
    wakeup.to_bytes().to_vec()
}

/// Decodes a wakeup command received from CHAR_WAKEUP_COMMAND.
pub fn decode_wakeup_command(slice: &[u8]) -> Result<HotspotWakeup0x0D, WirelessError> {
    HotspotWakeup0x0D::parse(slice)
}

/// Encodes HotspotCredentials into JSON bytes for CHAR_LOHS_CREDENTIALS.
pub fn encode_credentials(creds: &HotspotCredentials) -> Result<Vec<u8>, WirelessError> {
    creds.to_bytes().map_err(WirelessError::from)
}

/// Decodes HotspotCredentials received from CHAR_LOHS_CREDENTIALS.
pub fn decode_credentials(slice: &[u8]) -> Result<HotspotCredentials, WirelessError> {
    HotspotCredentials::from_bytes(slice).map_err(WirelessError::from)
}

/// Encodes telemetry status into bytes for CHAR_DEVICE_TELEMETRY.
pub fn encode_telemetry(status: &HotspotStatus0x0E) -> Vec<u8> {
    status.to_bytes().to_vec()
}

/// Decodes telemetry status received from CHAR_DEVICE_TELEMETRY.
pub fn decode_telemetry(slice: &[u8]) -> Result<HotspotStatus0x0E, WirelessError> {
    HotspotStatus0x0E::parse(slice)
}

/// Encodes a clipboard packet into bytes for writing to CHAR_CLIPBOARD_SYNC.
pub fn encode_clipboard_packet(packet: &BleClipboardPacket) -> Vec<u8> {
    packet.to_bytes()
}

/// Decodes a clipboard packet received from CHAR_CLIPBOARD_SYNC.
pub fn decode_clipboard_packet(slice: &[u8]) -> Result<BleClipboardPacket, WirelessError> {
    BleClipboardPacket::parse(slice)
}

/// State container for local GATT server application.
#[derive(Clone, Default)]
pub struct GattServerState {
    /// Active credentials to be served to connecting centrals.
    pub credentials: Arc<RwLock<Option<HotspotCredentials>>>,
    /// Active telemetry to be served.
    pub telemetry: Arc<RwLock<Option<HotspotStatus0x0E>>>,
    /// Active clipboard sync payload to be served.
    pub clipboard_payload: Arc<RwLock<Option<Vec<u8>>>>,
    /// Channel for forwarding incoming GATT writes on CHAR_CLIPBOARD_SYNC.
    pub clipboard_write_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
}

/// Builds a local `bluer::gatt::local::Application` exposing Arch Drop's service and characteristics.
pub fn build_gatt_application(state: GattServerState) -> Application {
    let creds_state = state.credentials.clone();
    let telem_state = state.telemetry.clone();
    let clip_payload_state = state.clipboard_payload.clone();
    let clip_tx = state.clipboard_write_tx.clone();

    Application {
        services: vec![Service {
            uuid: ARCHDROP_SERVICE_UUID,
            primary: true,
            characteristics: vec![
                // 1. Wakeup command (Write)
                Characteristic {
                    uuid: CHAR_WAKEUP_COMMAND_UUID,
                    write: Some(CharacteristicWrite {
                        write: true,
                        write_without_response: true,
                        method: CharacteristicWriteMethod::Fun(Box::new(|val, _req| {
                            tracing::info!(len = val.len(), "GATT Write received on Wakeup Command characteristic");
                            async move { Ok(()) }.boxed()
                        })),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                // 2. LOHS credentials (Read)
                Characteristic {
                    uuid: CHAR_LOHS_CREDENTIALS_UUID,
                    read: Some(bluer::gatt::local::CharacteristicRead {
                        read: true,
                        fun: Box::new(move |_req| {
                            let creds = creds_state.clone();
                            async move {
                                let guard = creds.read().await;
                                if let Some(ref c) = *guard {
                                    encode_credentials(c).map_err(|e| {
                                        tracing::error!("Failed to encode credentials: {e}");
                                        bluer::gatt::local::ReqError::Failed
                                    })
                                } else {
                                    tracing::warn!("GATT Read on credentials, but credentials not ready");
                                    Err(bluer::gatt::local::ReqError::NotPermitted)
                                }
                            }
                            .boxed()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                // 3. Telemetry (Read)
                Characteristic {
                    uuid: CHAR_DEVICE_TELEMETRY_UUID,
                    read: Some(bluer::gatt::local::CharacteristicRead {
                        read: true,
                        fun: Box::new(move |_req| {
                            let telem = telem_state.clone();
                            async move {
                                let guard = telem.read().await;
                                if let Some(ref t) = *guard {
                                    Ok(encode_telemetry(t))
                                } else {
                                    Ok(Vec::new())
                                }
                            }
                            .boxed()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                // 4. Zero-Wi-Fi Clipboard Sync (Write / WriteWithoutResponse / Read / Notify)
                Characteristic {
                    uuid: CHAR_CLIPBOARD_SYNC_UUID,
                    write: Some(CharacteristicWrite {
                        write: true,
                        write_without_response: true,
                        method: CharacteristicWriteMethod::Fun(Box::new(move |val, _req| {
                            tracing::info!(len = val.len(), "GATT Write received on Clipboard Sync characteristic");
                            let tx = clip_tx.clone();
                            async move {
                                if let Some(ref sender) = tx {
                                    let _ = sender.send(val).await;
                                }
                                Ok(())
                            }
                            .boxed()
                        })),
                        ..Default::default()
                    }),
                    read: Some(bluer::gatt::local::CharacteristicRead {
                        read: true,
                        fun: Box::new(move |_req| {
                            let clip = clip_payload_state.clone();
                            async move {
                                let guard = clip.read().await;
                                if let Some(ref data) = *guard {
                                    Ok(data.clone())
                                } else {
                                    Ok(Vec::new())
                                }
                            }
                            .boxed()
                        }),
                        ..Default::default()
                    }),
                    notify: Some(bluer::gatt::local::CharacteristicNotify {
                        notify: true,
                        method: Default::default(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    }
}
