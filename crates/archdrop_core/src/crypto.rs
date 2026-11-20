//! Cryptographic identity, self-signed X.509 certificate generation,
//! private key management, and LocalSend SHA-256 fingerprint extraction.
//!
//! Compatible with upstream LocalSend v2 (Dart/Flutter and Android companions).

use crate::error::CryptoError;
use ring::digest::{digest, SHA256};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::Path;
use x509_parser::asn1_rs::FromDer;
use x509_parser::certificate::X509Certificate;
use x509_parser::pem::Pem;
use x509_parser::x509::SubjectPublicKeyInfo;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// A complete in-memory device identity consisting of an RSA-2048 private key,
/// self-signed X.509 certificate, DER representations, and computed SHA-256 fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CryptoIdentity {
    /// Private key in PEM format (PKCS#8).
    pub private_key_pem: String,
    /// Public certificate in PEM format.
    pub certificate_pem: String,
    /// Raw DER-encoded X.509 certificate bytes.
    pub certificate_der: Vec<u8>,
    /// Raw DER-encoded PKCS#8 private key bytes.
    pub private_key_der: Vec<u8>,
    /// 64-character uppercase hexadecimal SHA-256 fingerprint of `certificate_der`.
    pub fingerprint: String,
}

impl CryptoIdentity {
    /// Generates a new self-signed identity using RSA-2048 and `rcgen`.
    ///
    /// - Common Name is set to `LocalSend User` (matching upstream convention).
    /// - Validity period covers ~10 years (rcgen default: 1975 to 4096).
    /// - Fingerprint is extracted as uppercase-hex SHA-256 of the certificate DER.
    pub fn generate() -> Result<Self, CryptoError> {
        use rsa::pkcs8::{EncodePrivateKey, LineEnding};

        // 1. Generate RSA-2048 private key using system CSPRNG
        let mut rng = rsa::rand_core::OsRng;
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 2048)
            .map_err(|e| CryptoError::Rsa(format!("Failed to generate RSA-2048 key: {e}")))?;

