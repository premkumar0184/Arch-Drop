//! Ed25519 peer public-key cryptography and mutual challenge authentication.
//!
//! Powered by `ring::signature::ED25519` for constant-time, side-channel resistant
//! key generation, signing, and verification.

use crate::error::MeshError;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::agreement::{
    agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey as UnparsedAgreementPublicKey, X25519,
};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Challenge protocol domain separation tag.
pub const CHALLENGE_TAG: &[u8] = b"ARCHDROP-DERP-V1";

/// Default allowable clock skew for handshake timestamps (60 seconds).
pub const MAX_HANDSHAKE_SKEW_SECS: u64 = 60;

/// Compact 32-byte Ed25519 peer public key used for mesh addressing and routing.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshPublicKey(pub [u8; 32]);

impl MeshPublicKey {
    pub fn new(key: [u8; 32]) -> Self {
        Self(key)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode_upper(self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, MeshError> {
        let cleaned: String = s.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
        let bytes = hex::decode(cleaned)
            .map_err(|e| MeshError::Crypto(format!("Invalid public key hex: {e}")))?;
        if bytes.len() != 32 {
            return Err(MeshError::Crypto(format!(
                "Invalid public key length: expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    /// Verifies an Ed25519 signature over a message with this public key.
    pub fn verify(&self, message: &[u8], signature: &[u8; 64]) -> Result<(), MeshError> {
        let peer_pubkey = UnparsedPublicKey::new(&ED25519, &self.0);
        peer_pubkey
            .verify(message, signature)
            .map_err(|_| MeshError::AuthFailed("Ed25519 signature verification failed".into()))
    }

    /// Converts this Ed25519 public key to a Montgomery Curve25519 (X25519) public key
    /// via birational equivalence: u = (1 + y) * (1 - y)^(-1) mod (2^255 - 19).
    pub fn to_x25519(&self) -> Result<[u8; 32], MeshError> {
        edwards_to_montgomery_pub(&self.0)
    }
}

impl fmt::Debug for MeshPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MeshPublicKey({})", self.to_hex())
    }
}

impl fmt::Display for MeshPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Serialize for MeshPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for MeshPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        MeshPublicKey::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// An Ed25519 keypair for local device mesh identity and session authentication.
pub struct MeshKeyPair {
    pkcs8_bytes: Vec<u8>,
    key_pair: Ed25519KeyPair,
    public_key: MeshPublicKey,
}

impl MeshKeyPair {
    /// Generates a new random Ed25519 keypair using the OS secure random number generator.
    pub fn generate() -> Result<Self, MeshError> {
        let rng = SystemRandom::new();
        let pkcs8_doc = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| MeshError::Crypto("Failed to generate Ed25519 keypair".into()))?;
        let pkcs8_bytes = pkcs8_doc.as_ref().to_vec();
        Self::from_pkcs8(&pkcs8_bytes)
    }

    /// Reconstructs a keypair from raw PKCS#8 DER bytes.
    pub fn from_pkcs8(bytes: &[u8]) -> Result<Self, MeshError> {
        let key_pair = Ed25519KeyPair::from_pkcs8(bytes)
            .map_err(|e| MeshError::Crypto(format!("Failed to parse Ed25519 PKCS#8 DER: {e}")))?;
        let pub_bytes = key_pair.public_key().as_ref();
        if pub_bytes.len() != 32 {
            return Err(MeshError::Crypto("Public key length mismatch".into()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(pub_bytes);
        Ok(Self {
            pkcs8_bytes: bytes.to_vec(),
            key_pair,
            public_key: MeshPublicKey(key),
        })
    }

    /// Returns the raw PKCS#8 DER bytes.
    pub fn pkcs8_bytes(&self) -> &[u8] {
        &self.pkcs8_bytes
    }

    /// Returns the associated 32-byte public key.
    pub fn public_key(&self) -> MeshPublicKey {
        self.public_key
    }

    /// Signs an arbitrary message, returning a 64-byte Ed25519 signature.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let sig = self.key_pair.sign(message);
        let mut out = [0u8; 64];
        out.copy_from_slice(sig.as_ref());
        out
    }

    /// Constructs the challenge message buffer for DERP authentication:
    /// `CHALLENGE_TAG || server_key || timestamp_be`
    pub fn make_challenge_message(server_key: &[u8; 32], timestamp: u64) -> Vec<u8> {
        let mut msg = Vec::with_capacity(CHALLENGE_TAG.len() + 32 + 8);
        msg.extend_from_slice(CHALLENGE_TAG);
        msg.extend_from_slice(server_key);
        msg.extend_from_slice(&timestamp.to_be_bytes());
        msg
    }

    /// Signs a challenge for the specified server key and timestamp.
    pub fn sign_challenge(&self, server_key: &[u8; 32], timestamp: u64) -> [u8; 64] {
        let msg = Self::make_challenge_message(server_key, timestamp);
        self.sign(&msg)
    }

    /// Verifies a challenge response from a client:
    /// Validates that timestamp skew is within `max_skew_secs` and signature is valid.
    pub fn verify_challenge(
        client_key: &MeshPublicKey,
        server_key: &[u8; 32],
        timestamp: u64,
        signature: &[u8; 64],
        max_skew_secs: u64,
    ) -> Result<(), MeshError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| MeshError::Crypto(format!("System time before UNIX epoch: {e}")))?
            .as_secs();

        let diff = now.abs_diff(timestamp);

        if diff > max_skew_secs {
            return Err(MeshError::AuthFailed(format!(
                "Handshake timestamp skew {diff}s exceeds maximum allowable {max_skew_secs}s"
            )));
        }

        let msg = Self::make_challenge_message(server_key, timestamp);
        client_key.verify(&msg, signature)
    }

    /// Persists the PKCS#8 keypair to disk with POSIX permissions 0o600.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), MeshError> {
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

        #[cfg(unix)]
        {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true).mode(0o600);
            let mut file = options.open(p)?;
            file.write_all(&self.pkcs8_bytes)?;
            file.sync_all()?;
        }

        #[cfg(not(unix))]
        {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            let mut file = options.open(p)?;
            file.write_all(&self.pkcs8_bytes)?;
            file.sync_all()?;
        }

        Ok(())
    }

    /// Loads the PKCS#8 keypair from a file.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, MeshError> {
        let bytes = fs::read(path.as_ref())?;
        Self::from_pkcs8(&bytes)
    }

    /// Loads an existing keypair from the specified file or generates and saves a new one.
    pub fn load_or_generate<P: AsRef<Path>>(path: P) -> Result<Self, MeshError> {
        let p = path.as_ref();
        if p.exists() {
            tracing::info!("Loading existing mesh key from {}", p.display());
            Self::load_from_file(p)
        } else {
            tracing::info!("Generating new mesh keypair at {}", p.display());
            let pair = Self::generate()?;
            pair.save_to_file(p)?;
            Ok(pair)
        }
    }
}

// ============================================================================
// Noise Protocol CipherState & ChaCha20-Poly1305 AEAD Implementation
// ============================================================================

/// Authentication tag length for ChaCha20-Poly1305 (16 bytes).
pub const TAG_LEN: usize = 16;

/// Authenticated AEAD cipher state implementing the Noise Protocol CipherState.
///
/// Encrypts and decrypts payloads using ChaCha20-Poly1305 with an auto-incrementing
/// 64-bit nonce formatted according to the Noise Protocol specification:
/// 32 bits of zero prefix || 64-bit little-endian counter (12 bytes total).
pub struct NoiseCipherState {
    key: LessSafeKey,
    raw_key: [u8; 32],
    nonce: u64,
}

impl NoiseCipherState {
    /// Initializes a new CipherState with a 32-byte key. Nonce is initialized to 0.
    pub fn new(key: &[u8; 32]) -> Result<Self, MeshError> {
        let unbound_key = UnboundKey::new(&CHACHA20_POLY1305, key)
            .map_err(|_| MeshError::Crypto("Failed to create ChaCha20-Poly1305 key".into()))?;
        let less_safe = LessSafeKey::new(unbound_key);
        Ok(Self {
            key: less_safe,
            raw_key: *key,
            nonce: 0,
        })
    }

