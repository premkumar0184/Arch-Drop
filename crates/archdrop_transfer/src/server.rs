use crate::error::TransferError;
use crate::session::SessionManager;
use archdrop_clipboard::{
    compute_sha256, from_kdeconnect_packet, ClipboardError, ClipboardPayloadCache,
    WaylandClipboardAdapter,
};
use archdrop_core::config::Config;
use archdrop_core::crypto::CryptoIdentity;
use archdrop_core::models::clipboard::{ClipboardPacket, KdeConnectPacket};
use archdrop_core::models::dto_v2::{
    DeviceType, InfoResponseDtoV2, PrepareUploadRequestDtoV2, RegisterDtoV2,
    RegisterResponseDtoV2, PROTOCOL_VERSION_V2,
};
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, StreamBody};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use archdrop_webcam::{MjpegStreamParser, WebcamSink, MODPROBE_DIAGNOSTIC};
use archdrop_audio::{
    AudioCaptureBackend, AudioCodec, AudioEncoder, AudioStreamSession, MockAudioCapture,
    MockVirtualSink, OpusAudioEncoder, PcmPassthroughEncoder, PipeWireCapture, PipeWireVirtualSink,
    VirtualSinkBackend, DEFAULT_VIRTUAL_SINK_NAME,
};
use tokio_stream::{Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

pub type ResponseBody = BoxBody<Bytes, std::convert::Infallible>;

pub type AudioCaptureFactory =
    Arc<dyn Fn() -> Box<dyn AudioCaptureBackend> + Send + Sync>;
pub type VirtualSinkFactory =
    Arc<dyn Fn() -> Box<dyn VirtualSinkBackend> + Send + Sync>;

struct GuardedAudioStream<S> {
    inner: S,
    _guard: AudioStreamingGuard,
}

impl<S: Stream + Unpin> Stream for GuardedAudioStream<S> {
    type Item = S::Item;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

pub struct AudioStreamingGuard {
    is_streaming: Arc<AtomicBool>,
    session: Option<AudioStreamSession>,
}

impl Drop for AudioStreamingGuard {
    fn drop(&mut self) {
        self.is_streaming.store(false, Ordering::SeqCst);
        if let Some(sess) = self.session.take() {
            drop(sess);
        }
        info!("Audio stream disconnected; virtual sink unlinked and streaming lock released");
    }
}

fn pipe_to_wl_copy(mime_type: &str, data: &[u8]) {
    let child = std::process::Command::new("wl-copy")
        .arg("--type")
        .arg(mime_type)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();
    if let Ok(mut c) = child {
        if let Some(mut stdin) = c.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(data);
        }
        let _ = c.wait();
    }
}

fn empty_response(status: StatusCode) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .body(BoxBody::new(Empty::new()))
        .unwrap()
}

