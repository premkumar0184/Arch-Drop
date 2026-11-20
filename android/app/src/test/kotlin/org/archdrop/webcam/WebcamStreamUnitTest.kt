package org.archdrop.webcam

import android.app.Application
import android.graphics.ImageFormat
import android.graphics.Rect
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageProxy
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.BufferedReader
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.InputStreamReader
import java.io.OutputStream
import java.lang.reflect.InvocationHandler
import java.lang.reflect.Proxy
import java.net.ServerSocket
import java.net.Socket
import java.nio.ByteBuffer
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

@RunWith(RobolectricTestRunner::class)
class WebcamStreamUnitTest {

    // =========================================================================
    // 1. ReusableByteArrayOutputStream Tests
    // =========================================================================

    @Test
    fun testReusableByteArrayOutputStream_initialCapacityAndWrite() {
        val initialCap = 1024
        val stream = ReusableByteArrayOutputStream(initialCap)

        assertEquals(0, stream.size())
        assertEquals(0, stream.dataLength())
        assertTrue("Underlying buffer must be at least initial capacity", stream.toByteArrayBuffer().size >= initialCap)

        val testData = ByteArray(100) { (it and 0xFF).toByte() }
        stream.write(testData)

        assertEquals(100, stream.size())
        assertEquals(100, stream.dataLength())

        val buf = stream.toByteArrayBuffer()
        for (i in 0 until 100) {
            assertEquals(testData[i], buf[i])
        }
    }

    @Test
    fun testReusableByteArrayOutputStream_resetWithoutReallocating() {
        val stream = ReusableByteArrayOutputStream(2048)
        val initialBufferRef = stream.toByteArrayBuffer()

        val payload1 = ByteArray(150) { 1.toByte() }
        stream.write(payload1)
        assertEquals(150, stream.size())
        assertSame("Buffer reference must remain identical after write", initialBufferRef, stream.toByteArrayBuffer())

        stream.reset()
        assertEquals(0, stream.size())
        assertEquals(0, stream.dataLength())
        assertSame("Buffer reference must not reallocate on reset", initialBufferRef, stream.toByteArrayBuffer())

        val payload2 = ByteArray(80) { 2.toByte() }
        stream.write(payload2)
        assertEquals(80, stream.size())
        assertSame("Buffer reference must remain identical after subsequent writes", initialBufferRef, stream.toByteArrayBuffer())
        for (i in 0 until 80) {
            assertEquals(2.toByte(), stream.toByteArrayBuffer()[i])
        }
    }

    @Test
    fun testReusableByteArrayOutputStream_expansionBeyondInitialCapacity() {
        val stream = ReusableByteArrayOutputStream(32)
        val largeData = ByteArray(256) { (it * 3).toByte() }
        stream.write(largeData)

        assertEquals(256, stream.size())
        assertTrue("Buffer must expand to accommodate large data", stream.toByteArrayBuffer().size >= 256)
        for (i in 0 until 256) {
            assertEquals(largeData[i], stream.toByteArrayBuffer()[i])
        }
    }

    // =========================================================================
    // 2. RollingFpsCalculator Tests
    // =========================================================================

    @Test
    fun testRollingFpsCalculator_initialStateAndReset() {
        val calc = RollingFpsCalculator(capacity = 60, windowMs = 1000L)
        assertEquals(0.0, calc.getFps(), 0.001)

        calc.onFrame()
        // Single frame within window cannot determine interval
        assertEquals(0.0, calc.getFps(), 0.001)

        calc.reset()
        assertEquals(0.0, calc.getFps(), 0.001)
    }

    @Test
    fun testRollingFpsCalculator_accurateFpsTracking() {
        val calc = RollingFpsCalculator(capacity = 60, windowMs = 1000L)

        // Record 10 frames in rapid succession
        for (i in 0 until 10) {
            calc.onFrame()
            Thread.sleep(10) // ~10ms per frame
        }

        val fps = calc.getFps()
        assertTrue("FPS should be positive, was $fps", fps > 0.0)

        calc.reset()
        assertEquals(0.0, calc.getFps(), 0.001)
    }

    // =========================================================================
    // 3. Stride-Aware YUV_420_888 to NV21 Conversion Tests
    // =========================================================================