    /// Returns the current 64-bit nonce counter.
    pub fn nonce(&self) -> u64 {
        self.nonce
    }

    /// Returns the raw 32-byte key.
    pub fn raw_key(&self) -> &[u8; 32] {
        &self.raw_key
    }

    /// Formats the 12-byte Noise nonce from a 64-bit counter:
    /// `[0x00, 0x00, 0x00, 0x00, (n & 0xff), ..., ((n >> 56) & 0xff)]`
    pub fn format_nonce(n: u64) -> Result<Nonce, MeshError> {
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[4..12].copy_from_slice(&n.to_le_bytes());
        Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| MeshError::Crypto("Failed to construct AEAD nonce".into()))
    }

    /// Encrypts a plaintext payload with empty associated data, appending the 16-byte Poly1305 tag.
    /// Increments the nonce counter by 1.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, MeshError> {
        self.encrypt_with_ad(&[], plaintext)
    }

    /// Encrypts a plaintext payload with associated data (`ad`), appending the 16-byte Poly1305 tag.
    /// Increments the nonce counter by 1. Refuses operation if nonce exhausts `u64::MAX`.
    pub fn encrypt_with_ad(&mut self, ad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, MeshError> {
        if self.nonce == u64::MAX {
            return Err(MeshError::Crypto(
                "Noise cipherstate nonce exhausted (2^64-1 limit reached)".into(),
            ));
        }

        let nonce = Self::format_nonce(self.nonce)?;
        let mut in_out = plaintext.to_vec();
        self.key
            .seal_in_place_append_tag(nonce, Aad::from(ad), &mut in_out)
            .map_err(|_| MeshError::Crypto("AEAD encryption failed".into()))?;

        self.nonce += 1;
        Ok(in_out)
    }

    /// Decrypts a ciphertext payload with empty associated data, verifying the 16-byte Poly1305 tag.
    /// Increments the nonce counter by 1 on success.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, MeshError> {
        self.decrypt_with_ad(&[], ciphertext)
    }

    /// Decrypts a ciphertext payload with associated data (`ad`), verifying the 16-byte Poly1305 tag.
    /// Increments the nonce counter by 1 on success.
    pub fn decrypt_with_ad(&mut self, ad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, MeshError> {
        if ciphertext.len() < TAG_LEN {
            return Err(MeshError::Crypto(format!(
                "Ciphertext too short: {} < {TAG_LEN}",
                ciphertext.len()
            )));
        }
        if self.nonce == u64::MAX {
            return Err(MeshError::Crypto("Noise cipherstate nonce exhausted".into()));
        }

        let nonce = Self::format_nonce(self.nonce)?;
        let mut in_out = ciphertext.to_vec();
        let decrypted_len = {
            let plaintext = self
                .key
                .open_in_place(nonce, Aad::from(ad), &mut in_out)
                .map_err(|_| {
                    MeshError::Crypto("AEAD authentication failed (tag mismatch or corrupted data)".into())
                })?;
            plaintext.len()
        };

        in_out.truncate(decrypted_len);
        self.nonce += 1;
        Ok(in_out)
    }

    /// Rekey according to Noise specification: encrypts 32 zero bytes under current key with max nonce.
    pub fn rekey(&mut self) -> Result<(), MeshError> {
        let max_nonce = Self::format_nonce(u64::MAX)?;
        let mut zeros = vec![0u8; 32];
        let tag = self
            .key
            .seal_in_place_separate_tag(max_nonce, Aad::empty(), &mut zeros)
            .map_err(|_| MeshError::Crypto("Failed to rekey".into()))?;
        let mut new_key_bytes = [0u8; 32];
        new_key_bytes[..16].copy_from_slice(&zeros[..16]);
        new_key_bytes[16..].copy_from_slice(tag.as_ref());
        *self = Self::new(&new_key_bytes)?;
        Ok(())
    }
}

