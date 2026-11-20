package org.archdrop.clipboard

import android.content.ClipData
import android.content.ContentProvider
import android.content.ContentResolver
import android.content.ContentValues
import android.content.Context
import android.content.ContextWrapper
import android.database.Cursor
import android.net.Uri
import androidx.test.core.app.ApplicationProvider
import org.robolectric.Robolectric
import java.io.BufferedReader
import java.io.InputStreamReader
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.archdrop.clipboard.models.ClipboardClearPacket
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
import java.io.OutputStreamWriter
import java.net.HttpURLConnection
import java.net.ServerSocket
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class ClipboardSyncUnitTest {

    @get:Rule
    val tempFolder = TemporaryFolder()

    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    private fun sha256(text: String): String {
        val md = MessageDigest.getInstance("SHA-256")
        return md.digest(text.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
    }

    @Test
    fun testClipboardInlinePacketSerialization() {
        val text = "Arch Drop universal clipboard sync inline test"
        val hash = sha256(text)
        val packet = ClipboardInlinePacket(
            deviceId = "DEVICE_ARCH_01",
            timestamp = 1700000000L,
            ttlSeconds = 120,
            payload = ClipboardInlinePayload(
                mimeType = "text/plain;charset=utf-8",
                content = text,
                sha256 = hash,
                sizeBytes = text.toByteArray(Charsets.UTF_8).size
            )
        )

        val jsonString = json.encodeToString(packet)
        assertTrue(jsonString.contains("\"type\":\"clipboard.inline\""))
        assertTrue(jsonString.contains("\"deviceId\":\"DEVICE_ARCH_01\""))
        assertTrue(jsonString.contains("\"content\":\"$text\""))
        assertTrue(jsonString.contains("\"sha256\":\"$hash\""))

        val decoded = json.decodeFromString<ClipboardInlinePacket>(jsonString)
        assertEquals("clipboard.inline", decoded.type)
        assertEquals("DEVICE_ARCH_01", decoded.deviceId)
        assertEquals(text, decoded.payload.content)
        assertEquals(hash, decoded.payload.sha256)
    }

    @Test
    fun testClipboardDescriptorPacketSerialization() {
        val packet = ClipboardDescriptorPacket(
            deviceId = "DEVICE_ARCH_01",
            timestamp = 1700000000L,
            ttlSeconds = 120,
            descriptor = ClipboardDescriptorPayload(
                payloadId = "payload-uuid-1234",
                primaryMime = "text/plain;charset=utf-8",
                availableMimes = listOf("text/plain;charset=utf-8"),
                sizeBytes = 10000,
                sha256 = "dummy_sha256_hash",
                ephemeralToken = "token-secret-xyz",
                downloadUrl = "http://10.2.0.2:53317/api/v2/clipboard/payload/payload-uuid-1234?token=token-secret-xyz"
            )
        )

        val jsonString = json.encodeToString(packet)
        assertTrue(jsonString.contains("\"type\":\"clipboard.descriptor\""))
        assertTrue(jsonString.contains("\"payloadId\":\"payload-uuid-1234\""))
        assertTrue(jsonString.contains("\"ephemeralToken\":\"token-secret-xyz\""))

        val decoded = json.decodeFromString<ClipboardDescriptorPacket>(jsonString)
        assertEquals("clipboard.descriptor", decoded.type)
        assertEquals("payload-uuid-1234", decoded.descriptor.payloadId)
        assertEquals(10000, decoded.descriptor.sizeBytes)
    }

    @Test
    fun testClipboardSyncManagerThresholdRules() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "DEVICE_ANDROID_TEST")

        // 1. Small payload <= 4096 bytes -> InlinePacket
        val smallText = "Hello from Android under 4KB threshold"
        val smallPacked = manager.packText(smallText)
        assertTrue(smallPacked is ClipboardInlinePacket)
        val inlinePacket = smallPacked as ClipboardInlinePacket
        assertEquals("clipboard.inline", inlinePacket.type)
        assertEquals(smallText, inlinePacket.payload.content)

        // 2. Large payload > 4096 bytes -> DescriptorPacket
        val largeBuilder = StringBuilder()
        for (i in 0 until 500) {
            largeBuilder.append("This is line number $i exceeding 4KB threshold.\n")
        }
        val largeText = largeBuilder.toString()
        assertTrue(largeText.toByteArray(Charsets.UTF_8).size > 4096)

        val largePacked = manager.packText(largeText, downloadUrlBase = "http://127.0.0.1:53317")
        assertTrue(largePacked is ClipboardDescriptorPacket)
        val descPacket = largePacked as ClipboardDescriptorPacket
        assertEquals("clipboard.descriptor", descPacket.type)
        assertEquals(largeText.toByteArray(Charsets.UTF_8).size, descPacket.descriptor.sizeBytes)
        assertNotNull(descPacket.descriptor.downloadUrl)
    }

    @Test
    fun testClipboardSyncManagerEchoSuppression() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "DEVICE_ANDROID_TEST")

        val text = "Synchronized text snippet"
        assertFalse(manager.isRecentPaste(text))

        manager.pasteToSystem(text)
        assertTrue(manager.isRecentPaste(text))

        // Different text should not match
        assertFalse(manager.isRecentPaste("Different text snippet"))

        // Reset hash
        manager.setLastPastedHash(null)
        assertFalse(manager.isRecentPaste(text))
    }

    @Test
    fun testHttpReceiverClipboardEndpoint() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val downloadDir = tempFolder.newFolder("downloads")

        val freePort = ServerSocket(0).use { it.localPort }
        val clipboardManager = ClipboardSyncManager(context, "DEVICE_ANDROID_RECV")

        val receiver = HttpReceiver(
            port = freePort,
            downloadDir = downloadDir,
            deviceAlias = "Android Receiver",
            fingerprint = "ANDROID_TEST_FP",
            clipboardManager = clipboardManager
        )

        val receivedContent = AtomicReference<String>()
        val receivedHost = AtomicReference<String>()
        val latch = CountDownLatch(1)

        receiver.onClipboardReceived = { content, remoteHost ->
            receivedContent.set(content)
            receivedHost.set(remoteHost)
            latch.countDown()
        }

        receiver.start()
        // Wait briefly for server accept loop
        Thread.sleep(100)

        val testText = "Remote clipboard payload from desktop to android"
        val packet = ClipboardInlinePacket(
            deviceId = "DESKTOP_PEER",
            timestamp = System.currentTimeMillis(),
            ttlSeconds = 120,
            payload = ClipboardInlinePayload(
                mimeType = "text/plain",
                content = testText,
                sha256 = sha256(testText),
                sizeBytes = testText.toByteArray(Charsets.UTF_8).size
            )
        )
        val packetJson = json.encodeToString(packet)

        val url = URL("http://127.0.0.1:$freePort/api/v2/clipboard")
        val conn = url.openConnection() as HttpURLConnection
        conn.requestMethod = "POST"
        conn.setRequestProperty("Content-Type", "application/json")
        conn.doOutput = true
        conn.connectTimeout = 3000
        conn.readTimeout = 3000

        OutputStreamWriter(conn.outputStream).use { writer ->
            writer.write(packetJson)
            writer.flush()
        }

        val responseCode = conn.responseCode
        assertEquals(200, responseCode)

        assertTrue("Callback should be invoked within 3 seconds", latch.await(3, TimeUnit.SECONDS))
        assertEquals(testText, receivedContent.get())
        assertNotNull(receivedHost.get())

        // Ensure echo suppression registered this paste
        assertTrue(clipboardManager.isRecentPaste(testText))

        // Duplicate packet must succeed with 200 OK, but echo is suppressed (callback not re-invoked)
        val dupConn = url.openConnection() as HttpURLConnection
        dupConn.requestMethod = "POST"
        dupConn.setRequestProperty("Content-Type", "application/json")
        dupConn.doOutput = true
        dupConn.connectTimeout = 3000
        dupConn.readTimeout = 3000
        OutputStreamWriter(dupConn.outputStream).use { writer ->
            writer.write(packetJson)
            writer.flush()
        }
        assertEquals(200, dupConn.responseCode)

        receiver.stop()
    }

    @Test
    fun testHttpReceiverDescriptorAndPayloadEndpoint() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val downloadDir = tempFolder.newFolder("downloads")

        val freePort = ServerSocket(0).use { it.localPort }
        val clipboardManager = ClipboardSyncManager(context, "DEVICE_ANDROID_RECV")

        val receiver = HttpReceiver(
            port = freePort,
            downloadDir = downloadDir,
            deviceAlias = "Android Receiver",
            fingerprint = "ANDROID_TEST_FP",
            clipboardManager = clipboardManager
        )

        val receivedContent = AtomicReference<String>()
        val latch = CountDownLatch(1)

        receiver.onClipboardReceived = { content, _ ->
            receivedContent.set(content)
            latch.countDown()
        }

        receiver.start()
        Thread.sleep(100)

        // Generate text > 4096 bytes
        val largeBuilder = StringBuilder()
        for (i in 0 until 300) {
            largeBuilder.append("Descriptor payload line $i for cross-device retrieval.\n")
        }
        val largeText = largeBuilder.toString()
        assertTrue(largeText.toByteArray(Charsets.UTF_8).size > 4096)

        val packed = clipboardManager.packText(largeText, downloadUrlBase = "http://127.0.0.1:$freePort")
        assertTrue(packed is ClipboardDescriptorPacket)
        val descPacket = packed as ClipboardDescriptorPacket

        // 1. Verify GET /api/v2/clipboard/payload/:id?token=... directly
        val payloadUrl = URL(descPacket.descriptor.downloadUrl!!)
        val payloadConn = payloadUrl.openConnection() as HttpURLConnection
        payloadConn.requestMethod = "GET"
        assertEquals(200, payloadConn.responseCode)
        val payloadBytes = payloadConn.inputStream.readBytes()
        assertEquals(largeText, String(payloadBytes, Charsets.UTF_8))

        // 2. Post descriptor packet to HTTP receiver
        val descJson = json.encodeToString(descPacket)
        // Reset last pasted hash to simulate incoming from remote
        clipboardManager.setLastPastedHash(null)

        val postUrl = URL("http://127.0.0.1:$freePort/api/v2/clipboard")
        val postConn = postUrl.openConnection() as HttpURLConnection
        postConn.requestMethod = "POST"
        postConn.setRequestProperty("Content-Type", "application/json")
        postConn.doOutput = true
        OutputStreamWriter(postConn.outputStream).use { writer ->
            writer.write(descJson)
            writer.flush()
        }
        assertEquals(200, postConn.responseCode)

        assertTrue("Callback should be invoked after downloading descriptor payload", latch.await(5, TimeUnit.SECONDS))
        assertEquals(largeText, receivedContent.get())

        receiver.stop()
    }

    @Test
    fun testHttpReceiverRelativeUrlAndUppercaseHash() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val downloadDir = tempFolder.newFolder("downloads-rel")

        val freePort = ServerSocket(0).use { it.localPort }
        val clipboardManager = ClipboardSyncManager(context, "DEVICE_ANDROID_RECV_2")

        val receiver = HttpReceiver(
            port = freePort,
            downloadDir = downloadDir,
            deviceAlias = "Android Receiver Rel",
            fingerprint = "ANDROID_TEST_FP",
            clipboardManager = clipboardManager
        )

        val receivedContent = AtomicReference<String>()
        val latch = CountDownLatch(1)

        receiver.onClipboardReceived = { content, _ ->
            receivedContent.set(content)
            latch.countDown()
        }

        receiver.start()
        Thread.sleep(100)

        val largeBuilder = StringBuilder()
        for (i in 0 until 300) {
            largeBuilder.append("Relative URL test payload line $i.\n")
        }
        val largeText = largeBuilder.toString()

        val packed = clipboardManager.packText(largeText, downloadUrlBase = "http://127.0.0.1:$freePort")
        assertTrue(packed is ClipboardDescriptorPacket)
        val origDesc = packed as ClipboardDescriptorPacket

        // Rewrite downloadUrl to a relative URL: /api/v2/clipboard/payload/...
        val relativeUrl = "/api/v2/clipboard/payload/${origDesc.descriptor.payloadId}?token=${origDesc.descriptor.ephemeralToken}"
        val uppercaseHash = origDesc.descriptor.sha256.uppercase()

        val relDescPacket = ClipboardDescriptorPacket(
            deviceId = "DESKTOP_PEER",
            timestamp = System.currentTimeMillis(),
            ttlSeconds = 120,
            descriptor = ClipboardDescriptorPayload(
                payloadId = origDesc.descriptor.payloadId,
                primaryMime = origDesc.descriptor.primaryMime,
                availableMimes = origDesc.descriptor.availableMimes,
                sizeBytes = origDesc.descriptor.sizeBytes,
                sha256 = uppercaseHash,
                ephemeralToken = origDesc.descriptor.ephemeralToken,
                downloadUrl = relativeUrl
            )
        )

        clipboardManager.setLastPastedHash(null)

        val postUrl = URL("http://127.0.0.1:$freePort/api/v2/clipboard")
        val postConn = postUrl.openConnection() as HttpURLConnection
        postConn.requestMethod = "POST"
        postConn.setRequestProperty("Content-Type", "application/json")
        postConn.doOutput = true
        OutputStreamWriter(postConn.outputStream).use { writer ->
            writer.write(json.encodeToString(relDescPacket))
            writer.flush()
        }
        assertEquals(200, postConn.responseCode)

        assertTrue("Callback should be invoked for relative URL descriptor payload", latch.await(5, TimeUnit.SECONDS))
        assertEquals(largeText, receivedContent.get())

        // Verify case-insensitive echo suppression
        assertTrue(clipboardManager.isRecentHash(uppercaseHash))
        assertTrue(clipboardManager.isRecentHash(uppercaseHash.lowercase()))
        assertTrue(clipboardManager.isRecentPaste(largeText))

        receiver.stop()
    }

    @Test
    fun testResolveClipTextAndEmpty() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "DEVICE_TEST")

        // 1. Text clip
        val textClip = ClipData.newPlainText("note", "Sample plain text clip")
        val content = manager.resolveClip(textClip)
        assertTrue(content is ClipContent.Text)
        assertEquals("Sample plain text clip", (content as ClipContent.Text).text)

        // 2. Empty clip
        assertEquals(ClipContent.Empty, manager.resolveClip(null))
        val emptyClip = ClipData.newPlainText("", "")
        // text is empty
        val resolvedEmpty = manager.resolveClip(emptyClip)
        assertEquals(ClipContent.Empty, resolvedEmpty)
    }

    @Test
    fun testResolveSmallImageAndInlinePacking() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "DEVICE_TEST")

        val imgFile = tempFolder.newFile("sample.png")
        val testBytes = ByteArray(2048) { (it % 256).toByte() }
        java.io.FileOutputStream(imgFile).use { it.write(testBytes) }

        val uri = Uri.fromFile(imgFile)
        val clip = ClipData.newUri(context.contentResolver, "image", uri)
        val content = manager.resolveClip(clip)

        assertTrue("Expected ClipContent.Image for 2KB PNG", content is ClipContent.Image)
        val img = content as ClipContent.Image
        assertEquals("sample.png", img.fileName)
        assertEquals("image/png", img.mimeType)
        assertEquals(2048, img.bytes.size)

        val packet = manager.packImage(img.bytes, img.mimeType)
        assertTrue("Expected ClipboardInlinePacket for image <= 4096 bytes", packet is ClipboardInlinePacket)
        val inline = packet as ClipboardInlinePacket
        assertEquals("clipboard.inline", inline.type)
        assertEquals("image/png", inline.payload.mimeType)
        assertTrue(inline.payload.content.startsWith("data:image/png;base64,"))
        assertEquals(2048, inline.payload.sizeBytes)
        assertEquals(img.sha256, inline.payload.sha256)
    }

    @Test
    fun testResolveMediumImageAndDescriptorPacking() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "DEVICE_TEST")

        val imgFile = tempFolder.newFile("photo.jpg")
        val size = 64 * 1024 // 64 KB (> 4KB, <= 4MB)
        val testBytes = ByteArray(size) { (it % 128).toByte() }
        java.io.FileOutputStream(imgFile).use { it.write(testBytes) }

        val uri = Uri.fromFile(imgFile)
        val clip = ClipData.newUri(context.contentResolver, "image", uri)
        val content = manager.resolveClip(clip)

        assertTrue("Expected ClipContent.Image for 64KB JPG", content is ClipContent.Image)
        val img = content as ClipContent.Image
        assertEquals("image/jpeg", img.mimeType)
        assertEquals(size, img.bytes.size)

        val packet = manager.packImage(img.bytes, img.mimeType, downloadUrlBase = "http://127.0.0.1:53317")
        assertTrue("Expected ClipboardDescriptorPacket for image > 4096 bytes", packet is ClipboardDescriptorPacket)
        val desc = packet as ClipboardDescriptorPacket
        assertEquals("clipboard.descriptor", desc.type)
        assertEquals("image/jpeg", desc.descriptor.primaryMime)
        assertEquals(size, desc.descriptor.sizeBytes)
        assertTrue(desc.descriptor.downloadUrl!!.contains("/api/v2/clipboard/payload/"))

        // Verify retrieval from payloadCache
        val cached = manager.getCachedPayload(desc.descriptor.payloadId, desc.descriptor.ephemeralToken)
        assertNotNull("Payload must be cached in memory", cached)
        assertEquals(size, cached!!.size)
    }

    @Test
    fun testResolveLargeImageFallbackToFilePayload() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "DEVICE_TEST")

        val imgFile = tempFolder.newFile("huge.png")
        // 4.5 MB (> 4MB limit)
        val size = 4500 * 1024
        val testBytes = ByteArray(size) { 1 }
        java.io.FileOutputStream(imgFile).use { it.write(testBytes) }

        val uri = Uri.fromFile(imgFile)
        val clip = ClipData.newUri(context.contentResolver, "huge", uri)
        val content = manager.resolveClip(clip)

        assertTrue("Images > 4 MB must fallback to ClipContent.FilePayload", content is ClipContent.FilePayload)
        val filePayload = content as ClipContent.FilePayload
        assertEquals("huge.png", filePayload.fileName)
        assertEquals("image/png", filePayload.mimeType)
        assertTrue(filePayload.fileSize > 4 * 1024 * 1024)
    }

    @Test
    fun testResolveNonImageDocumentToFilePayload() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val manager = ClipboardSyncManager(context, "DEVICE_TEST")

        val pdfFile = tempFolder.newFile("contract.pdf")
        val testBytes = "PDF content simulated".toByteArray()
        java.io.FileOutputStream(pdfFile).use { it.write(testBytes) }

        val uri = Uri.fromFile(pdfFile)
        val clip = ClipData.newUri(context.contentResolver, "pdf", uri)
        val content = manager.resolveClip(clip)

        assertTrue("Non-image files must resolve to ClipContent.FilePayload", content is ClipContent.FilePayload)
        val filePayload = content as ClipContent.FilePayload
        assertEquals("contract.pdf", filePayload.fileName)
        assertEquals("application/pdf", filePayload.mimeType)
        assertEquals(testBytes.size.toLong(), filePayload.fileSize)
    }

    class SecurityDenyingContentProvider : ContentProvider() {
        override fun onCreate(): Boolean = true
        override fun getType(uri: Uri): String? = throw SecurityException("Permission denied by test security policy")
        override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? = throw SecurityException("Permission denied by test security policy")
        override fun insert(uri: Uri, values: ContentValues?): Uri? = null
        override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0
        override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = 0
    }

    @Test
    fun testSafeUriResolutionGuardsSecurityException() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        Robolectric.buildContentProvider(SecurityDenyingContentProvider::class.java).create("denied.provider")

        val manager = ClipboardSyncManager(context, "DEVICE_SEC_TEST")
        val secretUri = Uri.parse("content://denied.provider/private.png")

        val result = manager.resolveUri(secretUri)
        assertTrue("SecurityException must be caught gracefully and return ClipContent.Error", result is ClipContent.Error)
        val error = result as ClipContent.Error
        assertTrue("Error message must contain permission denial info", error.message.contains("Permission denied"))
    }

    @Test
    fun testOutboundTransferClientStagedFileUpload() = runBlocking {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val server = ServerSocket(0)
        val serverPort = server.localPort

        val testFile = tempFolder.newFile("staged_doc.pdf")
        val contentBytes = "Sample staged PDF payload across devices".toByteArray(Charsets.UTF_8)
        java.io.FileOutputStream(testFile).use { it.write(contentBytes) }

        val receivedPrepareHeader = AtomicReference<String>()
        val receivedUploadHeader = AtomicReference<String>()
        val receivedUploadedBytes = AtomicReference<ByteArray>()
        val latch = CountDownLatch(2)

        val serverThread = Thread {
            try {
                // Request 1: POST /api/localsend/v2/prepare-upload
                val sock1 = server.accept()
                val reader1 = BufferedReader(InputStreamReader(sock1.getInputStream()))
                var contentLen1 = 0
                while (true) {
                    val line1 = reader1.readLine() ?: break
                    if (line1.isEmpty()) break
                    if (line1.startsWith("X-Clipboard-Staged:", ignoreCase = true)) {
                        receivedPrepareHeader.set(line1.split(":")[1].trim())
                    }
                    if (line1.startsWith("Content-Length:", ignoreCase = true)) {
                        contentLen1 = line1.split(":")[1].trim().toIntOrNull() ?: 0
                    }
                }
                val bodyChars1 = CharArray(contentLen1)
                var r1 = 0
                while (r1 < contentLen1) {
                    val read = reader1.read(bodyChars1, r1, contentLen1 - r1)
                    if (read == -1) break
                    r1 += read
                }
                val prepareReqJson = String(bodyChars1, 0, r1)
                val prepDto = json.decodeFromString<PrepareUploadRequestDto>(prepareReqJson)
                val fileId = prepDto.files.keys.first()

                val prepResp = PrepareUploadResponseDto(
                    sessionId = "test-session-100",
                    files = mapOf(fileId to "upload-token-200")
                )
                val prepRespJson = json.encodeToString(prepResp)
                val out1 = sock1.getOutputStream()
                out1.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${prepRespJson.length}\r\n\r\n$prepRespJson".toByteArray())
                out1.flush()
                sock1.close()
                latch.countDown()

                // Request 2: POST /api/localsend/v2/upload?...
                val sock2 = server.accept()
                val in2 = sock2.getInputStream()
                var line2 = ""
                var contentLen2 = 0
                while (true) {
                    val l = readLineFromStream(in2)
                    if (l.isEmpty()) break
                    if (l.startsWith("X-Clipboard-Staged:", ignoreCase = true)) {
                        receivedUploadHeader.set(l.split(":")[1].trim())
                    }
                    if (l.startsWith("Content-Length:", ignoreCase = true)) {
                        contentLen2 = l.split(":")[1].trim().toIntOrNull() ?: 0
                    }
                }
                val bodyBytes2 = ByteArray(contentLen2)
                var r2 = 0
                while (r2 < contentLen2) {
                    val read = in2.read(bodyBytes2, r2, contentLen2 - r2)
                    if (read == -1) break
                    r2 += read
                }
                receivedUploadedBytes.set(bodyBytes2)
                val out2 = sock2.getOutputStream()
                out2.write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out2.flush()
                sock2.close()
                latch.countDown()
            } catch (_: Exception) {}
        }
        serverThread.start()

        val client = TransferClient(context)
        val result = client.sendStagedFile(
            targetHost = "127.0.0.1",
            targetPort = serverPort,
            uri = Uri.fromFile(testFile),
            fileName = "staged_doc.pdf",
            fileSize = testFile.length(),
            mimeType = "application/pdf"
        )

        assertTrue("TransferClient sendStagedFile must succeed", result.success)
        assertEquals("test-session-100", result.sessionId)
        assertTrue("Both server exchanges must complete within 5 seconds", latch.await(5, TimeUnit.SECONDS))
        assertEquals("true", receivedPrepareHeader.get())
        assertEquals("true", receivedUploadHeader.get())
        assertEquals(String(contentBytes), String(receivedUploadedBytes.get() ?: ByteArray(0)))

        server.close()
    }

    @Test
    fun testSymmetricalEchoSuppressionOnInlineImagePacket() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val downloadDir = tempFolder.newFolder("downloads-img")
        val freePort = ServerSocket(0).use { it.localPort }
        val clipboardManager = ClipboardSyncManager(context, "DEVICE_IMG_ECHO")

        val receiver = HttpReceiver(
            port = freePort,
            downloadDir = downloadDir,
            deviceAlias = "Android Receiver Image",
            fingerprint = "TEST_FP",
            clipboardManager = clipboardManager
        )

        val receivedCount = java.util.concurrent.atomic.AtomicInteger(0)
        receiver.onClipboardReceived = { _, _ ->
            receivedCount.incrementAndGet()
        }
        receiver.start()
        Thread.sleep(100)

        val rawImageBytes = "fake PNG raw bytes".toByteArray()
        val binaryHash = ClipboardSyncManager.sha256(rawImageBytes)
        val b64Content = "data:image/png;base64," + java.util.Base64.getEncoder().encodeToString(rawImageBytes)

        val packet = ClipboardInlinePacket(
            deviceId = "DESKTOP_PEER",
            timestamp = System.currentTimeMillis(),
            ttlSeconds = 120,
            payload = ClipboardInlinePayload(
                mimeType = "image/png",
                content = b64Content,
                sha256 = binaryHash,
                sizeBytes = rawImageBytes.size
            )
        )
        val packetJson = json.encodeToString(packet)

        // 1. Post packet
        val url = URL("http://127.0.0.1:$freePort/api/v2/clipboard")
        val conn1 = url.openConnection() as HttpURLConnection
        conn1.requestMethod = "POST"
        conn1.setRequestProperty("Content-Type", "application/json")
        conn1.doOutput = true
        OutputStreamWriter(conn1.outputStream).use { it.write(packetJson) }
        assertEquals(200, conn1.responseCode)
        Thread.sleep(150)
        assertEquals(1, receivedCount.get())

        // Ensure lastPastedHash was updated directly to the packet's binary SHA-256
        assertTrue(clipboardManager.isRecentHash(binaryHash))

        // 2. Post duplicate packet -> must suppress echo and NOT invoke onClipboardReceived again
        val conn2 = url.openConnection() as HttpURLConnection
        conn2.requestMethod = "POST"
        conn2.setRequestProperty("Content-Type", "application/json")
        conn2.doOutput = true
        OutputStreamWriter(conn2.outputStream).use { it.write(packetJson) }
        assertEquals(200, conn2.responseCode)
        Thread.sleep(150)
        assertEquals("Callback must not be re-invoked for duplicate SHA-256 echo", 1, receivedCount.get())

        receiver.stop()
    }

    private fun readLineFromStream(inputStream: java.io.InputStream): String {
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