    private fun createMockPlane(
        buffer: ByteBuffer,
        pixelStride: Int,
        rowStride: Int
    ): ImageProxy.PlaneProxy {
        val handler = InvocationHandler { _, method, _ ->
            when (method.name) {
                "getBuffer" -> buffer
                "getPixelStride" -> pixelStride
                "getRowStride" -> rowStride
                else -> null
            }
        }
        return Proxy.newProxyInstance(
            ImageProxy.PlaneProxy::class.java.classLoader,
            arrayOf(ImageProxy.PlaneProxy::class.java),
            handler
        ) as ImageProxy.PlaneProxy
    }

    private fun createMockImageProxy(
        width: Int,
        height: Int,
        planes: Array<ImageProxy.PlaneProxy>,
        format: Int = ImageFormat.YUV_420_888,
        onClose: (() -> Unit)? = null
    ): ImageProxy {
        val handler = InvocationHandler { _, method, _ ->
            when (method.name) {
                "getWidth" -> width
                "getHeight" -> height
                "getFormat" -> format
                "getPlanes" -> planes
                "close" -> {
                    onClose?.invoke()
                    null
                }
                "getCropRect" -> Rect(0, 0, width, height)
                "toString" -> "MockImageProxy(${width}x${height})"
                else -> null
            }
        }
        return Proxy.newProxyInstance(
            ImageProxy::class.java.classLoader,
            arrayOf(ImageProxy::class.java),
            handler
        ) as ImageProxy
    }

    @Test
    fun testYuv420ToNv21_contiguousPlanar() {
        val width = 4
        val height = 4
        // Y: 16 bytes
        val yData = ByteArray(16) { it.toByte() }
        val yBuffer = ByteBuffer.wrap(yData)
        val yPlane = createMockPlane(yBuffer, pixelStride = 1, rowStride = 4)

        // Chroma: 2x2 = 4 pixels each
        // NV21 requires V then U
        val uData = byteArrayOf(10, 11, 12, 13)
        val vData = byteArrayOf(20, 21, 22, 23)
        val uBuffer = ByteBuffer.wrap(uData)
        val vBuffer = ByteBuffer.wrap(vData)
        val uPlane = createMockPlane(uBuffer, pixelStride = 1, rowStride = 2)
        val vPlane = createMockPlane(vBuffer, pixelStride = 1, rowStride = 2)

        val image = createMockImageProxy(width, height, arrayOf(yPlane, uPlane, vPlane))
        val outputNv21 = ByteArray(width * height * 3 / 2) // 24 bytes

        WebcamStreamManager.yuv420ToNv21(image, outputNv21)

        // 1. Verify Y plane (first 16 bytes)
        for (i in 0 until 16) {
            assertEquals("Y[$i] mismatch", i.toByte(), outputNv21[i])
        }

        // 2. Verify UV plane (next 8 bytes, interleaved V then U: V0, U0, V1, U1...)
        val expectedUv = byteArrayOf(
            20, 10, // row 0, col 0
            21, 11, // row 0, col 1
            22, 12, // row 1, col 0
            23, 13  // row 1, col 1
        )
        for (i in 0 until 8) {
            assertEquals("UV[$i] mismatch", expectedUv[i], outputNv21[16 + i])
        }
    }

    @Test
    fun testYuv420ToNv21_withRowStridePadding() {
        val width = 4
        val height = 4
        // Y plane with rowStride = 6 (4 bytes data + 2 bytes padding per row = 24 bytes total)
        val yPadded = byteArrayOf(
            1, 2, 3, 4, 99, 99,       // Row 0
            5, 6, 7, 8, 99, 99,       // Row 1
            9, 10, 11, 12, 99, 99,    // Row 2
            13, 14, 15, 16, 99, 99    // Row 3
        )
        val yBuffer = ByteBuffer.wrap(yPadded)
        val yPlane = createMockPlane(yBuffer, pixelStride = 1, rowStride = 6)

        // U plane with rowStride = 4 (2 bytes data + 2 bytes padding per row)
        val uPadded = byteArrayOf(
            30, 31, 99, 99,
            32, 33, 99, 99
        )
        val uPlane = createMockPlane(ByteBuffer.wrap(uPadded), pixelStride = 1, rowStride = 4)

        // V plane with rowStride = 4 (2 bytes data + 2 bytes padding per row)
        val vPadded = byteArrayOf(
            40, 41, 99, 99,
            42, 43, 99, 99
        )
        val vPlane = createMockPlane(ByteBuffer.wrap(vPadded), pixelStride = 1, rowStride = 4)

        val image = createMockImageProxy(width, height, arrayOf(yPlane, uPlane, vPlane))
        val outputNv21 = ByteArray(width * height * 3 / 2)

        WebcamStreamManager.yuv420ToNv21(image, outputNv21)

        // Verify Y stripped of padding
        for (i in 0 until 16) {
            assertEquals((i + 1).toByte(), outputNv21[i])
        }

        // Verify interleaved VU without padding
        val expectedUv = byteArrayOf(
            40, 30,
            41, 31,
            42, 32,
            43, 33
        )
        for (i in 0 until 8) {
            assertEquals(expectedUv[i], outputNv21[16 + i])
        }
    }

