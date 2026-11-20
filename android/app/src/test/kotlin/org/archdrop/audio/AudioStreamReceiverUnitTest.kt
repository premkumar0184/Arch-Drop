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
import java.io.OutputStream
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

@RunWith(RobolectricTestRunner::class)
class AudioStreamReceiverUnitTest {

    @Test
    fun testVolumeBoost_SaturationClamping() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val receiver = AudioStreamReceiver(
            context = context,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )

        // Verify boost factor clamping
        receiver.setVolumeBoost(0.5f)
        assertEquals(1.0f, receiver.volumeBoost, 0.001f)

        receiver.setVolumeBoost(2.5f)
        assertEquals(2.0f, receiver.volumeBoost, 0.001f)

        receiver.setVolumeBoost(1.5f)
        assertEquals(1.5f, receiver.volumeBoost, 0.001f)

        // 16-bit PCM test samples:
        // sample 0: 10000 -> 10000 * 1.5 = 15000
        // sample 1: -10000 -> -10000 * 1.5 = -15000
        // sample 2: 25000 -> 25000 * 1.5 = 37500 -> clamped to 32767
        // sample 3: -25000 -> -25000 * 1.5 = -37500 -> clamped to -32768
        val pcm = ByteArray(8)
        writeShortLe(pcm, 0, 10000.toShort())
        writeShortLe(pcm, 2, (-10000).toShort())
        writeShortLe(pcm, 4, 25000.toShort())
        writeShortLe(pcm, 6, (-25000).toShort())

        receiver.applyVolumeAndMute(pcm, pcm.size)

