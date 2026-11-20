//! Virtual bidirectional stream multiplexer over DERP packets.
//!
//! Provides `DerpStream`, implementing `tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static`.
//! Multiplexes reliable byte streams over DERP `SendPacket` and `RecvPacket` frames with control flags:
//! - SYN (0x01): Stream initiation
//! - DATA (0x02): Payload data
//! - FIN (0x04): Half-close shutdown
//! - RST (0x08): Immediate abort
//! - ACK (0x10): Window acknowledgment

use crate::crypto::NoiseSession;
use crate::error::MeshError;
use crate::frame::DerpFrame;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tracing::trace;

pub const FLAG_SYN: u8 = 0x01;
pub const FLAG_DATA: u8 = 0x02;
pub const FLAG_FIN: u8 = 0x04;
pub const FLAG_RST: u8 = 0x08;
pub const FLAG_ACK: u8 = 0x10;

pub const STREAM_HEADER_LEN: usize = 10;
pub const DEFAULT_CHUNK_SIZE: usize = 32 * 1024;

const STATE_OPEN: u8 = 0;
const STATE_LOCAL_CLOSED: u8 = 1;
const STATE_REMOTE_CLOSED: u8 = 2;
const STATE_CLOSED: u8 = 3;

/// Encodes a virtual stream chunk with the 10-byte header:
/// `[stream_id: 8B BE][flags: 1B][reserved: 1B][data]`
pub fn encode_stream_payload(stream_id: u64, flags: u8, data: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(STREAM_HEADER_LEN + data.len());
    buf.put_u64(stream_id);
    buf.put_u8(flags);
    buf.put_u8(0x00);
    buf.put_slice(data);
    buf.freeze()
}

/// Decodes the 10-byte virtual stream header and payload from raw packet bytes:
/// Returns `(stream_id, flags, data)`
pub fn decode_stream_payload(mut payload: Bytes) -> Result<(u64, u8, Bytes), MeshError> {
    if payload.len() < STREAM_HEADER_LEN {
        return Err(MeshError::Protocol(format!(
            "Stream packet too short: {} < {STREAM_HEADER_LEN}",
            payload.len()
        )));
    }
    let stream_id = payload.get_u64();
    let flags = payload.get_u8();
    let _reserved = payload.get_u8();
    Ok((stream_id, flags, payload))
}

/// Virtual bidirectional stream multiplexed over DERP packet envelopes.
pub struct DerpStream {
    stream_id: u64,
    local_key: [u8; 32],
    remote_key: [u8; 32],
    outbound_tx: mpsc::Sender<DerpFrame>,
    inbound_rx: mpsc::Receiver<Bytes>,
    read_buffer: Bytes,
    write_buffer: BytesMut,
    pending_frame: Option<DerpFrame>,
    state: Arc<AtomicU8>,
    chunk_size: usize,
    session: Option<NoiseSession>,
}

impl DerpStream {
    /// Creates a new `DerpStream` attached to an outbound frame queue and inbound data receiver.
    pub fn new(
        stream_id: u64,
        local_key: [u8; 32],
        remote_key: [u8; 32],
        outbound_tx: mpsc::Sender<DerpFrame>,
        inbound_rx: mpsc::Receiver<Bytes>,
    ) -> Self {
        Self {
            stream_id,
            local_key,
            remote_key,
            outbound_tx,
            inbound_rx,
            read_buffer: Bytes::new(),
            write_buffer: BytesMut::with_capacity(DEFAULT_CHUNK_SIZE),
            pending_frame: None,
            state: Arc::new(AtomicU8::new(STATE_OPEN)),
            chunk_size: DEFAULT_CHUNK_SIZE,
            session: None,
        }
    }

    /// Attaches an established Noise full-duplex session for AEAD payload encryption.
    pub fn with_session(mut self, session: NoiseSession) -> Self {
        self.session = Some(session);
        self
    }

    /// Sets or replaces the Noise session on this stream.
    pub fn set_session(&mut self, session: NoiseSession) {
        self.session = Some(session);
    }

    /// Returns a reference to the active Noise session, if encrypted.
    pub fn session(&self) -> Option<&NoiseSession> {
        self.session.as_ref()
    }

    /// Returns a mutable reference to the active Noise session, if encrypted.
    pub fn session_mut(&mut self) -> Option<&mut NoiseSession> {
        self.session.as_mut()
    }

    /// Returns whether this stream is operating in Noise AEAD encrypted mode.
    pub fn is_encrypted(&self) -> bool {
        self.session.is_some()
    }

    /// Automatically derives and enables deterministic end-to-end AEAD encryption.
    pub fn enable_encryption(&mut self, is_initiator: bool) -> Result<(), MeshError> {
        let session = crate::crypto::derive_stream_keys(
            &self.local_key,
            &self.remote_key,
            self.stream_id,
            is_initiator,
        )?;
        self.session = Some(session);
        Ok(())
    }

