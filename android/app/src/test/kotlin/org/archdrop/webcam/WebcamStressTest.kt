package org.archdrop.webcam

import android.graphics.ImageFormat
import android.graphics.Rect
import androidx.camera.core.ImageProxy
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.lang.reflect.InvocationHandler
import java.lang.reflect.Proxy
import java.nio.ByteBuffer
import java.util.Random
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

@RunWith(RobolectricTestRunner::class)
class WebcamStressTest {

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
        format: Int = ImageFormat.YUV_420_888
    ): ImageProxy {
        val handler = InvocationHandler { _, method, _ ->
            when (method.name) {
                "getWidth" -> width
                "getHeight" -> height
                "getFormat" -> format
                "getPlanes" -> planes
                "close" -> null
                "getCropRect" -> Rect(0, 0, width, height)
                else -> null
            }
        }
        return Proxy.newProxyInstance(
            ImageProxy::class.java.classLoader,
            arrayOf(ImageProxy::class.java),
            handler
        ) as ImageProxy
    }

    // =========================================================================
    // 1. yuv420ToNv21 Adversarial Stress Tests
    // =========================================================================

    @Test
    fun testYuv420ToNv21_stressIrregularRowStrides() {
        val random = Random(42)
        val resolutions = listOf(
            Pair(16, 16),
            Pair(32, 24),
            Pair(64, 48),
            Pair(128, 96),
            Pair(320, 240)
        )

        for ((width, height) in resolutions) {
            // Irregular padding: rowStride > width
            val yPadding = random.nextInt(32) + 1
            val yRowStride = width + yPadding
            val yBufferSize = yRowStride * height
            val yRaw = ByteArray(yBufferSize) { (it and 0x7F).toByte() }
            val yPlane = createMockPlane(ByteBuffer.wrap(yRaw), pixelStride = 1, rowStride = yRowStride)

            val chromaWidth = width / 2
            val chromaHeight = height / 2

            val uPadding = random.nextInt(16) + 1
            val uRowStride = chromaWidth + uPadding
            val uRaw = ByteArray(uRowStride * chromaHeight) { ((it + 10) and 0x7F).toByte() }
            val uPlane = createMockPlane(ByteBuffer.wrap(uRaw), pixelStride = 1, rowStride = uRowStride)

            val vPadding = random.nextInt(16) + 1
            val vRowStride = chromaWidth + vPadding
            val vRaw = ByteArray(vRowStride * chromaHeight) { ((it + 50) and 0x7F).toByte() }
            val vPlane = createMockPlane(ByteBuffer.wrap(vRaw), pixelStride = 1, rowStride = vRowStride)

            val image = createMockImageProxy(width, height, arrayOf(yPlane, uPlane, vPlane))
            val output = ByteArray(width * height * 3 / 2)

            WebcamStreamManager.yuv420ToNv21(image, output)

            // Verify Y plane stripped of row padding
            var outYOffset = 0
            for (row in 0 until height) {
                val inRowOffset = row * yRowStride
                for (col in 0 until width) {
                    assertEquals(
                        "Mismatch in Y at ($col, $row) for res ${width}x${height}",
                        yRaw[inRowOffset + col],
                        output[outYOffset++]
                    )
                }
            }

            // Verify UV plane interleaved (V followed by U)
            var outUvOffset = width * height
            for (row in 0 until chromaHeight) {
                val vRowOffset = row * vRowStride
                val uRowOffset = row * uRowStride
                for (col in 0 until chromaWidth) {
                    val expectedV = vRaw[vRowOffset + col]
                    val expectedU = uRaw[uRowOffset + col]
                    assertEquals("Mismatch in V at ($col, $row)", expectedV, output[outUvOffset++])
                    assertEquals("Mismatch in U at ($col, $row)", expectedU, output[outUvOffset++])
                }
            }
        }
    }

    @Test
    fun testYuv420ToNv21_planarVsInterleavedChromaStress() {
        val width = 32
        val height = 32
        val chromaWidth = width / 2
        val chromaHeight = height / 2

        // Planar: pixelStride = 1
        val yData = ByteArray(width * height) { (it and 0xFF).toByte() }
        val yPlane = createMockPlane(ByteBuffer.wrap(yData), 1, width)

        val uDataPlanar = ByteArray(chromaWidth * chromaHeight) { (it + 1).toByte() }
        val vDataPlanar = ByteArray(chromaWidth * chromaHeight) { (it + 50).toByte() }
        val uPlanePlanar = createMockPlane(ByteBuffer.wrap(uDataPlanar), 1, chromaWidth)
        val vPlanePlanar = createMockPlane(ByteBuffer.wrap(vDataPlanar), 1, chromaWidth)

        val imagePlanar = createMockImageProxy(width, height, arrayOf(yPlane, uPlanePlanar, vPlanePlanar))
        val outputPlanar = ByteArray(width * height * 3 / 2)
        WebcamStreamManager.yuv420ToNv21(imagePlanar, outputPlanar)

        // Interleaved: pixelStride = 2 (VU interleaved in memory)
        val vuInterleaved = ByteArray(chromaWidth * chromaHeight * 2)
        for (i in 0 until chromaWidth * chromaHeight) {
            vuInterleaved[i * 2] = (i + 50).toByte()     // V
            vuInterleaved[i * 2 + 1] = (i + 1).toByte() // U
        }
        val uvInterleaved = ByteArray(chromaWidth * chromaHeight * 2)
        for (i in 0 until chromaWidth * chromaHeight) {
            uvInterleaved[i * 2] = (i + 1).toByte()     // U
            uvInterleaved[i * 2 + 1] = (i + 50).toByte() // V
        }

        val vPlaneInterleaved = createMockPlane(ByteBuffer.wrap(vuInterleaved), 2, chromaWidth * 2)
        val uPlaneInterleaved = createMockPlane(ByteBuffer.wrap(uvInterleaved), 2, chromaWidth * 2)

        val imageInterleaved = createMockImageProxy(width, height, arrayOf(yPlane, uPlaneInterleaved, vPlaneInterleaved))
        val outputInterleaved = ByteArray(width * height * 3 / 2)
        WebcamStreamManager.yuv420ToNv21(imageInterleaved, outputInterleaved)

        // Both planar and interleaved inputs must yield exact identical NV21 output
        assertArrayEquals("Planar vs Interleaved NV21 output must be byte-for-byte identical", outputPlanar, outputInterleaved)
    }

    @Test
    fun testYuv420ToNv21_bufferOverflowBounds() {
        val width = 16
        val height = 16
        val required = width * height * 3 / 2 // 384 bytes
        val yPlane = createMockPlane(ByteBuffer.wrap(ByteArray(256)), 1, 16)
        val uPlane = createMockPlane(ByteBuffer.wrap(ByteArray(64)), 1, 8)
        val vPlane = createMockPlane(ByteBuffer.wrap(ByteArray(64)), 1, 8)

        // 1. Output buffer too small by 1 byte must throw IllegalArgumentException
        var threwTooSmall = false
        try {
            val smallBuffer = ByteArray(required - 1)
            val image = createMockImageProxy(width, height, arrayOf(yPlane, uPlane, vPlane))
            WebcamStreamManager.yuv420ToNv21(image, smallBuffer)
        } catch (e: IllegalArgumentException) {
            threwTooSmall = true
        }
        assertTrue("Must throw IllegalArgumentException when output buffer is smaller than required", threwTooSmall)

        // 2. Fewer than 3 planes must throw IllegalArgumentException
        var threwFewerPlanes = false
        try {
            val image2Planes = createMockImageProxy(width, height, arrayOf(yPlane, uPlane))
            WebcamStreamManager.yuv420ToNv21(image2Planes, ByteArray(required))
        } catch (e: IllegalArgumentException) {
            threwFewerPlanes = true
        }
        assertTrue("Must throw IllegalArgumentException when fewer than 3 planes", threwFewerPlanes)

        // 3. Exact buffer size must succeed cleanly
        val exactBuffer = ByteArray(required)
        val imageExact = createMockImageProxy(width, height, arrayOf(yPlane, uPlane, vPlane))
        WebcamStreamManager.yuv420ToNv21(imageExact, exactBuffer)

        // 4. Oversized buffer must not corrupt trailing bytes
        val oversizedBuffer = ByteArray(required + 100) { 0x55.toByte() }
        WebcamStreamManager.yuv420ToNv21(imageExact, oversizedBuffer)
        for (i in required until required + 100) {
            assertEquals("Oversized buffer trailing bytes must not be overwritten", 0x55.toByte(), oversizedBuffer[i])
        }
    }

    // =========================================================================
    // 2. ReusableByteArrayOutputStream Stress Tests
    // =========================================================================

    @Test
    fun testReusableByteArrayOutputStream_repeatedCyclesVaryingResolutions() {
        val stream = ReusableByteArrayOutputStream(initialCapacity = 64 * 1024)
        val random = Random(123)

        // Frame sizes typical for 480p, 720p, 1080p
        val frameSizes = listOf(35_000, 85_000, 220_000)

        // Warm up stream to 1080p peak capacity
        val maxFrame = ByteArray(250_000) { (it and 0xFF).toByte() }
        stream.write(maxFrame)
        stream.reset()

        val steadyStateBuf = stream.toByteArrayBuffer()
        val steadyStateCapacity = steadyStateBuf.size
        assertTrue("Buffer must expand to at least 250 KB", steadyStateCapacity >= 250_000)

        // Run 5,000 write/reset cycles simulating streaming at 30 FPS for ~2.7 minutes
        for (cycle in 0 until 5000) {
            val size = frameSizes[cycle % frameSizes.size]
            val chunk = ByteArray(size) { ((it + cycle) and 0xFF).toByte() }

            stream.write(chunk)
            assertEquals(size, stream.dataLength())
            assertEquals(size, stream.size())

            val buf = stream.toByteArrayBuffer()
            // Verify zero allocation / resizing once steady state is reached
            assertSame("Backing array reference must remain strictly identical in steady state", steadyStateBuf, buf)
            assertEquals("Capacity must not fluctuate in steady state", steadyStateCapacity, buf.size)

            // Spot-check content integrity
            assertEquals(chunk[0], buf[0])
            assertEquals(chunk[size / 2], buf[size / 2])
            assertEquals(chunk[size - 1], buf[size - 1])

            stream.reset()
            assertEquals(0, stream.dataLength())
            assertEquals(0, stream.size())
            assertSame("Reset must not reallocate buffer", steadyStateBuf, stream.toByteArrayBuffer())
        }
    }

    @Test
    fun testReusableByteArrayOutputStream_bufferReferenceAccuracy() {
        val stream = ReusableByteArrayOutputStream(1024)
        val data = ByteArray(500) { (it * 7).toByte() }
        stream.write(data)

        val liveBuf = stream.toByteArrayBuffer()
        // Mutating liveBuf reflects in subsequent reads because it is the actual buffer
        liveBuf[10] = 42.toByte()
        assertEquals(42.toByte(), stream.toByteArrayBuffer()[10])

        // Verify standard toByteArray() produces a defensive clone, while toByteArrayBuffer() does not
        val copy = stream.toByteArray()
        assertFalse("toByteArray() must return a cloned array", liveBuf === copy)
        assertEquals(stream.dataLength(), copy.size)
    }

    // =========================================================================
    // 3. RollingFpsCalculator Stress Tests
    // =========================================================================

    @Test
    fun testRollingFpsCalculator_burstsPausesTimestampResets() {
        val calc = RollingFpsCalculator(capacity = 120, windowMs = 1000L)

        // 1. Zero frames -> 0.0 FPS
        assertEquals(0.0, calc.getFps(), 0.001)

        // 2. Single frame -> 0.0 FPS (needs interval)
        val fps1 = calc.onFrame()
        assertEquals(0.0, fps1, 0.001)

        // 3. Burst of 120 frames with tiny intervals
        for (i in 0 until 120) {
            calc.onFrame()
            Thread.sleep(2) // 2ms interval -> ~500 FPS theoretical
        }
        val burstFps = calc.getFps()
        assertTrue("Burst FPS should be positive and high, was $burstFps", burstFps > 50.0)

        // 4. Long pause (exceeding 1000ms window)
        Thread.sleep(1100)
        val pausedFps = calc.getFps()
        assertEquals("FPS must drop to 0.0 after window expires without frames", 0.0, pausedFps, 0.001)

        // 5. Subsequent frame after pause should return 0.0 (first of new window)
        val firstAfterPause = calc.onFrame()
        assertEquals("First frame after pause cannot determine interval", 0.0, firstAfterPause, 0.001)

        // 6. Reset clears internal ring buffer
        calc.onFrame()
        calc.reset()
        assertEquals(0.0, calc.getFps(), 0.001)

        // 7. Simulated timestamp reflection / rollback via reflection
        val timestampsField = RollingFpsCalculator::class.java.getDeclaredField("timestamps").apply {
            isAccessible = true
        }
        val headField = RollingFpsCalculator::class.java.getDeclaredField("head").apply {
            isAccessible = true
        }
        val countField = RollingFpsCalculator::class.java.getDeclaredField("count").apply {
            isAccessible = true
        }
        val timestamps = timestampsField.get(calc) as LongArray

        // Inject timestamps where a frame is in the future compared to computeFps
        val now = System.currentTimeMillis()
        timestamps[0] = now + 500L // 500ms in the future (NTP rollback simulation)
        timestamps[1] = now
        headField.set(calc, 2)
        countField.set(calc, 2)

        val fpsAfterRollback = calc.getFps()
        // Must not throw, divide by zero, or return NaN/Infinity
        assertFalse("FPS after timestamp rollback must not be NaN", fpsAfterRollback.isNaN())
        assertFalse("FPS after timestamp rollback must not be Infinite", fpsAfterRollback.isInfinite())
        assertTrue("FPS after timestamp rollback must be non-negative", fpsAfterRollback >= 0.0)
    }

    @Test
    fun testRollingFpsCalculator_concurrentMultiThreadedAccess() {
        val calc = RollingFpsCalculator(capacity = 120, windowMs = 1000L)
        val numThreads = 8
        val framesPerThread = 500
        val executor = Executors.newFixedThreadPool(numThreads)
        val latch = CountDownLatch(numThreads)
        val errors = AtomicInteger(0)

        for (t in 0 until numThreads) {
            executor.execute {
                try {
                    for (f in 0 until framesPerThread) {
                        calc.onFrame()
                        calc.getFps()
                    }
                } catch (e: Exception) {
                    errors.incrementAndGet()
                } finally {
                    latch.countDown()
                }
            }
        }

        assertTrue("Threads must complete within 5 seconds", latch.await(5, TimeUnit.SECONDS))
        executor.shutdown()
        assertEquals("Concurrent access to RollingFpsCalculator must produce 0 race-condition errors", 0, errors.get())
    }
}
