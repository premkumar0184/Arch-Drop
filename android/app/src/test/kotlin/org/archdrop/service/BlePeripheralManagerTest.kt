package org.archdrop.service

import android.bluetooth.BluetoothAdapter
import android.content.Context
import androidx.test.core.app.ApplicationProvider
import org.archdrop.clipboard.ClipboardSyncManager
import org.archdrop.db.PeerEntity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.nio.ByteBuffer
import java.nio.ByteOrder

@RunWith(RobolectricTestRunner::class)
class BlePeripheralManagerTest {

    private val pairedKey = "test_paired_secret_key_12345678".toByteArray(Charsets.UTF_8)
    private val peerMeshKeyHex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    private val peerFingerprintHex = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"

    private fun buildPacket(
        subtype: Byte = BlePeripheralManager.CONTINUITY_TYPE_HOTSPOT_WAKEUP,
        length: Byte = BlePeripheralManager.WAKEUP_PAYLOAD_LEN,
        controlFlags: Byte = 0x01,
        targetBand: Byte = 0x02,
        authToken: ByteArray,
        port: Int = 53317
    ): ByteArray {
        val buf = ByteBuffer.allocate(10).order(ByteOrder.BIG_ENDIAN)
        buf.put(subtype)
        buf.put(length)
        buf.put(controlFlags)
        buf.put(targetBand)
        buf.put(authToken)
        buf.putShort(port.toShort())
        return buf.array()
    }