    /// Creates a new `DerpStream` with deterministic end-to-end encryption enabled.
    pub fn new_encrypted(
        stream_id: u64,
        local_key: [u8; 32],
        remote_key: [u8; 32],
        outbound_tx: mpsc::Sender<DerpFrame>,
        inbound_rx: mpsc::Receiver<Bytes>,
        is_initiator: bool,
    ) -> Result<Self, MeshError> {
        let mut stream = Self::new(stream_id, local_key, remote_key, outbound_tx, inbound_rx);
        stream.enable_encryption(is_initiator)?;
        Ok(stream)
    }

    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub fn local_key(&self) -> &[u8; 32] {
        &self.local_key
    }

    pub fn remote_key(&self) -> &[u8; 32] {
        &self.remote_key
    }

    /// Sets the maximum chunk size before an automatic flush is triggered.
    pub fn set_chunk_size(&mut self, chunk_size: usize) {
        self.chunk_size = chunk_size.max(512);
    }

    /// Creates an interconnected pair of encrypted in-memory `DerpStream` instances for testing.
    pub fn encrypted_pair() -> Result<(Self, Self), MeshError> {
        let key_a = [0x01u8; 32];
        let key_b = [0x02u8; 32];
        let (mut stream_a, mut stream_b) = Self::pair();
        let session_a = crate::crypto::derive_stream_keys(&key_a, &key_b, 1, true)?;
        let session_b = crate::crypto::derive_stream_keys(&key_b, &key_a, 1, false)?;
        stream_a.set_session(session_a);
        stream_b.set_session(session_b);
        Ok((stream_a, stream_b))
    }

    /// Creates an interconnected pair of in-memory `DerpStream` instances for testing.
    pub fn pair() -> (Self, Self) {
        let (outbound_a_tx, mut outbound_a_rx) = mpsc::channel::<DerpFrame>(128);
        let (inbound_a_tx, inbound_a_rx) = mpsc::channel::<Bytes>(128);

        let (outbound_b_tx, mut outbound_b_rx) = mpsc::channel::<DerpFrame>(128);
        let (inbound_b_tx, inbound_b_rx) = mpsc::channel::<Bytes>(128);

        let key_a = [0x01u8; 32];
        let key_b = [0x02u8; 32];

        let stream_a = Self::new(1, key_a, key_b, outbound_a_tx, inbound_a_rx);
        let stream_b = Self::new(1, key_b, key_a, outbound_b_tx, inbound_b_rx);

        // Forward frames from A to B
        tokio::spawn(async move {
            while let Some(frame) = outbound_a_rx.recv().await {
                if let DerpFrame::SendPacket { payload, .. } = frame {
                    if let Ok((_sid, flags, data)) = decode_stream_payload(payload) {
                        if flags & FLAG_FIN != 0 {
                            let _ = inbound_b_tx.send(Bytes::new()).await;
                            break;
                        }
                        if flags & FLAG_RST != 0
                            || (flags & FLAG_DATA != 0 && inbound_b_tx.send(data).await.is_err())
                        {
                            break;
                        }
                    }
                }
            }
        });

        // Forward frames from B to A
        tokio::spawn(async move {
            while let Some(frame) = outbound_b_rx.recv().await {
                if let DerpFrame::SendPacket { payload, .. } = frame {
                    if let Ok((_sid, flags, data)) = decode_stream_payload(payload) {
                        if flags & FLAG_FIN != 0 {
                            let _ = inbound_a_tx.send(Bytes::new()).await;
                            break;
                        }
                        if flags & FLAG_RST != 0
                            || (flags & FLAG_DATA != 0 && inbound_a_tx.send(data).await.is_err())
                        {
                            break;
                        }
                    }
                }
            }
        });

        (stream_a, stream_b)
    }
}

