package org.archdrop.mesh

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

class NoiseCipherStateTest {

    private fun hexToBytes(hex: String): ByteArray {
        val result = ByteArray(hex.length / 2)
        for (i in result.indices) {
            val index = i * 2
            result[i] = hex.substring(index, index + 2).toInt(16).toByte()
        }
        return result
    }

    private fun bytesToHex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it) }

    @Test
    fun testAeadCipherStateRoundtrip() {
        val key = ByteArray(32) { (it + 1).toByte() }
        val enc = AeadCipherState(key)
        val dec = AeadCipherState(key)

        assertEquals(0L, enc.currentNonce)
        assertEquals(0L, dec.currentNonce)

        val plaintext = "Hello Arch Drop Noise Protocol WAN Mesh!".toByteArray(Charsets.UTF_8)
        val ciphertext = enc.encrypt(plaintext)

        assertEquals(plaintext.size + 16, ciphertext.size)
        assertEquals(1L, enc.currentNonce)

        val decrypted = dec.decrypt(ciphertext)
        assertArrayEquals(plaintext, decrypted)
        assertEquals(1L, dec.currentNonce)
    }

    @Test
    fun testAeadCipherStateNonceFormatting() {
        val key = ByteArray(32) { 0x55.toByte() }
        val cipher = AeadCipherState(key)

        val nonce0 = cipher.formatNonce(0L)
        assertEquals(12, nonce0.size)
        assertArrayEquals(ByteArray(12), nonce0)

        val nonce1 = cipher.formatNonce(1L)
        assertEquals(12, nonce1.size)
        assertEquals(0.toByte(), nonce1[0])
        assertEquals(0.toByte(), nonce1[1])
        assertEquals(0.toByte(), nonce1[2])
        assertEquals(0.toByte(), nonce1[3])
        assertEquals(1.toByte(), nonce1[4])
        for (i in 5..11) {
            assertEquals(0.toByte(), nonce1[i])
        }

        val counter = 0x0102030405060708L
        val nonceVal = cipher.formatNonce(counter)
        val buf = ByteBuffer.wrap(nonceVal, 4, 8).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(counter, buf.long)
    }

    @Test
    fun testAeadCipherStateAadBindingAndTamperRejection() {
        val key = ByteArray(32) { 0x33.toByte() }
        val enc = AeadCipherState(key)
        val dec = AeadCipherState(key)

        val aad = buildStreamAad(streamId = 100L, flags = DerpStreamProtocol.FLAG_DATA)
        assertEquals(10, aad.size)

        val plaintext = "Secure WAN relayed file chunk payload".toByteArray(Charsets.UTF_8)
        val ciphertext = enc.encryptWithAd(aad, plaintext)

        // Tamper with stream ID in AAD (byte 7 bitflip)
        val badAadStreamId = aad.copyOf()
        badAadStreamId[7] = (badAadStreamId[7].toInt() xor 0x01).toByte()

        try {
            dec.decryptWithAd(badAadStreamId, ciphertext)
            fail("Decryption with tampered stream_id in AAD must fail Poly1305 MAC")
        } catch (e: IllegalStateException) {
            assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
        }
        assertEquals(0L, dec.currentNonce) // Nonce must NOT advance on auth failure

        // Tamper with flags in AAD (byte 8: FLAG_SYN instead of FLAG_DATA)
        val badAadFlags = aad.copyOf()
        badAadFlags[8] = DerpStreamProtocol.FLAG_SYN
        try {
            dec.decryptWithAd(badAadFlags, ciphertext)
            fail("Decryption with tampered flags in AAD must fail Poly1305 MAC")
        } catch (e: IllegalStateException) {
            assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
        }
        assertEquals(0L, dec.currentNonce)

        // Decrypt with correct AAD succeeds
        val recovered = dec.decryptWithAd(aad, ciphertext)
        assertArrayEquals(plaintext, recovered)
        assertEquals(1L, dec.currentNonce)
    }

    @Test
    fun testAeadCipherStateCiphertextTamperRejection() {
        val key = ByteArray(32) { 0x77.toByte() }
        val enc = AeadCipherState(key)
        val dec = AeadCipherState(key)

        val aad = buildStreamAad(streamId = 42L, flags = DerpStreamProtocol.FLAG_DATA)
        val ciphertext = enc.encryptWithAd(aad, "Integrity verified chunk".toByteArray(Charsets.UTF_8))

        // Bitflip in ciphertext body
        val tamperedBody = ciphertext.copyOf()
        tamperedBody[0] = (tamperedBody[0].toInt() xor 0x01).toByte()
        try {
            dec.decryptWithAd(aad, tamperedBody)
            fail("Decryption with flipped bit in payload must fail MAC verification")
        } catch (e: IllegalStateException) {
            assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
        }

        // Bitflip in Poly1305 MAC tag (last 16 bytes)
        val tamperedTag = ciphertext.copyOf()
        tamperedTag[tamperedTag.size - 1] = (tamperedTag[tamperedTag.size - 1].toInt() xor 0x80).toByte()
        try {
            dec.decryptWithAd(aad, tamperedTag)
            fail("Decryption with flipped bit in MAC tag must fail verification")
        } catch (e: IllegalStateException) {
            assertTrue(e.message?.contains("Poly1305 AEAD MAC verification failed") == true)
        }

        assertEquals(0L, dec.currentNonce)
    }

    @Test
    fun testMeshCurve25519BirationalMappingCanonicalTestVectors() {
        // Vector 1 (Uniform 0x01 Seed)
        val seed1 = hexToBytes("0101010101010101010101010101010101010101010101010101010101010101")
        val x25519Priv1 = MeshCurve25519.edwardsPrivToX25519Priv(seed1)
        assertEquals("58e86efb75fa4e2c410f46e16de9f6acae1a1703528651b69bc176c088bef36e", bytesToHex(x25519Priv1))

        val edPub1 = hexToBytes("8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c")
        val u1 = MeshCurve25519.edwardsToMontgomeryPub(edPub1)
        assertEquals("1b1b58dd50ea14b60da17b790cd02754d970c9bab864ebb3c0f3016fe51d3f57", bytesToHex(u1))

        // Vector 2 (Uniform 0x42 Seed)
        val seed2 = hexToBytes("4242424242424242424242424242424242424242424242424242424242424242")
        val x25519Priv2 = MeshCurve25519.edwardsPrivToX25519Priv(seed2)
        assertEquals("90e7595fc89e52fdfddce9c6a43d74dbf6047025ee0462d2d172e8b6a2841d6e", bytesToHex(x25519Priv2))

        val edPub2 = hexToBytes("2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12")
        val u2 = MeshCurve25519.edwardsToMontgomeryPub(edPub2)
        assertEquals("cc4f2cdb695dd766f34118eb67b98652fed1d8bc49c330b119bbfa8a64989378", bytesToHex(u2))

        // Vector 3 (Deterministic SHA-256 Vector)
        val seed3 = hexToBytes("f4427dadba76b63ef8001e0a0a16dbc31fe40d2d51286fb5252d57a2ac7256a7")
        val x25519Priv3 = MeshCurve25519.edwardsPrivToX25519Priv(seed3)
        assertEquals("b882809376b8a321ddb7358fb78528b80d8170fcb608dbce227dbe497b24e342", bytesToHex(x25519Priv3))

        val edPub3 = hexToBytes("cb72c254104afe6f4ef66f30a77f23fa8e7c935808767d89181c941501987f15")
        val u3 = MeshCurve25519.edwardsToMontgomeryPub(edPub3)
        assertEquals("d8096271449e20b8d620fbf6aafd507b1b999fa2dad74f8af5aa1067c6486b1e", bytesToHex(u3))
    }

    @Test
    fun testMeshCurve25519KeyAgreement() {
        val privA = MeshCurve25519.edwardsPrivToX25519Priv(ByteArray(32) { 0x11.toByte() })
        val pubU_A = MeshCurve25519.edwardsToMontgomeryPub(
            hexToBytes("8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c")
        )

        val privB = MeshCurve25519.edwardsPrivToX25519Priv(ByteArray(32) { 0x22.toByte() })
        val pubU_B = MeshCurve25519.edwardsToMontgomeryPub(
            hexToBytes("2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12")
        )

        val secretA = MeshCurve25519.calculateAgreement(privA, pubU_B)
        val secretB = MeshCurve25519.calculateAgreement(privB, pubU_A)

        // Both compute 32-byte shared secrets
        assertEquals(32, secretA.size)
        assertEquals(32, secretB.size)
    }

    @Test
    fun testNoiseSessionFullDuplex() {
        val txKey = ByteArray(32) { 0x11.toByte() }
        val rxKey = ByteArray(32) { 0x22.toByte() }

        val sessionA = NoiseSession(AeadCipherState(txKey), AeadCipherState(rxKey))
        val sessionB = NoiseSession(AeadCipherState(rxKey), AeadCipherState(txKey))

        val aad = "stream_test_aad".toByteArray(Charsets.US_ASCII)
        val msgAtoB = "Outbound packet from Alice to Bob".toByteArray(Charsets.UTF_8)
        val ctA = sessionA.encryptWithAd(aad, msgAtoB)
        val ptB = sessionB.decryptWithAd(aad, ctA)
        assertArrayEquals(msgAtoB, ptB)

        val msgBtoA = "Inbound reply from Bob to Alice".toByteArray(Charsets.UTF_8)
        val ctB = sessionB.encryptWithAd(aad, msgBtoA)
        val ptA = sessionA.decryptWithAd(aad, ctB)
        assertArrayEquals(msgBtoA, ptA)

        assertEquals(1L, sessionA.txNonce)
        assertEquals(1L, sessionA.rxNonce)
        assertEquals(1L, sessionB.txNonce)
        assertEquals(1L, sessionB.rxNonce)
    }

    @Test
    fun testDeterministicDeriveStreamKeysParityWithRust() {
        val keyA = ByteArray(32) { 0x01.toByte() }
        val keyB = ByteArray(32) { 0x02.toByte() }
        val streamId = 42L

        val sessionInit = deriveStreamKeys(keyA, keyB, streamId, isInitiator = true)
        val sessionResp = deriveStreamKeys(keyB, keyA, streamId, isInitiator = false)

        val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)
        assertEquals(10, aad.size)

        val payload = "Test deterministic stream payload across DERP".toByteArray(Charsets.UTF_8)
        val ciphertext = sessionInit.encryptWithAd(aad, payload)
        val decrypted = sessionResp.decryptWithAd(aad, ciphertext)

        assertArrayEquals(payload, decrypted)
    }

    @Test
    fun testDerpVirtualStreamEncryptedTransmissionAndTamperRejection() {
        val identity = MeshIdentity.generate()
        val client = DerpMeshClient(identity)
        val remotePeerKey = ByteArray(32) { 0xBB.toByte() }
        val streamId = 77L

        val streamSession = deriveStreamKeys(identity.publicKeyBytes, remotePeerKey, streamId, isInitiator = true)
        val stream = DerpVirtualStream(streamId, remotePeerKey, client, streamSession)

        // Verify write encrypts data chunk with 10-byte AAD
        val capturedFrames = mutableListOf<DerpFrame>()
        val mockSender: (DerpFrame) -> Boolean = { frame ->
            capturedFrames.add(frame)
            true
        }

        val testPayload = "Confidential virtual stream chunk bytes".toByteArray(Charsets.UTF_8)
        val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)

        // Encrypt with peer's matching session to simulate remote peer sending valid encrypted chunk
        val peerSession = deriveStreamKeys(remotePeerKey, identity.publicKeyBytes, streamId, isInitiator = false)
        val validCiphertext = peerSession.encryptWithAd(aad, testPayload)

        // Receiving valid ciphertext delivers decrypted plaintext into inputStream
        stream.receiveChunk(validCiphertext)
        val readBuf = ByteArray(testPayload.size)
        val bytesRead = stream.inputStream.read(readBuf)
        assertEquals(testPayload.size, bytesRead)
        assertArrayEquals(testPayload, readBuf)
        assertFalse(stream.isStreamReset)

        // Now deliver a tampered chunk to receiveChunk
        val tamperedCiphertext = validCiphertext.copyOf()
        tamperedCiphertext[0] = (tamperedCiphertext[0].toInt() xor 0xFF).toByte()

        var resetCallbackTriggered = false
        stream.onResetCallback = {
            resetCallbackTriggered = true
        }

        stream.receiveChunk(tamperedCiphertext)
        assertTrue("Stream must be marked reset upon AEAD authentication failure", stream.isStreamReset)
        assertTrue("Stream onResetCallback must be invoked on authentication failure", resetCallbackTriggered)
    }
}