fn json_response<T: serde::Serialize>(status: StatusCode, data: &T) -> Response<ResponseBody> {
    match serde_json::to_vec(data) {
        Ok(bytes) => Response::builder()
            .status(status)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(BoxBody::new(Full::new(Bytes::from(bytes))))
            .unwrap(),
        Err(e) => {
            error!("JSON serialization error in response: {}", e);
            empty_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn parse_query_string(query: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = query {
        for pair in q.split('&') {
            let mut parts = pair.splitn(2, '=');
            if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                if let Ok(decoded_k) = urlencoding_decode(k) {
                    if let Ok(decoded_v) = urlencoding_decode(v) {
                        map.insert(decoded_k, decoded_v);
                    }
                }
            }
        }
    }
    map
}

fn urlencoding_decode(s: &str) -> Result<String, ()> {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next().ok_or(())?;
            let h2 = chars.next().ok_or(())?;
            let hex_str = format!("{}{}", h1, h2);
            let byte = u8::from_str_radix(&hex_str, 16).map_err(|_| ())?;
            result.push(byte as char);
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    Ok(result)
}

/// Progress event emitted during file uploads.
#[derive(Debug, Clone)]
pub struct TransferProgressEvent {
    pub session_id: String,
    pub file_id: String,
    pub file_name: String,
    pub current_bytes: u64,
    pub total_bytes: u64,
}

pub type TransferProgressCallback = Arc<dyn Fn(TransferProgressEvent) + Send + Sync + 'static>;

/// Event emitted when an upload transfer completes successfully.
#[derive(Debug, Clone)]
pub struct TransferCompletedEvent {
    pub session_id: String,
    pub file_id: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub peer_alias: String,
    pub is_clipboard_staged: bool,
}

pub type TransferCompletedCallback = Arc<dyn Fn(TransferCompletedEvent) + Send + Sync + 'static>;

/// Event emitted when an upload transfer fails.
#[derive(Debug, Clone)]
pub struct TransferFailedEvent {
    pub session_id: String,
    pub file_id: String,
    pub file_name: String,
    pub error: String,
    pub peer_alias: String,
}

pub type TransferFailedCallback = Arc<dyn Fn(TransferFailedEvent) + Send + Sync + 'static>;

/// Interactive authorization callback invoked for incoming file transfers.
pub type TransferAuthCallback = Arc<
    dyn Fn(String, usize, u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
        + Send
        + Sync
        + 'static,
>;

pub type StagedClipboardCallback = Arc<
    dyn Fn(String, PathBuf, u64, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync
        + 'static,
>;

#[derive(Debug, Clone)]
pub struct PeerEndpoint {
    pub ip: String,
    pub port: u16,
    pub fingerprint: Option<String>,
    pub alias: Option<String>,
    pub last_seen: std::time::Instant,
}

pub type ClipboardReceivedCallback = Arc<dyn Fn(ClipboardPacket) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct TransferServerContext {
    pub config: Arc<Config>,
    pub identity: Arc<CryptoIdentity>,
    pub session_manager: SessionManager,
    pub download_dir: PathBuf,
    pub progress_callback: Option<TransferProgressCallback>,
    pub completion_callback: Option<TransferCompletedCallback>,
    pub failure_callback: Option<TransferFailedCallback>,
    pub auth_callback: Option<TransferAuthCallback>,
    pub clipboard_adapter: Option<WaylandClipboardAdapter>,
    pub clipboard_cache: Option<ClipboardPayloadCache>,
    pub registered_peers: Arc<tokio::sync::RwLock<HashMap<String, PeerEndpoint>>>,
    pub clipboard_callback: Option<ClipboardReceivedCallback>,
    pub server_port: u16,
    pub clipboard_staged_sessions: Arc<tokio::sync::RwLock<HashSet<String>>>,
    pub staged_clipboard_callback: Option<StagedClipboardCallback>,
    pub webcam_sink: Arc<tokio::sync::RwLock<Option<Arc<WebcamSink>>>>,
    pub is_webcam_streaming: Arc<AtomicBool>,
    pub sysfs_root: Option<PathBuf>,
    pub is_audio_streaming: Arc<AtomicBool>,
    pub audio_capture_factory: Arc<tokio::sync::RwLock<Option<AudioCaptureFactory>>>,
    pub virtual_sink_factory: Arc<tokio::sync::RwLock<Option<VirtualSinkFactory>>>,
}

impl TransferServerContext {
    pub fn is_webcam_active(&self) -> bool {
        self.is_webcam_streaming.load(Ordering::SeqCst)
    }

    pub fn is_audio_active(&self) -> bool {
        self.is_audio_streaming.load(Ordering::SeqCst)
    }

    pub async fn get_webcam_sink(&self) -> Option<Arc<WebcamSink>> {
        self.webcam_sink.read().await.clone()
    }

    pub async fn set_webcam_sink_async(&self, sink: Option<Arc<WebcamSink>>) {
        *self.webcam_sink.write().await = sink;
    }
}

/// Transparent stream adapter that yields a single-byte prefix before delegating to the inner stream.
struct PrefixedStream<S> {
    prefix: Option<u8>,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(b) = self.prefix {
            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            self.prefix = None;
            buf.put_slice(&[b]);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub struct TransferServer {
    context: TransferServerContext,
    port: u16,
    bind_addr: String,
    use_tls: bool,
    tls_acceptor: Option<TlsAcceptor>,
}

impl TransferServer {
    pub fn new(
        config: Arc<Config>,
        identity: Arc<CryptoIdentity>,
        download_dir: PathBuf,
        port: u16,
        use_tls: bool,
    ) -> Result<Self, TransferError> {
        let tls_acceptor = if use_tls {
            Some(Self::create_tls_acceptor(&identity)?)
        } else {
            None
        };

        Ok(Self {
            context: TransferServerContext {
                config,
                identity,
                session_manager: SessionManager::new(),
                download_dir,
                progress_callback: None,
                completion_callback: None,
                failure_callback: None,
                auth_callback: None,
                clipboard_adapter: None,
                clipboard_cache: None,
                registered_peers: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
                clipboard_callback: None,
                server_port: port,
                clipboard_staged_sessions: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
                staged_clipboard_callback: None,
                webcam_sink: Arc::new(tokio::sync::RwLock::new(None)),
                is_webcam_streaming: Arc::new(AtomicBool::new(false)),
                sysfs_root: None,
                is_audio_streaming: Arc::new(AtomicBool::new(false)),
                audio_capture_factory: Arc::new(tokio::sync::RwLock::new(None)),
                virtual_sink_factory: Arc::new(tokio::sync::RwLock::new(None)),
            },
            port,
            bind_addr: "0.0.0.0".to_string(),
            use_tls,
            tls_acceptor,
        })
    }

    pub fn set_sysfs_root(&mut self, root: PathBuf) {
        self.context.sysfs_root = Some(root);
    }

    pub fn set_progress_callback(&mut self, callback: TransferProgressCallback) {
        self.context.progress_callback = Some(callback);
    }

    pub fn set_completion_callback(&mut self, callback: TransferCompletedCallback) {
        self.context.completion_callback = Some(callback);
    }

    pub fn set_staged_clipboard_callback(&mut self, callback: StagedClipboardCallback) {
        self.context.staged_clipboard_callback = Some(callback);
    }

    pub fn set_failure_callback(&mut self, callback: TransferFailedCallback) {
        self.context.failure_callback = Some(callback);
    }

    pub fn set_auth_callback(&mut self, callback: TransferAuthCallback) {
        self.context.auth_callback = Some(callback);
    }

    pub fn set_clipboard_adapter(&mut self, adapter: WaylandClipboardAdapter) {
        self.context.clipboard_adapter = Some(adapter);
    }

    pub fn set_clipboard_cache(&mut self, cache: ClipboardPayloadCache) {
        self.context.clipboard_cache = Some(cache);
    }

    pub fn set_clipboard_callback(&mut self, callback: ClipboardReceivedCallback) {
        self.context.clipboard_callback = Some(callback);
    }

    pub fn set_webcam_sink(&mut self, sink: Arc<WebcamSink>) {
        if let Ok(mut lock) = self.context.webcam_sink.try_write() {
            *lock = Some(sink);
        } else {
            let ws = self.context.webcam_sink.clone();
            tokio::spawn(async move {
                *ws.write().await = Some(sink);
            });
        }
    }

    pub fn webcam_sink(&self) -> Arc<tokio::sync::RwLock<Option<Arc<WebcamSink>>>> {
        self.context.webcam_sink.clone()
    }

    pub fn is_webcam_streaming(&self) -> Arc<AtomicBool> {
        self.context.is_webcam_streaming.clone()
    }

    pub fn is_audio_streaming(&self) -> Arc<AtomicBool> {
        self.context.is_audio_streaming.clone()
    }

    pub fn set_audio_capture_factory<F>(&mut self, factory: F)
    where
        F: Fn() -> Box<dyn AudioCaptureBackend> + Send + Sync + 'static,
    {
        let f: AudioCaptureFactory = Arc::new(factory);
        if let Ok(mut lock) = self.context.audio_capture_factory.try_write() {
            *lock = Some(f);
        } else {
            let acf = self.context.audio_capture_factory.clone();
            tokio::spawn(async move {
                *acf.write().await = Some(f);
            });
        }
    }

    pub fn set_virtual_sink_factory<F>(&mut self, factory: F)
    where
        F: Fn() -> Box<dyn VirtualSinkBackend> + Send + Sync + 'static,
    {
        let f: VirtualSinkFactory = Arc::new(factory);
        if let Ok(mut lock) = self.context.virtual_sink_factory.try_write() {
            *lock = Some(f);
        } else {
            let vsf = self.context.virtual_sink_factory.clone();
            tokio::spawn(async move {
                *vsf.write().await = Some(f);
            });
        }
    }

    pub fn set_mock_audio_capture(&mut self, mock: MockAudioCapture) {
        self.set_audio_capture_factory(move || Box::new(mock.clone()));
    }

    pub fn set_mock_virtual_sink(&mut self) {
        self.set_virtual_sink_factory(|| Box::new(MockVirtualSink::new()));
    }

    pub fn registered_peers(&self) -> Arc<tokio::sync::RwLock<HashMap<String, PeerEndpoint>>> {
        self.context.registered_peers.clone()
    }

    fn create_tls_acceptor(identity: &CryptoIdentity) -> Result<TlsAcceptor, TransferError> {
        let cert_der = identity.rustls_cert();
        let key_der = identity.rustls_key();

        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .map_err(|e| TransferError::Tls(format!("Failed to build TLS server config: {}", e)))?;

        Ok(TlsAcceptor::from(Arc::new(server_config)))
    }

    pub fn session_manager(&self) -> SessionManager {
        self.context.session_manager.clone()
    }

    async fn handle_incoming_connection<S>(
        mut stream: S,
        remote_addr: SocketAddr,
        ctx: TransferServerContext,
        tls_acceptor: Option<TlsAcceptor>,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let mut first_byte = [0u8; 1];
        let (prefixed_stream, is_tls) = match stream.read(&mut first_byte).await {
            Ok(1) => {
                let is_tls = first_byte[0] == 0x16;
                (
                    PrefixedStream {
                        prefix: Some(first_byte[0]),
                        inner: stream,
                    },
                    is_tls,
                )
            }
            Ok(_) => return, // EOF on empty connection
            Err(e) => {
                debug!("Failed to read initial byte from {}: {}", remote_addr, e);
                return;
            }
        };

        if let (true, Some(acceptor)) = (is_tls, tls_acceptor) {
            match acceptor.accept(prefixed_stream).await {
                Ok(tls_stream) => {
                    let io = TokioIo::new(tls_stream);
                    let service = service_fn(move |req| {
                        let ctx = ctx.clone();
                        async move {
                            Ok::<_, std::convert::Infallible>(Self::handle_request(req, ctx, remote_addr).await)
                        }
                    });

                    if let Err(err) = auto::Builder::new(TokioExecutor::new())
                        .serve_connection(io, service)
                        .await
                    {
                        debug!("TLS connection error from {}: {}", remote_addr, err);
                    }
                }
                Err(e) => {
                    debug!("TLS handshake failed from {}: {}", remote_addr, e);
                }
            }
        } else {
            let io = TokioIo::new(prefixed_stream);
            let service = service_fn(move |req| {
                let ctx = ctx.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(Self::handle_request(req, ctx, remote_addr).await)
                }
            });

            if let Err(err) = auto::Builder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {
                debug!("Plain HTTP connection error from {}: {}", remote_addr, err);
            }
        }
    }

    pub async fn run(&self, shutdown_rx: tokio::sync::watch::Receiver<bool>) -> Result<(), TransferError> {
        self.run_with_mesh(None, shutdown_rx).await
    }

    pub async fn run_with_mesh(
        &self,
        derp_listener: Option<archdrop_mesh::client::DerpListener>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), TransferError> {
        let addr: SocketAddr = format!("{}:{}", self.bind_addr, self.port)
            .parse()
            .map_err(|e| TransferError::Network(format!("Invalid socket address: {}", e)))?;

        let listener = TcpListener::bind(addr).await?;
        info!(
            address = %addr,
            tls = self.use_tls,
            derp_mesh = derp_listener.is_some(),
            "Arch Drop Transfer Server listening"
        );

        let mut shutdown = shutdown_rx;

        loop {
            tokio::select! {
                res = listener.accept() => {
                    match res {
                        Ok((stream, remote_addr)) => {
                            let ctx = self.context.clone();
                            let tls_acceptor = self.tls_acceptor.clone();
                            tokio::spawn(Self::handle_incoming_connection(stream, remote_addr, ctx, tls_acceptor));
                        }
                        Err(e) => {
                            warn!("Accept error on transfer listener: {}", e);
                        }
                    }
                }
                Some(derp_res) = async {
                    if let Some(ref l) = derp_listener {
                        Some(l.accept().await)
                    } else {
                        std::future::pending::<Option<Result<(archdrop_mesh::stream::DerpStream, [u8; 32]), archdrop_mesh::error::MeshError>>>().await
                    }
                } => {
                    match derp_res {
                        Ok((derp_stream, remote_peer_key)) => {
                            let ctx = self.context.clone();
                            let tls_acceptor = self.tls_acceptor.clone();
                            let virtual_addr = SocketAddr::from(([127, 0, 0, 1], 0));
                            info!(
                                peer = %hex::encode_upper(remote_peer_key),
                                "Accepted relayed transfer session from DERP mesh"
                            );
                            tokio::spawn(Self::handle_incoming_connection(derp_stream, virtual_addr, ctx, tls_acceptor));
                        }
                        Err(e) => {
                            warn!("Error accepting virtual stream from DERP listener: {e}");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("Transfer server received shutdown signal");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_request(
        req: Request<Incoming>,
        ctx: TransferServerContext,
        remote_addr: SocketAddr,
    ) -> Response<ResponseBody> {
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let query_str = req.uri().query().map(|q| q.to_string());

        debug!(method = %method, path = %path, remote = %remote_addr, "Handling transfer HTTP request");

        match (method, path.as_str()) {
            (Method::GET, "/api/localsend/v2/info") => {
                Self::handle_info(&ctx).await
            }
            (Method::POST, "/api/localsend/v2/register") => {
                Self::handle_register(req, &ctx, remote_addr).await
            }
            (Method::POST, "/api/localsend/v2/prepare-upload") => {
                Self::handle_prepare_upload(req, &ctx).await
            }
            (Method::POST, "/api/localsend/v2/upload") => {
                Self::handle_upload(req, &ctx, query_str.as_deref()).await
            }
            (Method::POST, "/api/localsend/v2/cancel") => {
                Self::handle_cancel(&ctx, query_str.as_deref()).await
            }
            (Method::POST, "/api/v2/clipboard") => {
                Self::handle_clipboard(req, &ctx, remote_addr, query_str.as_deref()).await
            }
            (Method::POST, "/api/v2/webcam/stream") => {
                Self::handle_webcam_stream(req, &ctx, remote_addr, query_str.as_deref()).await
            }
            (Method::GET, "/api/v2/audio/stream") => {
                Self::handle_audio_stream(req, &ctx, remote_addr, query_str.as_deref()).await
            }
            (Method::GET, p) if p.starts_with("/api/v2/clipboard/payload/") => {
                let payload_id = p.trim_start_matches("/api/v2/clipboard/payload/");
                Self::handle_clipboard_payload(&ctx, payload_id, query_str.as_deref()).await
            }
            _ => empty_response(StatusCode::NOT_FOUND),
        }
    }

    async fn handle_info(ctx: &TransferServerContext) -> Response<ResponseBody> {
        let info = InfoResponseDtoV2 {
            alias: ctx
                .config
                .device
                .alias
                .clone()
                .unwrap_or_else(|| "Arch Drop".to_string()),
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: Some("Arch Linux (Hyprland)".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: ctx.identity.fingerprint.clone(),
            download: true,
        };
        json_response(StatusCode::OK, &info)
    }

    async fn handle_register(
        req: Request<Incoming>,
        ctx: &TransferServerContext,
        remote_addr: SocketAddr,
    ) -> Response<ResponseBody> {
        let body_bytes = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(e) => {
                warn!("Failed to read register request body: {}", e);
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        let reg_dto: RegisterDtoV2 = match serde_json::from_slice(&body_bytes) {
            Ok(dto) => dto,
            Err(e) => {
                warn!("Invalid RegisterDtoV2 JSON: {}", e);
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        info!(
            alias = %reg_dto.alias,
            fingerprint = %reg_dto.fingerprint,
            "Received peer registration request"
        );

        let ip = remote_addr.ip().to_string();
        let endpoint = PeerEndpoint {
            ip: ip.clone(),
            port: reg_dto.port,
            fingerprint: Some(reg_dto.fingerprint.clone()),
            alias: Some(reg_dto.alias.clone()),
            last_seen: std::time::Instant::now(),
        };
        ctx.registered_peers.write().await.insert(ip, endpoint);

        let response = RegisterResponseDtoV2 {
            alias: ctx
                .config
                .device
                .alias
                .clone()
                .unwrap_or_else(|| "Arch Drop".to_string()),
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: Some("Arch Linux (Hyprland)".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: ctx.identity.fingerprint.clone(),
            download: true,
        };

        json_response(StatusCode::OK, &response)
    }

    async fn handle_prepare_upload(
        req: Request<Incoming>,
        ctx: &TransferServerContext,
    ) -> Response<ResponseBody> {
        let is_staged = req
            .headers()
            .get("X-Clipboard-Staged")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false)
            || req.uri().query().map(|q| {
                let params = parse_query_string(Some(q));
                params.get("clipboard").map(|s| s.eq_ignore_ascii_case("true") || s == "1").unwrap_or(false)
                    || params.get("staged").map(|s| s.eq_ignore_ascii_case("true") || s == "1").unwrap_or(false)
            }).unwrap_or(false);

        let body_bytes = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(e) => {
                warn!("Failed to read prepare-upload body: {}", e);
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        let prepare_req: PrepareUploadRequestDtoV2 = match serde_json::from_slice(&body_bytes) {
            Ok(dto) => dto,
            Err(e) => {
                warn!("Invalid PrepareUploadRequestDtoV2 JSON: {}", e);
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        // Verification & Pairing Check:
        let peer_fp = &prepare_req.info.fingerprint;
        let is_paired = ctx.config.is_peer_paired(peer_fp);
        let has_paired_list = !ctx.config.security.paired_fingerprints.is_empty();

        let auto_accepted = ctx.config.security.auto_accept_paired && (is_paired || !has_paired_list);

        if !auto_accepted {
            if let Some(ref auth_cb) = ctx.auth_callback {
                let file_count = prepare_req.files.len();
                let total_bytes: u64 = prepare_req.files.values().map(|f| f.size).sum();
                let authorized = auth_cb(prepare_req.info.alias.clone(), file_count, total_bytes).await;
                if !authorized {
                    warn!(
                        peer_alias = %prepare_req.info.alias,
                        fingerprint = %peer_fp,
                        "Transfer rejected: User declined authorization or request timed out"
                    );
                    return empty_response(StatusCode::FORBIDDEN);
                }
            } else if has_paired_list && !is_paired {
                warn!(
                    peer_alias = %prepare_req.info.alias,
                    fingerprint = %peer_fp,
                    "Transfer rejected: Peer fingerprint is not paired"
                );
                return empty_response(StatusCode::FORBIDDEN);
            }
        }

        match ctx
            .session_manager
            .create_session(
                prepare_req.info,
                prepare_req.files,
                ctx.download_dir.clone(),
            )
            .await
        {
            Ok(session_res) => {
                if is_staged {
                    ctx.clipboard_staged_sessions
                        .write()
                        .await
                        .insert(session_res.session_id.clone());
                    info!(
                        session_id = %session_res.session_id,
                        "Transfer session marked as staged for Wayland clipboard"
                    );
                }
                json_response(StatusCode::OK, &session_res)
            }
            Err(e) => {
                error!("Failed to initialize transfer session: {}", e);
                empty_response(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }

    async fn handle_upload(
        req: Request<Incoming>,
        ctx: &TransferServerContext,
        query_str: Option<&str>,
    ) -> Response<ResponseBody> {
        let params = parse_query_string(query_str);
        let session_id = match params.get("sessionId") {
            Some(s) => s.as_str(),
            None => return empty_response(StatusCode::BAD_REQUEST),
        };
        let file_id = match params.get("fileId") {
            Some(f) => f.as_str(),
            None => return empty_response(StatusCode::BAD_REQUEST),
        };
        let token = match params.get("token") {
            Some(t) => t.as_str(),
            None => return empty_response(StatusCode::BAD_REQUEST),
        };

        let is_clipboard_staged = {
            let staged = ctx.clipboard_staged_sessions.read().await;
            staged.contains(session_id)
        } || req
            .headers()
            .get("X-Clipboard-Staged")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false)
          || params
            .get("clipboard")
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false)
          || params
            .get("staged")
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false);

        let (temp_path, final_path, expected_size, expected_sha256) = match ctx
            .session_manager
            .validate_upload_token(session_id, file_id, token)
            .await
        {
            Ok(paths) => paths,
            Err(TransferError::InvalidToken) => return empty_response(StatusCode::FORBIDDEN),
            Err(TransferError::SessionNotFound(_)) => return empty_response(StatusCode::NOT_FOUND),
            Err(TransferError::FileNotFound(_)) => return empty_response(StatusCode::NOT_FOUND),
            Err(e) => {
                warn!("Token validation error: {}", e);
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        let peer_alias = ctx
            .session_manager
            .get_peer_alias(session_id)
            .await
            .unwrap_or_else(|| "Peer".to_string());

        let file_name = final_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        // Open temp file for streaming write
        let mut file = match tokio::fs::File::create(&temp_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create temporary file {:?}: {}", temp_path, e);
                let _ = ctx
                    .session_manager
                    .mark_file_failed(session_id, file_id, e.to_string())
                    .await;
                if let Some(ref cb) = ctx.failure_callback {
                    cb(TransferFailedEvent {
                        session_id: session_id.to_string(),
                        file_id: file_id.to_string(),
                        file_name: file_name.clone(),
                        error: e.to_string(),
                        peer_alias: peer_alias.clone(),
                    });
                }
                return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

        let mut body = req.into_body();
        let mut hasher = Sha256::new();
        let mut received_bytes: u64 = 0;
        let mut last_progress_emit = std::time::Instant::now();
        let throttle_interval = std::time::Duration::from_millis(250);

        while let Some(frame_res) = body.frame().await {
            match frame_res {
                Ok(frame) => {
                    if let Some(chunk) = frame.data_ref() {
                        if let Err(e) = file.write_all(chunk).await {
                            error!("Error writing file stream chunk: {}", e);
                            let _ = ctx
                                .session_manager
                                .mark_file_failed(session_id, file_id, e.to_string())
                                .await;
                            if let Some(ref cb) = ctx.failure_callback {
                                cb(TransferFailedEvent {
                                    session_id: session_id.to_string(),
                                    file_id: file_id.to_string(),
                                    file_name: file_name.clone(),
                                    error: e.to_string(),
                                    peer_alias: peer_alias.clone(),
                                });
                            }
                            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
                        }
                        hasher.update(chunk);
                        received_bytes += chunk.len() as u64;

                        let now = std::time::Instant::now();
                        if now.duration_since(last_progress_emit) >= throttle_interval
                            || (expected_size > 0 && received_bytes == expected_size)
                        {
                            if let Some(ref cb) = ctx.progress_callback {
                                cb(TransferProgressEvent {
                                    session_id: session_id.to_string(),
                                    file_id: file_id.to_string(),
                                    file_name: file_name.clone(),
                                    current_bytes: received_bytes,
                                    total_bytes: expected_size,
                                });
                            }
                            last_progress_emit = now;
                        }
                    }
                }
                Err(e) => {
                    error!("Error receiving incoming body frame: {}", e);
                    let _ = ctx
                        .session_manager
                        .mark_file_failed(session_id, file_id, e.to_string())
                        .await;
                    if let Some(ref cb) = ctx.failure_callback {
                        cb(TransferFailedEvent {
                            session_id: session_id.to_string(),
                            file_id: file_id.to_string(),
                            file_name: file_name.clone(),
                            error: e.to_string(),
                            peer_alias: peer_alias.clone(),
                        });
                    }
                    return empty_response(StatusCode::BAD_REQUEST);
                }
            }
        }

        if let Err(e) = file.flush().await {
            error!("Error flushing temp file: {}", e);
            let _ = ctx
                .session_manager
                .mark_file_failed(session_id, file_id, e.to_string())
                .await;
            if let Some(ref cb) = ctx.failure_callback {
                cb(TransferFailedEvent {
                    session_id: session_id.to_string(),
                    file_id: file_id.to_string(),
                    file_name: file_name.clone(),
                    error: e.to_string(),
                    peer_alias: peer_alias.clone(),
                });
            }
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
        drop(file);

        // Verify file size
        if expected_size > 0 && received_bytes != expected_size {
            warn!(
                expected = expected_size,
                received = received_bytes,
                "Uploaded file size does not match expected size"
            );
            let _ = tokio::fs::remove_file(&temp_path).await;
            let err_msg = format!("Size mismatch: {} != {}", received_bytes, expected_size);
            let _ = ctx
                .session_manager
                .mark_file_failed(session_id, file_id, err_msg.clone())
                .await;
            if let Some(ref cb) = ctx.failure_callback {
                cb(TransferFailedEvent {
                    session_id: session_id.to_string(),
                    file_id: file_id.to_string(),
                    file_name: file_name.clone(),
                    error: err_msg,
                    peer_alias: peer_alias.clone(),
                });
            }
            return empty_response(StatusCode::BAD_REQUEST);
        }

        // Verify SHA-256 hash if provided
        let calculated_hash = hex::encode(hasher.finalize());
        if let Some(ref exp_hash) = expected_sha256 {
            if !calculated_hash.eq_ignore_ascii_case(exp_hash) {
                warn!(
                    expected = %exp_hash,
                    calculated = %calculated_hash,
                    "Uploaded file checksum mismatch"
                );
                let _ = tokio::fs::remove_file(&temp_path).await;
                let err_msg = "Checksum mismatch".to_string();
                let _ = ctx
                    .session_manager
                    .mark_file_failed(session_id, file_id, err_msg.clone())
                    .await;
                if let Some(ref cb) = ctx.failure_callback {
                    cb(TransferFailedEvent {
                        session_id: session_id.to_string(),
                        file_id: file_id.to_string(),
                        file_name: file_name.clone(),
                        error: err_msg,
                        peer_alias: peer_alias.clone(),
                    });
                }
                return empty_response(StatusCode::BAD_REQUEST);
            }
        }

        // Atomically rename temp file to final target destination
        if let Err(e) = tokio::fs::rename(&temp_path, &final_path).await {
            error!(
                "Failed to rename temp file {:?} to final {:?}: {}",
                temp_path, final_path, e
            );
            let _ = ctx
                .session_manager
                .mark_file_failed(session_id, file_id, e.to_string())
                .await;
            if let Some(ref cb) = ctx.failure_callback {
                cb(TransferFailedEvent {
                    session_id: session_id.to_string(),
                    file_id: file_id.to_string(),
                    file_name: file_name.clone(),
                    error: e.to_string(),
                    peer_alias: peer_alias.clone(),
                });
            }
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }

        if let Err(e) = ctx
            .session_manager
            .mark_file_completed(session_id, file_id, received_bytes)
            .await
        {
            error!("Error marking file completed: {}", e);
        }

        if is_clipboard_staged {
            let uri_list = format!("file://{}\r\n", final_path.display());
            info!(
                file = %final_path.display(),
                "Injecting staged file text/uri-list into Wayland clipboard"
            );
            if let Some(ref adapter) = ctx.clipboard_adapter {
                if let Err(e) = adapter.paste_remote_payload("text/uri-list", uri_list.as_bytes()) {
                    warn!("Failed to paste staged file uri-list to Wayland: {e}");
                }
            } else {
                pipe_to_wl_copy("text/uri-list", uri_list.as_bytes());
            }

            if let Some(ref cb) = ctx.staged_clipboard_callback {
                let cb = cb.clone();
                let f_name = file_name.clone();
                let f_path = final_path.clone();
                let p_alias = peer_alias.clone();
                tokio::spawn(async move {
                    cb(f_name, f_path, received_bytes, p_alias).await;
                });
            }
        }

        if let Some(ref cb) = ctx.completion_callback {
            cb(TransferCompletedEvent {
                session_id: session_id.to_string(),
                file_id: file_id.to_string(),
                file_name: file_name.clone(),
                total_bytes: received_bytes,
                peer_alias: peer_alias.clone(),
                is_clipboard_staged,
            });
        }

        empty_response(StatusCode::OK)
    }

    async fn handle_cancel(
        ctx: &TransferServerContext,
        query_str: Option<&str>,
    ) -> Response<ResponseBody> {
        let params = parse_query_string(query_str);
        if let Some(session_id) = params.get("sessionId") {
            let _ = ctx.session_manager.cancel_session(session_id).await;
            empty_response(StatusCode::OK)
        } else {
            empty_response(StatusCode::BAD_REQUEST)
        }
    }

    async fn handle_clipboard(
        req: Request<Incoming>,
        ctx: &TransferServerContext,
        remote_addr: SocketAddr,
        query_str: Option<&str>,
    ) -> Response<ResponseBody> {
        let headers = req.headers().clone();
        let body_bytes = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(e) => {
                warn!("Failed to read clipboard request body: {}", e);
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        // Parse ClipboardPacket (tagged), untagged inline/descriptor, or legacy KdeConnectPacket
        let packet: ClipboardPacket = match serde_json::from_slice(&body_bytes) {
            Ok(p) => p,
            Err(e) => {
                if let Ok(inline) = serde_json::from_slice::<archdrop_core::models::clipboard::ClipboardInlinePacket>(&body_bytes) {
                    ClipboardPacket::Inline(inline)
                } else if let Ok(desc) = serde_json::from_slice::<archdrop_core::models::clipboard::ClipboardDescriptorPacket>(&body_bytes) {
                    ClipboardPacket::Descriptor(desc)
                } else if let Ok(kde) = serde_json::from_slice::<KdeConnectPacket>(&body_bytes) {
                    from_kdeconnect_packet(&remote_addr.ip().to_string(), &kde)
                } else {
                    warn!("Invalid clipboard packet JSON: {}", e);
                    return empty_response(StatusCode::BAD_REQUEST);
                }
            }
        };

        // 1. Validate sender trust against paired fingerprints
        let query_map = parse_query_string(query_str);
        let header_fp = headers
            .get("x-fingerprint")
            .or_else(|| headers.get("fingerprint"))
            .and_then(|v| v.to_str().ok());
        let query_fp = query_map.get("fingerprint").map(|s| s.as_str());

        let device_id = match &packet {
            ClipboardPacket::Inline(p) => p.device_id.as_str(),
            ClipboardPacket::Descriptor(p) => p.device_id.as_str(),
            ClipboardPacket::Clear(p) => p.device_id.as_str(),
        };

        let has_paired_list = !ctx.config.security.paired_fingerprints.is_empty();
        if has_paired_list {
            let sender_fp = header_fp.or(query_fp).unwrap_or(device_id);
            if !ctx.config.is_peer_paired(sender_fp) {
                warn!(
                    remote = %remote_addr,
                    sender_fp = %sender_fp,
                    "Rejected clipboard packet from untrusted/unpaired peer"
                );
                return empty_response(StatusCode::FORBIDDEN);
            }
        }

        // Record peer as active/known
        let peer_ip = remote_addr.ip().to_string();
        let peer_fp = header_fp.or(query_fp).map(|s| s.to_string());
        let default_peer_port = if remote_addr.ip().is_loopback() {
            ctx.server_port
        } else {
            53317
        };
        let existing_port = ctx
            .registered_peers
            .read()
            .await
            .get(&peer_ip)
            .map(|ep| ep.port)
            .unwrap_or(default_peer_port);
        ctx.registered_peers.write().await.insert(
            peer_ip.clone(),
            PeerEndpoint {
                ip: peer_ip,
                port: existing_port,
                fingerprint: peer_fp,
                alias: Some(device_id.to_string()),
                last_seen: std::time::Instant::now(),
            },
        );

        // 2. Process packet, apply loopback echo suppression, and paste to local Wayland
        match &packet {
            ClipboardPacket::Inline(p) => {
                let hash = &p.payload.sha256;

                // Loopback echo suppression check against last_pasted_sha256
                if let Some(ref adapter) = ctx.clipboard_adapter {
                    if adapter.is_recent_paste(hash) {
                        debug!("Incoming clipboard packet matches last_pasted_sha256; suppressing loopback echo");
                        return empty_response(StatusCode::OK);
                    }
                    match archdrop_clipboard::unpack_clipboard_packet(&packet) {
                        Ok(archdrop_clipboard::UnpackedClipboard::InlineBinary { mime_type, data, .. }) => {
                            if let Err(e) = adapter.paste_remote_payload(&mime_type, &data) {
                                debug!("paste_remote_payload binary returned: {e}");
                            }
                        }
                        Ok(archdrop_clipboard::UnpackedClipboard::InlineText { mime_type, text, .. }) => {
                            if let Err(e) = adapter.paste_remote_payload(&mime_type, text.as_bytes()) {
                                debug!("paste_remote_payload text returned: {e}");
                            }
                        }
                        _ => {
                            let _ = adapter.paste_remote_payload(&p.payload.mime_type, p.payload.content.as_bytes());
                        }
                    }
                } else {
                    match archdrop_clipboard::unpack_clipboard_packet(&packet) {
                        Ok(archdrop_clipboard::UnpackedClipboard::InlineBinary { mime_type, data, .. }) => {
                            pipe_to_wl_copy(&mime_type, &data);
                        }
                        Ok(archdrop_clipboard::UnpackedClipboard::InlineText { mime_type, text, .. }) => {
                            pipe_to_wl_copy(&mime_type, text.as_bytes());
                        }
                        _ => {
                            pipe_to_wl_copy(&p.payload.mime_type, p.payload.content.as_bytes());
                        }
                    }
                }

                if let Some(ref cb) = ctx.clipboard_callback {
                    cb(packet.clone());
                }

                empty_response(StatusCode::OK)
            }
            ClipboardPacket::Descriptor(p) => {
                let hash = &p.descriptor.sha256;
                let mime = &p.descriptor.primary_mime;

                if let Some(ref adapter) = ctx.clipboard_adapter {
                    if adapter.is_recent_paste(hash) {
                        debug!("Incoming descriptor matches last_pasted_sha256; suppressing loopback echo");
                        return empty_response(StatusCode::OK);
                    }
                }

                if let Some(ref raw_url) = p.descriptor.download_url {
                    let mut url = raw_url.clone();
                    let remote_ip = remote_addr.ip().to_string();
                    let peer_port = if remote_addr.ip().is_loopback() {
                        ctx.server_port
                    } else {
                        ctx.registered_peers
                            .read()
                            .await
                            .get(&remote_ip)
                            .map(|ep| ep.port)
                            .unwrap_or(53317)
                    };

                    if url.contains("://127.0.0.1:") {
                        url = url.replace("://127.0.0.1:", &format!("://{}:", remote_ip));
                    } else if url.contains("://localhost:") {
                        url = url.replace("://localhost:", &format!("://{}:", remote_ip));
                    } else if url.contains("://0.0.0.0:") {
                        url = url.replace("://0.0.0.0:", &format!("://{}:", remote_ip));
                    } else if url.starts_with('/') {
                        url = format!("http://{}:{}{}", remote_ip, peer_port, url);
                    }

                    let mime = mime.clone();
                    let expected_hash = hash.clone();
                    let adapter = ctx.clipboard_adapter.clone();
                    tokio::spawn(async move {
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(10))
                            .build()
                            .unwrap_or_default();
                        if let Ok(resp) = client.get(&url).send().await {
                            if resp.status().is_success() {
                                if let Ok(bytes) = resp.bytes().await {
                                    let computed = compute_sha256(&bytes);
                                    if computed.eq_ignore_ascii_case(&expected_hash) {
                                        if let Some(a) = adapter {
                                            let _ = a.paste_remote_payload(&mime, &bytes);
                                        } else {
                                            pipe_to_wl_copy(&mime, &bytes);
                                        }
                                    } else {
                                        warn!(expected = %expected_hash, computed = %computed, "Descriptor payload hash mismatch");
                                    }
                                }
                            }
                        }
                    });
                }

                if let Some(ref cb) = ctx.clipboard_callback {
                    cb(packet.clone());
                }

                empty_response(StatusCode::OK)
            }
            ClipboardPacket::Clear(_) => {
                if let Some(ref adapter) = ctx.clipboard_adapter {
                    let _ = adapter.clear_local_clipboard();
                }
                if let Some(ref cb) = ctx.clipboard_callback {
                    cb(packet.clone());
                }
                empty_response(StatusCode::OK)
            }
        }
    }

    async fn handle_clipboard_payload(
        ctx: &TransferServerContext,
        payload_id: &str,
        query_str: Option<&str>,
    ) -> Response<ResponseBody> {
        let query_map = parse_query_string(query_str);
        let token = query_map.get("token").cloned().unwrap_or_default();
        if let Some(ref cache) = ctx.clipboard_cache {
            match cache.get(payload_id, &token).await {
                Ok(data) => Response::builder()
                    .status(StatusCode::OK)
                    .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
                    .body(BoxBody::new(Full::new(Bytes::from(data.to_vec()))))
                    .unwrap(),
                Err(ClipboardError::InvalidToken) => empty_response(StatusCode::FORBIDDEN),
                Err(_) => empty_response(StatusCode::NOT_FOUND),
            }
        } else {
            empty_response(StatusCode::NOT_FOUND)
        }
    }

    async fn handle_webcam_stream(
        req: Request<Incoming>,
        ctx: &TransferServerContext,
        remote_addr: SocketAddr,
        query_str: Option<&str>,
    ) -> Response<ResponseBody> {
        let headers = req.headers().clone();

        // 1. Authenticate peer via X-Fingerprint header or query string
        let query_map = parse_query_string(query_str);
        let header_fp = headers
            .get("x-fingerprint")
            .or_else(|| headers.get("fingerprint"))
            .and_then(|v| v.to_str().ok());
        let query_fp = query_map.get("fingerprint").map(|s| s.as_str());

        let has_paired_list = !ctx.config.security.paired_fingerprints.is_empty();
        if has_paired_list {
            let sender_fp = header_fp.or(query_fp).unwrap_or("");
            if sender_fp.is_empty() || !ctx.config.is_peer_paired(sender_fp) {
                warn!(
                    remote = %remote_addr,
                    sender_fp = %sender_fp,
                    "Rejecting unauthorized webcam stream from unpaired peer"
                );
                return empty_response(StatusCode::FORBIDDEN);
            }
        }

        // 2. Enforce mutual exclusion: single active stream (HTTP 409 Conflict)
        if ctx
            .is_webcam_streaming
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            warn!(
                remote = %remote_addr,
                "Rejecting concurrent webcam stream: another stream session is already in progress (409 Conflict)"
            );
            return empty_response(StatusCode::CONFLICT);
        }

        // RAII guard ensuring streaming flag is reset on any exit / client disconnect
        struct WebcamStreamingGuard(Arc<AtomicBool>);
        impl Drop for WebcamStreamingGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
                debug!("Webcam stream session ended; mutual exclusion lock released");
            }
        }
        let _guard = WebcamStreamingGuard(ctx.is_webcam_streaming.clone());

        // 3. Resolve or initialize WebcamSink
        let active_sink = match ctx.get_webcam_sink().await {
            Some(s) => Some(s),
            None => match archdrop_webcam::discover_loopback_device(ctx.sysfs_root.as_deref()) {
                Ok(Some(dev)) => match archdrop_webcam::LinuxV4l2Sink::open(&dev.device_path) {
                    Ok(v4l2_sink) => {
                        let sink = Arc::new(WebcamSink::new(Box::new(v4l2_sink)));
                        ctx.set_webcam_sink_async(Some(sink.clone())).await;
                        Some(sink)
                    }
                    Err(e) => {
                        warn!("Failed to open loopback video device {:?}: {}", dev.device_path, e);
                        None
                    }
                },
                Ok(None) | Err(_) => {
                    debug!("No v4l2loopback virtual video device discovered");
                    None
                }
            },
        };

        // If no sink is configured or discovered, return 503 with diagnostic
        let sink = match active_sink {
            Some(s) => s,
            None => {
                warn!(
                    "Cannot process webcam stream: v4l2loopback not loaded ({})",
                    MODPROBE_DIAGNOSTIC
                );
                return Response::builder()
                    .status(StatusCode::SERVICE_UNAVAILABLE)
                    .header("X-ArchDrop-Diagnostic", MODPROBE_DIAGNOSTIC)
                    .body(BoxBody::new(Full::new(Bytes::from(MODPROBE_DIAGNOSTIC))))
                    .unwrap();
            }
        };

        // 4. Extract multipart boundary from Content-Type header
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let boundary = if let Some((_, b)) = content_type.split_once("boundary=") {
            let clean = b.split(';').next().unwrap_or(b).trim().trim_matches('"');
            if clean.is_empty() {
                "frame"
            } else {
                clean
            }
        } else {
            "frame"
        };

        let mut parser = MjpegStreamParser::new(boundary);
        let mut body = req.into_body();

        info!(
            remote = %remote_addr,
            boundary = %boundary,
            device = ?sink.device_path(),
            "Starting ingestion of multipart MJPEG webcam stream"
        );

        // 5. Streamingly parse chunks and feed frames to sink with idle timeout
        let idle_timeout = std::time::Duration::from_secs(3);
        let mut last_frame_time = tokio::time::Instant::now();
        loop {
            let frame_res = match tokio::time::timeout(idle_timeout, body.frame()).await {
                Ok(Some(res)) => res,
                Ok(None) => break,
                Err(_) => {
                    warn!(
                        remote = %remote_addr,
                        "Webcam stream idle timeout exceeded (3s with no data); closing session"
                    );
                    break;
                }
            };

            match frame_res {
                Ok(frame) => {
                    if let Some(chunk) = frame.data_ref() {
                        parser.push_chunk(chunk);

                        loop {
                            match parser.next_frame() {
                                Ok(Some(mjpeg_frame)) => {
                                    last_frame_time = tokio::time::Instant::now();
                                    if let Err(e) = sink.push_frame(mjpeg_frame.data) {
                                        warn!("Error writing frame to virtual webcam sink: {}", e);
                                    }
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    warn!("MJPEG stream parser error: {}", e);
                                    return empty_response(StatusCode::BAD_REQUEST);
                                }
                            }
                        }

                        if parser.is_finished() {
                            info!(remote = %remote_addr, "Multipart terminating boundary received; concluding stream");
                            break;
                        }

                        if last_frame_time.elapsed() >= idle_timeout {
                            warn!(
                                remote = %remote_addr,
                                "Webcam stream idle timeout exceeded (3s with no valid video frames); closing session"
                            );
                            break;
                        }
                    }
                }
                Err(e) => {
                    debug!("Webcam stream connection terminated: {}", e);
                    break;
                }
            }
        }

        info!(
            remote = %remote_addr,
            frames_received = sink.stats().frames_received,
            frames_written = sink.stats().frames_written,
            frames_dropped = sink.stats().frames_dropped,
            "Webcam stream ingestion concluded successfully"
        );

        empty_response(StatusCode::OK)
    }

    async fn handle_audio_stream(
        req: Request<Incoming>,
        ctx: &TransferServerContext,
        remote_addr: SocketAddr,
        query_str: Option<&str>,
    ) -> Response<ResponseBody> {
        let headers = req.headers().clone();

        // 1. Authenticate peer via X-Fingerprint header or query string
        let query_map = parse_query_string(query_str);
        let header_fp = headers
            .get("x-fingerprint")
            .or_else(|| headers.get("fingerprint"))
            .and_then(|v| v.to_str().ok());
        let query_fp = query_map.get("fingerprint").map(|s| s.as_str());

        let has_paired_list = !ctx.config.security.paired_fingerprints.is_empty();
        if has_paired_list {
            let sender_fp = header_fp.or(query_fp).unwrap_or("");
            if sender_fp.is_empty() || !ctx.config.is_peer_paired(sender_fp) {
                warn!(
                    remote = %remote_addr,
                    sender_fp = %sender_fp,
                    "Rejecting unauthorized audio stream from unpaired peer"
                );
                return empty_response(StatusCode::FORBIDDEN);
            }
        }

        // 2. Parse and validate stream parameters
        let codec_str = query_map.get("codec").map(|s| s.as_str()).unwrap_or("opus");
        let codec = match codec_str {
            "opus" => AudioCodec::Opus,
            "pcm" => AudioCodec::Pcm,
            other => {
                warn!(remote = %remote_addr, codec = other, "Invalid audio codec requested");
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        let bitrate: u32 = match query_map.get("bitrate") {
            Some(s) => match s.parse::<u32>() {
                Ok(b) if (64000..=128000).contains(&b) => b,
                _ => {
                    warn!(remote = %remote_addr, bitrate = s, "Invalid audio bitrate requested");
                    return empty_response(StatusCode::BAD_REQUEST);
                }
            },
            None => 96000,
        };

        let sink_mode = query_map.get("sink").map(|s| s.as_str()).unwrap_or("monitor");
        let use_virtual_sink = match sink_mode {
            "monitor" => false,
            "virtual" => true,
            other => {
                warn!(remote = %remote_addr, sink = other, "Invalid audio sink mode requested");
                return empty_response(StatusCode::BAD_REQUEST);
            }
        };

        // 3. Enforce single-stream mutual exclusion (HTTP 409 Conflict)
        if ctx
            .is_audio_streaming
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            warn!(
                remote = %remote_addr,
                "Rejecting concurrent audio stream: another audio stream is already in progress (409 Conflict)"
            );
            return empty_response(StatusCode::CONFLICT);
        }

        let guard = AudioStreamingGuard {
            is_streaming: ctx.is_audio_streaming.clone(),
            session: None,
        };

        // 4. Instantiate capture backend and virtual sink
        let capture_backend: Box<dyn AudioCaptureBackend> = {
            let factory_guard = ctx.audio_capture_factory.read().await;
            if let Some(factory) = factory_guard.as_ref() {
                factory()
            } else if use_virtual_sink {
                Box::new(PipeWireCapture::new(format!(
                    "input.{}",
                    DEFAULT_VIRTUAL_SINK_NAME
                )))
            } else {
                Box::new(PipeWireCapture::default_sink())
            }
        };

        let virtual_sink_backend: Option<Box<dyn VirtualSinkBackend>> = if use_virtual_sink {
            let factory_guard = ctx.virtual_sink_factory.read().await;
            if let Some(factory) = factory_guard.as_ref() {
                Some(factory())
            } else {
                Some(Box::new(PipeWireVirtualSink::new()))
            }
        } else {
            None
        };

        let encoder: Box<dyn AudioEncoder> = match codec {
            AudioCodec::Opus => match OpusAudioEncoder::new(48000, 2, bitrate) {
                Ok(enc) => Box::new(enc),
                Err(e) => {
                    error!("Failed to initialize Opus encoder: {e}");
                    drop(guard);
                    return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            },
            AudioCodec::Pcm => Box::new(PcmPassthroughEncoder::default()),
        };

        // 5. Start audio streaming session
        let mut session = match AudioStreamSession::start(
            capture_backend,
            encoder,
            virtual_sink_backend,
        )
        .await
        {
            Ok(sess) => sess,
            Err(e) => {
                warn!("Failed to start audio streaming session: {e}");
                drop(guard);
                return empty_response(StatusCode::SERVICE_UNAVAILABLE);
            }
        };

        let rx = match session.take_receiver() {
            Some(r) => r,
            None => {
                drop(guard);
                return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
            .map(|bytes| Ok::<_, std::convert::Infallible>(hyper::body::Frame::data(bytes)));

        let full_guard = AudioStreamingGuard {
            is_streaming: ctx.is_audio_streaming.clone(),
            session: Some(session),
        };
        std::mem::forget(guard);

        let guarded_stream = GuardedAudioStream {
            inner: stream,
            _guard: full_guard,
        };

        info!(
            remote = %remote_addr,
            codec = codec_str,
            bitrate = bitrate,
            sink = sink_mode,
            "Starting chunked audio stream transmission"
        );

        let body = BoxBody::new(StreamBody::new(guarded_stream));

        Response::builder()
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_TYPE, "audio/x-archdrop-audio")
            .header(hyper::header::TRANSFER_ENCODING, "chunked")
            .header(hyper::header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
            .header(hyper::header::CONNECTION, "close")
            .header("X-ArchDrop-Audio-Codec", codec_str)
            .header("X-ArchDrop-Sample-Rate", "48000")
            .header("X-ArchDrop-Channels", "2")
            .header("X-ArchDrop-Bit-Depth", "16")
            .header("X-ArchDrop-Frame-Duration-Ms", "20")
            .header("X-ArchDrop-Bitrate", bitrate.to_string())
            .header("X-ArchDrop-Protocol-Version", "2")
            .body(body)
            .unwrap()
    }
}