impl AsyncRead for DerpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            // 1. Drain any remaining bytes from the current chunk buffer
            if self.read_buffer.has_remaining() {
                let to_copy = self.read_buffer.remaining().min(buf.remaining());
                buf.put_slice(&self.read_buffer[..to_copy]);
                self.read_buffer.advance(to_copy);
                return Poll::Ready(Ok(()));
            }

            // 2. Buffer is empty; check if stream is marked closed
            let current_state = self.state.load(Ordering::Relaxed);
            if current_state == STATE_REMOTE_CLOSED || current_state == STATE_CLOSED {
                return Poll::Ready(Ok(()));
            }

            // 3. Poll inbound channel for the next chunk
            match self.inbound_rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    if chunk.is_empty() {
                        // Empty chunk represents EOF (remote sent FIN)
                        let prev = self.state.fetch_or(STATE_REMOTE_CLOSED, Ordering::SeqCst);
                        if prev & STATE_LOCAL_CLOSED != 0 {
                            self.state.store(STATE_CLOSED, Ordering::SeqCst);
                        }
                        return Poll::Ready(Ok(()));
                    }

                    let stream_id = self.stream_id;
                    let plaintext = if let Some(ref mut session) = self.session {
                        let mut aad = [0u8; 10];
                        aad[..8].copy_from_slice(&stream_id.to_be_bytes());
                        aad[8] = FLAG_DATA;
                        aad[9] = 0x00;
                        let decrypted = session.decrypt_with_ad(&aad, &chunk).map_err(|e| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("Stream decryption / AEAD authentication failed: {e}"),
                            )
                        })?;
                        Bytes::from(decrypted)
                    } else {
                        chunk
                    };

                    self.read_buffer = plaintext;
                    // Loop around to copy bytes into buf
                }
                Poll::Ready(None) => {
                    // Inbound channel closed
                    self.state.store(STATE_CLOSED, Ordering::SeqCst);
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

impl AsyncWrite for DerpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let current_state = self.state.load(Ordering::Relaxed);
        if current_state & STATE_LOCAL_CLOSED != 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "DerpStream local end is closed",
            )));
        }

        // If write buffer is full, flush it first
        if self.write_buffer.len() >= self.chunk_size {
            match self.as_mut().poll_flush(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }

        let available = self.chunk_size.saturating_sub(self.write_buffer.len());
        let to_write = buf.len().min(available);
        if to_write > 0 {
            self.write_buffer.put_slice(&buf[..to_write]);
            Poll::Ready(Ok(to_write))
        } else {
            // Buffer exactly full, try flushing
            match self.as_mut().poll_flush(cx) {
                Poll::Ready(Ok(())) => {
                    let to_write = buf.len().min(self.chunk_size);
                    self.write_buffer.put_slice(&buf[..to_write]);
                    Poll::Ready(Ok(to_write))
                }
                Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        // 1. Flush any pending frame that was delayed due to a full channel
        if let Some(frame) = this.pending_frame.take() {
            match this.outbound_tx.try_send(frame) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(frame)) => {
                    this.pending_frame = Some(frame);
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "DERP outbound channel closed",
                    )));
                }
            }
        }

        // 2. Package write buffer into SendPacket if not empty
        if !this.write_buffer.is_empty() {
            let chunk_data = this.write_buffer.split().freeze();
            let chunk_len = chunk_data.len();
            let stream_id = this.stream_id;
            let payload_bytes = if let Some(ref mut session) = this.session {
                let mut aad = [0u8; 10];
                aad[..8].copy_from_slice(&stream_id.to_be_bytes());
                aad[8] = FLAG_DATA;
                aad[9] = 0x00;
                let encrypted = session.encrypt_with_ad(&aad, &chunk_data).map_err(|e| {
                    io::Error::new(io::ErrorKind::Other, format!("Stream encryption error: {e}"))
                })?;
                Bytes::from(encrypted)
            } else {
                chunk_data
            };

            let payload = encode_stream_payload(stream_id, FLAG_DATA, &payload_bytes);
            let frame = DerpFrame::SendPacket {
                dst_key: this.remote_key,
                payload,
            };

            match this.outbound_tx.try_send(frame) {
                Ok(()) => {
                    trace!(
                        stream_id = stream_id,
                        bytes = chunk_len,
                        "Flushed DerpStream chunk"
                    );
                }
                Err(mpsc::error::TrySendError::Full(f)) => {
                    this.pending_frame = Some(f);
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "DERP outbound channel closed",
                    )));
                }
            }
        }

        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // 1. Flush any pending data
        match self.as_mut().poll_flush(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }

        let this = self.get_mut();
        let current_state = this.state.load(Ordering::SeqCst);
        if current_state & STATE_LOCAL_CLOSED != 0 {
            return Poll::Ready(Ok(()));
        }

        // 2. Send FLAG_FIN frame
        let payload = encode_stream_payload(this.stream_id, FLAG_FIN, &[]);
        let frame = DerpFrame::SendPacket {
            dst_key: this.remote_key,
            payload,
        };

        match this.outbound_tx.try_send(frame) {
            Ok(()) => {
                let prev = this.state.fetch_or(STATE_LOCAL_CLOSED, Ordering::SeqCst);
                if prev & STATE_REMOTE_CLOSED != 0 {
                    this.state.store(STATE_CLOSED, Ordering::SeqCst);
                }
                Poll::Ready(Ok(()))
            }
            Err(mpsc::error::TrySendError::Full(f)) => {
                this.pending_frame = Some(f);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                this.state.store(STATE_CLOSED, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl Drop for DerpStream {
    fn drop(&mut self) {
        let prev = self.state.load(Ordering::SeqCst);
        if prev != STATE_CLOSED && (prev & STATE_LOCAL_CLOSED == 0) {
            // Attempt best-effort RST notification
            let payload = encode_stream_payload(self.stream_id, FLAG_RST, &[]);
            let frame = DerpFrame::SendPacket {
                dst_key: self.remote_key,
                payload,
            };
            let _ = self.outbound_tx.try_send(frame);
        }
    }
}
