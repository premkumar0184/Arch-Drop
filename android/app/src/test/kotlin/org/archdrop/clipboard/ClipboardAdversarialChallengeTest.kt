package org.archdrop.clipboard

import android.content.Context
import android.net.Uri
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.archdrop.clipboard.models.ClipboardDescriptorPacket
import org.archdrop.clipboard.models.ClipboardDescriptorPayload
import org.archdrop.clipboard.models.ClipboardInlinePacket
import org.archdrop.clipboard.models.ClipboardInlinePayload
import org.archdrop.transfer.HttpReceiver
import org.archdrop.transfer.TransferClient
import org.archdrop.transfer.models.PrepareUploadRequestDto
import org.archdrop.transfer.models.PrepareUploadResponseDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.BufferedReader
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.HttpURLConnection
import java.net.ServerSocket
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class ClipboardAdversarialChallengeTest {

    @get:Rule
    val tempFolder = TemporaryFolder()

    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    private fun sha256(bytes: ByteArray): String {
        val md = MessageDigest.getInstance("SHA-256")
        return md.digest(bytes).joinToString("") { "%02x".format(it) }
    }

    // =========================================================================
    // 1. Threshold Boundary Stress Tests (4096 bytes vs 4097 bytes)
    // =========================================================================

    @Test
    fun testPackTextExactBoundary4096vs4097() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "ADVERSARIAL_TEST_DEVICE")

        // 4096 ASCII bytes: exactly at boundary -> MUST be ClipboardInlinePacket
        val text4096 = "a".repeat(4096)
        assertEquals(4096, text4096.toByteArray(Charsets.UTF_8).size)
        val packed4096 = manager.packText(text4096)
        assertTrue("4096 bytes text must be ClipboardInlinePacket", packed4096 is ClipboardInlinePacket)
        val inline4096 = packed4096 as ClipboardInlinePacket
        assertEquals(4096, inline4096.payload.sizeBytes)
        assertEquals(text4096, inline4096.payload.content)
        assertEquals(sha256(text4096.toByteArray(Charsets.UTF_8)), inline4096.payload.sha256)

        // 4097 ASCII bytes: 1 byte above boundary -> MUST transition to ClipboardDescriptorPacket
        val text4097 = "a".repeat(4097)
        assertEquals(4097, text4097.toByteArray(Charsets.UTF_8).size)
        val packed4097 = manager.packText(text4097, downloadUrlBase = "http://127.0.0.1:53317")
        assertTrue("4097 bytes text must be ClipboardDescriptorPacket", packed4097 is ClipboardDescriptorPacket)
        val desc4097 = packed4097 as ClipboardDescriptorPacket
        assertEquals(4097, desc4097.descriptor.sizeBytes)
        assertEquals(sha256(text4097.toByteArray(Charsets.UTF_8)), desc4097.descriptor.sha256)
        assertTrue(desc4097.descriptor.downloadUrl!!.contains("/api/v2/clipboard/payload/"))

        // Verify cached data retrieval
        val cached = manager.getCachedPayload(desc4097.descriptor.payloadId, desc4097.descriptor.ephemeralToken)
        assertNotNull("Payload must be in cache", cached)
        assertEquals(4097, cached!!.size)
    }

    @Test
    fun testPackImageExactBoundary4096vs4097() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "ADVERSARIAL_TEST_DEVICE")

        // 4096 bytes image -> MUST be ClipboardInlinePacket
        val imageBytes4096 = ByteArray(4096) { (it % 256).toByte() }
        val packedImage4096 = manager.packImage(imageBytes4096, "image/png")
        assertTrue("4096 bytes image must be ClipboardInlinePacket", packedImage4096 is ClipboardInlinePacket)
        val inlineImage4096 = packedImage4096 as ClipboardInlinePacket
        assertEquals(4096, inlineImage4096.payload.sizeBytes)
        assertTrue(inlineImage4096.payload.content.startsWith("data:image/png;base64,"))
        assertEquals(sha256(imageBytes4096), inlineImage4096.payload.sha256)

        // 4097 bytes image -> MUST transition to ClipboardDescriptorPacket
        val imageBytes4097 = ByteArray(4097) { (it % 256).toByte() }
        val packedImage4097 = manager.packImage(imageBytes4097, "image/png", downloadUrlBase = "http://127.0.0.1:53317")
        assertTrue("4097 bytes image must be ClipboardDescriptorPacket", packedImage4097 is ClipboardDescriptorPacket)
        val descImage4097 = packedImage4097 as ClipboardDescriptorPacket
        assertEquals(4097, descImage4097.descriptor.sizeBytes)
        assertEquals("image/png", descImage4097.descriptor.primaryMime)
        assertEquals(sha256(imageBytes4097), descImage4097.descriptor.sha256)
    }

    @Test
    fun testResolveImageExactBoundary4MBvs4MBPlus1() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "ADVERSARIAL_TEST_DEVICE")

        // 4 MB (4 * 1024 * 1024 = 4194304 bytes) -> ClipContent.Image
        val file4MB = tempFolder.newFile("exact_4mb.png")
        val bytes4MB = ByteArray(4 * 1024 * 1024) { 0x42.toByte() }
        FileOutputStream(file4MB).use { it.write(bytes4MB) }

        val uri4MB = Uri.fromFile(file4MB)
        val content4MB = manager.resolveUri(uri4MB)
        assertTrue("Image of exactly 4MB must resolve to ClipContent.Image", content4MB is ClipContent.Image)
        val img = content4MB as ClipContent.Image
        assertEquals(4 * 1024 * 1024, img.bytes.size)
        assertEquals("image/png", img.mimeType)

        // 4 MB + 1 byte (4194305 bytes) -> Fallback to ClipContent.FilePayload
        val file4MBPlus1 = tempFolder.newFile("exceed_4mb.png")
        val bytes4MBPlus1 = ByteArray(4 * 1024 * 1024 + 1) { 0x42.toByte() }
        FileOutputStream(file4MBPlus1).use { it.write(bytes4MBPlus1) }

        val uri4MBPlus1 = Uri.fromFile(file4MBPlus1)
        val content4MBPlus1 = manager.resolveUri(uri4MBPlus1)
        assertTrue("Image > 4MB must fallback to ClipContent.FilePayload", content4MBPlus1 is ClipContent.FilePayload)
        val filePayload = content4MBPlus1 as ClipContent.FilePayload
        assertEquals(4 * 1024 * 1024 + 1L, filePayload.fileSize)
        assertEquals("image/png", filePayload.mimeType)
    }

    // =========================================================================
    // 2. Symmetrical Echo Suppression Tests (Lowercase vs Uppercase SHA-256)
    // =========================================================================

    @Test
    fun testHttpReceiverSymmetricalEchoSuppressionInline() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val downloadDir = tempFolder.newFolder("downloads_adv_echo_inline")
        val freePort = ServerSocket(0).use { it.localPort }
        val clipboardManager = ClipboardSyncManager(context, "DEVICE_ADVERSARIAL_RECV")

        val receiver = HttpReceiver(
            port = freePort,
            downloadDir = downloadDir,
            deviceAlias = "Android Receiver Echo",
            fingerprint = "TEST_FP_ECHO",
            clipboardManager = clipboardManager
        )

        val receivedCount = AtomicInteger(0)
        receiver.onClipboardReceived = { _, _ ->
            receivedCount.incrementAndGet()
        }
        receiver.start()
        Thread.sleep(100)

        val text = "Adversarial test string for symmetrical echo suppression"
        val lowerHash = sha256(text.toByteArray(Charsets.UTF_8)).lowercase()
        val upperHash = lowerHash.uppercase()
        // Ensure the hash actually has hex digits a-f / A-F
        assertTrue("Hash must contain alphabetic hex characters", lowerHash != lowerHash.uppercase())

        // SCENARIO 1: lastPastedHash is set to UPPERCASE.
        // Incoming packet has LOWERCASE hash.
        // Result: reflection must be suppressed (200 OK, callback NOT invoked).
        clipboardManager.setLastPastedHash(upperHash)

        val packetLower = ClipboardInlinePacket(
            deviceId = "DESKTOP_PEER",
            timestamp = System.currentTimeMillis(),
            ttlSeconds = 120,
            payload = ClipboardInlinePayload(
                mimeType = "text/plain",
                content = text,
                sha256 = lowerHash,
                sizeBytes = text.toByteArray(Charsets.UTF_8).size
            )
        )
        val packetLowerJson = json.encodeToString(packetLower)

        val url = URL("http://127.0.0.1:$freePort/api/v2/clipboard")
        var conn = url.openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.doOutput = true
        OutputStreamWriter(conn.outputStream).use { it.write(packetLowerJson) }
        assertEquals(200, conn.responseCode)
        Thread.sleep(100)
        assertEquals("Callback must NOT be called when incoming lower matches upper lastPastedHash", 0, receivedCount.get())

        // SCENARIO 2: lastPastedHash is set to LOWERCASE.
        // Incoming packet has UPPERCASE hash.
        // Result: reflection must be suppressed (200 OK, callback NOT invoked).
        clipboardManager.setLastPastedHash(lowerHash)

        val packetUpper = ClipboardInlinePacket(
            deviceId = "DESKTOP_PEER",
            timestamp = System.currentTimeMillis(),
            ttlSeconds = 120,
            payload = ClipboardInlinePayload(
                mimeType = "text/plain",
                content = text,
                sha256 = upperHash,
                sizeBytes = text.toByteArray(Charsets.UTF_8).size
            )
        )
        val packetUpperJson = json.encodeToString(packetUpper)

        conn = url.openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.doOutput = true
        OutputStreamWriter(conn.outputStream).use { it.write(packetUpperJson) }
        assertEquals(200, conn.responseCode)
        Thread.sleep(100)
        assertEquals("Callback must NOT be called when incoming upper matches lower lastPastedHash", 0, receivedCount.get())

        // SCENARIO 3: Fresh content (no match).
        // Callback MUST be invoked exactly once, and lastPastedHash updated.
        clipboardManager.setLastPastedHash(null)

        conn = url.openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.doOutput = true
        OutputStreamWriter(conn.outputStream).use { it.write(packetLowerJson) }
        assertEquals(200, conn.responseCode)
        Thread.sleep(100)
        assertEquals("Callback MUST be invoked for new non-reflected content", 1, receivedCount.get())
        assertTrue("lastPastedHash must now match lowercase", clipboardManager.isRecentHash(lowerHash))
        assertTrue("lastPastedHash must now match uppercase", clipboardManager.isRecentHash(upperHash))

        // Immediate reflection with uppercase should now be suppressed!
        conn = url.openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.doOutput = true
        OutputStreamWriter(conn.outputStream).use { it.write(packetUpperJson) }
        assertEquals(200, conn.responseCode)
        Thread.sleep(100)
        assertEquals("Callback must remain at 1 after reflection of newly pasted content", 1, receivedCount.get())

        receiver.stop()
    }

    @Test
    fun testHttpReceiverSymmetricalEchoSuppressionDescriptor() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val downloadDir = tempFolder.newFolder("downloads_adv_echo_desc")
        val freePort = ServerSocket(0).use { it.localPort }
        val clipboardManager = ClipboardSyncManager(context, "DEVICE_ADVERSARIAL_DESC")

        val receiver = HttpReceiver(
            port = freePort,
            downloadDir = downloadDir,
            deviceAlias = "Android Receiver Echo Desc",
            fingerprint = "TEST_FP_DESC",
            clipboardManager = clipboardManager
        )

        val receivedCount = AtomicInteger(0)
        receiver.onClipboardReceived = { _, _ ->
            receivedCount.incrementAndGet()
        }
        receiver.start()
        Thread.sleep(100)

        val largePayload = "x".repeat(5000)
        val lowerHash = sha256(largePayload.toByteArray(Charsets.UTF_8)).lowercase()
        val upperHash = lowerHash.uppercase()

        // Set lastPastedHash to lowercase
        clipboardManager.setLastPastedHash(lowerHash)

        // Incoming descriptor packet with uppercase sha256
        val descUpper = ClipboardDescriptorPacket(
            deviceId = "DESKTOP_PEER",
            timestamp = System.currentTimeMillis(),
            ttlSeconds = 120,
            descriptor = ClipboardDescriptorPayload(
                payloadId = "dummy-uuid",
                primaryMime = "text/plain",
                availableMimes = listOf("text/plain"),
                sizeBytes = 5000,
                sha256 = upperHash,
                ephemeralToken = "dummy-token",
                downloadUrl = "http://127.0.0.1:$freePort/api/v2/clipboard/payload/dummy-uuid?token=dummy-token"
            )
        )
        val descUpperJson = json.encodeToString(descUpper)

        val url = URL("http://127.0.0.1:$freePort/api/v2/clipboard")
        var conn = url.openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.doOutput = true
        OutputStreamWriter(conn.outputStream).use { it.write(descUpperJson) }
        assertEquals(200, conn.responseCode)
        Thread.sleep(100)
        assertEquals("Descriptor callback must NOT be called on uppercase echo", 0, receivedCount.get())

        // Set lastPastedHash to uppercase
        clipboardManager.setLastPastedHash(upperHash)

        // Incoming descriptor packet with lowercase sha256
        val descLower = ClipboardDescriptorPacket(
            deviceId = "DESKTOP_PEER",
            timestamp = System.currentTimeMillis(),
            ttlSeconds = 120,
            descriptor = ClipboardDescriptorPayload(
                payloadId = "dummy-uuid-2",
                primaryMime = "text/plain",
                availableMimes = listOf("text/plain"),
                sizeBytes = 5000,
                sha256 = lowerHash,
                ephemeralToken = "dummy-token",
                downloadUrl = "http://127.0.0.1:$freePort/api/v2/clipboard/payload/dummy-uuid-2?token=dummy-token"
            )
        )
        val descLowerJson = json.encodeToString(descLower)

        conn = url.openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.doOutput = true
        OutputStreamWriter(conn.outputStream).use { it.write(descLowerJson) }
        assertEquals(200, conn.responseCode)
        Thread.sleep(100)
        assertEquals("Descriptor callback must NOT be called on lowercase echo", 0, receivedCount.get())

        receiver.stop()
    }

    // =========================================================================
    // 3. Outbound LocalSend v2 Upload X-Clipboard-Staged Header Stress Tests
    // =========================================================================

    @Test
    fun testTransferClientOutboundHeadersInBothStages() = runBlocking {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val server = ServerSocket(0)
        val serverPort = server.localPort

        val testFile = tempFolder.newFile("stress_staged.bin")
        val fileContent = ByteArray(8192) { (it % 127).toByte() }
        FileOutputStream(testFile).use { it.write(fileContent) }

        val prepareHeaders = mutableMapOf<String, String>()
        val uploadHeaders = mutableMapOf<String, String>()
        val uploadBytesReceived = AtomicInteger(0)
        val stagesCompleted = CountDownLatch(2)

        val serverThread = Thread {
            try {
                // 1. Handle POST /api/localsend/v2/prepare-upload
                val s1 = server.accept()
                val r1 = BufferedReader(InputStreamReader(s1.getInputStream()))
                var line1: String?
                var contentLen1 = 0
                while (r1.readLine().also { line1 = it } != null) {
                    if (line1!!.isEmpty()) break
                    val idx = line1!!.indexOf(':')
                    if (idx != -1) {
                        val k = line1!!.substring(0, idx).trim().lowercase()
                        val v = line1!!.substring(idx + 1).trim()
                        prepareHeaders[k] = v
                    }
                }
                contentLen1 = prepareHeaders["content-length"]?.toIntOrNull() ?: 0
                val bodyBuf = CharArray(contentLen1)
                var read1 = 0
                while (read1 < contentLen1) {
                    val n = r1.read(bodyBuf, read1, contentLen1 - read1)
                    if (n == -1) break
                    read1 += n
                }
                val reqJson = String(bodyBuf, 0, read1)
                val prepDto = json.decodeFromString<PrepareUploadRequestDto>(reqJson)
                val fileId = prepDto.files.keys.first()

                val respDto = PrepareUploadResponseDto(
                    sessionId = "session-staged-xyz",
                    files = mapOf(fileId to "upload-token-abc")
                )
                val respJson = json.encodeToString(respDto)
                val out1 = s1.getOutputStream()
                out1.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${respJson.length}\r\n\r\n$respJson".toByteArray())
                out1.flush()
                s1.close()
                stagesCompleted.countDown()

                // 2. Handle POST /api/localsend/v2/upload
                val s2 = server.accept()
                val in2 = s2.getInputStream()
                var l2 = ""
                while (true) {
                    l2 = readLine(in2)
                    if (l2.isEmpty()) break
                    val idx = l2.indexOf(':')
                    if (idx != -1) {
                        val k = l2.substring(0, idx).trim().lowercase()
                        val v = l2.substring(idx + 1).trim()
                        uploadHeaders[k] = v
                    }
                }
                val contentLen2 = uploadHeaders["content-length"]?.toIntOrNull() ?: 0
                val streamBuf = ByteArray(4096)
                var totalStreamed = 0
                while (totalStreamed < contentLen2) {
                    val toRead = minOf(streamBuf.size, contentLen2 - totalStreamed)
                    val n = in2.read(streamBuf, 0, toRead)
                    if (n == -1) break
                    totalStreamed += n
                }
                uploadBytesReceived.set(totalStreamed)

                val out2 = s2.getOutputStream()
                out2.write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out2.flush()
                s2.close()
                stagesCompleted.countDown()
            } catch (_: Exception) {}
        }
        serverThread.start()

        val client = TransferClient(context)
        val result = client.sendStagedFile(
            targetHost = "127.0.0.1",
            targetPort = serverPort,
            uri = Uri.fromFile(testFile),
            fileName = "stress_staged.bin",
            fileSize = testFile.length(),
            mimeType = "application/octet-stream",
            senderFingerprint = "CUSTOM_FP_999"
        )

        assertTrue("sendStagedFile must succeed", result.success)
        assertEquals("session-staged-xyz", result.sessionId)
        assertTrue("Both HTTP stages must complete within 5 seconds", stagesCompleted.await(5, TimeUnit.SECONDS))

        // VERIFY PREPARE-UPLOAD HEADERS
        assertEquals("Prepare-upload must include X-Clipboard-Staged: true", "true", prepareHeaders["x-clipboard-staged"])
        assertEquals("Prepare-upload must include X-Fingerprint", "CUSTOM_FP_999", prepareHeaders["x-fingerprint"])

        // VERIFY UPLOAD STREAM HEADERS
        assertEquals("Upload must include X-Clipboard-Staged: true", "true", uploadHeaders["x-clipboard-staged"])
        assertEquals("Upload must include X-Fingerprint", "CUSTOM_FP_999", uploadHeaders["x-fingerprint"])
        assertEquals("application/octet-stream", uploadHeaders["content-type"])
        assertEquals(8192, uploadBytesReceived.get())

        server.close()
    }

    private fun readLine(input: InputStream): String {
        val sb = StringBuilder()
        var prev = 0
        while (true) {
            val b = input.read()
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