/// Full-duplex Noise session managing separate TX (encryption) and RX (decryption) CipherStates.
pub struct NoiseSession {
    tx: NoiseCipherState,
    rx: NoiseCipherState,
}

impl NoiseSession {
    /// Creates a full-duplex session from distinct transmit and receive keys.
    pub fn from_keys(tx_key: &[u8; 32], rx_key: &[u8; 32]) -> Result<Self, MeshError> {
        Ok(Self {
            tx: NoiseCipherState::new(tx_key)?,
            rx: NoiseCipherState::new(rx_key)?,
        })
    }

    /// Creates a session from existing CipherState instances.
    pub fn from_cipher_states(tx: NoiseCipherState, rx: NoiseCipherState) -> Self {
        Self { tx, rx }
    }

    /// Splits the session into separate TX and RX CipherStates according to Noise specification.
    pub fn split(self) -> (NoiseCipherState, NoiseCipherState) {
        (self.tx, self.rx)
    }

    /// Encrypts an outbound payload chunk using the TX cipherstate.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, MeshError> {
        self.tx.encrypt(plaintext)
    }

    /// Encrypts an outbound payload chunk with associated data using the TX cipherstate.
    pub fn encrypt_with_ad(&mut self, ad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, MeshError> {
        self.tx.encrypt_with_ad(ad, plaintext)
    }

    /// Decrypts an inbound payload chunk using the RX cipherstate.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, MeshError> {
        self.rx.decrypt(ciphertext)
    }

    /// Decrypts an inbound payload chunk with associated data using the RX cipherstate.
    pub fn decrypt_with_ad(&mut self, ad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, MeshError> {
        self.rx.decrypt_with_ad(ad, ciphertext)
    }

    /// Current transmit nonce.
    pub fn tx_nonce(&self) -> u64 {
        self.tx.nonce()
    }

    /// Current receive nonce.
    pub fn rx_nonce(&self) -> u64 {
        self.rx.nonce()
    }

    /// Mutable reference to TX cipherstate.
    pub fn tx_mut(&mut self) -> &mut NoiseCipherState {
        &mut self.tx
    }

    /// Mutable reference to RX cipherstate.
    pub fn rx_mut(&mut self) -> &mut NoiseCipherState {
        &mut self.rx
    }
}

