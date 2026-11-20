package org.archdrop.mesh

import java.io.File
import java.math.BigInteger
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.PrivateKey
import java.security.PublicKey
import java.security.Signature
import java.security.spec.PKCS8EncodedKeySpec
import java.security.spec.X509EncodedKeySpec
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import kotlin.math.abs
import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.AEADParameters
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.math.ec.rfc7748.X25519

/**
 * Ed25519 peer public-key cryptography and mutual challenge authentication.
 *
 * Implements the wire challenge format from crates/archdrop_mesh/src/crypto.rs:
 * `CHALLENGE_TAG (16B) || server_key (32B) || timestamp_be (8B)`
 */
object MeshCrypto {
    val CHALLENGE_TAG = "ARCHDROP-DERP-V1".toByteArray(Charsets.US_ASCII)
    const val MAX_HANDSHAKE_SKEW_SECS = 60L

    // X.509 SubjectPublicKeyInfo header for Ed25519: 12 bytes
    // 30 2a (SEQUENCE of 42 bytes)
    //    30 05 06 03 2b 65 70 (AlgorithmIdentifier: 1.3.101.112 ed25519)
    //    03 21 00 (BIT STRING of 33 bytes, 0 unused bits)
    private val ED25519_X509_HEADER = byteArrayOf(
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00
    )

    fun makeChallengeMessage(serverKey: ByteArray, timestamp: Long): ByteArray {
        require(serverKey.size == 32) { "serverKey must be 32 bytes" }
        val buf = ByteBuffer.allocate(CHALLENGE_TAG.size + 32 + 8).order(ByteOrder.BIG_ENDIAN)
        buf.put(CHALLENGE_TAG)
        buf.put(serverKey)
        buf.putLong(timestamp)
        return buf.array()
    }

    fun rawToPublicKey(raw32: ByteArray): PublicKey {
        require(raw32.size == 32) { "raw32 must be 32 bytes" }
        val fullX509 = ED25519_X509_HEADER + raw32
        val kf = try {
            KeyFactory.getInstance("Ed25519", "BC")
        } catch (_: Exception) {
            KeyFactory.getInstance("Ed25519")
        }
        return kf.generatePublic(X509EncodedKeySpec(fullX509))
    }

    fun publicKeyToRaw(pubKey: PublicKey): ByteArray {
        val encoded = pubKey.encoded
        return if (encoded.size >= 32) {
            encoded.takeLast(32).toByteArray()
        } else {
            throw IllegalArgumentException("Invalid encoded public key length: ${encoded.size}")
        }
    }

    fun verify(publicKeyBytes: ByteArray, message: ByteArray, signature: ByteArray): Boolean {
        if (signature.size != 64) return false
        return try {
            val pubKey = rawToPublicKey(publicKeyBytes)
            val verifier = try {
                Signature.getInstance("Ed25519", "BC")
            } catch (_: Exception) {
                Signature.getInstance("Ed25519")
            }
            verifier.initVerify(pubKey)
            verifier.update(message)
            verifier.verify(signature)
        } catch (e: Exception) {
            false
        }
    }

    fun verifyChallenge(
        clientKey: ByteArray,
        serverKey: ByteArray,
        timestamp: Long,
        signature: ByteArray,
        maxSkewSecs: Long = MAX_HANDSHAKE_SKEW_SECS
    ): Boolean {
        val nowSecs = System.currentTimeMillis() / 1000L
        val diff = abs(nowSecs - timestamp)
        if (diff > maxSkewSecs) return false

        val msg = makeChallengeMessage(serverKey, timestamp)
        return verify(clientKey, msg, signature)
    }
}

/**
 * An Ed25519 keypair for local device mesh identity and session authentication.
 */
