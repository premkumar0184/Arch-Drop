package org.archdrop.mesh

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.IOException
import java.math.BigInteger
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.SecureRandom
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Adversarial Crypto Stress Harness for Android Companion:
 * 1. AeadCipherState & DerpVirtualStream with forged MAC tags, corrupted ciphertext bits, and mutated AAD headers.
 * 2. Immediate stream local reset (`resetLocally()`) upon auth failure.
 * 3. Stress testing X25519 agreement and birational conversion with edge case points (identity point, low order points, canonical vectors).
 * 4. Replay rejection, out-of-order chunk injection, and cross-stream chunk confusion.
 */
class AdversarialCryptoStressTest {

    private val random = SecureRandom()

    private fun hexToBytes(hex: String): ByteArray {
        val clean = hex.trim().replace(" ", "")
        val result = ByteArray(clean.length / 2)
        for (i in result.indices) {
            val index = i * 2
            result[i] = clean.substring(index, index + 2).toInt(16).toByte()
        }
        return result
    }

    private fun bytesToHex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it) }

    // ========================================================================
    // 1. AeadCipherState & DerpVirtualStream Forged MACs, Corruptions & AAD
    // ========================================================================

    @Test
    fun testAeadCipherStateForgedMacTagsExhaustive() {
        val key = ByteArray(32).also { random.nextBytes(it) }
        val enc = AeadCipherState(key)
        val dec = AeadCipherState(key)

        val aad = buildStreamAad(streamId = 12345L, flags = DerpStreamProtocol.FLAG_DATA)
        val plaintext = "Adversarial MAC forgery test payload".toByteArray(Charsets.UTF_8)
        val ciphertext = enc.encryptWithAd(aad, plaintext)

        val payloadLen = plaintext.size
        assertEquals(payloadLen + 16, ciphertext.size)

        // Test 1.1: Flip every bit position in the 16-byte Poly1305 MAC tag
        for (tagByteOffset in 0 until 16) {
            val tagIndex = payloadLen + tagByteOffset
            for (bit in 0 until 8) {
                val forged = ciphertext.copyOf()
                forged[tagIndex] = (forged[tagIndex].toInt() xor (1 shl bit)).toByte()

                try {
                    dec.decryptWithAd(aad, forged)
                    fail("Decryption must reject forged MAC tag at tag byte $tagByteOffset, bit $bit")
                } catch (e: IllegalStateException) {
                    assertTrue(
                        "Expected Poly1305 verification failure message",
                        e.message?.contains("Poly1305 AEAD MAC verification failed") == true
                    )
                }
                // Confirm nonce did not advance on failed authentication
                assertEquals(0L, dec.currentNonce)
            }
        }

        // Test 1.2: All-zeros tag
        val zeroTag = ciphertext.copyOf()
        for (i in payloadLen until zeroTag.size) {
            zeroTag[i] = 0x00.toByte()
        }
        try {
            dec.decryptWithAd(aad, zeroTag)
            fail("Decryption must reject all-zeros MAC tag")
        } catch (e: IllegalStateException) {
            assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
        }
        assertEquals(0L, dec.currentNonce)

        // Test 1.3: All-ones (0xFF) tag
        val onesTag = ciphertext.copyOf()
        for (i in payloadLen until onesTag.size) {
            onesTag[i] = 0xFF.toByte()
        }
        try {
            dec.decryptWithAd(aad, onesTag)
            fail("Decryption must reject all-ones MAC tag")
        } catch (e: IllegalStateException) {
            assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
        }
        assertEquals(0L, dec.currentNonce)

        // Test 1.4: Truncated ciphertext (< 16 bytes tag)
        for (shortLen in 0..15) {
            val truncated = ciphertext.copyOf(shortLen)
            try {
                dec.decryptWithAd(aad, truncated)
                fail("Decryption must reject ciphertext shorter than 16 bytes (len=$shortLen)")
            } catch (e: IllegalArgumentException) {
                assertTrue(
                    "Expected length error: ${e.message}",
                    e.message?.contains("Ciphertext too short for Poly1305 tag") == true
                )
            }
            assertEquals(0L, dec.currentNonce)
        }

        // Valid decrypt still works after all failed forgery attempts
        val validDecrypted = dec.decryptWithAd(aad, ciphertext)
        assertArrayEquals(plaintext, validDecrypted)
        assertEquals(1L, dec.currentNonce)
    }

    @Test
    fun testAeadCipherStateCiphertextBitFlipsAtAllBoundaries() {
        val key = ByteArray(32).also { random.nextBytes(it) }
        val enc = AeadCipherState(key)
        val dec = AeadCipherState(key)

        val aad = buildStreamAad(streamId = 999L, flags = DerpStreamProtocol.FLAG_DATA)
        val payload = ByteArray(128).also { random.nextBytes(it) }
        val ciphertext = enc.encryptWithAd(aad, payload)

        // Boundary offsets: byte 0, byte 1, middle byte, last payload byte (127)
        val testOffsets = listOf(0, 1, 64, 127)
        for (offset in testOffsets) {
            val tampered = ciphertext.copyOf()
            tampered[offset] = (tampered[offset].toInt() xor 0x01).toByte()

            try {
                dec.decryptWithAd(aad, tampered)
                fail("Decryption must fail with corrupted ciphertext bit at offset $offset")
            } catch (e: IllegalStateException) {
                assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
            }
            assertEquals(0L, dec.currentNonce)
        }

        // Decrypt untampered succeeds
        val recovered = dec.decryptWithAd(aad, ciphertext)
        assertArrayEquals(payload, recovered)
        assertEquals(1L, dec.currentNonce)
    }

    @Test
    fun testAeadCipherStateMutatedAadHeaders() {
        val key = ByteArray(32).also { random.nextBytes(it) }
        val enc = AeadCipherState(key)
        val dec = AeadCipherState(key)

        val streamId = 0x0102030405060708L
        val aad = buildStreamAad(streamId = streamId, flags = DerpStreamProtocol.FLAG_DATA)
        assertEquals(10, aad.size)

        val plaintext = "Stream header binding test".toByteArray(Charsets.UTF_8)
        val ciphertext = enc.encryptWithAd(aad, plaintext)

        // Test mutating every single byte of the 10-byte AAD header
        for (byteIdx in 0 until 10) {
            val mutatedAad = aad.copyOf()
            mutatedAad[byteIdx] = (mutatedAad[byteIdx].toInt() xor 0x80).toByte()

            try {
                dec.decryptWithAd(mutatedAad, ciphertext)
                fail("Decryption must reject mutated AAD header at byte index $byteIdx")
            } catch (e: IllegalStateException) {
                assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
            }
            assertEquals(0L, dec.currentNonce)
        }

        // Test flags variation (FLAG_SYN, FLAG_FIN, FLAG_ACK, unknown flag 0xFE)
        val flagsToTest = listOf(
            DerpStreamProtocol.FLAG_SYN,
            DerpStreamProtocol.FLAG_FIN,
            DerpStreamProtocol.FLAG_ACK,
            0xFE.toByte()
        )
        for (flag in flagsToTest) {
            val wrongAad = buildStreamAad(streamId = streamId, flags = flag)
            try {
                dec.decryptWithAd(wrongAad, ciphertext)
                fail("Decryption must fail when AAD flag is changed to $flag")
            } catch (e: IllegalStateException) {
                assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
            }
            assertEquals(0L, dec.currentNonce)
        }

        // Test different stream IDs (streamId + 1, streamId xor 1, 0L)
        val otherStreamIds = listOf(streamId + 1, streamId xor 1, 0L, Long.MAX_VALUE)
        for (otherId in otherStreamIds) {
            val wrongStreamAad = buildStreamAad(streamId = otherId, flags = DerpStreamProtocol.FLAG_DATA)
            try {
                dec.decryptWithAd(wrongStreamAad, ciphertext)
                fail("Decryption must fail when AAD stream ID is changed to $otherId")
            } catch (e: IllegalStateException) {
                assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
            }
            assertEquals(0L, dec.currentNonce)
        }

        // Valid decrypt
        assertArrayEquals(plaintext, dec.decryptWithAd(aad, ciphertext))
        assertEquals(1L, dec.currentNonce)
    }

    @Test
    fun testDerpVirtualStreamAuthFailureTriggersImmediateLocalReset() {
        val identity = MeshIdentity.generate()
        val client = DerpMeshClient(identity)
        val remotePeerKey = ByteArray(32).also { random.nextBytes(it) }
        val streamId = 555L

        val localSession = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamId, isInitiator = true)
        val remoteSession = deriveStreamKeys(remotePeerKey, identity.publicKeyBytes, streamId, isInitiator = false)

        val stream = DerpVirtualStream(streamId, remotePeerKey, client, localSession)

        val resetTriggered = AtomicBoolean(false)
        stream.onResetCallback = {
            resetTriggered.set(true)
        }

        // Deliver 1 valid chunk to ensure stream is active and operational
        val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)
        val validMsg = "Prequel chunk before attack".toByteArray(Charsets.UTF_8)
        val validCiphertext = remoteSession.encryptWithAd(aad, validMsg)

        stream.receiveChunk(validCiphertext)
        val buf = ByteArray(validMsg.size)
        val read = stream.inputStream.read(buf)
        assertEquals(validMsg.size, read)
        assertArrayEquals(validMsg, buf)
        assertFalse("Stream must not be reset after valid chunk", stream.isStreamReset)
        assertFalse("Stream must not be closed after valid chunk", stream.isStreamClosed)
        assertFalse(resetTriggered.get())

        // Adversary delivers corrupted ciphertext (flipped bit)
        val attackCiphertext = validCiphertext.copyOf()
        attackCiphertext[5] = (attackCiphertext[5].toInt() xor 0x01).toByte()

        stream.receiveChunk(attackCiphertext)

        // Verify IMMEDIATE local reset
        assertTrue("Stream must be reset locally upon MAC auth failure", stream.isStreamReset)
        assertTrue("Stream must be closed locally upon MAC auth failure", stream.isStreamClosed)
        assertTrue("onResetCallback must be invoked", resetTriggered.get())

        // Verify read on input stream immediately signals EOF (-1)
        val postResetRead = stream.inputStream.read()
        assertEquals(-1, postResetRead)

        // Verify write on output stream throws IOException
        try {
            stream.outputStream.write("Payload after reset".toByteArray(Charsets.UTF_8))
            fail("Output stream write after stream reset must throw IOException")
        } catch (e: IOException) {
            assertTrue("Expected closed message: ${e.message}", e.message?.contains("closed") == true)
        }

        // Verify subsequent chunks are dropped
        val newValidChunk = remoteSession.encryptWithAd(aad, "Chunk after reset".toByteArray(Charsets.UTF_8))
        stream.receiveChunk(newValidChunk)
        assertEquals(-1, stream.inputStream.read())
    }

    @Test
    fun testDerpVirtualStreamTruncatedChunkTriggersImmediateReset() {
        val identity = MeshIdentity.generate()
        val client = DerpMeshClient(identity)
        val remotePeerKey = ByteArray(32).also { random.nextBytes(it) }
        val streamId = 777L

        val localSession = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamId, isInitiator = true)
        val stream = DerpVirtualStream(streamId, remotePeerKey, client, localSession)

        val resetTriggered = AtomicBoolean(false)
        stream.onResetCallback = {
            resetTriggered.set(true)
        }

        // Deliver a 5-byte corrupted fragment (< 16 byte Poly1305 tag)
        val fragment = byteArrayOf(0x01, 0x02, 0x03, 0x04, 0x05)
        stream.receiveChunk(fragment)

        assertTrue("Stream must be reset when receiving truncated fragment", stream.isStreamReset)
        assertTrue("Stream must be closed", stream.isStreamClosed)
        assertTrue("onResetCallback must be invoked", resetTriggered.get())
        assertEquals(-1, stream.inputStream.read())
    }

    // ========================================================================
    // 2. X25519 Agreement & Birational Mapping Edge Case Points
    // ========================================================================

    @Test
    fun testMeshCurve25519BirationalMappingIdentityAndEdgeCoordinates() {
        // Edwards identity point: (0, 1), coordinate y = 1
        // In little-endian bytes: 01 00 ... 00
        val edIdentityBytes = ByteArray(32).also { it[0] = 0x01 }
        try {
            MeshCurve25519.edwardsToMontgomeryPub(edIdentityBytes)
            fail("Edwards identity point (y=1) maps to point at infinity and must be rejected")
        } catch (e: IllegalArgumentException) {
            assertTrue(
                "Expected invalid coordinate message: ${e.message}",
                e.message?.contains("Invalid Ed25519 public key coordinate") == true
            )
        }

        // Edwards coordinate y >= P (P = 2^255 - 19)
        // P in little-endian: ed ff ff ... ff 7f (32 bytes)
        val pBytes = hexToBytes("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
        try {
            MeshCurve25519.edwardsToMontgomeryPub(pBytes)
            fail("Coordinate y = P must be rejected")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("Invalid Ed25519 public key coordinate") == true)
        }

        // y = P + 1 (ee ff ff ... ff 7f)
        val pPlusOneBytes = hexToBytes("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
        try {
            MeshCurve25519.edwardsToMontgomeryPub(pPlusOneBytes)
            fail("Coordinate y = P + 1 must be rejected")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("Invalid Ed25519 public key coordinate") == true)
        }

        // y = 2^255 - 1 (ff ff ff ... ff 7f)
        val max255Bytes = hexToBytes("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
        try {
            MeshCurve25519.edwardsToMontgomeryPub(max255Bytes)
            fail("Coordinate y = 2^255 - 1 must be rejected")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("Invalid Ed25519 public key coordinate") == true)
        }

        // y = 0: u = (1 + 0)/(1 - 0) = 1 mod P
        val zeroBytes = ByteArray(32)
        val uForZero = MeshCurve25519.edwardsToMontgomeryPub(zeroBytes)
        val expectedU1 = ByteArray(32).also { it[0] = 0x01 }
        assertArrayEquals("y = 0 must map to u = 1", expectedU1, uForZero)

        // y = -1 = P - 1 = 2^255 - 20: u = (1 + (-1))/(1 - (-1)) = 0 mod P
        // P - 1 in little-endian: ec ff ff ... ff 7f
        val minusOneBytes = hexToBytes("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
        val uForMinusOne = MeshCurve25519.edwardsToMontgomeryPub(minusOneBytes)
        val expectedU0 = ByteArray(32) // all zeros
        assertArrayEquals("y = -1 must map to u = 0", expectedU0, uForMinusOne)

        // Sign bit clearing: y with MSB set (bit 7 of byte 31) must produce identical result as MSB cleared
        val canonicalPub = hexToBytes("8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c")
        val pubWithSignBit = canonicalPub.copyOf().also { it[31] = (it[31].toInt() or 0x80).toByte() }
        val uFromCanonical = MeshCurve25519.edwardsToMontgomeryPub(canonicalPub)
        val uFromSignBit = MeshCurve25519.edwardsToMontgomeryPub(pubWithSignBit)
        assertArrayEquals("Sign bit must be masked off transparently", uFromCanonical, uFromSignBit)

        // Invalid key byte sizes (31 bytes, 33 bytes)
        try {
            MeshCurve25519.edwardsToMontgomeryPub(ByteArray(31))
            fail("31-byte key must throw IllegalArgumentException")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("32 bytes") == true)
        }
        try {
            MeshCurve25519.edwardsToMontgomeryPub(ByteArray(33))
            fail("33-byte key must throw IllegalArgumentException")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("32 bytes") == true)
        }
    }

    @Test
    fun testX25519AgreementRejectsLowOrderAndNonContributoryPoints() {
        val privScalar = MeshCurve25519.edwardsPrivToX25519Priv(ByteArray(32).also { random.nextBytes(it) })

        // RFC 7748 Section 6.1 non-contributory points produce an all-zero scalar multiplication output.
        // Bouncy Castle and RFC 7748 implementations must reject all-zero shared secrets as non-contributory:
        val nonContributoryPoints = listOf(
            // Order 2 (u = 0)
            "0000000000000000000000000000000000000000000000000000000000000000",
            // Order 1 (u = 1)
            "0100000000000000000000000000000000000000000000000000000000000000",
            // Order 2 (u = -1 = 2^255 - 20 = P - 1)
            "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            // u = 2^255 - 19 = P (order 1 on twist)
            "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            // u = 2^255 - 18 = P + 1
            "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"
        )

        for (pointHex in nonContributoryPoints) {
            val pointBytes = hexToBytes(pointHex)
            try {
                MeshCurve25519.calculateAgreement(privScalar, pointBytes)
                fail("X25519 agreement must reject non-contributory all-zero point: $pointHex")
            } catch (e: IllegalStateException) {
                assertTrue(
                    "Expected non-contributory rejection error: ${e.message}",
                    e.message?.contains("contributory behavior detected") == true
                )
            }
        }

        // Other small subgroup / twist / non-canonical points from RFC 7748:
        val otherTwistPoints = listOf(
            "5f9c92bc1a105cd31a62e4595e64ba26eb709fd297a8cb9d7828cd81fd791e7d",
            "e0eb7a7c3b07be01e524a30a19534aa51db7d92496135f0acdeb1c9db19b0d00",
            "daae6be328c10f6a6c7eec718b3042452377ac9074d093654a37938a8300cc5b",
            "ee26e14769c11f4ba70f61f282602ec89302d6993d0483251e622609ac705600",
            "d90385e48da43d54832fa1258b6e1b8e7da0f464eff209c252237122e22e1d74",
            // u = 2^255 - 1 (non-canonical encoding of 18 mod P)
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"
        )
        for (twistHex in otherTwistPoints) {
            val twistBytes = hexToBytes(twistHex)
            val secret = MeshCurve25519.calculateAgreement(privScalar, twistBytes)
            assertEquals(32, secret.size)
            // Ensure result is not all zeros
            assertFalse("Shared secret must not be all zeros", secret.all { it == 0.toByte() })
        }
    }

    @Test
    fun testEdwardsPrivToX25519ClampingInvariants() {
        // RFC 7748 clamping:
        // scalar[0] &= 248 (lowest 3 bits cleared)
        // scalar[31] &= 127 (highest bit cleared)
        // scalar[31] |= 64 (second highest bit set)
        for (i in 0 until 50) {
            val seed = ByteArray(32).also { random.nextBytes(it) }
            val scalar = MeshCurve25519.edwardsPrivToX25519Priv(seed)

            assertEquals(32, scalar.size)
            assertEquals("Lowest 3 bits must be 0", 0, scalar[0].toInt() and 0x07)
            assertEquals("Highest bit (bit 255) must be 0", 0, scalar[31].toInt() and 0x80)
            assertEquals("Bit 254 must be 1", 0x40, scalar[31].toInt() and 0x40)
        }
    }

    // ========================================================================
    // 3. Replay Rejection, Nonce Desync & Out-of-Order Injection
    // ========================================================================

    @Test
    fun testVirtualStreamReplayRejection() {
        val identity = MeshIdentity.generate()
        val client = DerpMeshClient(identity)
        val remotePeerKey = ByteArray(32).also { random.nextBytes(it) }
        val streamId = 404L

        val localSession = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamId, isInitiator = true)
        val remoteSession = deriveStreamKeys(remotePeerKey, identity.publicKeyBytes, streamId, isInitiator = false)

        val stream = DerpVirtualStream(streamId, remotePeerKey, client, localSession)
        val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)

        // Remote peer sends chunk 0 (nonce 0)
        val chunk0Msg = "Legitimate chunk 0".toByteArray(Charsets.UTF_8)
        val chunk0Ciphertext = remoteSession.encryptWithAd(aad, chunk0Msg)

        stream.receiveChunk(chunk0Ciphertext)
        val buf = ByteArray(chunk0Msg.size)
        val read = stream.inputStream.read(buf)
        assertEquals(chunk0Msg.size, read)
        assertArrayEquals(chunk0Msg, buf)
        assertFalse(stream.isStreamReset)

        // Adversary captures chunk 0 and replays it!
        // Local session RX nonce is now 1. Decrypting chunk 0 (nonce 0) will fail Poly1305 MAC.
        stream.receiveChunk(chunk0Ciphertext)

        assertTrue("Stream must be reset locally upon replayed chunk", stream.isStreamReset)
        assertTrue("Stream must be closed upon replayed chunk", stream.isStreamClosed)
        assertEquals(-1, stream.inputStream.read())
    }

    @Test
    fun testVirtualStreamOutOfOrderChunkInjection() {
        val identity = MeshIdentity.generate()
        val client = DerpMeshClient(identity)
        val remotePeerKey = ByteArray(32).also { random.nextBytes(it) }
        val streamId = 505L

        val localSession = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamId, isInitiator = true)
        val remoteSession = deriveStreamKeys(remotePeerKey, identity.publicKeyBytes, streamId, isInitiator = false)

        val stream = DerpVirtualStream(streamId, remotePeerKey, client, localSession)
        val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)

        // Remote peer encrypts chunk 0, chunk 1, chunk 2
        val chunk0 = remoteSession.encryptWithAd(aad, "Chunk 0".toByteArray(Charsets.UTF_8))
        val chunk1 = remoteSession.encryptWithAd(aad, "Chunk 1".toByteArray(Charsets.UTF_8))
        val chunk2 = remoteSession.encryptWithAd(aad, "Chunk 2".toByteArray(Charsets.UTF_8))

        // Deliver chunk 0 successfully
        stream.receiveChunk(chunk0)
        val buf = ByteArray(7)
        assertEquals(7, stream.inputStream.read(buf))
        assertArrayEquals("Chunk 0".toByteArray(Charsets.UTF_8), buf)

        // Adversary injects chunk 2 before chunk 1 (out of order)
        // Local session expects nonce 1, but chunk 2 has nonce 2.
        stream.receiveChunk(chunk2)

        assertTrue("Stream must be reset locally upon out-of-order chunk injection", stream.isStreamReset)
        assertTrue("Stream must be closed", stream.isStreamClosed)
        assertEquals(-1, stream.inputStream.read())
    }

    @Test
    fun testVirtualStreamCrossStreamChunkConfusion() {
        val identity = MeshIdentity.generate()
        val client = DerpMeshClient(identity)
        val remotePeerKey = ByteArray(32).also { random.nextBytes(it) }

        val streamIdA = 1001L
        val streamIdB = 1002L

        val sessionA = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamIdA, isInitiator = true)
        val remoteSessionA = deriveStreamKeys(remotePeerKey, identity.publicKeyBytes, streamIdA, isInitiator = false)

        val sessionB = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamIdB, isInitiator = true)

        val streamA = DerpVirtualStream(streamIdA, remotePeerKey, client, sessionA)
        val streamB = DerpVirtualStream(streamIdB, remotePeerKey, client, sessionB)

        // Remote peer encrypts chunk for stream A (bound to streamIdA AAD)
        val aadA = buildStreamAad(streamIdA, DerpStreamProtocol.FLAG_DATA)
        val chunkForA = remoteSessionA.encryptWithAd(aadA, "Stream A confidential packet".toByteArray(Charsets.UTF_8))

        // Adversary tries to cross-inject this packet into Stream B
        streamB.receiveChunk(chunkForA)

        assertTrue("Stream B must immediately reset upon receiving chunk from another stream", streamB.isStreamReset)
        assertTrue("Stream B must be closed", streamB.isStreamClosed)
        assertEquals(-1, streamB.inputStream.read())

        // Stream A remains pristine and healthy
        assertFalse(streamA.isStreamReset)
        streamA.receiveChunk(chunkForA)
        val readA = ByteArray(28)
        assertEquals(28, streamA.inputStream.read(readA))
        assertEquals("Stream A confidential packet", String(readA, Charsets.UTF_8))
    }

    @Test
    fun testVirtualStreamLoopbackReflectionRejection() {
        val identity = MeshIdentity.generate()
        val client = DerpMeshClient(identity)
        val remotePeerKey = ByteArray(32).also { random.nextBytes(it) }
        val streamId = 2020L

        val localSession = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamId, isInitiator = true)
        val stream = DerpVirtualStream(streamId, remotePeerKey, client, localSession)

        // Local peer encrypts an outbound message with txKey
        val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)
        val outboundCiphertext = localSession.encryptWithAd(aad, "Alice outbound packet".toByteArray(Charsets.UTF_8))

        // Adversary reflects Alice's outbound packet right back into Alice's inbound stream
        // Because Alice decrypts with rxKey (different from txKey), MAC verification must fail!
        stream.receiveChunk(outboundCiphertext)

        assertTrue("Reflected outbound packet must fail MAC check and reset stream", stream.isStreamReset)
        assertTrue("Stream must be closed", stream.isStreamClosed)
        assertEquals(-1, stream.inputStream.read())
    }

    // ========================================================================
    // 4. Nonce Counter Limit / Exhaustion Protection
    // ========================================================================

    @Test
    fun testAeadCipherStateNonceLimitEnforcement() {
        val key = ByteArray(32).also { random.nextBytes(it) }
        val cipher = AeadCipherState(key)

        // Reflective manipulation to set nonce to -1L (Long.MAX_VALUE + 1 unsigned / 2^64 - 1 limit)
        val field = AeadCipherState::class.java.getDeclaredField("nonceCounter")
        field.isAccessible = true
        field.setLong(cipher, -1L)

        try {
            cipher.encrypt("Exhausted nonce".toByteArray(Charsets.UTF_8))
            fail("Encrypt with exhausted nonce (-1L) must throw IllegalStateException")
        } catch (e: IllegalStateException) {
            assertTrue(
                "Expected nonce exhausted error: ${e.message}",
                e.message?.contains("Noise cipherstate nonce exhausted") == true
            )
        }

        try {
            cipher.decrypt(ByteArray(32))
            fail("Decrypt with exhausted nonce (-1L) must throw IllegalStateException")
        } catch (e: IllegalStateException) {
            assertTrue(
                "Expected nonce exhausted error: ${e.message}",
                e.message?.contains("Noise cipherstate nonce exhausted") == true
            )
        }
    }
}