        let private_key_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| {
                CryptoError::Rsa(format!(
                    "Failed to serialize PKCS#8 private key to PEM: {e}"
                ))
            })?
            .to_string();

        let pkcs8_der_doc = private_key.to_pkcs8_der().map_err(|e| {
            CryptoError::Rsa(format!(
                "Failed to serialize PKCS#8 private key to DER: {e}"
            ))
        })?;
        let private_key_der = pkcs8_der_doc.as_bytes().to_vec();

        // 2. Wrap private key into rcgen KeyPair
        let key_pair =
            rcgen::KeyPair::try_from(private_key_der.as_slice()).map_err(CryptoError::Rcgen)?;

        // 3. Configure self-signed X.509 parameters
        let mut params = rcgen::CertificateParams::default();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "LocalSend User");

        // 4. Generate certificate
        let cert = params.self_signed(&key_pair).map_err(CryptoError::Rcgen)?;

        let certificate_pem = cert.pem();
        let certificate_der = cert.der().to_vec();
        let fingerprint = fingerprint_from_cert_der(&certificate_der);

        Ok(Self {
            private_key_pem,
            certificate_pem,
            certificate_der,
            private_key_der,
            fingerprint,
        })
    }

    /// Constructs a `CryptoIdentity` from separate PEM strings for certificate and private key.
    pub fn from_pem_parts(cert_pem: &str, key_pem: &str) -> Result<Self, CryptoError> {
        let (parsed_cert_pem, _) = Pem::read(Cursor::new(cert_pem.as_bytes()))
            .map_err(|e| CryptoError::Pem(format!("Failed to parse certificate PEM: {e}")))?;
        let certificate_der = parsed_cert_pem.contents;

        let (parsed_key_pem, _) = Pem::read(Cursor::new(key_pem.as_bytes()))
            .map_err(|e| CryptoError::Pem(format!("Failed to parse private key PEM: {e}")))?;
        let private_key_der = parsed_key_pem.contents;

        let fingerprint = fingerprint_from_cert_der(&certificate_der);

        Ok(Self {
            private_key_pem: key_pem.trim().to_string(),
            certificate_pem: cert_pem.trim().to_string(),
            certificate_der,
            private_key_der,
            fingerprint,
        })
    }

    /// Constructs a `CryptoIdentity` from a combined PEM string containing both private key and certificate.
    pub fn from_combined_pem(combined_pem: &str) -> Result<Self, CryptoError> {
        let mut cert_pem_opt = None;
        let mut key_pem_opt = None;
        let mut cert_der_opt = None;
        let mut key_der_opt = None;

        let mut cursor = Cursor::new(combined_pem.as_bytes());
        while let Ok((pem, _)) = Pem::read(&mut cursor) {
            match pem.label.as_str() {
                "CERTIFICATE" => {
                    cert_der_opt = Some(pem.contents.clone());
                    cert_pem_opt = Some(pem::encode(&pem::Pem::new("CERTIFICATE", pem.contents)));
                }
                "PRIVATE KEY" | "RSA PRIVATE KEY" => {
                    key_der_opt = Some(pem.contents.clone());
                    key_pem_opt = Some(pem::encode(&pem::Pem::new("PRIVATE KEY", pem.contents)));
                }
                _ => {}
            }
        }

        let certificate_der = cert_der_opt.ok_or(CryptoError::MissingIdentityComponent)?;
        let private_key_der = key_der_opt.ok_or(CryptoError::MissingIdentityComponent)?;
        let certificate_pem = cert_pem_opt.ok_or(CryptoError::MissingIdentityComponent)?;
        let private_key_pem = key_pem_opt.ok_or(CryptoError::MissingIdentityComponent)?;

        let fingerprint = fingerprint_from_cert_der(&certificate_der);

        Ok(Self {
            private_key_pem,
            certificate_pem,
            certificate_der,
            private_key_der,
            fingerprint,
        })
    }

    /// Serializes identity as a combined PEM document suitable for writing to `identity.pem`.
    pub fn to_combined_pem(&self) -> String {
        format!(
            "{}\n\n{}\n",
            self.private_key_pem.trim(),
            self.certificate_pem.trim()
        )
    }

    /// Loads the identity from a PEM file.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, CryptoError> {
        let content = fs::read_to_string(path.as_ref()).map_err(CryptoError::Io)?;
        Self::from_combined_pem(&content)
    }

    /// Saves the identity to a combined PEM file with strict POSIX permissions (`0o600`).
    ///
    /// Parent directories are automatically created with `0o700` permissions if missing.
    /// Employs atomic file replacement: writes to a uniquely named temporary file
    /// (`<path>.tmp.<pid>.<uuid>`) with mode `0o600`, flushes and syncs to disk (`sync_all()`),
    /// and atomically renames over the target path. This guarantees concurrent readers
    /// never observe zero-byte or partially written files, and prevents data corruption
    /// on crash or power interruption.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), CryptoError> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
                }
            }
        }

        let combined = self.to_combined_pem();

        // Construct unique temporary file path in the same parent directory to ensure atomic rename
        let nonce = uuid::Uuid::new_v4().simple();
        let tmp_extension = match p.extension() {
            Some(ext) => format!(
                "{}.tmp.{}.{}",
                ext.to_string_lossy(),
                std::process::id(),
                nonce
            ),
            None => format!("tmp.{}.{}", std::process::id(), nonce),
        };
        let tmp_path = p.with_extension(tmp_extension);

        #[cfg(unix)]
        {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let mut file = match options.open(&tmp_path) {
                Ok(f) => f,
                Err(_) => {
                    // Fallback to create + truncate if create_new is not supported by filesystem
                    let mut fallback = OpenOptions::new();
                    fallback.write(true).create(true).truncate(true).mode(0o600);
                    fallback.open(&tmp_path)?
                }
            };

            // Explicit fchmod on open file descriptor to guarantee 0o600 regardless of umask
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));

            if let Err(e) = file.write_all(combined.as_bytes()) {
                let _ = fs::remove_file(&tmp_path);
                return Err(CryptoError::Io(e));
            }
            if let Err(e) = file.sync_all() {
                let _ = fs::remove_file(&tmp_path);
                return Err(CryptoError::Io(e));
            }
        }

        #[cfg(not(unix))]
        {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = match options.open(&tmp_path) {
                Ok(f) => f,
                Err(_) => {
                    let mut fallback = OpenOptions::new();
                    fallback.write(true).create(true).truncate(true);
                    fallback.open(&tmp_path)?
                }
            };

            if let Err(e) = file.write_all(combined.as_bytes()) {
                let _ = fs::remove_file(&tmp_path);
                return Err(CryptoError::Io(e));
            }
            if let Err(e) = file.sync_all() {
                let _ = fs::remove_file(&tmp_path);
                return Err(CryptoError::Io(e));
            }
        }

        if let Err(e) = fs::rename(&tmp_path, p) {
            let _ = fs::remove_file(&tmp_path);
            return Err(CryptoError::Io(e));
        }

        Ok(())
    }

    /// Loads the identity from the specified path, or generates and saves a new one if missing.
    pub fn load_or_generate<P: AsRef<Path>>(path: P) -> Result<Self, CryptoError> {
        let p = path.as_ref();
        if p.exists() {
            tracing::info!("Loading existing identity from {}", p.display());
            Self::load_from_file(p)
        } else {
            tracing::info!("Generating new identity at {}", p.display());
            let identity = Self::generate()?;
            identity.save_to_file(p)?;
            Ok(identity)
        }
    }

    /// Returns the DER certificate wrapped as a `rustls_pki_types::CertificateDer`.
    pub fn rustls_cert(&self) -> CertificateDer<'static> {
        CertificateDer::from(self.certificate_der.clone())
    }

    /// Returns the DER private key wrapped as a `rustls_pki_types::PrivateKeyDer::Pkcs8`.
    pub fn rustls_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.private_key_der.clone()))
    }
}

