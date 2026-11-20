//! Binary DERP frame codec and wire protocol definitions.
//!
//! Implements length-prefixed framing:
//! `[FrameType: 1B][PayloadLength: 4B BE][Payload: N bytes]`
//! Supports all 10 canonical frame types:
//! - ServerKey (0x01)
//! - ClientInfo (0x02)
//! - ServerInfo (0x03)
//! - SendPacket (0x04)
//! - RecvPacket (0x05)
//! - KeepAlive (0x06)
//! - PeerGone (0x07)
//! - PeerPresent (0x08)
//! - Ping (0x09)
//! - Pong (0x0a)

use crate::error::MeshError;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Maximum allowed payload size for a single DERP frame (16 MiB) to guard against DoS / memory exhaustion.
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Canonical DERP frame type codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameType {
    ServerKey = 0x01,
    ClientInfo = 0x02,
    ServerInfo = 0x03,
    SendPacket = 0x04,
    RecvPacket = 0x05,
    KeepAlive = 0x06,
    PeerGone = 0x07,
    PeerPresent = 0x08,
    Ping = 0x09,
    Pong = 0x0a,
}

impl FrameType {
    pub fn from_u8(val: u8) -> Result<Self, MeshError> {
        match val {
            0x01 => Ok(FrameType::ServerKey),
            0x02 => Ok(FrameType::ClientInfo),
            0x03 => Ok(FrameType::ServerInfo),
            0x04 => Ok(FrameType::SendPacket),
            0x05 => Ok(FrameType::RecvPacket),
            0x06 => Ok(FrameType::KeepAlive),
            0x07 => Ok(FrameType::PeerGone),
            0x08 => Ok(FrameType::PeerPresent),
            0x09 => Ok(FrameType::Ping),
            0x0a => Ok(FrameType::Pong),
            // Legacy / alternate type aliases for compatibility
            0x0c => Ok(FrameType::Ping),
            0x0d => Ok(FrameType::Pong),
            other => Err(MeshError::Frame(format!("Unknown frame type: 0x{:02x}", other))),
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// Strongly typed DERP protocol frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerpFrame {
    /// Relay public key and optional challenge salt sent to connecting client.
    ServerKey {
        server_key: [u8; 32],
        challenge: Bytes,
    },
    /// Client authentication sent to relay: client public key, Unix timestamp, and Ed25519 signature.
    ClientInfo {
        client_key: [u8; 32],
        timestamp: u64,
        signature: [u8; 64],
    },
    /// Server acceptance metadata and session parameters (JSON or UTF-8).
    ServerInfo {
        data: Bytes,
    },
    /// Forward packet destined to a remote peer.
    SendPacket {
        dst_key: [u8; 32],
        payload: Bytes,
    },
    /// Packet delivered to client from a remote peer.
    RecvPacket {
        src_key: [u8; 32],
        payload: Bytes,
    },
    /// Empty heartbeat frame to maintain NAT mappings.
    KeepAlive,
    /// Relay notification that a peer is offline.
    PeerGone {
        peer_key: [u8; 32],
    },
    /// Relay notification that a peer is online.
    PeerPresent {
        peer_key: [u8; 32],
    },
    /// Ping probe with 8-byte nonce.
    Ping {
        nonce: [u8; 8],
    },
    /// Pong echo with matching 8-byte nonce.
    Pong {
        nonce: [u8; 8],
    },
}

impl DerpFrame {
    /// Returns the frame type for this variant.
    pub fn frame_type(&self) -> FrameType {
        match self {
            DerpFrame::ServerKey { .. } => FrameType::ServerKey,
            DerpFrame::ClientInfo { .. } => FrameType::ClientInfo,
            DerpFrame::ServerInfo { .. } => FrameType::ServerInfo,
            DerpFrame::SendPacket { .. } => FrameType::SendPacket,
            DerpFrame::RecvPacket { .. } => FrameType::RecvPacket,
            DerpFrame::KeepAlive => FrameType::KeepAlive,
            DerpFrame::PeerGone { .. } => FrameType::PeerGone,
            DerpFrame::PeerPresent { .. } => FrameType::PeerPresent,
            DerpFrame::Ping { .. } => FrameType::Ping,
            DerpFrame::Pong { .. } => FrameType::Pong,
        }
    }

    /// Serializes the frame into binary length-prefixed bytes.
    pub fn encode(&self, dst: &mut BytesMut) -> Result<(), MeshError> {
        let ftype = self.frame_type();
        match self {
            DerpFrame::ServerKey { server_key, challenge } => {
                let payload_len = 32 + challenge.len();
                if payload_len > MAX_FRAME_SIZE {
                    return Err(MeshError::Frame("ServerKey payload exceeds limit".into()));
                }
                dst.reserve(5 + payload_len);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(payload_len as u32);
                dst.put_slice(server_key);
                dst.put_slice(challenge);
            }
            DerpFrame::ClientInfo { client_key, timestamp, signature } => {
                let payload_len = 32 + 8 + 64;
                dst.reserve(5 + payload_len);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(payload_len as u32);
                dst.put_slice(client_key);
                dst.put_u64(*timestamp);
                dst.put_slice(signature);
            }
            DerpFrame::ServerInfo { data } => {
                let payload_len = data.len();
                if payload_len > MAX_FRAME_SIZE {
                    return Err(MeshError::Frame("ServerInfo payload exceeds limit".into()));
                }
                dst.reserve(5 + payload_len);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(payload_len as u32);
                dst.put_slice(data);
            }
            DerpFrame::SendPacket { dst_key, payload } => {
                let payload_len = 32 + payload.len();
                if payload_len > MAX_FRAME_SIZE {
                    return Err(MeshError::Frame("SendPacket payload exceeds limit".into()));
                }
                dst.reserve(5 + payload_len);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(payload_len as u32);
                dst.put_slice(dst_key);
                dst.put_slice(payload);
            }
            DerpFrame::RecvPacket { src_key, payload } => {
                let payload_len = 32 + payload.len();
                if payload_len > MAX_FRAME_SIZE {
                    return Err(MeshError::Frame("RecvPacket payload exceeds limit".into()));
                }
                dst.reserve(5 + payload_len);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(payload_len as u32);
                dst.put_slice(src_key);
                dst.put_slice(payload);
            }
            DerpFrame::KeepAlive => {
                dst.reserve(5);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(0);
            }
            DerpFrame::PeerGone { peer_key } => {
                dst.reserve(5 + 32);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(32);
                dst.put_slice(peer_key);
            }
            DerpFrame::PeerPresent { peer_key } => {
                dst.reserve(5 + 32);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(32);
                dst.put_slice(peer_key);
            }
            DerpFrame::Ping { nonce } => {
                dst.reserve(5 + 8);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(8);
                dst.put_slice(nonce);
            }
            DerpFrame::Pong { nonce } => {
                dst.reserve(5 + 8);
                dst.put_u8(ftype.to_u8());
                dst.put_u32(8);
                dst.put_slice(nonce);
            }
        }
        Ok(())
    }

    /// Attempts to parse a frame from the provided buffer.
    ///
    /// Returns `Ok(Some(frame))` if a complete frame is decoded, advancing the buffer.
    /// Returns `Ok(None)` if more data is required.
    /// Returns `Err(MeshError)` on format or boundary violations.
    pub fn decode(src: &mut BytesMut) -> Result<Option<Self>, MeshError> {
        Self::decode_with_limit(src, MAX_FRAME_SIZE)
    }

    /// Attempts to parse a frame from the provided buffer with an explicit maximum payload length limit.
    pub fn decode_with_limit(src: &mut BytesMut, max_limit: usize) -> Result<Option<Self>, MeshError> {
        if src.len() < 5 {
            return Ok(None);
        }

        let frame_type_byte = src[0];
        let payload_len = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;

        if payload_len > max_limit {
            return Err(MeshError::Frame(format!(
                "Frame length {payload_len} exceeds maximum allowed {max_limit}"
            )));
        }

        let total_len = 5 + payload_len;
        if src.len() < total_len {
            // Wait for more data
            src.reserve(total_len - src.len());
            return Ok(None);
        }

        let ftype = FrameType::from_u8(frame_type_byte)?;
        src.advance(5);
        let mut payload = src.split_to(payload_len);

        let frame = match ftype {
            FrameType::ServerKey => {
                if payload.len() < 32 {
                    return Err(MeshError::Frame(format!(
                        "ServerKey payload too short: {} < 32",
                        payload.len()
                    )));
                }
                let mut server_key = [0u8; 32];
                server_key.copy_from_slice(&payload[..32]);
                let challenge = payload.split_off(32).freeze();
                DerpFrame::ServerKey {
                    server_key,
                    challenge,
                }
            }
            FrameType::ClientInfo => {
                if payload.len() < 104 {
                    return Err(MeshError::Frame(format!(
                        "ClientInfo payload too short: {} < 104",
                        payload.len()
                    )));
                }
                let mut client_key = [0u8; 32];
                client_key.copy_from_slice(&payload[..32]);
                let timestamp = u64::from_be_bytes(payload[32..40].try_into().unwrap());
                let mut signature = [0u8; 64];
                signature.copy_from_slice(&payload[40..104]);
                DerpFrame::ClientInfo {
                    client_key,
                    timestamp,
                    signature,
                }
            }
            FrameType::ServerInfo => DerpFrame::ServerInfo {
                data: payload.freeze(),
            },
            FrameType::SendPacket => {
                if payload.len() < 32 {
                    return Err(MeshError::Frame(format!(
                        "SendPacket payload too short: {} < 32",
                        payload.len()
                    )));
                }
                let mut dst_key = [0u8; 32];
                dst_key.copy_from_slice(&payload[..32]);
                let packet = payload.split_off(32).freeze();
                DerpFrame::SendPacket { dst_key, payload: packet }
            }
            FrameType::RecvPacket => {
                if payload.len() < 32 {
                    return Err(MeshError::Frame(format!(
                        "RecvPacket payload too short: {} < 32",
                        payload.len()
                    )));
                }
                let mut src_key = [0u8; 32];
                src_key.copy_from_slice(&payload[..32]);
                let packet = payload.split_off(32).freeze();
                DerpFrame::RecvPacket { src_key, payload: packet }
            }
            FrameType::KeepAlive => DerpFrame::KeepAlive,
            FrameType::PeerGone => {
                if payload.len() < 32 {
                    return Err(MeshError::Frame(format!(
                        "PeerGone payload too short: {} < 32",
                        payload.len()
                    )));
                }
                let mut peer_key = [0u8; 32];
                peer_key.copy_from_slice(&payload[..32]);
                DerpFrame::PeerGone { peer_key }
            }
            FrameType::PeerPresent => {
                if payload.len() < 32 {
                    return Err(MeshError::Frame(format!(
                        "PeerPresent payload too short: {} < 32",
                        payload.len()
                    )));
                }
                let mut peer_key = [0u8; 32];
                peer_key.copy_from_slice(&payload[..32]);
                DerpFrame::PeerPresent { peer_key }
            }
            FrameType::Ping => {
                if payload.len() < 8 {
                    return Err(MeshError::Frame(format!(
                        "Ping payload too short: {} < 8",
                        payload.len()
                    )));
                }
                let mut nonce = [0u8; 8];
                nonce.copy_from_slice(&payload[..8]);
                DerpFrame::Ping { nonce }
            }
            FrameType::Pong => {
                if payload.len() < 8 {
                    return Err(MeshError::Frame(format!(
                        "Pong payload too short: {} < 8",
                        payload.len()
                    )));
                }
                let mut nonce = [0u8; 8];
                nonce.copy_from_slice(&payload[..8]);
                DerpFrame::Pong { nonce }
            }
        };

        Ok(Some(frame))
    }
}

/// Tokio codec adapter for reading and writing `DerpFrame` instances over async streams.
#[derive(Debug, Clone)]
pub struct DerpCodec {
    max_frame_size: usize,
}

impl Default for DerpCodec {
    fn default() -> Self {
        Self {
            max_frame_size: MAX_FRAME_SIZE,
        }
    }
}

impl DerpCodec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_frame_size(max_frame_size: usize) -> Self {
        Self { max_frame_size }
    }

    pub fn max_frame_size(&self) -> usize {
        self.max_frame_size
    }
}

impl Decoder for DerpCodec {
    type Item = DerpFrame;
    type Error = MeshError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        DerpFrame::decode_with_limit(src, self.max_frame_size)
    }
}

impl Encoder<DerpFrame> for DerpCodec {
    type Error = MeshError;

    fn encode(&mut self, item: DerpFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst)
    }
}