    @Test
    fun testVerifyWakeupPayload_ValidTokenExactMinute_ReturnsTrue() {
        val epochMin = 29910000L
        val token = BlePeripheralManager.computeHmacToken(pairedKey, epochMin)
        val packet = buildPacket(authToken = token)

        val verified = BlePeripheralManager.verifyWakeupPayload(
            payload = packet,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertTrue("Valid token for exact epoch minute must verify", verified)
    }

    @Test
    fun testVerifyWakeupPayload_ClockSkewToleranceWithinTwoMinutes_ReturnsTrue() {
        val currentMin = 29910000L
        for (offset in listOf(-2L, -1L, 1L, 2L)) {
            val token = BlePeripheralManager.computeHmacToken(pairedKey, currentMin + offset)
            val packet = buildPacket(authToken = token)
            val verified = BlePeripheralManager.verifyWakeupPayload(
                payload = packet,
                pairedKeys = listOf(pairedKey),
                currentEpochMinute = currentMin
            )
            assertTrue("Token generated at offset $offset min must verify under +/- 2m skew", verified)
        }
    }

    @Test
    fun testVerifyWakeupPayload_ClockSkewExceedingTwoMinutes_ReturnsFalse() {
        val currentMin = 29910000L
        for (offset in listOf(-3L, 3L, -10L, 10L)) {
            val token = BlePeripheralManager.computeHmacToken(pairedKey, currentMin + offset)
            val packet = buildPacket(authToken = token)
            val verified = BlePeripheralManager.verifyWakeupPayload(
                payload = packet,
                pairedKeys = listOf(pairedKey),
                currentEpochMinute = currentMin
            )
            assertFalse("Token generated at offset $offset min must fail", verified)
        }
    }

    @Test
    fun testVerifyWakeupPayload_InvalidKey_ReturnsFalse() {
        val epochMin = 29910000L
        val wrongKey = "wrong_attacker_secret_key_876543".toByteArray(Charsets.UTF_8)
        val token = BlePeripheralManager.computeHmacToken(wrongKey, epochMin)
        val packet = buildPacket(authToken = token)

        val verified = BlePeripheralManager.verifyWakeupPayload(
            payload = packet,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertFalse("Token generated with wrong key must fail verification", verified)
    }

    @Test
    fun testVerifyWakeupPayload_MultiplePairedKeys_FindsMatchingKey() {
        val epochMin = 29910000L
        val key1 = "key_one_111111111111111111111111".toByteArray(Charsets.UTF_8)
        val key2 = "key_two_222222222222222222222222".toByteArray(Charsets.UTF_8)
        val key3 = "key_three_3333333333333333333333".toByteArray(Charsets.UTF_8)

        val token2 = BlePeripheralManager.computeHmacToken(key2, epochMin)
        val packet = buildPacket(authToken = token2)

        val verified = BlePeripheralManager.verifyWakeupPayload(
            payload = packet,
            pairedKeys = listOf(key1, key2, key3),
            currentEpochMinute = epochMin
        )
        assertTrue("Must verify successfully when second paired key matches", verified)
    }

    @Test
    fun testVerifyWakeupPayload_CorruptedTokenBit_ReturnsFalse() {
        val epochMin = 29910000L
        val token = BlePeripheralManager.computeHmacToken(pairedKey, epochMin)
        token[0] = (token[0].toInt() xor 0x01).toByte() // Flip 1 bit
        val packet = buildPacket(authToken = token)

        val verified = BlePeripheralManager.verifyWakeupPayload(
            payload = packet,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertFalse("Tampered token must fail verification", verified)
    }

    @Test
    fun testVerifyWakeupPayload_TruncatedPayload_ReturnsFalse() {
        val epochMin = 29910000L
        val token = BlePeripheralManager.computeHmacToken(pairedKey, epochMin)
        val fullPacket = buildPacket(authToken = token)

        for (len in 0..9) {
            val truncated = fullPacket.copyOfRange(0, len)
            val verified = BlePeripheralManager.verifyWakeupPayload(
                payload = truncated,
                pairedKeys = listOf(pairedKey),
                currentEpochMinute = epochMin
            )
            assertFalse("Truncated packet length $len must return false", verified)
        }
    }

    @Test
    fun testVerifyWakeupPayload_RawManufacturerDataFormat12Bytes_ReturnsTrue() {
        val epochMin = 29910000L
        val token = BlePeripheralManager.computeHmacToken(pairedKey, epochMin)
        val subPacket = buildPacket(authToken = token)

        val raw12Bytes = ByteBuffer.allocate(12)
            .order(ByteOrder.LITTLE_ENDIAN)
            .putShort(0x004C.toShort()) // Company ID 0x004C
            .put(subPacket)
            .array()

        val verified = BlePeripheralManager.verifyWakeupPayload(
            payload = raw12Bytes,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertTrue("Raw 12-byte Apple Manufacturer Data frame must parse and verify", verified)
    }

    @Test
    fun testExtractKeysFromPeer_DerivesHexAndUtf8Bytes() {
        val peer = PeerEntity(
            meshKey = peerMeshKeyHex,
            alias = "Arch Desktop",
            fingerprint = peerFingerprintHex,
            port = 53317,
            isTrusted = true
        )
        val keys = BlePeripheralManager.extractKeysFromPeer(peer)
        assertTrue("Must extract at least derived keys and binary bytes", keys.size >= 4)
        // Check derivePairedKey output is 32 bytes
        val derived = BlePeripheralManager.derivePairedKey(peerMeshKeyHex)
        assertEquals(32, derived.size)
    }

    @Test
    fun testParseWakeupPayload_InvalidHeaders_ReturnsNull() {
        val badType = byteArrayOf(0x0E, 0x08, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00)
        assertNull(BlePeripheralManager.parseWakeupPayload(badType))

        val badLen = byteArrayOf(0x0D, 0x05, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00)
        assertNull(BlePeripheralManager.parseWakeupPayload(badLen))

        assertNull(BlePeripheralManager.parseWakeupPayload(null))
        assertNull(BlePeripheralManager.parseWakeupPayload(byteArrayOf(0x01, 0x02)))
    }

    @Test
    fun testConstantTimeEq4_VerifiesExactMatch() {
        val a = byteArrayOf(0x11, 0x22, 0x33, 0x44)
        val b = byteArrayOf(0x11, 0x22, 0x33, 0x44)
        val c = byteArrayOf(0x11, 0x22, 0x33, 0x45)

        assertTrue(BlePeripheralManager.constantTimeEq4(a, b))
        assertFalse(BlePeripheralManager.constantTimeEq4(a, c))
        assertFalse(BlePeripheralManager.constantTimeEq4(a, byteArrayOf(0x11)))
    }

    // ============================================================================
    // Zero-Wi-Fi BLE GATT Clipboard Synchronization Tests
    // ============================================================================

    @Test
    fun testGattServiceAndCharacteristicUuids() {
        assertEquals("a4c80001-7d1a-4c91-9e28-3e2840000001", BlePeripheralManager.SERVICE_UUID.toString())
        assertEquals("a4c80002-7d1a-4c91-9e28-3e2840000001", BlePeripheralManager.CHAR_WAKEUP_COMMAND.toString())
        assertEquals("a4c80003-7d1a-4c91-9e28-3e2840000001", BlePeripheralManager.CHAR_LOHS_CREDENTIALS.toString())
        assertEquals("a4c80004-7d1a-4c91-9e28-3e2840000001", BlePeripheralManager.CHAR_DEVICE_TELEMETRY.toString())
        assertEquals("a4c80005-7d1a-4c91-9e28-3e2840000001", BlePeripheralManager.CHAR_CLIPBOARD_SYNC.toString())
    }

    @Test
    fun testEncodeAndParseClipboardPacket_Roundtrip_Succeeds() {
        val epochMin = 29910000L
        val text = "Zero-Wi-Fi clipboard text payload test"
        val bytes = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = text,
            epochMinute = epochMin
        )

        val parsed = BlePeripheralManager.parseClipboardPacket(bytes)
        assertNotNull("Must parse valid clipboard packet", parsed)
        assertEquals(BlePeripheralManager.MIME_TYPE_TEXT_PLAIN, parsed!!.mimeType)
        assertEquals(text.toByteArray(Charsets.UTF_8).size, parsed.payloadLength)
        assertEquals(text, parsed.text)

        // Verify HMAC auth
        val verified = BlePeripheralManager.verifyClipboardPacket(
            packet = bytes,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertTrue("Packet must verify with correct paired key and epoch minute", verified)
    }

    @Test
    fun testVerifyClipboardPacket_ClockSkewToleranceWithinTwoMinutes_ReturnsTrue() {
        val currentMin = 29910000L
        val text = "Clock skew test payload"

        for (offset in listOf(-2L, -1L, 1L, 2L)) {
            val bytes = BlePeripheralManager.encodeClipboardPacket(
                key = pairedKey,
                mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
                text = text,
                epochMinute = currentMin + offset
            )

            val verified = BlePeripheralManager.verifyClipboardPacket(
                packet = bytes,
                pairedKeys = listOf(pairedKey),
                currentEpochMinute = currentMin
            )
            assertTrue("Token at offset $offset must verify under +/- 2m skew", verified)
        }
    }

    @Test
    fun testVerifyClipboardPacket_ClockSkewExceedingTwoMinutes_ReturnsFalse() {
        val currentMin = 29910000L
        val text = "Old expired clipboard payload"

        for (offset in listOf(-5L, -3L, 3L, 10L)) {
            val bytes = BlePeripheralManager.encodeClipboardPacket(
                key = pairedKey,
                mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
                text = text,
                epochMinute = currentMin + offset
            )

            val verified = BlePeripheralManager.verifyClipboardPacket(
                packet = bytes,
                pairedKeys = listOf(pairedKey),
                currentEpochMinute = currentMin
            )
            assertFalse("Token at offset $offset must fail verification", verified)
        }
    }

    @Test
    fun testVerifyClipboardPacket_WrongKey_ReturnsFalse() {
        val epochMin = 29910000L
        val wrongKey = "wrong_attacker_secret_87654321".toByteArray(Charsets.UTF_8)
        val text = "Forged payload"

        val bytes = BlePeripheralManager.encodeClipboardPacket(
            key = wrongKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = text,
            epochMinute = epochMin
        )

        val verified = BlePeripheralManager.verifyClipboardPacket(
            packet = bytes,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertFalse("Forged token with wrong key must fail verification", verified)
    }

    @Test
    fun testVerifyClipboardPacket_TamperedAuthToken_ReturnsFalse() {
        val epochMin = 29910000L
        val text = "Tamper payload"
        val bytes = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = text,
            epochMinute = epochMin
        )

        bytes[0] = (bytes[0].toInt() xor 0x01).toByte()

        val verified = BlePeripheralManager.verifyClipboardPacket(
            packet = bytes,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertFalse("Tampered auth token must fail verification", verified)
    }

    @Test
    fun testParseClipboardPacket_TruncatedData_ReturnsNull() {
        val text = "Truncation test"
        val full = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = text
        )

        for (len in 0 until 7) {
            val truncated = full.copyOfRange(0, len)
            assertNull("Truncated header length $len must return null", BlePeripheralManager.parseClipboardPacket(truncated))
        }

        val truncatedPayload = full.copyOfRange(0, full.size - 1)
        assertNull("Truncated payload must return null", BlePeripheralManager.parseClipboardPacket(truncatedPayload))
    }

    @Test
    fun testClipboardDeduplicationHash() {
        val text = "Repeated clipboard paste content"
        val hash = org.archdrop.clipboard.ClipboardSyncManager.sha256(text)
        val hash2 = org.archdrop.clipboard.ClipboardSyncManager.sha256(text)
        assertEquals(hash, hash2)

        val different = org.archdrop.clipboard.ClipboardSyncManager.sha256("Different text")
        assertFalse(hash.equals(different, ignoreCase = true))
    }

    @Test
    fun testCrossPlatformDerivePairedKeyMatchesRustHmacSha256() {
        val testHex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        val derived = BlePeripheralManager.derivePairedKey(testHex)
        val expectedHex = "b63564f6c066f11750f1958cb23f6513a772ef29c22da9bb0197ae204fa98379"
        val derivedHex = derived.joinToString("") { "%02x".format(it) }
        assertEquals(expectedHex, derivedHex)
    }

    @Test
    fun testFragmentedPreparedWriteAssemblyAndVerification() {
        val epochMin = 29910000L
        val text = "A".repeat(400) // 400 bytes payload, exceeds default 23B / 256B MTU
        val fullPacket = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = text,
            epochMinute = epochMin
        )
        assertEquals(7 + 400, fullPacket.size)

        // Split into 3 chunks simulating GATT PrepareWrite with 150-byte chunks
        val chunk1 = fullPacket.copyOfRange(0, 150)
        val chunk2 = fullPacket.copyOfRange(150, 300)
        val chunk3 = fullPacket.copyOfRange(300, fullPacket.size)

        // Chunks individually must NOT parse as complete clipboard packets
        assertNull("Chunk 1 must not parse as full packet", BlePeripheralManager.parseClipboardPacket(chunk1))
        assertNull("Chunk 2 must not parse as full packet", BlePeripheralManager.parseClipboardPacket(chunk2))

        // Assemble chunks in memory as onExecuteWrite does
        val stream = java.io.ByteArrayOutputStream()
        stream.write(chunk1)
        stream.write(chunk2)
        stream.write(chunk3)
        val assembled = stream.toByteArray()

        assertEquals(fullPacket.size, assembled.size)
        val parsed = BlePeripheralManager.parseClipboardPacket(assembled)
        assertNotNull("Assembled payload must parse successfully", parsed)
        assertEquals(text, parsed!!.text)

        val verified = BlePeripheralManager.verifyClipboardPacket(
            packet = assembled,
            pairedKeys = listOf(pairedKey),
            currentEpochMinute = epochMin
        )
        assertTrue("Assembled payload must pass HMAC authentication", verified)
    }

    @Test
    fun testMaximum512BytesPayloadBoundary() {
        val epochMin = 29910000L
        val maxText = "X".repeat(512)
        val packet = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = maxText,
            epochMinute = epochMin
        )
        assertEquals(7 + 512, packet.size)
        val parsed = BlePeripheralManager.parseClipboardPacket(packet)
        assertNotNull(parsed)
        assertEquals(512, parsed!!.payloadLength)
        assertEquals(maxText, parsed.text)
        assertTrue(BlePeripheralManager.verifyClipboardPacket(packet, listOf(pairedKey), epochMin))

        // Over 512 bytes is truncated to 512 by encodeClipboardPacket
        val overText = "Y".repeat(600)
        val clampedPacket = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = overText,
            epochMinute = epochMin
        )
        assertEquals(7 + 512, clampedPacket.size)
        val clampedParsed = BlePeripheralManager.parseClipboardPacket(clampedPacket)
        assertNotNull(clampedParsed)
        assertEquals(512, clampedParsed!!.payloadLength)
    }

    @Test
    fun testCccdUuidStandardFormat() {
        assertEquals("00002902-0000-1000-8000-00805f9b34fb", BlePeripheralManager.CCCD_UUID.toString())
    }

    @Test
    fun testFallbackSecretParityWithRust() {
        assertEquals("ARCHDROP_SECRET", String(BlePeripheralManager.FALLBACK_SECRET, Charsets.UTF_8))
    }

    @Test
    fun testHandleClipboardPayload_ValidPacket_PastesAndTriggersCallback() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val lohs = LOHSManager(context)
        val manager = BlePeripheralManager(context, lohs)
        val cm = ClipboardSyncManager(context, "test_device_id")
        manager.clipboardSyncManager = cm
        manager.addPairedKey(pairedKey)

        var receivedText: String? = null
        var receivedOrigin: String? = null
        manager.onClipboardReceived = { text, origin ->
            receivedText = text
            receivedOrigin = origin
        }

        val text = "Direct GATT write to Android companion clipboard"
        val packet = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = text
        )