/// Ephemeral X25519 key agreement using `ring::agreement::X25519`.
pub struct EphemeralX25519 {
    private_key: EphemeralPrivateKey,
    public_key: [u8; 32],
}

impl EphemeralX25519 {
    /// Generates a new random ephemeral X25519 keypair.
    pub fn generate() -> Result<Self, MeshError> {
        let rng = SystemRandom::new();
        let private_key = EphemeralPrivateKey::generate(&X25519, &rng)
            .map_err(|_| MeshError::Crypto("Failed to generate ephemeral X25519 key".into()))?;
        let pub_key = private_key
            .compute_public_key()
            .map_err(|_| MeshError::Crypto("Failed to compute X25519 public key".into()))?;

        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(pub_key.as_ref());

        Ok(Self {
            private_key,
            public_key,
        })
    }

    /// Returns the 32-byte ephemeral public key.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    /// Completes the Diffie-Hellman agreement with a peer's X25519 public key,
    /// returning the 32-byte shared secret.
    pub fn agree(self, peer_public_key: &[u8; 32]) -> Result<[u8; 32], MeshError> {
        let peer_pub = UnparsedAgreementPublicKey::new(&X25519, peer_public_key);
        let mut shared_secret = [0u8; 32];
        agree_ephemeral(
            self.private_key,
            &peer_pub,
            |key_material| {
                if key_material.len() == 32 {
                    shared_secret.copy_from_slice(key_material);
                    Ok(())
                } else {
                    Err(MeshError::Crypto("Unexpected shared secret length".into()))
                }
            },
        )
        .map_err(|_| MeshError::Crypto("X25519 key agreement failed".into()))?
        .map_err(|e| MeshError::Crypto(format!("Key agreement closure failed: {e:?}")))?;

        Ok(shared_secret)
    }
}

/// Internal constant-time HMAC-SHA256 helper.
pub fn compute_hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let hash = Sha256::digest(key);
        k[..32].copy_from_slice(&hash);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut i_key_pad = [0x36u8; 64];
    let mut o_key_pad = [0x5cu8; 64];
    for idx in 0..64 {
        i_key_pad[idx] ^= k[idx];
        o_key_pad[idx] ^= k[idx];
    }

    let mut inner = Sha256::new();
    inner.update(&i_key_pad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&o_key_pad);
    outer.update(&inner_hash);
    let outer_hash = outer.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_hash);
    out
}

/// Derives full-duplex session keys (tx_key, rx_key) from an agreed shared secret
/// and peer public keys using HKDF-SHA256.
pub fn derive_session_keys(
    shared_secret: &[u8; 32],
    initiator_key: &[u8; 32],
    responder_key: &[u8; 32],
    is_initiator: bool,
) -> Result<NoiseSession, MeshError> {
    let salt = b"ARCHDROP-NOISE-SESSION-V1";
    let prk = compute_hmac_sha256(salt, shared_secret);

    let mut info1 = Vec::with_capacity(64);
    info1.extend_from_slice(b"init_to_resp");
    info1.extend_from_slice(initiator_key);
    info1.extend_from_slice(responder_key);
    info1.push(0x01);
    let k_init_to_resp = compute_hmac_sha256(&prk, &info1);

    let mut info2 = Vec::with_capacity(64);
    info2.extend_from_slice(b"resp_to_init");
    info2.extend_from_slice(initiator_key);
    info2.extend_from_slice(responder_key);
    info2.push(0x02);
    let k_resp_to_init = compute_hmac_sha256(&prk, &info2);

    let (tx_key, rx_key) = if is_initiator {
        (k_init_to_resp, k_resp_to_init)
    } else {
        (k_resp_to_init, k_init_to_resp)
    };

    NoiseSession::from_keys(&tx_key, &rx_key)
}

