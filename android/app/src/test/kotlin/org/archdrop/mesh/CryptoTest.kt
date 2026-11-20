package org.archdrop.mesh

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.X509EncodedKeySpec

class CryptoTest {
    @Test
    fun testJavaEd25519Availability() {
        val kpg = KeyPairGenerator.getInstance("Ed25519")
        val kp = kpg.generateKeyPair()
        assertNotNull(kp.public)
        assertNotNull(kp.private)

        val pubBytes = kp.public.encoded
        // Check X.509 header for Ed25519: 44 bytes total, last 32 bytes are raw pubkey
        assertEquals(44, pubBytes.size)
        val rawPub = pubBytes.takeLast(32).toByteArray()
        assertEquals(32, rawPub.size)

        // Can we reconstruct the PublicKey from rawPub?
        val x509Header = byteArrayOf(
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00
        )
        val fullX509 = x509Header + rawPub
        val kf = KeyFactory.getInstance("Ed25519")
        val reconstructedPub = kf.generatePublic(X509EncodedKeySpec(fullX509))
        assertEquals(kp.public, reconstructedPub)

        val sig = Signature.getInstance("Ed25519")
        sig.initSign(kp.private)
        val msg = "Hello Arch Drop".toByteArray(Charsets.UTF_8)
        sig.update(msg)
        val signatureBytes = sig.sign()
        assertEquals(64, signatureBytes.size)

        val verifier = Signature.getInstance("Ed25519")
        verifier.initVerify(reconstructedPub)
        verifier.update(msg)
        assertTrue(verifier.verify(signatureBytes))
    }

    @Test
    fun testMeshIdentityPersistenceAcrossRestarts() {
        val tempFile = java.io.File.createTempFile("mesh_test_identity", ".key")
        try {
            // 1. Generate identity and verify it has valid public and private keys
            val original = MeshIdentity.generate()
            assertNotNull(original.publicKey)
            assertNotNull(original.privateKey)
            assertEquals(32, original.publicKeyBytes.size)

            // 2. Save identity to disk
            original.saveToFile(tempFile)
            assertTrue("Key file must exist on disk", tempFile.exists())
            assertTrue("Key file must have public key + private key bytes", tempFile.length() > 32)

            // 3. Load identity from disk (simulating service restart)
            val loaded = MeshIdentity.loadOrGenerate(tempFile)

            // Assert exact byte parity for public key and hex identity
            org.junit.Assert.assertArrayEquals("Public key bytes must match exactly", original.publicKeyBytes, loaded.publicKeyBytes)
            assertEquals("Hex device ID must be identical across restarts", original.toHex(), loaded.toHex())
            org.junit.Assert.assertArrayEquals("Encoded X.509 public key must match", original.publicKey.encoded, loaded.publicKey.encoded)
            org.junit.Assert.assertArrayEquals("Encoded PKCS#8 private key must match", original.privateKey.encoded, loaded.privateKey.encoded)

            // 4. Verify cryptographic signatures: message signed by loaded identity must verify against original public key
            val testMsg = "Arch Drop Mutual Verification Test Payload".toByteArray(Charsets.UTF_8)
            val sigFromLoaded = loaded.sign(testMsg)
            assertEquals(64, sigFromLoaded.size)
            assertTrue("Signature from reloaded identity must verify with original public key", MeshCrypto.verify(original.publicKeyBytes, testMsg, sigFromLoaded))

            // 5. Verify challenge signature
            val serverKey = ByteArray(32) { 0x42.toByte() }
            val timestamp = System.currentTimeMillis() / 1000L
            val challengeSig = loaded.signChallenge(serverKey, timestamp)
            assertTrue(
                "Challenge signature from reloaded identity must verify against original public key",
                MeshCrypto.verifyChallenge(
                    clientKey = original.publicKeyBytes,
                    serverKey = serverKey,
                    timestamp = timestamp,
                    signature = challengeSig
                )
            )

            // 6. Test second reload to ensure idempotency across multiple restarts
            val reloadedAgain = MeshIdentity.loadOrGenerate(tempFile)
            assertEquals("Second restart must yield identical device ID", original.toHex(), reloadedAgain.toHex())
            org.junit.Assert.assertArrayEquals("Second restart private key parity", original.privateKey.encoded, reloadedAgain.privateKey.encoded)
        } finally {
            tempFile.delete()
        }
    }
}