        assertEquals(15000.toShort(), readShortLe(pcm, 0))
        assertEquals((-15000).toShort(), readShortLe(pcm, 2))
        assertEquals(32767.toShort(), readShortLe(pcm, 4)) // Saturated
        assertEquals((-32768).toShort(), readShortLe(pcm, 6)) // Saturated
    }

    @Test
    fun testVolumeMute_ZeroesAllSamples() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val receiver = AudioStreamReceiver(
            context = context,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )

        val pcm = ByteArray(8)
        writeShortLe(pcm, 0, 12345.toShort())
        writeShortLe(pcm, 2, (-9999).toShort())
        writeShortLe(pcm, 4, 30000.toShort())
        writeShortLe(pcm, 6, (-30000).toShort())

        receiver.setMute(true)
        assertTrue(receiver.isMuted)

        receiver.applyVolumeAndMute(pcm, pcm.size)

        // All samples must be 0
        for (b in pcm) {
            assertEquals(0.toByte(), b)
        }

        // Unmute
        receiver.setMute(false)
        assertFalse(receiver.isMuted)
    }

    @Test
    fun testStreamingLoop_EndToEndWithMockServer() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val fakeDecoder = FakeAudioDecoder()
        val fakeSink = FakeAudioSink()
        val ringBuffer = JitterRingBuffer()

        val receiver = AudioStreamReceiver(
            context = context,
            decoder = fakeDecoder,
            sink = fakeSink,
            ringBuffer = ringBuffer
        )

        val serverSocket = ServerSocket(0)
        val port = serverSocket.localPort

        val clientConnectedLatch = CountDownLatch(1)
        val receivedFingerprint = AtomicReference<String?>(null)
        val serverDone = AtomicBoolean(false)

        // Generate 3 test ADAU frames
        val payload1 = ByteArray(100) { 1 }
        val payload2 = ByteArray(100) { 2 }
        val frame1 = AudioFrame(AudioHeader(codec = AudioCodec.OPUS, sequence = 0, timestampMs = 1000, payloadLen = 100), payload1)
        val frame2 = AudioFrame(AudioHeader(codec = AudioCodec.OPUS, sequence = 1, timestampMs = 1020, payloadLen = 100), payload2)

        val serverThread = Thread {
            try {
                val socket = serverSocket.accept()

                val reader = socket.getInputStream().bufferedReader()
                var line = reader.readLine()
                while (!line.isNullOrEmpty()) {
                    if (line.startsWith("X-Fingerprint:", ignoreCase = true)) {
                        receivedFingerprint.set(line.substringAfter(":").trim())
                    }
                    line = reader.readLine()
                }
                clientConnectedLatch.countDown()

                val out = socket.getOutputStream()
                out.write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: audio/x-archdrop-audio\r\n\r\n".toByteArray())
                out.flush()

                // Send frame 1
                val frame1Bytes = frame1.toByteArray()
                writeChunk(out, frame1Bytes)

                // Send frame 2
                val frame2Bytes = frame2.toByteArray()
                writeChunk(out, frame2Bytes)

                Thread.sleep(200)
                socket.close()
            } catch (_: Exception) {
            } finally {
                serverDone.set(true)
            }
        }
        serverThread.isDaemon = true
        serverThread.start()

        val testFingerprint = "abcd1234ef5678"
        receiver.start(
            host = "127.0.0.1",
            port = port,
            fingerprint = testFingerprint,
            codec = AudioCodec.OPUS,
            targetBitrate = 96000
        )

        assertTrue("Client should connect to mock server", clientConnectedLatch.await(5, TimeUnit.SECONDS))
        assertEquals(testFingerprint, receivedFingerprint.get())

        // Wait briefly for streaming to process frames
        runBlocking {
            var waitCount = 0
            while (fakeDecoder.decodeCount < 2 && waitCount < 30) {
                delay(50L)
                waitCount++
            }
        }

        assertTrue("Decoder should have received at least 2 frames", fakeDecoder.decodeCount >= 2)
        assertTrue("Sink should be in playing state", fakeSink.playCount >= 1)

        // Teardown
        receiver.stop()
        assertEquals(AudioReceiverState.Idle, receiver.state.value)
        assertTrue("Sink release count must be >= 1", fakeSink.releaseCount >= 1)
        assertTrue("Decoder release count must be >= 1", fakeDecoder.releaseCount >= 1)

        serverSocket.close()
    }

    @Test
    fun testAuthenticationError403Handling() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val receiver = AudioStreamReceiver(
            context = context,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )

        val serverSocket = ServerSocket(0)
        val port = serverSocket.localPort

        val serverThread = Thread {
            try {
                val socket = serverSocket.accept()
                val reader = socket.getInputStream().bufferedReader()
                var line = reader.readLine()
                while (!line.isNullOrEmpty()) {
                    line = reader.readLine()
                }

                val out = socket.getOutputStream()
                out.write("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                socket.close()
            } catch (_: Exception) {}
        }
        serverThread.isDaemon = true
        serverThread.start()

        receiver.start(
            host = "127.0.0.1",
            port = port,
            fingerprint = "invalid_fp"
        )

        runBlocking {
            var waitCount = 0
            while (receiver.state.value !is AudioReceiverState.Error && waitCount < 30) {
                delay(50L)
                waitCount++
            }
        }

        val currentState = receiver.state.value
        assertTrue("State must be Error on 403 Forbidden", currentState is AudioReceiverState.Error)
        assertEquals(403, (currentState as AudioReceiverState.Error).statusCode)

        receiver.stop()
        serverSocket.close()
    }

    @Test
    fun testMutualExclusionConflict409Handling() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val receiver = AudioStreamReceiver(
            context = context,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )

        val serverSocket = ServerSocket(0)
        val port = serverSocket.localPort

        val serverThread = Thread {
            try {
                val socket = serverSocket.accept()
                val reader = socket.getInputStream().bufferedReader()
                var line = reader.readLine()
                while (!line.isNullOrEmpty()) {
                    line = reader.readLine()
                }

                val out = socket.getOutputStream()
                out.write("HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n".toByteArray())
                out.flush()
                socket.close()
            } catch (_: Exception) {}
        }
        serverThread.isDaemon = true
        serverThread.start()

        receiver.start(
            host = "127.0.0.1",
            port = port
        )

        runBlocking {
            var waitCount = 0
            while (receiver.state.value !is AudioReceiverState.Error && waitCount < 30) {
                delay(50L)
                waitCount++
            }
        }

        val currentState = receiver.state.value
        assertTrue("State must be Error on 409 Conflict", currentState is AudioReceiverState.Error)
        assertEquals(409, (currentState as AudioReceiverState.Error).statusCode)

        receiver.stop()
        serverSocket.close()
    }

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