    @Test
    fun testYuv420ToNv21_interleavedChromaPixelStride2() {
        val width = 4
        val height = 4
        val yData = ByteArray(16) { it.toByte() }
        val yPlane = createMockPlane(ByteBuffer.wrap(yData), pixelStride = 1, rowStride = 4)

        // Android CameraX often interleaves U and V in shared buffer:
        // V plane starts at V0, pixelStride = 2
        // U plane starts at U0, pixelStride = 2
        val vuInterleaved = byteArrayOf(
            50, 60, 51, 61, // Row 0: V0, U0, V1, U1
            52, 62, 53, 63  // Row 1: V2, U2, V3, U3
        )
        val vBuffer = ByteBuffer.wrap(vuInterleaved)
        val vPlane = createMockPlane(vBuffer, pixelStride = 2, rowStride = 4)

        val uvInterleaved = byteArrayOf(
            60, 51, 61, 0,  // Row 0: U0, V1, U1 ...
            62, 53, 63, 0   // Row 1: U2, V3, U3 ...
        )
        val uBuffer = ByteBuffer.wrap(uvInterleaved)
        val uPlane = createMockPlane(uBuffer, pixelStride = 2, rowStride = 4)

        val image = createMockImageProxy(width, height, arrayOf(yPlane, uPlane, vPlane))
        val outputNv21 = ByteArray(24)

        WebcamStreamManager.yuv420ToNv21(image, outputNv21)

        val expectedUv = byteArrayOf(
            50, 60,
            51, 61,
            52, 62,
            53, 63
        )
        for (i in 0 until 8) {
            assertEquals("Chroma index $i mismatch", expectedUv[i], outputNv21[16 + i])
        }
    }

    @Test(expected = IllegalArgumentException::class)
    fun testYuv420ToNv21_bufferTooSmallThrowsException() {
        val width = 4
        val height = 4
        val yPlane = createMockPlane(ByteBuffer.wrap(ByteArray(16)), 1, 4)
        val uPlane = createMockPlane(ByteBuffer.wrap(ByteArray(4)), 1, 2)
        val vPlane = createMockPlane(ByteBuffer.wrap(ByteArray(4)), 1, 2)
        val image = createMockImageProxy(width, height, arrayOf(yPlane, uPlane, vPlane))

        // Requires 24 bytes, passing 20
        WebcamStreamManager.yuv420ToNv21(image, ByteArray(20))
    }

    // =========================================================================
    // 4. Conflated Frame Queue & Zero-GC Buffer Recycling Tests
    // =========================================================================

    class FakeCompressor : FrameCompressor {
        var compressCount = 0
        override fun compress(
            nv21: ByteArray,
            width: Int,
            height: Int,
            quality: Int,
            outputStream: OutputStream
        ): Boolean {
            compressCount++
            // Output standard JPEG SOI [0xFF, 0xD8], synthetic payload, and EOI [0xFF, 0xD9]
            outputStream.write(0xFF)
            outputStream.write(0xD8)
            outputStream.write(byteArrayOf(1, 2, 3, 4, 5))
            outputStream.write(0xFF)
            outputStream.write(0xD9)
            return true
        }
    }

