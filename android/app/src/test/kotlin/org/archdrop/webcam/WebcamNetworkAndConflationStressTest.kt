package org.archdrop.webcam

import android.app.Application
import android.graphics.ImageFormat
import android.graphics.Rect
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
import java.io.InputStreamReader
import java.io.OutputStream
import java.lang.reflect.Field
import java.lang.reflect.InvocationHandler
import java.lang.reflect.Method
import java.lang.reflect.Proxy
import java.net.ServerSocket
import java.net.Socket
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

/**
 * Adversarial Challenger 2 Stress Harness for Phase 6 Milestone 2:
 *
 * 1. Conflated Frame Queue & Buffer Recycling: 60 FPS Camera vs 5 FPS Network Consumer.
 * 2. Timestamp Monotonicity & Dropping of Stale Backlog.
 * 3. OkHttp Multipart Egress & Strict RFC Boundary Formatting.
 * 4. Error Handling & Abrupt Socket Disconnects (TCP RST / FIN).
 * 5. Rapid Start/Stop Lifecycle Churn.
 * 6. Server Rejection Codes (HTTP 403 Forbidden & HTTP 409 Conflict).
 * 7. Zero Memory Leak & Buffer Pool Conservation Under High Load.
 */
@RunWith(RobolectricTestRunner::class)
class WebcamNetworkAndConflationStressTest {

    private fun getField(clazz: Class<*>, fieldName: String): Field {
        return clazz.getDeclaredField(fieldName).apply { isAccessible = true }
    }

    private fun getMethod(clazz: Class<*>, methodName: String, vararg paramTypes: Class<*>): Method {
        return clazz.getDeclaredMethod(methodName, *paramTypes).apply { isAccessible = true }
    }

    private fun setIsStreaming(manager: WebcamStreamManager, active: Boolean) {
        val field = getField(WebcamStreamManager::class.java, "isStreaming")
        (field.get(manager) as AtomicBoolean).set(active)
    }

    private fun getBufferPool(manager: WebcamStreamManager): ConcurrentLinkedQueue<FrameBuffer> {
        val field = getField(WebcamStreamManager::class.java, "bufferPool")
        @Suppress("UNCHECKED_CAST")
        return field.get(manager) as ConcurrentLinkedQueue<FrameBuffer>
    }

    private fun getLatestReadyFrame(manager: WebcamStreamManager): AtomicReference<FrameBuffer?> {
        val field = getField(WebcamStreamManager::class.java, "latestReadyFrame")
        @Suppress("UNCHECKED_CAST")
        return field.get(manager) as AtomicReference<FrameBuffer?>
    }

    private fun getFrameSignal(manager: WebcamStreamManager): Semaphore {
        val field = getField(WebcamStreamManager::class.java, "frameSignal")
        return field.get(manager) as Semaphore
    }

    // =========================================================================
    // 1. Asymmetric Rate Stress: 60 FPS Camera vs 5 FPS Network Consumer
    // =========================================================================