/// Derives deterministic full-duplex virtual stream cipher keys from paired mesh identities and stream ID.
///
/// Enables 0-RTT end-to-end payload encryption between paired endpoints over DERP.
pub fn derive_stream_keys(
    local_key: &[u8; 32],
    remote_key: &[u8; 32],
    stream_id: u64,
    is_initiator: bool,
) -> Result<NoiseSession, MeshError> {
    let (initiator_key, responder_key) = if is_initiator {
        (local_key, remote_key)
    } else {
        (remote_key, local_key)
    };

    let mut ikm = [0u8; 64];
    if initiator_key < responder_key {
        ikm[..32].copy_from_slice(initiator_key);
        ikm[32..].copy_from_slice(responder_key);
    } else {
        ikm[..32].copy_from_slice(responder_key);
        ikm[32..].copy_from_slice(initiator_key);
    }

    let salt = b"ARCHDROP-DERP-STREAM-NOISE-V1";
    let prk = compute_hmac_sha256(salt, &ikm);

    let mut info1 = Vec::with_capacity(48);
    info1.extend_from_slice(b"stream_i2r:");
    info1.extend_from_slice(&stream_id.to_be_bytes());
    info1.extend_from_slice(initiator_key);
    info1.push(0x01);
    let k_init_to_resp = compute_hmac_sha256(&prk, &info1);

    let mut info2 = Vec::with_capacity(48);
    info2.extend_from_slice(b"stream_r2i:");
    info2.extend_from_slice(&stream_id.to_be_bytes());
    info2.extend_from_slice(initiator_key);
    info2.push(0x02);
    let k_resp_to_init = compute_hmac_sha256(&prk, &info2);

    let (tx_key, rx_key) = if is_initiator {
        (k_init_to_resp, k_resp_to_init)
    } else {
        (k_resp_to_init, k_init_to_resp)
    };

    NoiseSession::from_keys(&tx_key, &rx_key)
}

/// Derives deterministic full-duplex virtual stream cipher keys from a shared secret and stream ID.
pub fn derive_stream_keys_from_secret(
    shared_secret: &[u8; 32],
    stream_id: u64,
    is_initiator: bool,
) -> Result<NoiseSession, MeshError> {
    let salt = b"ARCHDROP-DERP-STREAM-NOISE-V1";
    let prk = compute_hmac_sha256(salt, shared_secret);

    let mut info1 = Vec::with_capacity(32);
    info1.extend_from_slice(b"stream_i2r:");
    info1.extend_from_slice(&stream_id.to_be_bytes());
    info1.push(0x01);
    let k_init_to_resp = compute_hmac_sha256(&prk, &info1);

    let mut info2 = Vec::with_capacity(32);
    info2.extend_from_slice(b"stream_r2i:");
    info2.extend_from_slice(&stream_id.to_be_bytes());
    info2.push(0x02);
    let k_resp_to_init = compute_hmac_sha256(&prk, &info2);

    let (tx_key, rx_key) = if is_initiator {
        (k_init_to_resp, k_resp_to_init)
    } else {
        (k_resp_to_init, k_init_to_resp)
    };

    NoiseSession::from_keys(&tx_key, &rx_key)
}

/// Converts an Ed25519 32-byte private seed to a clamped X25519 private scalar.
pub fn edwards_priv_to_x25519_priv(ed_priv_seed: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha512};
    let mut hasher = Sha512::new();
    hasher.update(ed_priv_seed);
    let digest = hasher.finalize();
    let mut s = [0u8; 32];
    s.copy_from_slice(&digest[..32]);
    s[0] &= 248;
    s[31] &= 127;
    s[31] |= 64;
    s
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct U256(pub [u64; 4]);

impl U256 {
    const ONE: U256 = U256([1, 0, 0, 0]);
    // P = 2^255 - 19
    const P: U256 = U256([
        0xffffffffffffffed,
        0xffffffffffffffff,
        0xffffffffffffffff,
        0x7fffffffffffffff,
    ]);

    fn from_bytes_le(b: &[u8; 32]) -> Self {
        let mut limbs = [0u64; 4];
        for i in 0..4 {
            limbs[i] = u64::from_le_bytes(b[i * 8..(i + 1) * 8].try_into().unwrap());
        }
        U256(limbs)
    }

