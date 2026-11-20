//! Network transport connectors for DERP relay sessions.
//!
//! Supports:
//! - WebSocket over TLS / Plain (`wss://`, `ws://`) via `tokio-tungstenite`
//! - Direct TLS stream (`tls://`, `https://`) via `tokio-rustls`
//! - Raw TCP stream (`tcp://`, `http://`) for local development and testing

use crate::error::MeshError;
use crate::frame::{DerpCodec, DerpFrame};
use bytes::BytesMut;
use futures::{Sink, SinkExt, Stream, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;
use tokio_util::codec::Framed;
use url::Url;

pub type FrameSink = Pin<Box<dyn Sink<DerpFrame, Error = MeshError> + Send>>;
pub type FrameStream = Pin<Box<dyn Stream<Item = Result<DerpFrame, MeshError>> + Send>>;

/// Creates a default rustls ClientConfig using Ring crypto provider.
pub fn create_default_tls_config() -> Arc<rustls::ClientConfig> {
    let root_store = rustls::RootCertStore::empty();
    // In production or tests, roots can be added; for custom servers, allow dangerous or standard verification
    let config = rustls::ClientConfig::builder_with_provider(
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
    .expect("TLS protocol versions supported")
    .with_root_certificates(root_store)
    .with_no_client_auth();

    Arc::new(config)
}

/// Creates a dangerous rustls ClientConfig that skips server certificate verification (for testing).
pub fn create_insecure_tls_config() -> Arc<rustls::ClientConfig> {
    #[derive(Debug)]
    struct NoVerifier;
    impl rustls::client::danger::ServerCertVerifier for NoVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
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
            cert: &rustls::pki_types::CertificateDer<'_>,
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

    let config = rustls::ClientConfig::builder_with_provider(
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
    .expect("TLS protocol versions supported")
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(NoVerifier))
    .with_no_client_auth();

    Arc::new(config)
}

/// Connects to a DERP relay over the URL-specified transport, returning split frame sink and stream.
pub async fn connect_transport(
    url_str: &str,
    tls_config: Option<Arc<rustls::ClientConfig>>,
) -> Result<(FrameSink, FrameStream), MeshError> {
    let parsed_url = Url::parse(url_str)
        .map_err(|e| MeshError::Config(format!("Invalid relay URL '{url_str}': {e}")))?;

    let scheme = parsed_url.scheme();
    let host = parsed_url
        .host_str()
        .ok_or_else(|| MeshError::Config("Missing host in relay URL".into()))?;
    let port = parsed_url.port().unwrap_or(match scheme {
        "ws" | "http" | "tcp" => 80,
        "wss" | "https" | "tls" => 443,
        _ => 443,
    });

    match scheme {
        "ws" | "wss" => {
            let (ws_stream, _) = tokio_tungstenite::connect_async(url_str)
                .await
                .map_err(|e| MeshError::WebSocket(format!("WebSocket connect failed: {e}")))?;

            let (ws_sink, ws_source) = ws_stream.split();

            // Adapt WebSocket sink to accept DerpFrame
            let sink = ws_sink
                .sink_map_err(|e| MeshError::WebSocket(format!("WebSocket send error: {e}")))
                .with(|frame: DerpFrame| async move {
                    let mut buf = BytesMut::new();
                    frame.encode(&mut buf)?;
                    Ok::<_, MeshError>(tokio_tungstenite::tungstenite::Message::Binary(
                        buf.to_vec(),
                    ))
                });

            // Adapt WebSocket stream to yield DerpFrame
            let source = ws_source.filter_map(|msg_res| async move {
                match msg_res {
                    Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes)) => {
                        let mut buf = BytesMut::from(&bytes[..]);
                        match DerpFrame::decode(&mut buf) {
                            Ok(Some(frame)) => Some(Ok(frame)),
                            Ok(None) => None,
                            Err(e) => Some(Err(e)),
                        }
                    }
                    Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                        Some(Err(MeshError::ConnectionClosed("Relay closed WebSocket".into())))
                    }
                    Ok(tokio_tungstenite::tungstenite::Message::Ping(_))
                    | Ok(tokio_tungstenite::tungstenite::Message::Pong(_)) => None,
                    Ok(tokio_tungstenite::tungstenite::Message::Text(_)) => None,
                    Ok(tokio_tungstenite::tungstenite::Message::Frame(_)) => None,
                    Err(e) => Some(Err(MeshError::WebSocket(format!(
                        "WebSocket recv error: {e}"
                    )))),
                }
            });

            Ok((Box::pin(sink), Box::pin(source)))
        }

        "tcp" | "http" => {
            let addr = format!("{host}:{port}");
            let tcp_stream = TcpStream::connect(&addr)
                .await
                .map_err(MeshError::Io)?;
            let framed = Framed::new(tcp_stream, DerpCodec::new());
            let (sink, source) = framed.split();
            Ok((Box::pin(sink), Box::pin(source)))
        }

        "tls" | "https" => {
            let addr = format!("{host}:{port}");
            let tcp_stream = TcpStream::connect(&addr)
                .await
                .map_err(MeshError::Io)?;

            let tls_conf = tls_config.unwrap_or_else(create_insecure_tls_config);
            let connector = TlsConnector::from(tls_conf);
            let server_name = ServerName::try_from(host.to_string())
                .map_err(|e| MeshError::Tls(format!("Invalid DNS server name '{host}': {e}")))?;

            let tls_stream = connector
                .connect(server_name, tcp_stream)
                .await
                .map_err(|e| MeshError::Tls(format!("TLS handshake failed: {e}")))?;

            let framed = Framed::new(tls_stream, DerpCodec::new());
            let (sink, source) = framed.split();
            Ok((Box::pin(sink), Box::pin(source)))
        }

        other => Err(MeshError::Config(format!(
            "Unsupported relay transport scheme '{other}'"
        ))),
    }
}