    @Test
    fun testConflatedHandoff_60FpsCameraVs5FpsNetworkConsumer() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)
        setIsStreaming(manager, true)

        val totalCameraFrames = 60
        val cameraFrameIntervalMs = 16L // ~60 FPS
        val networkFrameProcessingMs = 150L // ~6-7 FPS consumer

        val transmittedTimestamps = mutableListOf<Long>()
        val transmittedLock = Any()
        val networkRunning = AtomicBoolean(true)
        val networkFinishedLatch = CountDownLatch(1)

        val bufferPool = getBufferPool(manager)
        val latestReady = getLatestReadyFrame(manager)
        val frameSignal = getFrameSignal(manager)

        // Initial pool size must be 3
        assertEquals("Buffer pool should start with 3 preallocated buffers", 3, bufferPool.size)

        // Mock network consumer thread simulating slow Wi-Fi
        val networkThread = Thread {
            try {
                while (networkRunning.get() || frameSignal.availablePermits() > 0 || latestReady.get() != null) {
                    val acquired = frameSignal.tryAcquire(50, TimeUnit.MILLISECONDS)
                    if (!acquired) {
                        if (!networkRunning.get() && latestReady.get() == null) break
                        continue
                    }

                    val frame = latestReady.getAndSet(null) ?: continue
                    synchronized(transmittedLock) {
                        transmittedTimestamps.add(frame.timestampMs)
                    }

                    // Simulate slow network write (e.g., 150ms)
                    Thread.sleep(networkFrameProcessingMs)

                    // Return buffer to pool
                    val releaseMethod = getMethod(WebcamStreamManager::class.java, "releaseFrameBuffer", FrameBuffer::class.java)
                    releaseMethod.invoke(manager, frame)
                }
            } finally {
                networkFinishedLatch.countDown()
            }
        }
        networkThread.start()

        // Producer: Camera emitting 60 frames in rapid sequence
        val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x11, 0x22, 0xFF.toByte(), 0xD9.toByte())
        val startTime = 10_000_000L

        for (i in 0 until totalCameraFrames) {
            val frameTs = startTime + i * cameraFrameIntervalMs
            manager.sendDirectFrame(fakeJpeg, fakeJpeg.size, frameTs)
            Thread.sleep(cameraFrameIntervalMs)
        }

        // Signal producer done
        networkRunning.set(false)
        networkFinishedLatch.await(5, TimeUnit.SECONDS)

        manager.stopStreaming()

        // Empirical Assertions:
        // 1. Slow consumer should have transmitted only a fraction of total 60 frames (e.g. 5 to 15 frames)
        assertTrue(
            "Slow consumer must have conflated and dropped frames. Transmitted: ${transmittedTimestamps.size}",
            transmittedTimestamps.size in 5..18
        )

        // 2. Transmitted timestamps must be strictly monotonic
        for (i in 1 until transmittedTimestamps.size) {
            assertTrue(
                "Timestamps must be monotonically increasing: ${transmittedTimestamps[i]} > ${transmittedTimestamps[i - 1]}",
                transmittedTimestamps[i] > transmittedTimestamps[i - 1]
            )
        }

        // 3. The final transmitted frame should be one of the last generated frames (conflation delivered the latest)
        val lastTransmitted = transmittedTimestamps.last()
        val expectedLatestTs = startTime + (totalCameraFrames - 1) * cameraFrameIntervalMs
        assertTrue(
            "Last transmitted frame ($lastTransmitted) must be near the latest camera frame ($expectedLatestTs)",
            lastTransmitted >= expectedLatestTs - (cameraFrameIntervalMs * 5)
        )

        // 4. Memory leak / buffer pool conservation:
        // After stopStreaming(), any ready frame is recycled and all 3 buffers must be returned to pool
        assertEquals("Buffer pool must be fully restored without leak", 3, bufferPool.size)
    }

    // =========================================================================
    // 2. Stale Frame Dropping & Zero-Leak Buffer Recycle Verification
    // =========================================================================

    @Test
    fun testBufferPool_unconsumedFramesAreRecycledImmediately() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)
        setIsStreaming(manager, true)

        val bufferPool = getBufferPool(manager)
        val initialPoolSize = bufferPool.size
        assertEquals(3, initialPoolSize)

        val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x01, 0xFF.toByte(), 0xD9.toByte())

        // Push 20 frames without any network consumer pulling
        for (i in 1..20) {
            manager.sendDirectFrame(fakeJpeg, fakeJpeg.size, 1000L + i)
        }

        // 1. Exactly 1 frame is in latestReadyFrame
        val latestReady = getLatestReadyFrame(manager).get()
        assertNotNull("Latest ready frame must be present", latestReady)
        assertEquals(1020L, latestReady!!.timestampMs)

        // 2. The other 2 buffers must remain in the pool (since 1 is currently in latestReady)
        assertEquals("Remaining 2 buffers must be in bufferPool", 2, bufferPool.size)

        // 3. Stop streaming must recycle the remaining ready frame
        manager.stopStreaming()
        assertEquals("Buffer pool must have all 3 buffers back after stopStreaming()", 3, bufferPool.size)
    }

    // =========================================================================
    // 3. Wire-Level OkHttp Multipart Egress & Strict RFC Boundary Formatting
    // =========================================================================

    @Test
    fun testMultipartEgress_strictBoundaryAndHeaderConformance() = runBlocking {
        val server = ServerSocket(0).also { it.soTimeout = 5000 }
        val port = server.localPort

        val rawReceivedBytes = ByteArrayOutputStream()
        val serverLatch = CountDownLatch(1)

        val serverThread = Thread {
            try {
                server.soTimeout = 4000
                val socket = server.accept()
                val input = socket.getInputStream()

                // Read full request including multiple frames and closing boundary
                val buffer = ByteArray(2048)
                val start = System.currentTimeMillis()
                while (System.currentTimeMillis() - start < 6000) {
                    if (input.available() > 0) {
                        val n = input.read(buffer)
                        if (n > 0) {
                            rawReceivedBytes.write(buffer, 0, n)
                            val str = rawReceivedBytes.toString(Charsets.UTF_8.name())
                            if (str.contains("--frame--\r\n")) {
                                break
                            }
                        }
                    } else {
                        Thread.sleep(15)
                    }
                }

                // Send 200 OK response
                val out = socket.getOutputStream()
                out.write("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                socket.close()
            } catch (_: Exception) {
            } finally {
                serverLatch.countDown()
                server.close()
            }
        }
        serverThread.start()

        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)
        setIsStreaming(manager, true)

        val runLoopMethod = getMethod(
            WebcamStreamManager::class.java,
            "runNetworkStreamingLoop",
            String::class.java,
            Integer.TYPE,
            String::class.java
        )

        val networkThread = Thread {
            runLoopMethod.invoke(manager, "127.0.0.1", port, "FINGERPRINT_CHALLENGE_SHA256")
        }
        networkThread.start()

        Thread.sleep(100)

        // Send 3 distinct frames with verified payloads
        val frame1 = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x10, 0x11, 0xFF.toByte(), 0xD9.toByte())
        val frame2 = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x20, 0x21, 0x22, 0xFF.toByte(), 0xD9.toByte())
        val frame3 = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x30, 0xFF.toByte(), 0xD9.toByte())

        manager.sendDirectFrame(frame1, frame1.size, 1731819001000L)
        Thread.sleep(60)
        manager.sendDirectFrame(frame2, frame2.size, 1731819002000L)
        Thread.sleep(60)
        manager.sendDirectFrame(frame3, frame3.size, 1731819003000L)
        Thread.sleep(150)

        manager.stopStreaming()
        serverLatch.await(6, TimeUnit.SECONDS)

        val streamOutput = rawReceivedBytes.toString(Charsets.UTF_8.name())

        // 1. Verify HTTP Request Line and Headers
        assertTrue("Request must be POST /api/v2/webcam/stream", streamOutput.contains("POST /api/v2/webcam/stream HTTP/1.1\r\n"))
        assertTrue("Request must have multipart content type", streamOutput.contains("Content-Type: multipart/x-mixed-replace; boundary=frame\r\n"))
        assertTrue("Request must include X-Fingerprint", streamOutput.contains("X-Fingerprint: FINGERPRINT_CHALLENGE_SHA256\r\n"))
        assertTrue("Request must be chunked", streamOutput.contains("Transfer-Encoding: chunked\r\n") || streamOutput.contains("transfer-encoding: chunked\r\n"))

        // 2. Verify frame headers and boundary structure
        assertTrue("Must contain opening boundary --frame\\r\\n", streamOutput.contains("--frame\r\n"))
        assertTrue("Must contain Content-Type: image/jpeg\\r\\n", streamOutput.contains("Content-Type: image/jpeg\r\n"))
        assertTrue("Must contain X-Timestamp-Ms", streamOutput.contains("X-Timestamp-Ms: 1731819001000\r\n"))
        assertTrue("Must contain Content-Length: 6\\r\\n", streamOutput.contains("Content-Length: 6\r\n"))

        // 3. Verify clean closing boundary
        assertTrue("Must contain closing boundary --frame--\\r\\n", streamOutput.contains("--frame--\r\n"))
    }

    // =========================================================================
    // 4. Abrupt Socket Drop & Teardown Resilience
    // =========================================================================

    @Test
    fun testAbruptSocketDrop_transitionsToErrorGracefully() {
        val server = ServerSocket(0).also { it.soTimeout = 5000 }
        val port = server.localPort

        val serverThread = Thread {
            try {
                val socket = server.accept()
                val reader = BufferedReader(InputStreamReader(socket.getInputStream()))
                // Read HTTP request headers
                while (reader.readLine()?.isNotEmpty() == true) {}
                // Abruptly close socket from server side without response (simulate network drop/crash)
                Thread.sleep(50)
                socket.close()
            } catch (_: Exception) {
            } finally {
                server.close()
            }
        }
        serverThread.start()

        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)
        setIsStreaming(manager, true)

        val runLoopMethod = getMethod(
            WebcamStreamManager::class.java,
            "runNetworkStreamingLoop",
            String::class.java,
            Integer.TYPE,
            String::class.java
        )

        val networkThread = Thread {
            runLoopMethod.invoke(manager, "127.0.0.1", port, "DROP_TEST_FP")
        }
        networkThread.start()
        Thread.sleep(100)

        // Push frame to hit the closed socket
        val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xAA.toByte(), 0xFF.toByte(), 0xD9.toByte())
        manager.sendDirectFrame(fakeJpeg, fakeJpeg.size)

        networkThread.join(3000)

        // Stream manager should transition to Error state gracefully without uncaught crash
        val state = manager.connectionState.value
        assertTrue("Manager must enter Error or Disconnected state on socket drop, was $state",
            state is WebcamConnectionState.Error || state is WebcamConnectionState.Disconnected)

        assertFalse("Streaming should be terminated", manager.isStreamingActive())
    }

    // =========================================================================
    // 5. Rapid Start/Stop Churn Stress Test
    // =========================================================================

    @Test
    fun testRapidStartStopLifecycleChurn() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)

        val iterations = 15
        for (i in 0 until iterations) {
            setIsStreaming(manager, true)
            assertTrue("isStreamingActive must be true", manager.isStreamingActive())

            // Push a frame
            val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), (i and 0xFF).toByte(), 0xFF.toByte(), 0xD9.toByte())
            manager.sendDirectFrame(fakeJpeg, fakeJpeg.size, System.currentTimeMillis())

            // Immediately stop
            manager.stopStreaming()
            assertFalse("isStreamingActive must be false after stop", manager.isStreamingActive())

            // Verify buffer pool integrity after each cycle
            val pool = getBufferPool(manager)
            assertEquals("Buffer pool must remain 3 buffers after cycle $i", 3, pool.size)
        }
    }

    // =========================================================================
    // 6. Server Rejection Codes: HTTP 403 & HTTP 409
    // =========================================================================

    @Test
    fun testServerRejection_403Forbidden_verifiesStateAndMessage() {
        val server = ServerSocket(0).also { it.soTimeout = 5000 }
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
        setIsStreaming(manager, true)

        val runLoopMethod = getMethod(
            WebcamStreamManager::class.java,
            "runNetworkStreamingLoop",
            String::class.java,
            Integer.TYPE,
            String::class.java
        )

        val networkThread = Thread {
            runLoopMethod.invoke(manager, "127.0.0.1", port, "UNPAIRED_PEER_FP")
        }
        networkThread.start()
        Thread.sleep(100)

        val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x55, 0xFF.toByte(), 0xD9.toByte())
        manager.sendDirectFrame(fakeJpeg, fakeJpeg.size)

        networkThread.join(2500)

        val state = manager.connectionState.value
        assertTrue("State must be Error on 403, was $state", state is WebcamConnectionState.Error)
        val errorMsg = (state as WebcamConnectionState.Error).message
        assertTrue("Error message must mention 403 / Not paired: $errorMsg", errorMsg.contains("403") || errorMsg.contains("paired"))
        assertFalse("Streaming must be stopped", manager.isStreamingActive())
    }

    @Test
    fun testServerRejection_409Conflict_verifiesStateAndMessage() {
        val server = ServerSocket(0).also { it.soTimeout = 5000 }
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
        setIsStreaming(manager, true)

        val runLoopMethod = getMethod(
            WebcamStreamManager::class.java,
            "runNetworkStreamingLoop",
            String::class.java,
            Integer.TYPE,
            String::class.java
        )

        val networkThread = Thread {
            runLoopMethod.invoke(manager, "127.0.0.1", port, "PAIRED_PEER_FP")
        }
        networkThread.start()
        Thread.sleep(100)

        val fakeJpeg = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x66, 0xFF.toByte(), 0xD9.toByte())
        manager.sendDirectFrame(fakeJpeg, fakeJpeg.size)

        networkThread.join(2500)

        val state = manager.connectionState.value
        assertTrue("State must be Error on 409, was $state", state is WebcamConnectionState.Error)
        val errorMsg = (state as WebcamConnectionState.Error).message
        assertTrue("Error message must mention 409 / session active: $errorMsg", errorMsg.contains("409") || errorMsg.contains("active"))
        assertFalse("Streaming must be stopped", manager.isStreamingActive())
    }

    // =========================================================================
    // 7. High-Volume Frame Ingestion & Buffer Pool Conservation
    // =========================================================================

    @Test
    fun testHighVolumeFramePush_zeroMemoryLeak() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val manager = WebcamStreamManager(context)
        setIsStreaming(manager, true)

        val pool = getBufferPool(manager)
        val fakeJpeg = ByteArray(50_000) { (it and 0xFF).toByte() }

        // Push 1,000 frames rapidly
        for (i in 0 until 1000) {
            manager.sendDirectFrame(fakeJpeg, fakeJpeg.size, 10000L + i)
        }

        // At any point, maximum 1 frame in latestReadyFrame, exactly 2 in pool
        val latest = getLatestReadyFrame(manager).get()
        assertNotNull("Latest ready frame must be populated", latest)
        assertEquals("Latest ready frame must be the 1000th frame", 10999L, latest!!.timestampMs)
        assertEquals("Pool must contain remaining 2 buffers", 2, pool.size)

        manager.stopStreaming()
        assertEquals("All 3 buffers must be restored to pool", 3, pool.size)
    }
}
