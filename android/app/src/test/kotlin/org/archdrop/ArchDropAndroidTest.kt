package org.archdrop

import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.archdrop.service.BlePeripheralManager
import org.archdrop.service.LOHSManager
import org.archdrop.transfer.TransferSessionManager
import org.archdrop.transfer.models.FileDto
import org.archdrop.transfer.models.RegisterDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.MessageDigest
import java.util.UUID

class ArchDropAndroidTest {

    @get:Rule
    val tempFolder = TemporaryFolder()

    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    @Test
    fun testGattUuidsMatchDesktopSpecification() {
        assertEquals(
            "a4c80001-7d1a-4c91-9e28-3e2840000001",
            BlePeripheralManager.SERVICE_UUID.toString()
        )
        assertEquals(
            "a4c80002-7d1a-4c91-9e28-3e2840000001",
            BlePeripheralManager.CHAR_WAKEUP_COMMAND.toString()
        )
        assertEquals(
            "a4c80003-7d1a-4c91-9e28-3e2840000001",
            BlePeripheralManager.CHAR_LOHS_CREDENTIALS.toString()
        )
        assertEquals(
            "a4c80004-7d1a-4c91-9e28-3e2840000001",
            BlePeripheralManager.CHAR_DEVICE_TELEMETRY.toString()
        )
    }

    @Test
    fun testBleCredentialsPayloadSerialization() {
        val payload = BlePeripheralManager.BleCredentialsPayload(
            ssid = "ArchDrop_LOHS_9912",
            passphrase = "SecretPassphrase123!",
            gateway_ip = "192.168.43.1",
            gatewayIp = "192.168.43.1",
            port = 53317,
            httpPort = 53317,
            securityType = LOHSManager.SECURITY_TYPE_WPA2_PSK
        )

        val jsonStr = json.encodeToString(payload)
        assertTrue(jsonStr.contains("\"ssid\":\"ArchDrop_LOHS_9912\""))
        assertTrue(jsonStr.contains("\"passphrase\":\"SecretPassphrase123!\""))
        assertTrue(jsonStr.contains("\"gateway_ip\":\"192.168.43.1\""))
        assertTrue(jsonStr.contains("\"gatewayIp\":\"192.168.43.1\""))
        assertTrue(jsonStr.contains("\"port\":53317"))
        assertTrue(jsonStr.contains("\"httpPort\":53317"))
        assertTrue(jsonStr.contains("\"securityType\":1"))

        val decoded = json.decodeFromString<BlePeripheralManager.BleCredentialsPayload>(jsonStr)
        assertEquals("ArchDrop_LOHS_9912", decoded.ssid)
        assertEquals("SecretPassphrase123!", decoded.passphrase)
        assertEquals("192.168.43.1", decoded.gateway_ip)
        assertEquals(53317, decoded.port)
        assertEquals(1, decoded.securityType)
    }

    @Test
    fun testLohsCredentialsDataClass() {
        val creds = LOHSManager.LohsCredentials(
            ssid = "TestSSID",
            passphrase = "password",
            securityType = LOHSManager.SECURITY_TYPE_WPA3_SAE,
            gatewayIp = "192.168.49.1"
        )

        assertEquals("TestSSID", creds.ssid)
        assertEquals("password", creds.passphrase)
        assertEquals(LOHSManager.SECURITY_TYPE_WPA3_SAE, creds.securityType)
        assertEquals("192.168.49.1", creds.gatewayIp)
        assertEquals(53317, creds.httpPort)
    }

    @Test
    fun testTransferSessionManagerLifecycle() {
        val downloadDir = tempFolder.newFolder("downloads")
        val sessionManager = TransferSessionManager(downloadDir)

        val registerDto = RegisterDto(
            alias = "Arch Laptop",
            version = "2.2",
            deviceModel = "Linux",
            deviceType = "desktop",
            fingerprint = "DESKTOP_FP",
            port = 53317,
            protocol = "http"
        )

        val testContent = "Hello from Arch Drop P2P test!".toByteArray(Charsets.UTF_8)
        val testSha256 = MessageDigest.getInstance("SHA-256")
            .digest(testContent)
            .joinToString("") { "%02x".format(it) }

        val fileDto = FileDto(
            id = "file-1",
            fileName = "test_note.txt",
            size = testContent.size.toLong(),
            fileType = "text/plain",
            sha256 = testSha256
        )

        val prepareResponse = sessionManager.createSession(
            senderInfo = registerDto,
            files = mapOf("file-1" to fileDto)
        )

        assertNotNull(prepareResponse.sessionId)
        val token = prepareResponse.files["file-1"]
        assertNotNull(token)

        val state = sessionManager.getFileState(prepareResponse.sessionId, "file-1", token!!)
        assertNotNull(state)
        assertEquals("test_note.txt", state!!.finalFile.name)

        // Write content to temp file
        state.tempFile.writeBytes(testContent)

        // Complete file with matching checksum
        val success = sessionManager.completeFile(
            sessionId = prepareResponse.sessionId,
            fileId = "file-1",
            receivedBytes = testContent.size.toLong(),
            calculatedSha256 = testSha256
        )
        assertTrue(success)
        assertTrue(state.finalFile.exists())
        assertEquals(testContent.size.toLong(), state.finalFile.length())
        assertFalse(state.tempFile.exists())
    }
}