class MeshIdentity(
    val keyPair: KeyPair,
    val publicKeyBytes: ByteArray = MeshCrypto.publicKeyToRaw(keyPair.public)
) {
    val privateKey: PrivateKey get() = keyPair.private
    val publicKey: PublicKey get() = keyPair.public

    fun sign(message: ByteArray): ByteArray {
        val sig = try {
            Signature.getInstance("Ed25519", "BC")
        } catch (_: Exception) {
            Signature.getInstance("Ed25519")
        }
        sig.initSign(privateKey)
        sig.update(message)
        return sig.sign()
    }

    fun signChallenge(serverKey: ByteArray, timestamp: Long): ByteArray {
        val msg = MeshCrypto.makeChallengeMessage(serverKey, timestamp)
        return sign(msg)
    }

    fun toHex(): String = publicKeyBytes.joinToString("") { "%02X".format(it) }

    fun saveToFile(file: File) {
        file.parentFile?.mkdirs()
        file.writeBytes(publicKeyBytes + privateKey.encoded)
    }

    companion object {
        fun generate(): MeshIdentity {
            val kpg = try {
                KeyPairGenerator.getInstance("Ed25519", "BC")
            } catch (_: Exception) {
                KeyPairGenerator.getInstance("Ed25519")
            }
            val kp = kpg.generateKeyPair()
            return MeshIdentity(kp)
        }

        fun fromPkcs8(bytes: ByteArray): MeshIdentity {
            val kf = try {
                KeyFactory.getInstance("Ed25519", "BC")
            } catch (_: Exception) {
                KeyFactory.getInstance("Ed25519")
            }
            val privKey = kf.generatePrivate(PKCS8EncodedKeySpec(bytes))
            return MeshIdentity(KeyPair(derivePublicFromPrivate(privKey, bytes), privKey))
        }

        private fun derivePublicFromPrivate(privKey: PrivateKey, encoded: ByteArray): PublicKey {
            // If the encoded bytes have public key at the end (RFC 8410 OneAsymmetricKey format)
            if (encoded.size >= 48 && encoded[encoded.size - 34] == 0x03.toByte() && encoded[encoded.size - 33] == 0x21.toByte()) {
                val rawPub = encoded.takeLast(32).toByteArray()
                return MeshCrypto.rawToPublicKey(rawPub)
            }
            throw IllegalArgumentException(
                "Cannot derive Ed25519 public key solely from PKCS#8 private key without RFC 8410 public key bits; store public key alongside private key"
            )
        }

        fun loadOrGenerate(file: File): MeshIdentity {
            return if (file.exists() && file.length() >= 32) {
                try {
                    val bytes = file.readBytes()
                    require(bytes.size >= 32) { "Identity key file must be at least 32 bytes" }
                    val pubBytes = bytes.copyOfRange(0, 32)
                    val privBytes = bytes.copyOfRange(32, bytes.size)
                    val pubKey = MeshCrypto.rawToPublicKey(pubBytes)
                    val kf = try {
                        KeyFactory.getInstance("Ed25519", "BC")
                    } catch (_: Exception) {
                        KeyFactory.getInstance("Ed25519")
                    }
                    val privKey = kf.generatePrivate(PKCS8EncodedKeySpec(privBytes))
                    MeshIdentity(KeyPair(pubKey, privKey), pubBytes)
                } catch (e: Exception) {
                    val newId = generate()
                    newId.saveToFile(file)
                    newId
                }
            } else {
                val newId = generate()
                newId.saveToFile(file)
                newId
            }
        }
    }

    /**
     * Converts this device's Ed25519 public key to a Montgomery Curve25519 (X25519) public key.
     */
    fun toX25519PublicKey(): ByteArray = MeshCurve25519.edwardsToMontgomeryPub(publicKeyBytes)
}

// ============================================================================
// Milestone 2 Cryptographic Additions: X25519, Birational Mapping & ChaCha20-Poly1305
// ============================================================================

object MeshCurve25519 {
    val P: BigInteger = BigInteger.valueOf(2).pow(255).subtract(BigInteger.valueOf(19))

    /**
     * Converts a 32-byte Ed25519 public key point y to a 32-byte Montgomery Curve25519 (X25519) u coordinate:
     * u = (1 + y) * (1 - y)^(-1) mod (2^255 - 19)
     */
    fun edwardsToMontgomeryPub(edPubBytes: ByteArray): ByteArray {
        require(edPubBytes.size == 32) { "edPubBytes must be 32 bytes" }
        val yBytes = edPubBytes.copyOf()
        yBytes[31] = (yBytes[31].toInt() and 0x7F).toByte() // Clear sign bit

        // Convert little-endian to BigInteger
        val y = BigInteger(1, yBytes.reversedArray())
        if (y >= P || y == BigInteger.ONE) {
            throw IllegalArgumentException("Invalid Ed25519 public key coordinate")
        }

        val one = BigInteger.ONE
        val num = one.add(y).mod(P)
        val den = one.subtract(y).mod(P)
        val u = num.multiply(den.modInverse(P)).mod(P)

        val result = ByteArray(32)
        var temp = u
        for (i in 0 until 32) {
            result[i] = temp.and(BigInteger.valueOf(0xFF)).toByte()
            temp = temp.shiftRight(8)
        }
        return result
    }

