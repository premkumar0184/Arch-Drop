package org.archdrop.clipboard

import android.app.Application
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.runBlocking
import org.archdrop.clipboard.models.ClipboardDescriptorPacket
import org.archdrop.clipboard.models.ClipboardInlinePacket
import org.archdrop.db.ArchDropDatabase
import org.archdrop.ui.dashboard.DashboardViewModel
import org.archdrop.ui.dashboard.SendClipboardResult
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.BufferedReader
import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.InputStreamReader
import java.io.OutputStream
import java.net.ServerSocket
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class AdversarialClipboardStressTest {

    @get:Rule
    val tempFolder = TemporaryFolder()

    private lateinit var app: Application
    private lateinit var db: ArchDropDatabase
    private lateinit var clipboardManager: ClipboardManager

    @Before
    fun setUp() {
        app = ApplicationProvider.getApplicationContext<Application>()
        clipboardManager = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        db = Room.inMemoryDatabaseBuilder(app, ArchDropDatabase::class.java)
            .allowMainThreadQueries()
            .build()
    }

    @After
    fun tearDown() {
        db.close()
    }

    private fun sha256(bytes: ByteArray): String {
        val md = MessageDigest.getInstance("SHA-256")
        return md.digest(bytes).joinToString("") { "%02x".format(it) }
    }

    // =========================================================================
    // 1. EMPTY CLIPBOARD ADVERSARIAL STRESS TESTS
    // =========================================================================

    @Test
    fun testEmptyClipboardVariations() = runBlocking {
        val syncManager = ClipboardSyncManager(app, "DEVICE_TEST")
        val viewModel = DashboardViewModel(app, db)

        // Case A: System clipboard is cleared / null
        syncManager.clearSystemClipboard()
        assertEquals(ClipContent.Empty, syncManager.inspectPrimaryClip())
        var result = viewModel.sendClipboard(app)
        assertEquals(SendClipboardResult.Empty, result)
        assertFalse(viewModel.uiState.value.isSendingClipboard)

        // Case B: ClipData with null item
        assertEquals(ClipContent.Empty, syncManager.resolveClip(null))

        // Case C: ClipData with empty text item
        val emptyTextClip = ClipData.newPlainText("", "")
        clipboardManager.setPrimaryClip(emptyTextClip)
        assertEquals(ClipContent.Empty, syncManager.resolveClip(emptyTextClip))
        result = viewModel.sendClipboard(app)
        assertEquals(SendClipboardResult.Empty, result)
        assertFalse(viewModel.uiState.value.isSendingClipboard)

        // Case D: ClipData with only whitespace
        // Note: whitespace-only text is still text, but resolveClip inspects it:
        val whitespaceClip = ClipData.newPlainText("spaces", "   \t\n  ")
        val resolved = syncManager.resolveClip(whitespaceClip)
        assertTrue(resolved is ClipContent.Text)
        assertEquals("   \t\n  ", (resolved as ClipContent.Text).text)
    }

    // =========================================================================
    // 2. TEXT CLIPBOARD BOUNDARY & UNICODE STRESS TESTS
    // =========================================================================

    @Test
    fun testTextThresholdExactBoundaries() {
        val syncManager = ClipboardSyncManager(app, "DEVICE_TEST")

        // 4095 bytes -> Inline
        val b4095 = "a".repeat(4095)
        val p4095 = syncManager.packText(b4095)
        assertTrue("4095 bytes must be ClipboardInlinePacket", p4095 is ClipboardInlinePacket)
        assertEquals(4095, (p4095 as ClipboardInlinePacket).payload.sizeBytes)

        // 4096 bytes -> Inline (boundary: size <= INLINE_THRESHOLD)
        val b4096 = "a".repeat(4096)
        val p4096 = syncManager.packText(b4096)
        assertTrue("4096 bytes must be ClipboardInlinePacket", p4096 is ClipboardInlinePacket)
        assertEquals(4096, (p4096 as ClipboardInlinePacket).payload.sizeBytes)

        // 4097 bytes -> Descriptor (boundary: size > INLINE_THRESHOLD)
        val b4097 = "a".repeat(4097)
        val p4097 = syncManager.packText(b4097)
        assertTrue("4097 bytes must be ClipboardDescriptorPacket", p4097 is ClipboardDescriptorPacket)
        assertEquals(4097, (p4097 as ClipboardDescriptorPacket).descriptor.sizeBytes)
    }

    @Test
    fun testUnicodeAndControlCharacterPayloads() {
        val syncManager = ClipboardSyncManager(app, "DEVICE_TEST")

        val complexText = "ArchDrop 🚀 🦀 🐧 \u0000 \u0007 \u001B[31mRed\u001B[0m 汉语 한국어 العربية עברית"
        val packet = syncManager.packText(complexText)
        assertTrue(packet is ClipboardInlinePacket)
        val inline = packet as ClipboardInlinePacket
        assertEquals(complexText, inline.payload.content)
        assertEquals(sha256(complexText.toByteArray(Charsets.UTF_8)), inline.payload.sha256)
    }

    @Test
    fun testSendClipboardTextSuccessAndFailureModes() = runBlocking {
        val viewModel = DashboardViewModel(app, db)
        val server = ServerSocket(0)
        val port = server.localPort

        clipboardManager.setPrimaryClip(ClipData.newPlainText("note", "Empirical Test String"))

        // Mock 1: Server accepts and returns 200 OK
        val thread1 = Thread {
            try {
                val sock = server.accept()
                val reader = BufferedReader(InputStreamReader(sock.getInputStream()))
                while (true) {
                    val line = reader.readLine() ?: break
                    if (line.isEmpty()) break
                }
                sock.getOutputStream().write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                sock.getOutputStream().flush()
                sock.close()
            } catch (_: Exception) {}
        }
        thread1.start()

        val result1 = viewModel.sendClipboard(app, targetHost = "127.0.0.1", targetPort = port)
        assertTrue("Expected SendClipboardResult.Success on HTTP 200", result1 is SendClipboardResult.Success)
        val success1 = result1 as SendClipboardResult.Success
        assertTrue(success1.summary.contains("Text \"Empirical Test String\""))
        assertFalse("isSendingClipboard must reset to false", viewModel.uiState.value.isSendingClipboard)

        // Mock 2: Server returns 500 Internal Server Error
        val thread2 = Thread {
            try {
                val sock = server.accept()
                val reader = BufferedReader(InputStreamReader(sock.getInputStream()))
                while (true) {
                    val line = reader.readLine() ?: break
                    if (line.isEmpty()) break
                }
                sock.getOutputStream().write("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".toByteArray())
                sock.getOutputStream().flush()
                sock.close()
            } catch (_: Exception) {}
        }
        thread2.start()

        val result2 = viewModel.sendClipboard(app, targetHost = "127.0.0.1", targetPort = port)
        assertTrue("Expected SendClipboardResult.Error on HTTP 500", result2 is SendClipboardResult.Error)
        assertFalse("isSendingClipboard must reset to false", viewModel.uiState.value.isSendingClipboard)

        server.close()
    }

    // =========================================================================
    // 3. IMAGE CLIPBOARD BOUNDARY & PAYLOAD STRESS TESTS
    // =========================================================================

    @Test
    fun testImageExactBoundaries() {
        val syncManager = ClipboardSyncManager(app, "DEVICE_TEST")

        // Case A: 1-byte image (minimal valid payload)
        val b1 = ByteArray(1) { 0x42 }
        val p1 = syncManager.packImage(b1, "image/png")
        assertTrue(p1 is ClipboardInlinePacket)
        val inline1 = p1 as ClipboardInlinePacket
        assertEquals(1, inline1.payload.sizeBytes)
        assertEquals("image/png", inline1.payload.mimeType)
        assertTrue(inline1.payload.content.startsWith("data:image/png;base64,"))
        val decoded1 = Base64.getDecoder().decode(inline1.payload.content.removePrefix("data:image/png;base64,"))
        assertEquals(0x42.toByte(), decoded1[0])

        // Case B: Exactly 4096 bytes (inline threshold)
        val b4096 = ByteArray(4096) { (it % 256).toByte() }
        val p4096 = syncManager.packImage(b4096, "image/jpeg")
        assertTrue("4096 bytes image must be InlinePacket", p4096 is ClipboardInlinePacket)
        assertEquals(4096, (p4096 as ClipboardInlinePacket).payload.sizeBytes)
        assertEquals("image/jpeg", (p4096 as ClipboardInlinePacket).payload.mimeType)

        // Case C: Exactly 4097 bytes (descriptor threshold)
        val b4097 = ByteArray(4097) { (it % 256).toByte() }
        val p4097 = syncManager.packImage(b4097, "image/webp")
        assertTrue("4097 bytes image must be DescriptorPacket", p4097 is ClipboardDescriptorPacket)
        val desc4097 = p4097 as ClipboardDescriptorPacket
        assertEquals(4097, desc4097.descriptor.sizeBytes)
        assertEquals("image/webp", desc4097.descriptor.primaryMime)

        // Case D: Exactly 4 MB (MAX_IMAGE_PAYLOAD_SIZE = 4 * 1024 * 1024)
        val b4MB = ByteArray(4 * 1024 * 1024) { 1 }
        val p4MB = syncManager.packImage(b4MB, "image/png")
        assertTrue("4 MB image must still be DescriptorPacket", p4MB is ClipboardDescriptorPacket)
        assertEquals(4 * 1024 * 1024, (p4MB as ClipboardDescriptorPacket).descriptor.sizeBytes)

        // Case E: Payload Cache verification & token security
        val cached = syncManager.getCachedPayload(desc4097.descriptor.payloadId, desc4097.descriptor.ephemeralToken)
        assertNotNull("Cached payload must match", cached)
        assertEquals(4097, cached!!.size)

        // Invalid token returns null
        assertNull("Invalid token must return null", syncManager.getCachedPayload(desc4097.descriptor.payloadId, "wrong-token"))
        // Nonexistent payload ID returns null
        assertNull("Non-existent payload ID must return null", syncManager.getCachedPayload("nonexistent-id", desc4097.descriptor.ephemeralToken))
    }

    @Test
    fun testImageExceeding4MBFallbackToFilePayload() {
        val syncManager = ClipboardSyncManager(app, "DEVICE_TEST")

        val hugeFile = tempFolder.newFile("huge_banner.png")
        val size = 4 * 1024 * 1024 + 1024 // 4MB + 1KB
        val bytes = ByteArray(size) { (it % 100).toByte() }
        FileOutputStream(hugeFile).use { it.write(bytes) }

        val uri = Uri.fromFile(hugeFile)
        val resolved = syncManager.resolveUri(uri)

        assertTrue("Images > 4 MB must resolve to ClipContent.FilePayload", resolved is ClipContent.FilePayload)
        val fp = resolved as ClipContent.FilePayload
        assertEquals("huge_banner.png", fp.fileName)
        assertEquals("image/png", fp.mimeType)
        assertEquals(size.toLong(), fp.fileSize)
    }

    @Test
    fun testSendClipboardImageFlow() = runBlocking {
        val viewModel = DashboardViewModel(app, db)
        val server = ServerSocket(0)
        val port = server.localPort

        val imgFile = tempFolder.newFile("test_inline.png")
        val imgBytes = ByteArray(1024) { 0x55 }
        FileOutputStream(imgFile).use { it.write(imgBytes) }

        clipboardManager.setPrimaryClip(ClipData.newUri(app.contentResolver, "img", Uri.fromFile(imgFile)))

        val thread = Thread {
            try {
                val sock = server.accept()
                val reader = BufferedReader(InputStreamReader(sock.getInputStream()))
                while (true) {
                    val line = reader.readLine() ?: break
                    if (line.isEmpty()) break
                }
                sock.getOutputStream().write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                sock.getOutputStream().flush()
                sock.close()
            } catch (_: Exception) {}
        }
        thread.start()

        val result = viewModel.sendClipboard(app, targetHost = "127.0.0.1", targetPort = port)
        assertTrue("Expected SendClipboardResult.Success for image", result is SendClipboardResult.Success)
        val success = result as SendClipboardResult.Success
        assertTrue(success.summary.contains("Image"))
        assertFalse("isSendingClipboard must be false", viewModel.uiState.value.isSendingClipboard)

        server.close()
    }

    // =========================================================================
    // 4. FAULTY URIS & ADVERSARIAL CONTENT RESOLVER MOCKS
    // =========================================================================

    // Simulated Provider that throws SecurityException on query()
    class QuerySecurityDenyingProvider : ContentProvider() {
        override fun onCreate(): Boolean = true
        override fun getType(uri: Uri): String = "image/png"
        override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? {
            throw SecurityException("Security policy blocked query on $uri")
        }
        override fun insert(uri: Uri, values: ContentValues?): Uri? = null
        override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0
        override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = 0
    }

    // Simulated Provider that throws SecurityException on openInputStream()
    class StreamSecurityDenyingProvider : ContentProvider() {
        override fun onCreate(): Boolean = true
        override fun getType(uri: Uri): String = "image/png"
        override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? {
            val cursor = MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE))
            cursor.addRow(arrayOf("secret_photo.png", 2048L))
            return cursor
        }
        override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? {
            throw SecurityException("Security policy blocked opening stream on $uri")
        }
        override fun insert(uri: Uri, values: ContentValues?): Uri? = null
        override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0
        override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = 0
    }

    // Simulated Provider that returns corrupted/null stream
    class NullStreamProvider : ContentProvider() {
        override fun onCreate(): Boolean = true
        override fun getType(uri: Uri): String = "image/png"
        override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? {
            val cursor = MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE))
            cursor.addRow(arrayOf("null_stream.png", 1024L))
            return cursor
        }
        override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? = null
        override fun insert(uri: Uri, values: ContentValues?): Uri? = null
        override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0
        override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = 0
    }

    @Test
    fun testSecurityExceptionDuringQueryReturnsErrorGracefully() {
        Robolectric.buildContentProvider(QuerySecurityDenyingProvider::class.java).create("query.denied")
        val syncManager = ClipboardSyncManager(app, "DEVICE_SEC")

        val uri = Uri.parse("content://query.denied/image.png")
        val resolved = syncManager.resolveUri(uri)

        assertTrue("Query SecurityException must return ClipContent.Error", resolved is ClipContent.Error)
        val err = resolved as ClipContent.Error
        assertTrue("Error message must indicate permission denial", err.message.contains("Permission denied"))
    }

    @Test
    fun testSecurityExceptionDuringStreamReturnsErrorGracefully() {
        Robolectric.buildContentProvider(StreamSecurityDenyingProvider::class.java).create("stream.denied")
        val syncManager = ClipboardSyncManager(app, "DEVICE_SEC")

        val uri = Uri.parse("content://stream.denied/secret_photo.png")
        val resolved = syncManager.resolveUri(uri)

        assertTrue("Stream SecurityException must return ClipContent.Error", resolved is ClipContent.Error)
        val err = resolved as ClipContent.Error
        assertTrue("Error message must indicate permission denial", err.message.contains("Permission denied"))
    }

    @Test
    fun testNullStreamReturnsErrorGracefully() {
        Robolectric.buildContentProvider(NullStreamProvider::class.java).create("stream.null")
        val syncManager = ClipboardSyncManager(app, "DEVICE_SEC")

        val uri = Uri.parse("content://stream.null/null_stream.png")
        val resolved = syncManager.resolveUri(uri)

        assertTrue("Null stream must return ClipContent.Error", resolved is ClipContent.Error)
        val err = resolved as ClipContent.Error
        assertTrue("Error must describe inability to open stream", err.message.contains("Cannot open input stream") || err.message.contains("Failed to read image stream"))
    }

    @Test
    fun testMalformedAndNonExistentUris() {
        val syncManager = ClipboardSyncManager(app, "DEVICE_TEST")

        // Non-existent file URI
        val bogusFileUri = Uri.parse("file:///non_existent_directory_xyz/ghost_file.bin")
        val resolvedBogus = syncManager.resolveUri(bogusFileUri)
        // File does not exist, so it should resolve to FilePayload with size 0 or Error
        assertTrue(resolvedBogus is ClipContent.FilePayload || resolvedBogus is ClipContent.Error)

        // Custom protocol URI
        val customUri = Uri.parse("customproto://some/resource")
        val resolvedCustom = syncManager.resolveUri(customUri)
        assertTrue(resolvedCustom is ClipContent.FilePayload || resolvedCustom is ClipContent.Error)

        // Text clip containing malformed content URI string
        val textClip = ClipData.newPlainText("malformed", "content://///invalid uri line\nSecond line")
        val resolvedText = syncManager.resolveClip(textClip)
        assertNotNull(resolvedText)
    }

    @Test
    fun testDashboardViewModelHandlesContentResolverErrorWithoutCrashing() = runBlocking {
        Robolectric.buildContentProvider(QuerySecurityDenyingProvider::class.java).create("query.denied.vm")
        val uri = Uri.parse("content://query.denied.vm/denied.png")

        clipboardManager.setPrimaryClip(ClipData.newUri(app.contentResolver, "denied", uri))

        val viewModel = DashboardViewModel(app, db)
        val result = viewModel.sendClipboard(app)

        assertTrue("ViewModel sendClipboard must return SendClipboardResult.Error when access denied", result is SendClipboardResult.Error)
        val err = result as SendClipboardResult.Error
        assertTrue(err.message.contains("Permission denied"))
        assertFalse("isSendingClipboard must be reset to false", viewModel.uiState.value.isSendingClipboard)
    }

    // =========================================================================
    // 5. CONCURRENT PACKING & ECHO SUPPRESSION ADVERSARIAL STRESS
    // =========================================================================

    @Test
    fun testConcurrentPackImageStress() {
        val syncManager = ClipboardSyncManager(app, "DEVICE_CONCURRENT")
        val threads = 16
        val iterationsPerThread = 25
        val executor = Executors.newFixedThreadPool(threads)
        val latch = CountDownLatch(threads)
        val failureFlag = AtomicBoolean(false)

        for (t in 0 until threads) {
            executor.submit {
                try {
                    for (i in 0 until iterationsPerThread) {
                        val size = if (i % 2 == 0) 1024 else 8192
                        val bytes = ByteArray(size) { (it + t + i).toByte() }
                        val packet = syncManager.packImage(bytes, "image/png")
                        if (size <= 4096) {
                            if (packet !is ClipboardInlinePacket) failureFlag.set(true)
                        } else {
                            if (packet !is ClipboardDescriptorPacket) failureFlag.set(true)
                        }
                    }
                } catch (_: Exception) {
                    failureFlag.set(true)
                } finally {
                    latch.countDown()
                }
            }
        }

        assertTrue("Concurrent packing must complete within 10 seconds", latch.await(10, TimeUnit.SECONDS))
        assertFalse("No thread should have encountered corruption or failure", failureFlag.get())
        executor.shutdown()
    }

    @Test
    fun testEchoSuppressionAdversarialCases() {
        val syncManager = ClipboardSyncManager(app, "DEVICE_ECHO")

        // Upper vs lower case SHA-256 hash
        val testHashLower = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        val testHashUpper = testHashLower.uppercase()

        syncManager.setLastPastedHash(testHashLower)
        assertTrue("Lowercase check must match", syncManager.isRecentHash(testHashLower))
        assertTrue("Uppercase check must match (case-insensitive)", syncManager.isRecentHash(testHashUpper))

        syncManager.setLastPastedHash(testHashUpper)
        assertTrue("Lowercase check must match when stored upper", syncManager.isRecentHash(testHashLower))
        assertTrue("Uppercase check must match when stored upper", syncManager.isRecentHash(testHashUpper))

        // Non-matching hash
        assertFalse(syncManager.isRecentHash("0000000000000000000000000000000000000000000000000000000000000000"))

        // Null clearing
        syncManager.setLastPastedHash(null)
        assertFalse(syncManager.isRecentHash(testHashLower))
    }
}