        val device = BluetoothAdapter.getDefaultAdapter().getRemoteDevice("11:22:33:44:55:66")
        val success = manager.handleClipboardPayload(device, packet)
        assertTrue("handleClipboardPayload must succeed for authenticated packet", success)
        assertEquals(text, receivedText)
        assertEquals("11:22:33:44:55:66", receivedOrigin)
        assertEquals(ClipboardSyncManager.sha256(text), cm.getLastPastedHash())
    }

    @Test
    fun testHandleClipboardPayload_EchoSuppression_DoesNotRePaste() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val lohs = LOHSManager(context)
        val manager = BlePeripheralManager(context, lohs)
        val cm = ClipboardSyncManager(context, "test_device_id")
        manager.clipboardSyncManager = cm
        manager.addPairedKey(pairedKey)

        val text = "Echo suppression test payload"
        val hash = ClipboardSyncManager.sha256(text)
        cm.setLastPastedHash(hash)

        var callbackInvoked = false
        manager.onClipboardReceived = { _, _ -> callbackInvoked = true }

        val packet = BlePeripheralManager.encodeClipboardPacket(
            key = pairedKey,
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = text
        )

        val device = BluetoothAdapter.getDefaultAdapter().getRemoteDevice("11:22:33:44:55:66")
        val success = manager.handleClipboardPayload(device, packet)
        assertTrue("Echoed write still returns GATT success acknowledge", success)
        assertFalse("Echo loop must suppress onClipboardReceived callback invocation", callbackInvoked)
    }

    @Test
    fun testHandleClipboardPayload_InvalidAuthToken_Rejected() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val lohs = LOHSManager(context)
        val manager = BlePeripheralManager(context, lohs)
        val cm = ClipboardSyncManager(context, "test_device_id")
        manager.clipboardSyncManager = cm
        manager.addPairedKey(pairedKey)

        var callbackInvoked = false
        manager.onClipboardReceived = { _, _ -> callbackInvoked = true }

        val forgedPacket = BlePeripheralManager.encodeClipboardPacket(
            key = "unpaired_attacker_key_88888888".toByteArray(Charsets.UTF_8),
            mimeType = BlePeripheralManager.MIME_TYPE_TEXT_PLAIN,
            text = "Forged payload"
        )

        val device = BluetoothAdapter.getDefaultAdapter().getRemoteDevice("11:22:33:44:55:66")
        val success = manager.handleClipboardPayload(device, forgedPacket)
        assertFalse("Forged HMAC token write must be rejected", success)
        assertFalse(callbackInvoked)
    }

    @Test
    fun testWriteOrNotifyClipboard_SelectsGenuinePairedKeyAndEncodesUri() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val lohs = LOHSManager(context)
        val manager = BlePeripheralManager(context, lohs)
        manager.addPairedKey(pairedKey)

        val success = manager.writeOrNotifyClipboard("https://archlinux.org", BlePeripheralManager.MIME_TYPE_URI_LIST)
        assertTrue(success)

        val overLimit = manager.writeOrNotifyClipboard("M".repeat(513))
        assertFalse("writeOrNotifyClipboard must reject payloads > 512 bytes", overLimit)
    }
}
