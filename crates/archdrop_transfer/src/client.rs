use crate::error::TransferError;
use archdrop_core::config::Config;
use archdrop_core::crypto::{fingerprint_from_cert_der, fingerprint_matches, CryptoIdentity};
use archdrop_core::models::clipboard::ClipboardPacket;
use archdrop_core::models::dto_v2::{
    DeviceType, FileDto, InfoResponseDtoV2, PrepareUploadRequestDtoV2,
    PrepareUploadResponseDtoV2, ProtocolType, RegisterDtoV2, RegisterResponseDtoV2,
    DEFAULT_PORT, PROTOCOL_VERSION_V2,
};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::io::ReaderStream;
use tracing::{debug, info, warn};

#[derive(Debug)]
struct FingerprintCertVerifier {
    expected_fingerprint: Option<String>,
}

impl rustls::client::danger::ServerCertVerifier for FingerprintCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if let Some(ref exp) = self.expected_fingerprint {
            let actual = fingerprint_from_cert_der(end_entity.as_ref());
            if !fingerprint_matches(exp, &actual) {
                warn!(
                    expected = %exp,
                    actual = %actual,
                    "Peer certificate fingerprint mismatch"
                );
                return Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ));
            }
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug, Clone)]
pub struct TransferSummary {
    pub session_id: String,
    pub files_sent: usize,
    pub total_bytes: u64,
    pub target_alias: String,
}

#[derive(Clone, Debug, Default)]
pub struct TransferClient {
    plain_client: Client,
}

impl TransferClient {
    pub fn new() -> Self {
        Self {
            plain_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    fn create_tls_client(&self, expected_fingerprint: Option<String>) -> Result<Client, TransferError> {
        let verifier = Arc::new(FingerprintCertVerifier { expected_fingerprint });
        let rustls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

        Client::builder()
            .use_preconfigured_tls(rustls_config)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| TransferError::Tls(format!("Failed to build TLS client: {}", e)))
    }

    fn build_url(host: &str, port: u16, use_https: bool, path: &str) -> String {
        let scheme = if use_https { "https" } else { "http" };
        let formatted_host = if host.contains(':') && !host.starts_with('[') {
            format!("[{}]", host)
        } else {
            host.to_string()
        };
        format!("{}://{}:{}{}", scheme, formatted_host, port, path)
    }

    /// Fetches server info from target device.
    pub async fn get_info(
        &self,
        host: &str,
        port: u16,
        use_https: bool,
        expected_fingerprint: Option<String>,
    ) -> Result<InfoResponseDtoV2, TransferError> {
        let url = Self::build_url(host, port, use_https, "/api/localsend/v2/info");
        let client = if use_https {
            self.create_tls_client(expected_fingerprint)?
        } else {
            self.plain_client.clone()
        };

        debug!("Querying peer info from {}", url);
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| TransferError::Network(format!("Failed to reach {}: {}", url, e)))?;

        if !resp.status().is_success() {
            return Err(TransferError::Http(format!(
                "GET /info returned status {}",
                resp.status()
            )));
        }

        let info = resp
            .json::<InfoResponseDtoV2>()
            .await
            .map_err(|e| TransferError::Serialization(e.to_string()))?;

        Ok(info)
    }

    /// Registers this device with the peer.
    pub async fn register(
        &self,
        host: &str,
        port: u16,
        use_https: bool,
        expected_fingerprint: Option<String>,
        reg_dto: &RegisterDtoV2,
    ) -> Result<RegisterResponseDtoV2, TransferError> {
        let url = Self::build_url(host, port, use_https, "/api/localsend/v2/register");
        let client = if use_https {
            self.create_tls_client(expected_fingerprint)?
        } else {
            self.plain_client.clone()
        };

        let resp = client
            .post(&url)
            .json(reg_dto)
            .send()
            .await
            .map_err(|e| TransferError::Network(format!("Failed to send register to {}: {}", url, e)))?;

        if !resp.status().is_success() {
            return Err(TransferError::Http(format!(
                "POST /register returned status {}",
                resp.status()
            )));
        }

        let res = resp
            .json::<RegisterResponseDtoV2>()
            .await
            .map_err(|e| TransferError::Serialization(e.to_string()))?;

        Ok(res)
    }

