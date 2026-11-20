package org.archdrop.audio

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.ByteArrayInputStream
import java.io.OutputStream
import java.net.ServerSocket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import kotlin.random.Random

/**
 * Adversarial empirical stress tests authored by Challenger 2 for Milestone 2.
 * Validates streaming loop, peer fingerprint verification, volume saturation,
 * jitter ring buffer drift compensation, and parser fuzz resilience.
 */
@RunWith(RobolectricTestRunner::class)
class AudioEmpiricalStressTest {

    // =========================================================================
    // 1. Volume Boost, Saturation Clamping & Mute Zeroing
    // =========================================================================

    @Test
    fun testVolumeBoost_SaturationClampingAndBoundaries() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val receiver = AudioStreamReceiver(
            context = context,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )

        // 1. Verify clamping boundaries on volumeBoost property
        receiver.setVolumeBoost(-5.0f)
        assertEquals(1.0f, receiver.volumeBoost, 0.0001f)

        receiver.setVolumeBoost(0.0f)
        assertEquals(1.0f, receiver.volumeBoost, 0.0001f)

        receiver.setVolumeBoost(0.999f)
        assertEquals(1.0f, receiver.volumeBoost, 0.0001f)

        receiver.setVolumeBoost(2.001f)
        assertEquals(2.0f, receiver.volumeBoost, 0.0001f)

        receiver.setVolumeBoost(100.0f)
        assertEquals(2.0f, receiver.volumeBoost, 0.0001f)

        // 2. Exact 1.0f bypass optimization branch
        receiver.setVolumeBoost(1.0f)
        val pcmOriginal = ByteArray(8)
        writeShortLe(pcmOriginal, 0, 12345.toShort())
        writeShortLe(pcmOriginal, 2, (-12345).toShort())
        writeShortLe(pcmOriginal, 4, 32767.toShort())
        writeShortLe(pcmOriginal, 6, (-32768).toShort())

        val pcmCopy = pcmOriginal.clone()
        receiver.applyVolumeAndMute(pcmCopy, pcmCopy.size)
        assertArrayEquals("Gain 1.0f must leave PCM bytes completely untouched", pcmOriginal, pcmCopy)

        // 3. Saturation clamping at 2.0x boost
        receiver.setVolumeBoost(2.0f)
        val pcmTest = ByteArray(12)
        writeShortLe(pcmTest, 0, 10000.toShort())  // 10000 * 2 = 20000 (within range)
        writeShortLe(pcmTest, 2, (-10000).toShort()) // -10000 * 2 = -20000 (within range)
        writeShortLe(pcmTest, 4, 20000.toShort())  // 20000 * 2 = 40000 -> clamps to 32767
        writeShortLe(pcmTest, 6, (-20000).toShort()) // -20000 * 2 = -40000 -> clamps to -32768
        writeShortLe(pcmTest, 8, 32767.toShort())  // 32767 * 2 = 65534 -> clamps to 32767
        writeShortLe(pcmTest, 10, (-32768).toShort())// -32768 * 2 = -65536 -> clamps to -32768

        receiver.applyVolumeAndMute(pcmTest, pcmTest.size)

        assertEquals(20000.toShort(), readShortLe(pcmTest, 0))
        assertEquals((-20000).toShort(), readShortLe(pcmTest, 2))
        assertEquals(32767.toShort(), readShortLe(pcmTest, 4))
        assertEquals((-32768).toShort(), readShortLe(pcmTest, 6))
        assertEquals(32767.toShort(), readShortLe(pcmTest, 8))
        assertEquals((-32768).toShort(), readShortLe(pcmTest, 10))

        // 4. Mute zeroing regardless of boost factor
        receiver.setMute(true)
        assertTrue(receiver.isMuted)
        receiver.applyVolumeAndMute(pcmTest, pcmTest.size)

        for (i in pcmTest.indices) {
            assertEquals("Every byte must be zeroed when muted", 0.toByte(), pcmTest[i])
        }