/// Computes the SHA-256 fingerprint of a DER-encoded X.509 certificate.
///
/// Output format: 64 uppercase hexadecimal characters, matching upstream LocalSend.
pub fn fingerprint_from_cert_der(cert_der: &[u8]) -> String {
    let d = digest(&SHA256, cert_der);
    d.as_ref().iter().map(|b| format!("{b:02X}")).collect()
}

/// Computes the SHA-256 fingerprint from a PEM-encoded X.509 certificate.
pub fn fingerprint_from_cert_pem(cert_pem: &str) -> Result<String, CryptoError> {
    let (pem, _) = Pem::read(Cursor::new(cert_pem.as_bytes()))
        .map_err(|e| CryptoError::Pem(format!("Failed to parse PEM certificate: {e}")))?;
    Ok(fingerprint_from_cert_der(&pem.contents))
}

/// Validates that a string is a valid 64-character hexadecimal SHA-256 fingerprint.
pub fn validate_fingerprint(fingerprint: &str) -> bool {
    fingerprint.len() == 64 && fingerprint.chars().all(|c| c.is_ascii_hexdigit())
}

/// Normalizes a fingerprint by stripping colons, dashes, and whitespace,
/// validating it and returning the uppercase canonical form.
pub fn normalize_fingerprint(fingerprint: &str) -> Option<String> {
    let cleaned: String = fingerprint
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != ':' && *c != '-')
        .collect();

    if validate_fingerprint(&cleaned) {
        Some(cleaned.to_ascii_uppercase())
    } else {
        None
    }
}

/// Compares two fingerprints for equality in a delimiter-agnostic and case-insensitive manner.
pub fn fingerprint_matches(a: &str, b: &str) -> bool {
    match (normalize_fingerprint(a), normalize_fingerprint(b)) {
        (Some(norm_a), Some(norm_b)) => norm_a == norm_b,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// Verifies whether an X.509 certificate in DER format is valid:
/// - According to self-signed signature
/// - According to time validity window
/// - According to expected public key PEM (if provided)
pub fn verify_cert_from_der(
    cert_der: &[u8],
    expected_public_key_pem: Option<&str>,
) -> Result<(), CryptoError> {
    let (_, parsed_cert) = X509Certificate::from_der(cert_der)
        .map_err(|e| CryptoError::X509Parse(format!("Failed to parse X.509 DER: {e}")))?;

    verify_cert_inner(parsed_cert, expected_public_key_pem)
}

/// Verifies whether an X.509 certificate in PEM format is valid.
pub fn verify_cert_from_pem(
    cert_pem: &str,
    expected_public_key_pem: Option<&str>,
) -> Result<(), CryptoError> {
    let (parsed_pem, _) = Pem::read(Cursor::new(cert_pem.as_bytes()))
        .map_err(|e| CryptoError::Pem(format!("Failed to parse PEM: {e}")))?;
    let parsed_cert = parsed_pem
        .parse_x509()
        .map_err(|e| CryptoError::X509Parse(format!("Failed to parse X.509 from PEM: {e}")))?;

    verify_cert_inner(parsed_cert, expected_public_key_pem)
}

/// Extracts the public key from a DER certificate and encodes it as a PEM string.
pub fn public_key_from_cert_der(cert_der: &[u8]) -> Result<String, CryptoError> {
    let (_, parsed_cert) = X509Certificate::from_der(cert_der)
        .map_err(|e| CryptoError::X509Parse(format!("Failed to parse X.509 DER: {e}")))?;
    let cert_public_key = parsed_cert.tbs_certificate.subject_pki.raw;
    let public_key = pem::encode(&pem::Pem::new("PUBLIC KEY", cert_public_key.to_vec()));
    Ok(public_key)
}

fn verify_cert_inner(
    cert: X509Certificate,
    expected_public_key_pem: Option<&str>,
) -> Result<(), CryptoError> {
    if !cert.validity.is_valid() {
        return Err(CryptoError::ValidityExpired);
    }

    if let Some(expected_pk_pem) = expected_public_key_pem {
        let cert_public_key =
            cert.tbs_certificate.subject_pki.parsed().map_err(|e| {
                CryptoError::X509Parse(format!("Failed to parse cert public key: {e}"))
            })?;

        let (pk_pem, _) = Pem::read(Cursor::new(expected_pk_pem.as_bytes())).map_err(|e| {
            CryptoError::Pem(format!("Failed to parse expected public key PEM: {e}"))
        })?;
        let (_, pk_spki) = SubjectPublicKeyInfo::from_der(&pk_pem.contents)
            .map_err(|e| CryptoError::X509Parse(format!("Failed to parse SPKI: {e}")))?;
        let expected_public_key = pk_spki.parsed().map_err(|e| {
            CryptoError::X509Parse(format!("Failed to parse expected public key: {e}"))
        })?;

        if cert_public_key != expected_public_key {
            return Err(CryptoError::PublicKeyMismatch);
        }
    }

    cert.verify_signature(None)
        .map_err(|e| CryptoError::SignatureVerification(format!("{e}")))?;

    Ok(())
}