    fn to_bytes_le(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for i in 0..4 {
            bytes[i * 8..(i + 1) * 8].copy_from_slice(&self.0[i].to_le_bytes());
        }
        bytes
    }

    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        for i in (0..4).rev() {
            if self.0[i] < other.0[i] {
                return std::cmp::Ordering::Less;
            } else if self.0[i] > other.0[i] {
                return std::cmp::Ordering::Greater;
            }
        }
        std::cmp::Ordering::Equal
    }

    fn add_mod_p(&self, other: &Self) -> Self {
        let mut res = [0u64; 4];
        let mut carry = 0u128;
        for i in 0..4 {
            let sum = (self.0[i] as u128) + (other.0[i] as u128) + carry;
            res[i] = sum as u64;
            carry = sum >> 64;
        }
        let mut out = U256(res);
        if carry > 0 || out.cmp(&Self::P) != std::cmp::Ordering::Less {
            out = out.sub_p();
        }
        out
    }

    fn sub_mod_p(&self, other: &Self) -> Self {
        let mut res = [0u64; 4];
        let mut borrow = 0i128;
        for i in 0..4 {
            let diff = (self.0[i] as i128) - (other.0[i] as i128) - borrow;
            if diff < 0 {
                res[i] = (diff + (1i128 << 64)) as u64;
                borrow = 1;
            } else {
                res[i] = diff as u64;
                borrow = 0;
            }
        }
        let mut out = U256(res);
        if borrow != 0 {
            let mut carry = 0u128;
            for i in 0..4 {
                let sum = (out.0[i] as u128) + (Self::P.0[i] as u128) + carry;
                out.0[i] = sum as u64;
                carry = sum >> 64;
            }
        }
        out
    }

    fn sub_p(&self) -> Self {
        let mut res = [0u64; 4];
        let mut borrow = 0i128;
        for i in 0..4 {
            let diff = (self.0[i] as i128) - (Self::P.0[i] as i128) - borrow;
            if diff < 0 {
                res[i] = (diff + (1i128 << 64)) as u64;
                borrow = 1;
            } else {
                res[i] = diff as u64;
                borrow = 0;
            }
        }
        U256(res)
    }

    fn mul_mod_p(&self, other: &Self) -> Self {
        let mut prod = [0u64; 8];
        for i in 0..4 {
            let mut carry = 0u128;
            for j in 0..4 {
                let p = (self.0[i] as u128) * (other.0[j] as u128) + (prod[i + j] as u128) + carry;
                prod[i + j] = p as u64;
                carry = p >> 64;
            }
            let mut k = i + 4;
            while carry > 0 && k < 8 {
                let p = (prod[k] as u128) + carry;
                prod[k] = p as u64;
                carry = p >> 64;
                k += 1;
            }
        }
        Self::reduce512(&prod)
    }

    fn reduce512(prod: &[u64; 8]) -> Self {
        let l = [prod[0], prod[1], prod[2], prod[3] & 0x7fffffffffffffff];

        let mut h = [0u64; 5];
        h[0] = (prod[3] >> 63) | (prod[4] << 1);
        h[1] = (prod[4] >> 63) | (prod[5] << 1);
        h[2] = (prod[5] >> 63) | (prod[6] << 1);
        h[3] = (prod[6] >> 63) | (prod[7] << 1);
        h[4] = prod[7] >> 63;

        let mut h19 = [0u64; 5];
        let mut carry = 0u128;
        for i in 0..5 {
            let p = (h[i] as u128) * 19 + carry;
            h19[i] = p as u64;
            carry = p >> 64;
        }

        let mut y = [0u64; 5];
        carry = 0;
        for i in 0..4 {
            let sum = (l[i] as u128) + (h19[i] as u128) + carry;
            y[i] = sum as u64;
            carry = sum >> 64;
        }
        y[4] = (h19[4] as u128 + carry) as u64;

        let l2 = [y[0], y[1], y[2], y[3] & 0x7fffffffffffffff];
        let h2 = (y[3] >> 63) | (y[4] << 1);

        let z_add = (h2 as u128) * 19;
        let mut z = [0u64; 4];
        carry = z_add;
        for i in 0..4 {
            let sum = (l2[i] as u128) + carry;
            z[i] = sum as u64;
            carry = sum >> 64;
        }

        let mut out = U256(z);
        while carry > 0 || out.cmp(&Self::P) != std::cmp::Ordering::Less {
            out = out.sub_p();
            if carry > 0 {
                carry -= 1;
            }
        }
        out
    }

    fn invert_mod_p(&self) -> Self {
        let p_minus_2: [u64; 4] = [
            0xffffffffffffffeb,
            0xffffffffffffffff,
            0xffffffffffffffff,
            0x7fffffffffffffff,
        ];
        let mut res = Self::ONE;
        let mut base = *self;

        for limb in p_minus_2.iter() {
            let mut val = *limb;
            for _ in 0..64 {
                if (val & 1) != 0 {
                    res = res.mul_mod_p(&base);
                }
                base = base.mul_mod_p(&base);
                val >>= 1;
            }
        }
        res
    }
}