    /**
     * Converts an Ed25519 32-byte private seed to a clamped X25519 private scalar:
     * s = clamp(SHA-512(seed)[0..32])
     */
    fun edwardsPrivToX25519Priv(edPrivSeed: ByteArray): ByteArray {
        require(edPrivSeed.size == 32) { "edPrivSeed must be 32 bytes" }
        val md = java.security.MessageDigest.getInstance("SHA-512")
        val h = md.digest(edPrivSeed)
        val s = h.copyOfRange(0, 32)
        s[0] = (s[0].toInt() and 248).toByte()
        s[31] = (s[31].toInt() and 127).toByte()
        s[31] = (s[31].toInt() or 64).toByte()
        return s
    }

    /**
     * Constant-time X25519 Diffie-Hellman scalar multiplication via Bouncy Castle RFC 7748.
     */
    fun calculateAgreement(privScalar: ByteArray, peerPubU: ByteArray): ByteArray {
        require(privScalar.size == 32 && peerPubU.size == 32) { "Keys must be 32 bytes" }
        val sharedSecret = ByteArray(32)
        val success = X25519.calculateAgreement(privScalar, 0, peerPubU, 0, sharedSecret, 0)
        if (!success) {
            throw IllegalStateException("X25519 key agreement failed (contributory behavior detected)")
        }
        return sharedSecret
    }
}

/**
 * Noise Protocol CipherState implementing ChaCha20-Poly1305 with auto-incrementing 64-bit nonces.
 * Formats nonces according to Noise specification: 4 zero prefix bytes + 8 little-endian counter bytes.
 */
class AeadCipherState(
    val key: ByteArray
) {
    init {
        require(key.size == 32) { "Key must be 32 bytes" }
    }

    private var nonceCounter: Long = 0L

    val currentNonce: Long get() = nonceCounter

    /**
     * Formats the 12-byte Noise nonce: 4 zero bytes followed by 8-byte little-endian counter.
     */
    fun formatNonce(n: Long): ByteArray {
        val nonce = ByteArray(12)
        val buf = ByteBuffer.wrap(nonce, 4, 8).order(ByteOrder.LITTLE_ENDIAN)
        buf.putLong(n)
        return nonce
    }

    @Synchronized
    fun encrypt(plaintext: ByteArray): ByteArray = encryptWithAd(ByteArray(0), plaintext)

    @Synchronized
    fun encryptWithAd(ad: ByteArray, plaintext: ByteArray): ByteArray {
        if (nonceCounter == -1L) {
            throw IllegalStateException("Noise cipherstate nonce exhausted (2^64-1 limit reached)")
        }
        val nonce = formatNonce(nonceCounter)
        val cipher = ChaCha20Poly1305()
        val params = AEADParameters(KeyParameter(key), 128, nonce, ad)
        cipher.init(true, params)

        val out = ByteArray(cipher.getOutputSize(plaintext.size))
        val len = cipher.processBytes(plaintext, 0, plaintext.size, out, 0)
        val finalLen = cipher.doFinal(out, len)
        val totalLen = len + finalLen

        nonceCounter++
        return out.copyOf(totalLen)
    }

    @Synchronized
    fun decrypt(ciphertext: ByteArray): ByteArray = decryptWithAd(ByteArray(0), ciphertext)

    @Synchronized
    fun decryptWithAd(ad: ByteArray, ciphertext: ByteArray): ByteArray {
        require(ciphertext.size >= 16) { "Ciphertext too short for Poly1305 tag (${ciphertext.size} < 16)" }
        if (nonceCounter == -1L) {
            throw IllegalStateException("Noise cipherstate nonce exhausted (2^64-1 limit reached)")
        }
        val nonce = formatNonce(nonceCounter)
        val cipher = ChaCha20Poly1305()
        val params = AEADParameters(KeyParameter(key), 128, nonce, ad)
        cipher.init(false, params)

        val out = ByteArray(cipher.getOutputSize(ciphertext.size))
        val len = cipher.processBytes(ciphertext, 0, ciphertext.size, out, 0)
        try {
            val finalLen = cipher.doFinal(out, len)
            val totalLen = len + finalLen
            nonceCounter++
            return out.copyOf(totalLen)
        } catch (e: Exception) {
            throw IllegalStateException("Poly1305 AEAD MAC verification failed: ${e.message}", e)
        }
    }
}

/**
 * Full-duplex Noise session managing separate TX (encryption) and RX (decryption) CipherStates.
 */
class NoiseSession(
    val tx: AeadCipherState,
    val rx: AeadCipherState
) {
    fun encrypt(plaintext: ByteArray): ByteArray = tx.encrypt(plaintext)
    fun encryptWithAd(ad: ByteArray, plaintext: ByteArray): ByteArray = tx.encryptWithAd(ad, plaintext)
    fun decrypt(ciphertext: ByteArray): ByteArray = rx.decrypt(ciphertext)
    fun decryptWithAd(ad: ByteArray, ciphertext: ByteArray): ByteArray = rx.decryptWithAd(ad, ciphertext)
    val txNonce: Long get() = tx.currentNonce
    val rxNonce: Long get() = rx.currentNonce
}

