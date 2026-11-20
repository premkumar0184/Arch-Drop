package org.archdrop.ui

import android.app.Application
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.archdrop.clipboard.ClipboardSyncManager
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.archdrop.db.PeerEntity
import org.archdrop.transfer.models.PrepareUploadRequestDto
import org.archdrop.transfer.models.PrepareUploadResponseDto
import org.archdrop.ui.dashboard.DashboardViewModel
import org.archdrop.ui.dashboard.SendClipboardResult
import org.archdrop.ui.pairing.PairingPayload
import org.archdrop.ui.pairing.formatHexBlocks
import org.archdrop.ui.pairing.generateQrBitmap
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.BufferedReader
import java.io.File
import java.io.FileOutputStream
import java.io.InputStreamReader
import java.net.ServerSocket
import java.util.UUID

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class UiUnitTest {

    private lateinit var db: ArchDropDatabase

    private val testMeshKey = "3C02B81F89A24C9E7D8A1B2C3D4E5F6A7B8C9D0E1F2A3B4C5D6E7F8A9B0C1D2E"
    private val testFingerprint = "9F82A461C02847B9E83A1259D4F0B73819A26C38E0124872659B017D382A9C12"

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        db = Room.inMemoryDatabaseBuilder(context, ArchDropDatabase::class.java)
            .allowMainThreadQueries()
            .build()
    }

    @After
    fun tearDown() {
        db.close()
    }

    @Test
    fun testPairingPayloadSerializationAndUriParsing() {
        val payload = PairingPayload(
            version = 1,
            alias = "Arch Linux Desktop",
            meshKey = testMeshKey,
            fingerprint = testFingerprint,
            port = 53317
        )

        val uriString = payload.toUri()
        assertTrue("URI must start with archdrop://pair", uriString.startsWith("archdrop://pair"))
        assertTrue("URI must contain alias", uriString.contains("Arch+Linux+Desktop") || uriString.contains("Arch%20Linux%20Desktop"))
        assertTrue("URI must contain meshKey", uriString.contains(testMeshKey))
        assertTrue("URI must contain fp", uriString.contains(testFingerprint))

        val parsed = PairingPayload.parse(uriString)
        assertNotNull("Parsed payload must not be null", parsed)
        assertEquals(1, parsed!!.version)
        assertEquals("Arch Linux Desktop", parsed.alias)
        assertEquals(testMeshKey, parsed.meshKey)
        assertEquals(testFingerprint, parsed.fingerprint)
        assertEquals(53317, parsed.port)
    }

    @Test
    fun testPairingPayloadJsonParsing() {
        val jsonString = """
            {
                "version": 1,
                "alias": "Pixel 8 Pro",
                "meshKey": "$testMeshKey",
                "fingerprint": "$testFingerprint",
                "port": 53317,
                "timestamp": 1731086400
            }
        """.trimIndent()

        val parsed = PairingPayload.parse(jsonString)
        assertNotNull("JSON payload parsing must succeed", parsed)
        assertEquals(1, parsed!!.version)
        assertEquals("Pixel 8 Pro", parsed.alias)
        assertEquals(testMeshKey, parsed.meshKey)
        assertEquals(testFingerprint, parsed.fingerprint)
        assertEquals(53317, parsed.port)
    }

    @Test
    fun testPairingPayloadInvalidRejection() {
        // Too short mesh key
        val invalidUri = "archdrop://pair?v=1&alias=test&meshKey=ABCD&fp=1234&port=53317"
        assertNull("Short mesh key must be rejected", PairingPayload.parse(invalidUri))

        // Missing mesh key
        val missingKey = "archdrop://pair?v=1&alias=test&port=53317"
        assertNull("Missing mesh key must be rejected", PairingPayload.parse(missingKey))

        // Empty string
        assertNull("Empty string must return null", PairingPayload.parse(""))

        // Non-pairing random string
        assertNull("Random text must return null", PairingPayload.parse("https://example.com"))
    }

    @Test
    fun testGenerateQrBitmap() {
        val content = "archdrop://pair?v=1&alias=ArchDrop&meshKey=$testMeshKey&fp=$testFingerprint&port=53317"
        val bitmap = generateQrBitmap(content, 256, 0xFF000000.toInt(), 0xFFFFFFFF.toInt())
        assertNotNull("QR bitmap must be generated", bitmap)
        assertEquals(256, bitmap!!.width)
        assertEquals(256, bitmap.height)

        val emptyBitmap = generateQrBitmap("", 256, 0xFF000000.toInt(), 0xFFFFFFFF.toInt())
        assertNull("Empty content must yield null bitmap", emptyBitmap)
    }

    @Test
    fun testFormatHexBlocks() {
        val formatted = formatHexBlocks(testMeshKey)
        val lines = formatted.split("\n")
        assertEquals(2, lines.size)
        assertEquals(4, lines[0].split(" ").size)
        assertEquals(4, lines[1].split(" ").size)
        assertTrue(lines[0].startsWith("3C02B81F"))
    }

    @Test
    fun testFormatBytes() {
        assertEquals("0 B", DashboardViewModel.formatBytes(0L))
        assertEquals("512.0 B", DashboardViewModel.formatBytes(512L))
        assertEquals("1.0 KiB", DashboardViewModel.formatBytes(1024L))
        assertEquals("4.0 MiB", DashboardViewModel.formatBytes(4 * 1024 * 1024L))
        assertEquals("1.5 GiB", DashboardViewModel.formatBytes((1.5 * 1024 * 1024 * 1024).toLong()))
    }

    @Test
    fun testEventToTransferUiModelParsing() {
        val event = EventEntity(
            originDeviceId = "ORIGIN_DESKTOP",
            sequenceNumber = 42L,
            eventId = UUID.randomUUID().toString(),
            timestampMs = System.currentTimeMillis() - 120_000L,
            eventType = "transfer.completed",
            payload = """{"file":"archlinux-2026.iso","size":1048576000,"sha256":"abcdef1234567890"}""",
            payloadHash = "mockhash123",
            ackStatus = 0
        )

        val model = DashboardViewModel.eventToTransferUiModel(event)
        assertEquals("archlinux-2026.iso", model.fileName)
        assertEquals("1000.0 MiB", model.fileSizeFormatted)
        assertEquals("abcdef1234567890", model.sha256)
        assertTrue(model.isIncoming)
        assertTrue(model.isSuccess)
    }

    @Test
    fun testRoomPeerAndTransferFlows() = runBlocking {
        val peerDao = db.peerDao()
        val eventDao = db.eventDao()

        // 1. Insert Peer and observe
        val peer = PeerEntity(
            meshKey = testMeshKey,
            alias = "Arch Desktop",
            fingerprint = testFingerprint,
            port = 53317,
            pairedAtMs = 1000L,
            lastSeenMs = 2000L,
            isTrusted = true
        )
        peerDao.insertPeer(peer)

        val peers = peerDao.observeAllPeers().first()
        assertEquals(1, peers.size)
        assertEquals("Arch Desktop", peers[0].alias)

        // 2. Insert Events (transfers and non-transfers) and observe
        val tEvent1 = EventEntity(
            originDeviceId = testMeshKey,
            sequenceNumber = 1L,
            eventId = "evt-1",
            timestampMs = 1000L,
            eventType = "transfer.completed",
            payload = """{"file":"photo.jpg","size":2048}""",
            payloadHash = "hash1",
            ackStatus = 0
        )
        val tEvent2 = EventEntity(
            originDeviceId = testMeshKey,
            sequenceNumber = 2L,
            eventId = "evt-2",
            timestampMs = 2000L,
            eventType = "transfer.incoming",
            payload = """{"file":"video.mp4","size":10485760}""",
            payloadHash = "hash2",
            ackStatus = 0
        )
        val clipEvent = EventEntity(
            originDeviceId = testMeshKey,
            sequenceNumber = 3L,
            eventId = "evt-3",
            timestampMs = 3000L,
            eventType = "clipboard.updated",
            payload = "https://archlinux.org",
            payloadHash = "hash3",
            ackStatus = 0
        )

        eventDao.insertEvents(listOf(tEvent1, tEvent2, clipEvent))

        val transferEvents = eventDao.observeTransferEvents(10).first()
        // Should return only the 2 transfer events, ordered by timestampMs DESC
        assertEquals(2, transferEvents.size)
        assertEquals("evt-2", transferEvents[0].eventId)
        assertEquals("evt-1", transferEvents[1].eventId)

        // Count peers and events
        assertEquals(1, peerDao.countPeers())
        assertEquals(3, eventDao.countEvents())

        // Revoke peer
        peerDao.deletePeer(testMeshKey)
        val remainingPeers = peerDao.observeAllPeers().first()
        assertEquals(0, remainingPeers.size)
    }

    @Test
    fun testDashboardSendClipboardEmptyReturnsEmpty() = runBlocking {
        val app = ApplicationProvider.getApplicationContext<Application>()
        val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        cm.setPrimaryClip(ClipData.newPlainText("", ""))

        val viewModel = DashboardViewModel(app, db)
        val result = viewModel.sendClipboard(app)

        assertEquals(SendClipboardResult.Empty, result)
        assertEquals(false, viewModel.uiState.value.isSendingClipboard)
    }

    @Test
    fun testDashboardSendClipboardTextSuccess() = runBlocking {
        val app = ApplicationProvider.getApplicationContext<Application>()
        val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        cm.setPrimaryClip(ClipData.newPlainText("note", "Dashboard test copied text"))

        val server = ServerSocket(0)
        val port = server.localPort

        val serverThread = Thread {
            try {
                val sock = server.accept()
                val reader = BufferedReader(InputStreamReader(sock.getInputStream()))
                var line: String?
                var contentLen = 0
                while (reader.readLine().also { line = it } != null) {
                    if (line!!.isEmpty()) break
                    if (line!!.startsWith("Content-Length:", ignoreCase = true)) {
                        contentLen = line!!.split(":")[1].trim().toIntOrNull() ?: 0
                    }
                }
                val bodyChars = CharArray(contentLen)
                var r = 0
                while (r < contentLen) {
                    val read = reader.read(bodyChars, r, contentLen - r)
                    if (read == -1) break
                    r += read
                }
                val out = sock.getOutputStream()
                out.write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                sock.close()
            } catch (_: Exception) {}
        }
        serverThread.start()

        val viewModel = DashboardViewModel(app, db)
        val result = viewModel.sendClipboard(app, targetHost = "127.0.0.1", targetPort = port)

        assertTrue(result is SendClipboardResult.Success)
        val success = result as SendClipboardResult.Success
        assertTrue(success.summary.contains("Text"))
        assertEquals(false, viewModel.uiState.value.isSendingClipboard)

        server.close()
    }

    @Test
    fun testDashboardSendClipboardFileDelegatesToTransferAndInsertsEvent() = runBlocking {
        val app = ApplicationProvider.getApplicationContext<Application>()
        val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager

        val largeFile = File(app.cacheDir, "large_transfer_test.bin")
        val size = 4500 * 1024
        val bytes = ByteArray(size) { 7 }
        FileOutputStream(largeFile).use { it.write(bytes) }

        cm.setPrimaryClip(ClipData.newUri(app.contentResolver, "bin", Uri.fromFile(largeFile)))

        val server = ServerSocket(0)
        val port = server.localPort
        val json = Json { encodeDefaults = true; ignoreUnknownKeys = true }

        val serverThread = Thread {
            try {
                // Request 1: prepare-upload
                val sock1 = server.accept()
                val reader1 = BufferedReader(InputStreamReader(sock1.getInputStream()))
                var line1: String?
                var contentLen1 = 0
                while (reader1.readLine().also { line1 = it } != null) {
                    if (line1!!.isEmpty()) break
                    if (line1!!.startsWith("Content-Length:", ignoreCase = true)) {
                        contentLen1 = line1!!.split(":")[1].trim().toIntOrNull() ?: 0
                    }
                }
                val bodyChars1 = CharArray(contentLen1)
                var r1 = 0
                while (r1 < contentLen1) {
                    val read = reader1.read(bodyChars1, r1, contentLen1 - r1)
                    if (read == -1) break
                    r1 += read
                }
                val prepReqJson = String(bodyChars1, 0, r1)
                val prepDto = json.decodeFromString<PrepareUploadRequestDto>(prepReqJson)
                val fileId = prepDto.files.keys.first()

                val prepResp = PrepareUploadResponseDto(
                    sessionId = "sess-large-001",
                    files = mapOf(fileId to "tok-large-001")
                )
                val prepRespJson = json.encodeToString(prepResp)
                val out1 = sock1.getOutputStream()
                out1.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${prepRespJson.length}\r\n\r\n$prepRespJson".toByteArray())
                out1.flush()
                sock1.close()

                // Request 2: upload
                val sock2 = server.accept()
                val in2 = sock2.getInputStream()
                var contentLen2 = 0
                while (true) {
                    val l = readLineSimple(in2)
                    if (l.isEmpty()) break
                    if (l.startsWith("Content-Length:", ignoreCase = true)) {
                        contentLen2 = l.split(":")[1].trim().toIntOrNull() ?: 0
                    }
                }
                var r2 = 0
                val buf = ByteArray(8192)
                while (r2 < contentLen2) {
                    val toRead = if (contentLen2 - r2 > buf.size) buf.size else (contentLen2 - r2)
                    val read = in2.read(buf, 0, toRead)
                    if (read == -1) break
                    r2 += read
                }
                val out2 = sock2.getOutputStream()
                out2.write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out2.flush()
                sock2.close()
            } catch (_: Exception) {}
        }
        serverThread.start()

        val viewModel = DashboardViewModel(app, db)
        val result = viewModel.sendClipboard(app, targetHost = "127.0.0.1", targetPort = port)

        assertTrue(result is SendClipboardResult.Success)
        val success = result as SendClipboardResult.Success
        assertTrue(success.summary.contains("File large_transfer_test.bin"))
        assertEquals(false, viewModel.uiState.value.isSendingClipboard)

        // Event should be recorded in database
        val events = db.eventDao().observeTransferEvents(10).first()
        assertTrue(events.isNotEmpty())
        assertEquals("large_transfer_test.bin", org.json.JSONObject(events[0].payload).optString("file"))

        largeFile.delete()
        server.close()
    }

    @Test
    fun testDashboardSendClipboardUnreachableTargetReturnsError() = runBlocking {
        val app = ApplicationProvider.getApplicationContext<Application>()
        val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        cm.setPrimaryClip(ClipData.newPlainText("note", "Unreachable test"))

        val closedPort = ServerSocket(0).use { it.localPort }

        val viewModel = DashboardViewModel(app, db)
        val result = viewModel.sendClipboard(app, targetHost = "127.0.0.1", targetPort = closedPort)

        assertTrue(result is SendClipboardResult.Error)
        val err = result as SendClipboardResult.Error
        assertTrue(err.message.contains("Could not reach desktop") || err.message.contains("Failed to connect") || err.message.contains("Connection refused"))
        assertEquals(false, viewModel.uiState.value.isSendingClipboard)
    }

    private fun readLineSimple(inputStream: java.io.InputStream): String {
        val sb = StringBuilder()
        var prev = 0
        while (true) {
            val b = inputStream.read()
            if (b == -1) break
            if (prev == '\r'.code && b == '\n'.code) {
                sb.setLength(sb.length - 1)
                return sb.toString()
            }
            sb.append(b.toChar())
            prev = b
        }
        return sb.toString()
    }
}