    @Test
    fun testProcessCameraFrame_conflatesAndRecyclesBuffers() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)
        val fakeCompressor = FakeCompressor()
        manager.compressor = fakeCompressor

        // Enable streaming flag via reflection / direct testing
        val isStreamingField = WebcamStreamManager::class.java.getDeclaredField("isStreaming").apply {
            isAccessible = true
        }
        (isStreamingField.get(manager) as AtomicBoolean).set(true)

        val closedCount = AtomicInteger(0)
        // Push 5 camera frames in rapid succession
        for (f in 0 until 5) {
            val yPlane = createMockPlane(ByteBuffer.wrap(ByteArray(16)), 1, 4)
            val uPlane = createMockPlane(ByteBuffer.wrap(ByteArray(4)), 1, 2)
            val vPlane = createMockPlane(ByteBuffer.wrap(ByteArray(4)), 1, 2)
            val image = createMockImageProxy(4, 4, arrayOf(yPlane, uPlane, vPlane)) {
                closedCount.incrementAndGet()
            }
            manager.processCameraFrame(image)
        }

        // 1. All 5 images must be closed immediately
        assertEquals("All 5 CameraX ImageProxy instances must be closed", 5, closedCount.get())
        assertEquals("Compressor should have been called 5 times", 5, fakeCompressor.compressCount)

        // 2. Conflated slot should hold exactly 1 latest frame
        val latestReadyField = WebcamStreamManager::class.java.getDeclaredField("latestReadyFrame").apply {
            isAccessible = true
        }
        val latestRef = latestReadyField.get(manager) as java.util.concurrent.atomic.AtomicReference<*>
        val frame = latestRef.get()
        assertNotNull("Latest ready frame must be present in conflated slot", frame)

        manager.stopStreaming()
    }

    // =========================================================================
    // 5. OkHttp Chunked Multipart Network Streaming Tests
    // =========================================================================

    @Test
    fun testMultipartStreamingEgress_formatsValidBoundariesAndHeaders() = runBlocking {
        val server = ServerSocket(0)
        val port = server.localPort

        val requestMethod = AtomicReference<String>()
        val requestPath = AtomicReference<String>()
        val contentTypeHeader = AtomicReference<String>()
        val fingerprintHeader = AtomicReference<String>()
        val receivedFrames = mutableListOf<ByteArray>()
        val serverDone = CountDownLatch(1)

        val serverThread = Thread {
            try {
                server.soTimeout = 5000
                val socket = server.accept()
                val input = socket.getInputStream()
                fun readLineBytes(): String? {
                    val sb = StringBuilder()
                    while (true) {
                        val b = input.read()
                        if (b == -1) return if (sb.isEmpty()) null else sb.toString()
                        if (b == '\n'.code) {
                            val str = sb.toString()
                            return if (str.endsWith("\r")) str.dropLast(1) else str
                        }
                        sb.append(b.toChar())
                    }
                }

                // 1. Parse HTTP request line
                val requestLine = readLineBytes()
                if (requestLine != null) {
                    val parts = requestLine.split(" ")
                    requestMethod.set(parts[0])
                    requestPath.set(parts[1])
                }

                // 2. Parse headers
                var line: String?
                while (readLineBytes().also { line = it } != null) {
                    if (line!!.isEmpty()) break
                    if (line!!.startsWith("Content-Type:", ignoreCase = true)) {
                        contentTypeHeader.set(line!!.substringAfter(":").trim())
                    }
                    if (line!!.startsWith("X-Fingerprint:", ignoreCase = true)) {
                        fingerprintHeader.set(line!!.substringAfter(":").trim())
                    }
                }

                // 3. Read raw stream body bytes to detect multipart boundary
                val buffer = ByteArray(4096)
                val outBytes = ByteArrayOutputStream()
                val start = System.currentTimeMillis()
                while (System.currentTimeMillis() - start < 3000) {
                    if (input.available() > 0) {
                        val n = input.read(buffer)
                        if (n > 0) {
                            outBytes.write(buffer, 0, n)
                            if (outBytes.toString(Charsets.UTF_8.name()).contains("--frame--")) {
                                break
                            }
                        }
                    } else {
                        Thread.sleep(20)
                    }
                }

                val bodyStr = outBytes.toString(Charsets.UTF_8.name())
                // Send response
                val out = socket.getOutputStream()
                out.write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                socket.close()

                if (bodyStr.contains("--frame")) {
                    receivedFrames.add(outBytes.toByteArray())
                }
            } catch (e: Exception) {
                // Ignore socket closures
            } finally {
                serverDone.countDown()
                server.close()
            }
        }
        serverThread.start()

        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)

        // Start network streaming loop directly via reflection
        val isStreamingField = WebcamStreamManager::class.java.getDeclaredField("isStreaming").apply {
            isAccessible = true
        }
        (isStreamingField.get(manager) as AtomicBoolean).set(true)

        val runLoopMethod = WebcamStreamManager::class.java.getDeclaredMethod(
            "runNetworkStreamingLoop",
            String::class.java,
            Int::class.javaPrimitiveType,
            String::class.java
        ).apply { isAccessible = true }

        val networkThread = Thread {
            runLoopMethod.invoke(manager, "127.0.0.1", port, "TEST_FINGERPRINT_SHA256")
        }
        networkThread.start()

        // Give connection a moment to establish
        Thread.sleep(100)

        // Send two direct JPEG frames
        val fakeJpeg1 = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xAA.toByte(), 0xBB.toByte(), 0xFF.toByte(), 0xD9.toByte())
        manager.sendDirectFrame(fakeJpeg1, fakeJpeg1.size, 1731816810000L)

        Thread.sleep(100)

        val fakeJpeg2 = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xCC.toByte(), 0xDD.toByte(), 0xFF.toByte(), 0xD9.toByte())
        manager.sendDirectFrame(fakeJpeg2, fakeJpeg2.size, 1731816820000L)

        Thread.sleep(200)

        manager.stopStreaming()
        serverDone.await(3, TimeUnit.SECONDS)

        assertEquals("POST", requestMethod.get())
        assertEquals("/api/v2/webcam/stream", requestPath.get())
        assertEquals("multipart/x-mixed-replace; boundary=frame", contentTypeHeader.get())
        assertEquals("TEST_FINGERPRINT_SHA256", fingerprintHeader.get())

        assertTrue("Server should have received multipart stream body", receivedFrames.isNotEmpty())
        val bodyContent = String(receivedFrames[0], Charsets.UTF_8)

        // Verify boundary headers
        assertTrue("Body must contain initial boundary", bodyContent.contains("--frame\r\n"))
        assertTrue("Body must contain Content-Type: image/jpeg", bodyContent.contains("Content-Type: image/jpeg\r\n"))
        assertTrue("Body must contain Content-Length: 6", bodyContent.contains("Content-Length: 6\r\n"))
        assertTrue("Body must contain X-Timestamp-Ms", bodyContent.contains("X-Timestamp-Ms: 1731816810000\r\n"))
        assertTrue("Body must contain closing boundary", bodyContent.contains("--frame--\r\n"))
    }

    // =========================================================================
    // 6. Server Rejection Handling Tests (HTTP 403 Forbidden & HTTP 409 Conflict)
    // =========================================================================

    @Test
    fun testServerRejection_403Forbidden() {
        val server = ServerSocket(0)
        val port = server.localPort

        val serverThread = Thread {
            try {
                val socket = server.accept()
                val reader = BufferedReader(InputStreamReader(socket.getInputStream()))
                while (reader.readLine()?.isNotEmpty() == true) {}
                val out = socket.getOutputStream()
                out.write("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                socket.close()
            } catch (_: Exception) {}
            finally {
                server.close()
            }
        }
        serverThread.start()

        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)

        val isStreamingField = WebcamStreamManager::class.java.getDeclaredField("isStreaming").apply {
            isAccessible = true
        }
        (isStreamingField.get(manager) as AtomicBoolean).set(true)

        val runLoopMethod = WebcamStreamManager::class.java.getDeclaredMethod(
            "runNetworkStreamingLoop",
            String::class.java,
            Int::class.javaPrimitiveType,
            String::class.java
        ).apply { isAccessible = true }

        val networkThread = Thread {
            runLoopMethod.invoke(manager, "127.0.0.1", port, "UNPAIRED_FP")
        }
        networkThread.start()
        Thread.sleep(100)

        // Push frame to trigger network write against the closed/rejected socket
        val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0xD9.toByte())
        manager.sendDirectFrame(fakeJpeg, fakeJpeg.size)

        networkThread.join(2000)

        val state = manager.connectionState.value
        assertTrue("State must be Error on 403, was $state", state is WebcamConnectionState.Error)
    }

    @Test
    fun testServerRejection_409Conflict() {
        val server = ServerSocket(0)
        val port = server.localPort

        val serverThread = Thread {
            try {
                val socket = server.accept()
                val reader = BufferedReader(InputStreamReader(socket.getInputStream()))
                while (reader.readLine()?.isNotEmpty() == true) {}
                val out = socket.getOutputStream()
                out.write("HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                socket.close()
            } catch (_: Exception) {}
            finally {
                server.close()
            }
        }
        serverThread.start()

        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)

        val isStreamingField = WebcamStreamManager::class.java.getDeclaredField("isStreaming").apply {
            isAccessible = true
        }
        (isStreamingField.get(manager) as AtomicBoolean).set(true)

        val runLoopMethod = WebcamStreamManager::class.java.getDeclaredMethod(
            "runNetworkStreamingLoop",
            String::class.java,
            Int::class.javaPrimitiveType,
            String::class.java
        ).apply { isAccessible = true }

        val networkThread = Thread {
            runLoopMethod.invoke(manager, "127.0.0.1", port, "PAIRED_FP")
        }
        networkThread.start()
        Thread.sleep(100)

        val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0xD9.toByte())
        manager.sendDirectFrame(fakeJpeg, fakeJpeg.size)

        networkThread.join(2000)

        val state = manager.connectionState.value
        assertTrue("State must be Error on 409, was $state", state is WebcamConnectionState.Error)
    }

    // =========================================================================
    // 7. Enums, Resolution & Buffer Allocation Tests
    // =========================================================================

    @Test
    fun testWebcamResolutionAndLensEnums() {
        assertEquals(640, WebcamResolution.RES_480P.width)
        assertEquals(480, WebcamResolution.RES_480P.height)

        assertEquals(1280, WebcamResolution.RES_720P.width)
        assertEquals(720, WebcamResolution.RES_720P.height)

        assertEquals(1920, WebcamResolution.RES_1080P.width)
        assertEquals(1080, WebcamResolution.RES_1080P.height)

        assertEquals(CameraSelector.DEFAULT_BACK_CAMERA, WebcamLens.BACK.cameraSelector)
        assertEquals(CameraSelector.DEFAULT_FRONT_CAMERA, WebcamLens.FRONT.cameraSelector)
    }

    @Test
    fun testEnsureNv21BufferAllocation() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)

        val buf1 = manager.ensureNv21Buffer(1280, 720)
        assertEquals(1280 * 720 * 3 / 2, buf1.size)

        // Calling again with same resolution must reuse the exact same array (0 allocation)
        val buf2 = manager.ensureNv21Buffer(1280, 720)
        assertSame("Buffer must be reused without reallocation when resolution matches", buf1, buf2)

        // Changing resolution allocates new size
        val buf3 = manager.ensureNv21Buffer(640, 480)
        assertEquals(640 * 480 * 3 / 2, buf3.size)
    }

    @Test
    fun testCalculateCropRect_tallerImageCroppedTo16x9() {
        // Sensor delivering 4:3 (1280x960) when 720p HD (16:9) is requested
        val rect = WebcamStreamManager.calculateCropRect(1280, 960, WebcamResolution.RES_720P)
        assertEquals(0, rect.left)
        assertEquals(120, rect.top)
        assertEquals(1280, rect.right)
        assertEquals(840, rect.bottom)
        assertEquals(1280, rect.width())
        assertEquals(720, rect.height())
        assertEquals("Coordinates must be even for YUV420 subsampling", 0, rect.top % 2)
        assertEquals("Coordinates must be even for YUV420 subsampling", 0, rect.bottom % 2)
    }

    @Test
    fun testCalculateCropRect_exactMatch_preservesDimensions() {
        val rect = WebcamStreamManager.calculateCropRect(1280, 720, WebcamResolution.RES_720P)
        assertEquals(0, rect.left)
        assertEquals(0, rect.top)
        assertEquals(1280, rect.right)
        assertEquals(720, rect.bottom)
        assertEquals(1280, rect.width())
        assertEquals(720, rect.height())
    }

    @Test
    fun testCalculateCropRect_480pExactMatch() {
        val rect = WebcamStreamManager.calculateCropRect(640, 480, WebcamResolution.RES_480P)
        assertEquals(0, rect.left)
        assertEquals(0, rect.top)
        assertEquals(640, rect.right)
        assertEquals(480, rect.bottom)
    }
}