/**
 * Helper to compute HMAC-SHA256.
 */
fun computeHmacSha256(key: ByteArray, data: ByteArray): ByteArray {
    val mac = Mac.getInstance("HmacSHA256")
    mac.init(SecretKeySpec(key, "HmacSHA256"))
    return mac.doFinal(data)
}

/**
 * Derives deterministic full-duplex stream cipher keys from paired mesh identities and stream ID.
 * Matches crates/archdrop_mesh/src/crypto.rs derive_stream_keys.
 */
fun deriveStreamKeys(
    localKey: ByteArray,
    remoteKey: ByteArray,
    streamId: Long,
    isInitiator: Boolean
): NoiseSession {
    val (initiatorKey, responderKey) = if (isInitiator) {
        Pair(localKey, remoteKey)
    } else {
        Pair(remoteKey, localKey)
    }

    // Sort keys lexicographically for deterministic IKM
    val ikm = ByteArray(64)
    val comp = (0 until 32).fold(0) { acc, i ->
        if (acc != 0) acc
        else (initiatorKey[i].toInt() and 0xFF).compareTo(responderKey[i].toInt() and 0xFF)
    }
    if (comp < 0) {
        System.arraycopy(initiatorKey, 0, ikm, 0, 32)
        System.arraycopy(responderKey, 0, ikm, 32, 32)
    } else {
        System.arraycopy(responderKey, 0, ikm, 0, 32)
        System.arraycopy(initiatorKey, 0, ikm, 32, 32)
    }

    val salt = "ARCHDROP-DERP-STREAM-NOISE-V1".toByteArray(Charsets.US_ASCII)
    val prk = computeHmacSha256(salt, ikm)

    val sidBuf = ByteBuffer.allocate(8).order(ByteOrder.BIG_ENDIAN).putLong(streamId).array()

    val info1 = "stream_i2r:".toByteArray(Charsets.US_ASCII) + sidBuf + initiatorKey + byteArrayOf(0x01)
    val kInitToResp = computeHmacSha256(prk, info1)

    val info2 = "stream_r2i:".toByteArray(Charsets.US_ASCII) + sidBuf + initiatorKey + byteArrayOf(0x02)
    val kRespToInit = computeHmacSha256(prk, info2)

    val (txKey, rxKey) = if (isInitiator) {
        Pair(kInitToResp, kRespToInit)
    } else {
        Pair(kRespToInit, kInitToResp)
    }

    return NoiseSession(AeadCipherState(txKey), AeadCipherState(rxKey))
}

/**
 * Derives full-duplex session keys (txKey, rxKey) from an agreed shared secret
 * and peer public keys using HKDF-SHA256.
 */
fun deriveSessionKeys(
    sharedSecret: ByteArray,
    initiatorKey: ByteArray,
    responderKey: ByteArray,
    isInitiator: Boolean
): NoiseSession {
    val salt = "ARCHDROP-NOISE-SESSION-V1".toByteArray(Charsets.US_ASCII)
    val prk = computeHmacSha256(salt, sharedSecret)

    val info1 = "init_to_resp".toByteArray(Charsets.US_ASCII) + initiatorKey + responderKey + byteArrayOf(0x01)
    val kInitToResp = computeHmacSha256(prk, info1)

    val info2 = "resp_to_init".toByteArray(Charsets.US_ASCII) + initiatorKey + responderKey + byteArrayOf(0x02)
    val kRespToInit = computeHmacSha256(prk, info2)

    val (txKey, rxKey) = if (isInitiator) {
        Pair(kInitToResp, kRespToInit)
    } else {
        Pair(kRespToInit, kInitToResp)
    }

    return NoiseSession(AeadCipherState(txKey), AeadCipherState(rxKey))
}

/**
 * Constructs the 10-byte virtual stream AAD bound to each ChaCha20-Poly1305 AEAD chunk:
 * `[streamId (8B BE) || flags (1B) || reserved 0x00 (1B)]`
 */
fun buildStreamAad(streamId: Long, flags: Byte): ByteArray {
    val aad = ByteArray(10)
    val buf = ByteBuffer.wrap(aad).order(ByteOrder.BIG_ENDIAN)
    buf.putLong(streamId)
    buf.put(flags)
    buf.put(0x00.toByte())
    return aad
}