/// Converts a 32-byte Ed25519 public key to a Montgomery Curve25519 (X25519) public key:
/// u = (1 + y) * (1 - y)^(-1) mod (2^255 - 19)
pub fn edwards_to_montgomery_pub(ed_pub: &[u8; 32]) -> Result<[u8; 32], MeshError> {
    let mut y_bytes = *ed_pub;
    y_bytes[31] &= 0x7F; // Mask off sign bit

    let y = U256::from_bytes_le(&y_bytes);
    if y.cmp(&U256::P) != std::cmp::Ordering::Less {
        return Err(MeshError::Crypto(
            "Invalid Ed25519 public key: coordinate exceeds field prime 2^255 - 19".into(),
        ));
    }
    if y == U256::ONE {
        return Err(MeshError::Crypto(
            "Invalid Ed25519 public key: point at infinity on Montgomery curve".into(),
        ));
    }

    let num = U256::ONE.add_mod_p(&y);
    let den = U256::ONE.sub_mod_p(&y);
    let den_inv = den.invert_mod_p();
    let u = num.mul_mod_p(&den_inv);

    Ok(u.to_bytes_le())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noise_cipherstate_encryption_decryption() {
        let key = [0x42u8; 32];
        let mut cipher = NoiseCipherState::new(&key).expect("CipherState creation failed");
        assert_eq!(cipher.nonce(), 0);

        let plaintext = b"Hello ArchDrop WAN Noise Relay!";
        let aad = b"header-aad-metadata";

        let ciphertext = cipher
            .encrypt_with_ad(aad, plaintext)
            .expect("Encryption failed");
        assert_eq!(ciphertext.len(), plaintext.len() + TAG_LEN);
        assert_eq!(cipher.nonce(), 1);

        let mut dec_cipher = NoiseCipherState::new(&key).expect("Dec CipherState creation failed");
        let decrypted = dec_cipher
            .decrypt_with_ad(aad, &ciphertext)
            .expect("Decryption failed");
        assert_eq!(decrypted, plaintext);
        assert_eq!(dec_cipher.nonce(), 1);
    }

    #[test]
    fn test_noise_cipherstate_aad_tamper_rejection() {
        let key = [0x55u8; 32];
        let mut enc = NoiseCipherState::new(&key).unwrap();
        let mut dec = NoiseCipherState::new(&key).unwrap();

        let aad = b"stream_id_42_flags_02";
        let plaintext = b"Confidential mesh payload";
        let ciphertext = enc.encrypt_with_ad(aad, plaintext).unwrap();

        // Tamper with AAD
        let bad_aad = b"stream_id_43_flags_02";
        let err = dec.decrypt_with_ad(bad_aad, &ciphertext);
        assert!(err.is_err(), "Decryption with tampered AAD must fail");
        assert_eq!(dec.nonce(), 0, "Nonce must not increment on auth failure");

        // Decrypt with correct AAD succeeds
        let recovered = dec.decrypt_with_ad(aad, &ciphertext).unwrap();
        assert_eq!(recovered, plaintext);
        assert_eq!(dec.nonce(), 1);
    }

    #[test]
    fn test_noise_cipherstate_ciphertext_tamper_rejection() {
        let key = [0x77u8; 32];
        let mut enc = NoiseCipherState::new(&key).unwrap();
        let mut dec = NoiseCipherState::new(&key).unwrap();

        let aad = b"valid_aad";
        let mut ciphertext = enc.encrypt_with_ad(aad, b"Sensitive data").unwrap();

        // Mutate single bit in ciphertext
        ciphertext[0] ^= 0x01;
        let err = dec.decrypt_with_ad(aad, &ciphertext);
        assert!(err.is_err(), "Bitflipped ciphertext must fail Poly1305 MAC");
        assert_eq!(dec.nonce(), 0);
    }

    #[test]
    fn test_noise_session_full_duplex() {
        let tx_key = [0x11u8; 32];
        let rx_key = [0x22u8; 32];

        // Session A: sends on tx_key, receives on rx_key
        let mut session_a = NoiseSession::from_keys(&tx_key, &rx_key).unwrap();
        // Session B: sends on rx_key, receives on tx_key
        let mut session_b = NoiseSession::from_keys(&rx_key, &tx_key).unwrap();

        let msg_a_to_b = b"Ping from A";
        let aad_a = b"aad_a";
        let ct_a = session_a.encrypt_with_ad(aad_a, msg_a_to_b).unwrap();
        let pt_b = session_b.decrypt_with_ad(aad_a, &ct_a).unwrap();
        assert_eq!(pt_b, msg_a_to_b);

        let msg_b_to_a = b"Pong from B";
        let aad_b = b"aad_b";
        let ct_b = session_b.encrypt_with_ad(aad_b, msg_b_to_a).unwrap();
        let pt_a = session_a.decrypt_with_ad(aad_b, &ct_b).unwrap();
        assert_eq!(pt_a, msg_b_to_a);

        assert_eq!(session_a.tx_nonce(), 1);
        assert_eq!(session_a.rx_nonce(), 1);
        assert_eq!(session_b.tx_nonce(), 1);
        assert_eq!(session_b.rx_nonce(), 1);
    }

    #[test]
    fn test_ephemeral_x25519_key_agreement() {
        let alice = EphemeralX25519::generate().unwrap();
        let bob = EphemeralX25519::generate().unwrap();

        let alice_pub = *alice.public_key();
        let bob_pub = *bob.public_key();

        let secret_alice = alice.agree(&bob_pub).unwrap();
        let secret_bob = bob.agree(&alice_pub).unwrap();

        assert_eq!(secret_alice, secret_bob);
        assert_ne!(secret_alice, [0u8; 32]);
    }

    #[test]
    fn test_birational_mapping_canonical_test_vectors() {
        // Test Vector 1 (Uniform 0x01 Seed)
        let seed1 = hex::decode("0101010101010101010101010101010101010101010101010101010101010101").unwrap();
        let mut seed1_arr = [0u8; 32];
        seed1_arr.copy_from_slice(&seed1);
        let x25519_priv1 = edwards_priv_to_x25519_priv(&seed1_arr);
        assert_eq!(
            hex::encode(x25519_priv1),
            "58e86efb75fa4e2c410f46e16de9f6acae1a1703528651b69bc176c088bef36e"
        );

        let ed_pub1 = hex::decode("8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c").unwrap();
        let mut ed_pub1_arr = [0u8; 32];
        ed_pub1_arr.copy_from_slice(&ed_pub1);
        let u1 = edwards_to_montgomery_pub(&ed_pub1_arr).unwrap();
        assert_eq!(
            hex::encode(u1),
            "1b1b58dd50ea14b60da17b790cd02754d970c9bab864ebb3c0f3016fe51d3f57"
        );

        // Test Vector 2 (Uniform 0x42 Seed)
        let seed2 = hex::decode("4242424242424242424242424242424242424242424242424242424242424242").unwrap();
        let mut seed2_arr = [0u8; 32];
        seed2_arr.copy_from_slice(&seed2);
        let x25519_priv2 = edwards_priv_to_x25519_priv(&seed2_arr);
        assert_eq!(
            hex::encode(x25519_priv2),
            "90e7595fc89e52fdfddce9c6a43d74dbf6047025ee0462d2d172e8b6a2841d6e"
        );

        let ed_pub2 = hex::decode("2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12").unwrap();
        let mut ed_pub2_arr = [0u8; 32];
        ed_pub2_arr.copy_from_slice(&ed_pub2);
        let u2 = edwards_to_montgomery_pub(&ed_pub2_arr).unwrap();
        assert_eq!(
            hex::encode(u2),
            "cc4f2cdb695dd766f34118eb67b98652fed1d8bc49c330b119bbfa8a64989378"
        );

        // Test Vector 3 (Deterministic SHA-256 Vector)
        let seed3 = hex::decode("f4427dadba76b63ef8001e0a0a16dbc31fe40d2d51286fb5252d57a2ac7256a7").unwrap();
        let mut seed3_arr = [0u8; 32];
        seed3_arr.copy_from_slice(&seed3);
        let x25519_priv3 = edwards_priv_to_x25519_priv(&seed3_arr);
        assert_eq!(
            hex::encode(x25519_priv3),
            "b882809376b8a321ddb7358fb78528b80d8170fcb608dbce227dbe497b24e342"
        );

        let ed_pub3 = hex::decode("cb72c254104afe6f4ef66f30a77f23fa8e7c935808767d89181c941501987f15").unwrap();
        let mut ed_pub3_arr = [0u8; 32];
        ed_pub3_arr.copy_from_slice(&ed_pub3);
        let u3 = edwards_to_montgomery_pub(&ed_pub3_arr).unwrap();
        assert_eq!(
            hex::encode(u3),
            "d8096271449e20b8d620fbf6aafd507b1b999fa2dad74f8af5aa1067c6486b1e"
        );
    }

    #[test]
    fn test_deterministic_derive_stream_keys() {
        let key_a = [0x01u8; 32];
        let key_b = [0x02u8; 32];
        let stream_id = 42u64;

        let mut session_init = derive_stream_keys(&key_a, &key_b, stream_id, true).unwrap();
        let mut session_resp = derive_stream_keys(&key_b, &key_a, stream_id, false).unwrap();

        let mut aad = [0u8; 10];
        aad[..8].copy_from_slice(&stream_id.to_be_bytes());
        aad[8] = 0x02; // FLAG_DATA
        aad[9] = 0x00;

        let payload = b"Test deterministic stream payload across DERP";
        let ciphertext = session_init.encrypt_with_ad(&aad, payload).unwrap();
        let decrypted = session_resp.decrypt_with_ad(&aad, &ciphertext).unwrap();

        assert_eq!(decrypted, payload);
    }

    #[test]
    fn test_noise_cipherstate_rekey() {
        let key = [0x88u8; 32];
        let mut cipher = NoiseCipherState::new(&key).unwrap();
        let orig_ct = cipher.encrypt(b"before rekey").unwrap();

        cipher.rekey().expect("Rekey must succeed");
        assert_ne!(cipher.raw_key(), &key);
        assert_eq!(cipher.nonce(), 0);

        let new_ct = cipher.encrypt(b"after rekey").unwrap();
        assert_ne!(orig_ct, new_ct);
    }
}