    /// Prepares an upload session with the receiver.
    pub async fn prepare_upload(
        &self,
        host: &str,
        port: u16,
        use_https: bool,
        expected_fingerprint: Option<String>,
        prepare_req: &PrepareUploadRequestDtoV2,
    ) -> Result<PrepareUploadResponseDtoV2, TransferError> {
        let url = Self::build_url(host, port, use_https, "/api/localsend/v2/prepare-upload");
        let client = if use_https {
            self.create_tls_client(expected_fingerprint)?
        } else {
            self.plain_client.clone()
        };

        let resp = client
            .post(&url)
            .json(prepare_req)
            .send()
            .await
            .map_err(|e| TransferError::Network(format!("Failed to send prepare-upload to {}: {}", url, e)))?;

        if resp.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(TransferError::Rejected("Receiver rejected or declined the transfer".to_string()));
        }

        if !resp.status().is_success() {
            return Err(TransferError::Http(format!(
                "POST /prepare-upload returned status {}",
                resp.status()
            )));
        }

        let res = resp
            .json::<PrepareUploadResponseDtoV2>()
            .await
            .map_err(|e| TransferError::Serialization(e.to_string()))?;

        Ok(res)
    }

    /// Sends a clipboard packet to a remote peer's /api/v2/clipboard endpoint.
    pub async fn send_clipboard(
        &self,
        host: &str,
        port: u16,
        use_https: bool,
        expected_fingerprint: Option<String>,
        local_fingerprint: Option<&str>,
        packet: &ClipboardPacket,
    ) -> Result<(), TransferError> {
        let url = Self::build_url(host, port, use_https, "/api/v2/clipboard");
        let client = if use_https {
            self.create_tls_client(expected_fingerprint)?
        } else {
            self.plain_client.clone()
        };

        let mut req = client.post(&url).json(packet);
        if let Some(fp) = local_fingerprint {
            req = req.header("X-Fingerprint", fp);
        }

        debug!("Broadcasting clipboard packet to {}", url);
        let resp = req
            .send()
            .await
            .map_err(|e| TransferError::Network(format!("Failed to send clipboard to {}: {}", url, e)))?;

        if !resp.status().is_success() {
            return Err(TransferError::Http(format!(
                "POST /api/v2/clipboard returned status {}",
                resp.status()
            )));
        }

        Ok(())
    }

    /// Uploads a single file stream to the receiver.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_file(
        &self,
        host: &str,
        port: u16,
        use_https: bool,
        expected_fingerprint: Option<String>,
        session_id: &str,
        file_id: &str,
        token: &str,
        file_path: &Path,
    ) -> Result<(), TransferError> {
        self.upload_file_with_progress(
            host,
            port,
            use_https,
            expected_fingerprint,
            session_id,
            file_id,
            token,
            file_path,
            None,
        )
        .await
    }

    /// Uploads a single file stream to the receiver with throttled progress reporting.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_file_with_progress(
        &self,
        host: &str,
        port: u16,
        use_https: bool,
        expected_fingerprint: Option<String>,
        session_id: &str,
        file_id: &str,
        token: &str,
        file_path: &Path,
        progress: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<(), TransferError> {
        use tokio_stream::StreamExt;

        let path_str = format!(
            "/api/localsend/v2/upload?sessionId={}&fileId={}&token={}",
            session_id, file_id, token
        );
        let url = Self::build_url(host, port, use_https, &path_str);
        let client = if use_https {
            self.create_tls_client(expected_fingerprint)?
        } else {
            self.plain_client.clone()
        };

        let file = tokio::fs::File::open(file_path).await?;
        let total_size = file.metadata().await?.len();
        let stream = ReaderStream::with_capacity(file, 512 * 1024); // 512 KiB buffer

        let mut sent = 0u64;
        let mut last_emit = std::time::Instant::now();
        let throttle_interval = std::time::Duration::from_millis(250);
        let progress_cb = progress.clone();

        let mapped_stream = stream.map(move |chunk_res| {
            if let Ok(ref bytes) = chunk_res {
                sent += bytes.len() as u64;
                let now = std::time::Instant::now();
                if now.duration_since(last_emit) >= throttle_interval || sent == total_size {
                    if let Some(ref cb) = progress_cb {
                        cb(sent, total_size);
                    }
                    last_emit = now;
                }
            }
            chunk_res
        });

        let body = reqwest::Body::wrap_stream(mapped_stream);

        debug!(file = ?file_path, url = %url, "Streaming file upload");
        let resp = client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|e| TransferError::Network(format!("Failed to stream file to {}: {}", url, e)))?;

        if !resp.status().is_success() {
            return Err(TransferError::Http(format!(
                "POST /upload returned status {}",
                resp.status()
            )));
        }

        Ok(())
    }

    /// Cancels an active transfer session.
    pub async fn cancel(
        &self,
        host: &str,
        port: u16,
        use_https: bool,
        session_id: &str,
    ) -> Result<(), TransferError> {
        let path_str = format!("/api/localsend/v2/cancel?sessionId={}", session_id);
        let url = Self::build_url(host, port, use_https, &path_str);
        let client = if use_https {
            self.create_tls_client(None)?
        } else {
            self.plain_client.clone()
        };

        let _ = client.post(&url).send().await;
        Ok(())
    }

    /// High-level transfer orchestrator: sends a set of files to a remote target.
    pub async fn send_files(
        &self,
        target_ip: &str,
        port: u16,
        use_https: bool,
        file_paths: &[PathBuf],
        config: &Config,
        identity: &CryptoIdentity,
    ) -> Result<TransferSummary, TransferError> {
        if file_paths.is_empty() {
            return Err(TransferError::Protocol("No files specified to send".to_string()));
        }

        // 1. Inspect target peer
        let peer_info = self.get_info(target_ip, port, use_https, None).await?;
        info!(
            target_alias = %peer_info.alias,
            target_fp = %peer_info.fingerprint,
            version = %peer_info.version,
            "Connected to peer device"
        );

        // 2. Scan and prepare file descriptors
        let mut files_dto = HashMap::new();
        let mut file_path_map = HashMap::new();
        let mut total_bytes: u64 = 0;

        for (idx, path) in file_paths.iter().enumerate() {
            let metadata = tokio::fs::metadata(path).await?;
            if !metadata.is_file() {
                return Err(TransferError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Path is not a regular file: {:?}", path),
                )));
            }

            let size = metadata.len();
            total_bytes += size;

            // Compute SHA-256 for integrity verification
            let mut file = tokio::fs::File::open(path).await?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0u8; 128 * 1024];
            use tokio::io::AsyncReadExt;
            loop {
                let n = file.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            let sha256_hex = hex::encode(hasher.finalize());

            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();

            let file_id = format!("file-{}", idx);
            let dto = FileDto {
                id: file_id.clone(),
                file_name,
                size,
                file_type: "application/octet-stream".to_string(),
                sha256: Some(sha256_hex),
                preview: None,
                metadata: None,
            };

            files_dto.insert(file_id.clone(), dto);
            file_path_map.insert(file_id, path.clone());
        }

        // 3. Prepare upload session
        let sender_dto = RegisterDtoV2 {
            alias: config
                .device
                .alias
                .clone()
                .unwrap_or_else(|| "Arch Drop".to_string()),
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: Some("Arch Linux (Hyprland)".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: identity.fingerprint.clone(),
            port: DEFAULT_PORT,
            protocol: if use_https {
                ProtocolType::Https
            } else {
                ProtocolType::Http
            },
            download: true,
        };

        let prepare_req = PrepareUploadRequestDtoV2 {
            info: sender_dto,
            files: files_dto,
        };

        let prepare_res = self
            .prepare_upload(
                target_ip,
                port,
                use_https,
                Some(peer_info.fingerprint.clone()),
                &prepare_req,
            )
            .await?;

        info!(
            session_id = %prepare_res.session_id,
            accepted_count = prepare_res.files.len(),
            "Upload session accepted by peer"
        );

        let mut files_sent = 0;

        // 4. Stream each accepted file
        for (file_id, token) in prepare_res.files {
            if let Some(src_path) = file_path_map.get(&file_id) {
                self.upload_file(
                    target_ip,
                    port,
                    use_https,
                    Some(peer_info.fingerprint.clone()),
                    &prepare_res.session_id,
                    &file_id,
                    &token,
                    src_path,
                )
                .await?;
                files_sent += 1;
            }
        }

        info!(
            files_sent = files_sent,
            total_bytes = total_bytes,
            "Transfer completed successfully"
        );

        Ok(TransferSummary {
            session_id: prepare_res.session_id,
            files_sent,
            total_bytes,
            target_alias: peer_info.alias,
        })
    }

    /// Sends files to a target device using `FallbackDialer` (racing direct TCP with transparent DERP fallback).
    #[allow(clippy::too_many_arguments)]
    pub async fn send_files_with_dialer(
        &self,
        dialer: &archdrop_mesh::dialer::FallbackDialer,
        direct_target: Option<(&str, u16)>,
        target_peer_key: Option<&[u8; 32]>,
        use_https: bool,
        file_paths: &[PathBuf],
        config: &Config,
        identity: &CryptoIdentity,
    ) -> Result<TransferSummary, TransferError> {
        self.send_files_with_p2p_dialer(
            dialer,
            direct_target,
            None,
            target_peer_key,
            use_https,
            file_paths,
            config,
            identity,
        )
        .await
    }

    /// Sends files to a target device using `FallbackDialer` with 3-tier fallback (Direct LAN -> Offline P2P -> WAN DERP Relay).
    #[allow(clippy::too_many_arguments)]
    pub async fn send_files_with_p2p_dialer(
        &self,
        dialer: &archdrop_mesh::dialer::FallbackDialer,
        direct_target: Option<(&str, u16)>,
        target_peer: Option<&str>,
        target_peer_key: Option<&[u8; 32]>,
        use_https: bool,
        file_paths: &[PathBuf],
        config: &Config,
        identity: &CryptoIdentity,
    ) -> Result<TransferSummary, TransferError> {
        let direct_addr_str = direct_target.map(|(ip, port)| format!("{}:{}", ip, port));
        let peer_stream = dialer
            .dial_with_p2p_fallback(direct_addr_str.as_deref(), target_peer, target_peer_key)
            .await
            .map_err(|e| TransferError::Network(format!("Dial with 3-tier fallback failed: {e}")))?;

        match peer_stream {
            archdrop_mesh::dialer::PeerStream::Direct(_tcp) => {
                let (host, port) = direct_target.ok_or_else(|| {
                    TransferError::Network("Direct TCP connection succeeded without direct_target".into())
                })?;
                info!(tier = "Direct LAN", host = %host, port = port, "Transferring via Tier 1 Direct LAN");
                self.send_files(host, port, use_https, file_paths, config, identity).await
            }
            archdrop_mesh::dialer::PeerStream::P2p(stream) => {
                let peer_addr = stream.peer_addr().map_err(TransferError::Io)?;
                let host = peer_addr.ip().to_string();
                let port = peer_addr.port();
                info!(tier = "Peer-to-Peer", host = %host, port = port, "Transferring via Tier 2 Offline P2P SoftAP gateway");
                self.send_files(&host, port, use_https, file_paths, config, identity).await
            }
            archdrop_mesh::dialer::PeerStream::Relayed(derp_stream) => {
                info!(tier = "Cloud Relay", "Transferring via Tier 3 Userspace DERP Relay loopback bridge");
                // Relayed over DERP mesh: bridge transparently via local loopback forwarder
                let bridge_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .map_err(TransferError::Io)?;
                let bridge_port = bridge_listener.local_addr().map_err(TransferError::Io)?.port();

                let bridge_task = tokio::spawn(async move {
                    if let Ok((mut local_stream, _)) = bridge_listener.accept().await {
                        let mut derp = derp_stream;
                        let _ = tokio::io::copy_bidirectional(&mut local_stream, &mut derp).await;
                    }
                });

                let summary_res = self
                    .send_files("127.0.0.1", bridge_port, use_https, file_paths, config, identity)
                    .await;

                bridge_task.abort();
                summary_res
            }
        }
    }
}