        // Unmute restored
        receiver.setMute(false)
        assertFalse(receiver.isMuted)
    }

    // =========================================================================
    // 2. OkHttp Streaming Loop, Peer Fingerprint Header & HTTP Status Codes
    // =========================================================================

    @Test
    fun testStreamingLoop_PeerFingerprintVerificationAndMassiveFrames() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val fakeDecoder = FakeAudioDecoder()
        val fakeSink = FakeAudioSink()
        val ringBuffer = JitterRingBuffer(capacityMs = 500)

        val receiver = AudioStreamReceiver(
            context = context,
            decoder = fakeDecoder,
            sink = fakeSink,
            ringBuffer = ringBuffer
        )

        val serverSocket = ServerSocket(0).also { it.soTimeout = 5000 }
        val port = serverSocket.localPort

        val capturedFingerprint = AtomicReference<String?>(null)
        val capturedAcceptHeader = AtomicReference<String?>(null)
        val capturedUrl = AtomicReference<String?>(null)
        val serverDone = CountDownLatch(1)

        val expectedFingerprint = "7a8b9c0d1e2f34567890abcdef1234567890abcdef1234567890abcdef123456"

        val serverThread = Thread {
            try {
                val socket = serverSocket.accept()
                val reader = socket.getInputStream().bufferedReader()
                val requestLine = reader.readLine()
                capturedUrl.set(requestLine)

                var headerLine = reader.readLine()
                while (!headerLine.isNullOrEmpty()) {
                    if (headerLine.startsWith("X-Fingerprint:", ignoreCase = true)) {
                        capturedFingerprint.set(headerLine.substringAfter(":").trim())
                    } else if (headerLine.startsWith("Accept:", ignoreCase = true)) {
                        capturedAcceptHeader.set(headerLine.substringAfter(":").trim())
                    }
                    headerLine = reader.readLine()
                }

                val out = socket.getOutputStream()
                out.write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: audio/x-archdrop-audio\r\n\r\n".toByteArray())
                out.flush()

                // Stream 15 ADAU frames with varying payloads
                for (seq in 0L until 15L) {
                    val payloadSize = 120 + (seq.toInt() * 10)
                    val payload = ByteArray(payloadSize) { ((seq + it) and 0xFF).toByte() }
                    val header = AudioHeader(
                        codec = AudioCodec.OPUS,
                        channels = 2,
                        flags = FLAG_NORMAL,
                        sampleRate = 48000,
                        sequence = seq,
                        timestampMs = 1000L + (seq * 20L),
                        payloadLen = payloadSize
                    )
                    val frame = AudioFrame(header, payload)
                    writeChunk(out, frame.toByteArray())
                    Thread.sleep(10)
                }

                Thread.sleep(150)
                socket.close()
            } catch (_: Exception) {
            } finally {
                serverDone.countDown()
            }
        }
        serverThread.isDaemon = true
        serverThread.start()

        receiver.start(
            host = "127.0.0.1",
            port = port,
            fingerprint = expectedFingerprint,
            codec = AudioCodec.OPUS,
            targetBitrate = 96000,
            sinkMode = "monitor"
        )

        // Verify fingerprint header and request URL
        assertTrue("Server should complete streaming", serverDone.await(5, TimeUnit.SECONDS))
        assertEquals(expectedFingerprint, capturedFingerprint.get())
        assertEquals("audio/x-archdrop-audio", capturedAcceptHeader.get())
        assertTrue("URL must contain api/v2/audio/stream", capturedUrl.get()?.contains("/api/v2/audio/stream") == true)
        assertTrue("URL must specify codec=opus", capturedUrl.get()?.contains("codec=opus") == true)

        // Wait for decoder to process all frames
        runBlocking {
            var waitLoops = 0
            while (fakeDecoder.decodeCount < 15 && waitLoops < 30) {
                delay(50L)
                waitLoops++
            }
        }

        assertTrue("Fake decoder should have processed >= 15 frames", fakeDecoder.decodeCount >= 15)

        receiver.stop()
        assertEquals(AudioReceiverState.Idle, receiver.state.value)
        assertTrue(fakeSink.releaseCount >= 1)
        assertTrue(fakeDecoder.releaseCount >= 1)

        serverSocket.close()
    }

    @Test
    fun testStreamingLoop_SequenceGapsRecordedInTelemetry() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val fakeDecoder = FakeAudioDecoder()
        val fakeSink = FakeAudioSink()
        val ringBuffer = JitterRingBuffer(capacityMs = 500)

        val receiver = AudioStreamReceiver(
            context = context,
            decoder = fakeDecoder,
            sink = fakeSink,
            ringBuffer = ringBuffer
        )

        val serverSocket = ServerSocket(0).also { it.soTimeout = 5000 }
        val port = serverSocket.localPort

        val serverThread = Thread {
            try {
                val socket = serverSocket.accept()
                val reader = socket.getInputStream().bufferedReader()
                while (!reader.readLine().isNullOrEmpty()) {}

                val out = socket.getOutputStream()
                out.write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".toByteArray())
                out.flush()

                // Send frame seq 0
                val f0 = AudioFrame(AudioHeader(codec = AudioCodec.OPUS, sequence = 0, timestampMs = 1000, payloadLen = 64), ByteArray(64))
                writeChunk(out, f0.toByteArray())

                // Send frame seq 5 (dropped seq 1, 2, 3, 4 -> 4 lost packets!)
                val f5 = AudioFrame(AudioHeader(codec = AudioCodec.OPUS, sequence = 5, timestampMs = 1100, payloadLen = 64), ByteArray(64))
                writeChunk(out, f5.toByteArray())

                Thread.sleep(100)
                socket.close()
            } catch (_: Exception) {}
        }
        serverThread.isDaemon = true
        serverThread.start()

        receiver.start(host = "127.0.0.1", port = port)

        runBlocking {
            var waitLoops = 0
            while (fakeDecoder.decodeCount < 2 && waitLoops < 30) {
                delay(50L)
                waitLoops++
            }
        }

        val telemetry = ringBuffer.getTelemetry()
        assertEquals("Telemetry must record exactly 4 lost packets from sequence jump", 4L, telemetry.packetsLost)

        receiver.stop()
        serverSocket.close()
    }

    @Test
    fun testStreamingLoop_ErrorHandling503PipeWireFailure() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val receiver = AudioStreamReceiver(
            context = context,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )

        val serverSocket = ServerSocket(0).also { it.soTimeout = 5000 }
        val port = serverSocket.localPort

        val serverThread = Thread {
            try {
                val socket = serverSocket.accept()
                val reader = socket.getInputStream().bufferedReader()
                while (!reader.readLine().isNullOrEmpty()) {}

                val out = socket.getOutputStream()
                out.write("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                socket.close()
            } catch (_: Exception) {}
        }
        serverThread.isDaemon = true
        serverThread.start()

        receiver.start(host = "127.0.0.1", port = port)

        runBlocking {
            var waitLoops = 0
            while (receiver.state.value !is AudioReceiverState.Error && waitLoops < 30) {
                delay(50L)
                waitLoops++
            }
        }

        val state = receiver.state.value
        assertTrue("State must be Error on 503", state is AudioReceiverState.Error)
        assertEquals(503, (state as AudioReceiverState.Error).statusCode)

        receiver.stop()
        serverSocket.close()
    }

    // =========================================================================
    // 3. Jitter Ring Buffer Stress, Overruns, Underruns, Zero-Crossing Drift
    // =========================================================================

    @Test
    fun testJitterRingBuffer_SevereOverrunAndUnderrunStress() {
        val ring = JitterRingBuffer(capacityMs = 50) // 50 ms = 9600 bytes
        val capacity = ring.capacity
        assertEquals(9600, capacity)

        // 1. Stress underruns: 50 consecutive reads on empty buffer
        val drain = ByteArray(384)
        for (i in 0 until 50) {
            val read = ring.readPcm(drain, 0, drain.size)
            assertEquals(0, read)
        }
        assertEquals(50L, ring.getTelemetry().underruns)

        // 2. Stress moderate overruns: write 12,000 bytes into 9,600 byte capacity (1.25x capacity)
        val moderateOverrun = ByteArray(12000) { (it and 0xFF).toByte() }
        val written = ring.writeDirect(moderateOverrun, 0, moderateOverrun.size)
        assertEquals(12000, written)
        assertEquals(capacity, ring.availableBytes)
        assertTrue(ring.getTelemetry().overruns >= 1L)

        // 3. Read out exact capacity without IndexOutOfBoundsException
        val readOut = ByteArray(capacity)
        val actualRead = ring.readPcm(readOut, 0, capacity)
        assertEquals(capacity, actualRead)
        assertEquals(0, ring.availableBytes)

        // 4. Verify data read corresponds to the freshest bytes (tail of moderateOverrun)
        val expectedStart = moderateOverrun.size - capacity
        for (i in 0 until capacity) {
            assertEquals(moderateOverrun[expectedStart + i], readOut[i])
        }

        // 5. Large Overrun Robustness Verification:
        // Calling writeDirect with writeLen > 2 * capacityBytes (50,000 bytes) must handle large overruns
        // gracefully without throwing ArrayIndexOutOfBoundsException, keeping freshest capacity bytes and incrementing overruns.
        val extremeOverrun = ByteArray(50000) { 0x77.toByte() }
        val writtenExtreme = ring.writeDirect(extremeOverrun, 0, extremeOverrun.size)
        assertEquals(50000, writtenExtreme)
        assertEquals(capacity, ring.availableBytes)
        assertTrue("Overrun count must be recorded", ring.getTelemetry().overruns >= 2L)
    }

    @Test
    fun testJitterRingBuffer_PureDcOffsetZeroCrossingFallback() {
        val ring = JitterRingBuffer(
            capacityMs = 500,
            lowWaterMs = 20.0,
            highWaterMs = 70.0
        )

        // Synthesize 20 ms PCM with a pure constant DC offset (all 15000s) -> NO zero crossings exist!
        val pcmDc = ByteArray(3840)
        for (i in 0 until 3840 step 2) {
            writeShortLe(pcmDc, i, 15000.toShort())
        }

        // On low-water (<20ms), drift compensation should gracefully fall back without crash
        val expanded = ring.duplicateZeroCrossing(pcmDc, 0, pcmDc.size)
        assertNotNull("Should not return null even if no exact zero crossing exists", expanded)
        assertEquals(pcmDc.size + JitterRingBuffer.COMPENSATION_BYTES, expanded!!.size)

        // On high-water (>70ms), drift compensation should drop without crash
        val contracted = ring.dropZeroCrossing(pcmDc, 0, pcmDc.size)
        assertNotNull("Should not return null on DC signal", contracted)
        assertEquals(pcmDc.size - JitterRingBuffer.COMPENSATION_BYTES, contracted!!.size)
    }

    // =========================================================================
    // 4. Streaming Frame Parser Adversarial Fuzzing
    // =========================================================================

    @Test
    fun testFrameParser_LargeGarbageStreamWithEmbeddedValidFrames() {
        val parser = AudioFrameParser(maxPayloadLen = 128 * 1024)
        val rng = Random(42)

        // Generate 5,000 bytes of completely random garbage noise
        val garbage1 = ByteArray(5000)
        rng.nextBytes(garbage1)

        // Create valid frame 1
        val payload1 = ByteArray(200) { 0xAA.toByte() }
        val frame1 = AudioFrame(
            AudioHeader(codec = AudioCodec.OPUS, sequence = 1, timestampMs = 5000, payloadLen = payload1.size),
            payload1
        )

        // More garbage with partial "AD" prefixes
        val garbage2 = byteArrayOf(
            0x41, 0x44, 0x00, 0x41, 0x44, 0x41, 0x00, // Partial "AD", "ADA" false starts
            0x12, 0x34, 0x56, 0x78
        )

        // Create valid frame 2
        val payload2 = ByteArray(300) { 0xBB.toByte() }
        val frame2 = AudioFrame(
            AudioHeader(codec = AudioCodec.PCM, sequence = 2, timestampMs = 5020, payloadLen = payload2.size),
            payload2
        )

        val combined = garbage1 + frame1.toByteArray() + garbage2 + frame2.toByteArray()
        val stream = ByteArrayInputStream(combined)

        // Parser should resynchronize and successfully read both frames
        val read1 = parser.readNextFrame(stream)
        assertEquals(frame1, read1)
        assertArrayEquals(payload1, read1.payload)

        val read2 = parser.readNextFrame(stream)
        assertEquals(frame2, read2)
        assertArrayEquals(payload2, read2.payload)

        assertTrue(
            "Parser should have skipped garbage bytes",
            parser.corruptedBytesSkipped >= (garbage1.size + garbage2.size)
        )
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    private fun writeChunk(out: OutputStream, data: ByteArray) {
        val hexLen = Integer.toHexString(data.size)
        out.write("$hexLen\r\n".toByteArray())
        out.write(data)
        out.write("\r\n".toByteArray())
        out.flush()
    }

    private fun readShortLe(arr: ByteArray, offset: Int): Short {
        val b0 = arr[offset].toInt() and 0xFF
        val b1 = arr[offset + 1].toInt() and 0xFF
        return ((b1 shl 8) or b0).toShort()
    }

    private fun writeShortLe(arr: ByteArray, offset: Int, value: Short) {
        val v = value.toInt()
        arr[offset] = (v and 0xFF).toByte()
        arr[offset + 1] = ((v shr 8) and 0xFF).toByte()
    }
}
