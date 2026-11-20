package org.archdrop.audio

import okio.Buffer
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.EOFException
import kotlin.math.abs
import kotlin.math.pow
import kotlin.math.roundToInt

/**
 * Empirical Adversarial Stress Test Suite authored by Challenger 1 for Milestone 2.
 * Focuses on AudioFrameParser framing resilience and JitterRingBuffer arithmetic invariants.
 */
class AudioPipelineFramingAndBufferStressTest {

    // =========================================================================
    // PART 1: AudioFrameParser Framing, Edge Cases & Byte Resynchronization
    // =========================================================================

    @Test
    fun testParser_TruncatedHeaderThrowsEofException() {
        val parser = AudioFrameParser()

        // Test truncated headers at various boundary lengths (1 to 31 bytes)
        for (truncatedLen in listOf(1, 2, 3, 4, 5, 10, 20, 31)) {
            val partialBytes = ByteArray(truncatedLen) { 0x41.toByte() }
            // Ensure first 4 bytes match ADAU magic so it attempts to read the rest of the header
            if (truncatedLen >= 1) partialBytes[0] = 0x41 // 'A'
            if (truncatedLen >= 2) partialBytes[1] = 0x44 // 'D'
            if (truncatedLen >= 3) partialBytes[2] = 0x41 // 'A'
            if (truncatedLen >= 4) partialBytes[3] = 0x55 // 'U'

            val buffer = Buffer().write(partialBytes)
            try {
                parser.readNextFrame(buffer)
                fail("Expected EOFException for header truncated at length $truncatedLen")
            } catch (e: EOFException) {
                // Expected EOF
            }
        }
    }

    @Test
    fun testParser_TruncatedPayloadThrowsEofException() {
        val parser = AudioFrameParser()

        // Create header advertising 1000 bytes payload
        val header = AudioHeader(
            codec = AudioCodec.OPUS,
            channels = 2,
            flags = FLAG_NORMAL,
            sampleRate = 48000,
            sequence = 100L,
            timestampMs = 1700000000000L,
            payloadLen = 1000
        )
        val headerBytes = header.toByteArray()

        // Only provide 300 bytes of payload instead of 1000
        val partialPayload = ByteArray(300) { 0x7F.toByte() }
        val wireBytes = headerBytes + partialPayload

        val buffer = Buffer().write(wireBytes)
        try {
            parser.readNextFrame(buffer)
            fail("Expected EOFException when stream terminates before reading full payload")
        } catch (e: EOFException) {
            // Expected EOF
        }

        // Test with java.io.InputStream
        val stream = ByteArrayInputStream(wireBytes)
        try {
            parser.readNextFrame(stream)
            fail("Expected EOFException on InputStream when stream terminates before reading full payload")
        } catch (e: EOFException) {
            // Expected EOF
        }
    }

    @Test
    fun testParser_SequentialFramesWithMidStreamTruncation() {
        val parser = AudioFrameParser()

        val f1Payload = ByteArray(100) { 0x11.toByte() }
        val frame1 = AudioFrame(
            AudioHeader(codec = AudioCodec.OPUS, sequence = 1, timestampMs = 1000, payloadLen = f1Payload.size),
            f1Payload
        )

        val f2Header = AudioHeader(codec = AudioCodec.PCM, sequence = 2, timestampMs = 1020, payloadLen = 500)
        // Truncate frame 2 mid-payload (only 100 of 500 bytes provided)
        val f2Truncated = f2Header.toByteArray() + ByteArray(100) { 0x22.toByte() }

        val buffer = Buffer()
        buffer.write(frame1.toByteArray())
        buffer.write(f2Truncated)

        // Frame 1 must succeed cleanly
        val read1 = parser.readNextFrame(buffer)
        assertEquals(frame1, read1)
        assertArrayEquals(f1Payload, read1.payload)

        // Reading frame 2 must encounter EOFException
        try {
            parser.readNextFrame(buffer)
            fail("Expected EOFException on truncated second frame")
        } catch (e: EOFException) {
            // Expected EOF
        }
    }

    @Test
    fun testParser_SequenceAndTimestampExtremeValues() {
        // Test edge cases: sequence = Long.MAX_VALUE, timestamp = Long.MAX_VALUE, timestamp = 0
        val edgeHeader1 = AudioHeader(
            codec = AudioCodec.OPUS,
            channels = 2,
            flags = FLAG_NORMAL,
            sampleRate = 48000,
            sequence = Long.MAX_VALUE,
            timestampMs = Long.MAX_VALUE,
            payloadLen = 64
        )
        val bytes1 = edgeHeader1.toByteArray()
        val parsed1 = AudioHeader.fromByteArray(bytes1)
        assertEquals(Long.MAX_VALUE, parsed1.sequence)
        assertEquals(Long.MAX_VALUE, parsed1.timestampMs)

        // Sequence wrap around 0
        val edgeHeader2 = AudioHeader(
            codec = AudioCodec.PCM,
            channels = 2,
            flags = FLAG_DISCONTINUITY,
            sampleRate = 48000,
            sequence = 0L,
            timestampMs = 0L,
            payloadLen = 128
        )
        val bytes2 = edgeHeader2.toByteArray()
        val parsed2 = AudioHeader.fromByteArray(bytes2)
        assertEquals(0L, parsed2.sequence)
        assertEquals(0L, parsed2.timestampMs)
        assertTrue(parsed2.isDiscontinuity)
    }

    @Test
    fun testParser_SlidingWindowResync_PrefixOverlapBugDemonstration() {
        // Critical KMP failure mode: input contains "A D A D A U"
        // 'A' 'D' 'A' is matched, followed by 'D', which aborts candidate.
        // But the suffix 'A' 'D' combined with subsequent 'A' 'U' forms the valid "A D A U" magic!
        val parser = AudioFrameParser()

        val validPayload = ByteArray(80) { 0x33.toByte() }
        val validFrame = AudioFrame(
            AudioHeader(codec = AudioCodec.OPUS, sequence = 7, timestampMs = 2000, payloadLen = validPayload.size),
            validPayload
        )

        // Prefix: 'A', 'D' followed by the valid frame "ADAU..."
        // In wire bytes: 'A' (0x41), 'D' (0x44), then validFrame (starts with 'A' 0x41, 'D' 0x44, 'A' 0x41, 'U' 0x55)
        // Stream: 'A', 'D', 'A', 'D', 'A', 'U' ...
        // Notice that bytes 2..5 of this stream are the valid frame's 'A', 'D', 'A', 'U'!
        val trickyPrefix = byteArrayOf(0x41, 0x44) // "AD"
        val wireBytes = trickyPrefix + validFrame.toByteArray()

        val buffer = Buffer().write(wireBytes)

        // Empirical check: can the parser recover and find the frame?
        try {
            val parsed = parser.readNextFrame(buffer)
            assertEquals(validFrame, parsed)
            assertArrayEquals(validPayload, parsed.payload)
        } catch (e: EOFException) {
            // If the state machine drops state to 0 instead of 2, it misaligns and fails to find ADAU!
            fail("EMPIRICAL BUG CONFIRMED: AudioFrameParser state machine failed to resynchronize on 'ADADAU' pattern: ${e.message}")
        }
    }

    @Test
    fun testParser_FalsePositiveMagicInsideNoiseConsumesStreamBug() {
        // False positive magic scenario:
        // Noise contains "ADAU", followed by 28 bytes of garbage where payloadLen > maxPayloadLen (or invalid codec),
        // followed immediately by a valid ADAU frame.
        val parser = AudioFrameParser(maxPayloadLen = 1000)

        // False positive magic + invalid header (codec = 0x99)
        val falseMagic = byteArrayOf(0x41, 0x44, 0x41, 0x55)
        val falseRestOfHeader = ByteArray(28) { 0x99.toByte() } // Invalid codec 0x99

        val validPayload = ByteArray(50) { 0x44.toByte() }
        val validFrame = AudioFrame(
            AudioHeader(codec = AudioCodec.OPUS, sequence = 1, timestampMs = 500, payloadLen = validPayload.size),
            validPayload
        )

        // If the false positive occurs, the parser consumes the 28 bytes and resumes at the next byte
        val wireBytes = falseMagic + falseRestOfHeader + validFrame.toByteArray()
        val buffer = Buffer().write(wireBytes)

        val parsed = parser.readNextFrame(buffer)
        assertEquals(validFrame, parsed)
    }

    // =========================================================================
    // PART 2: JitterRingBuffer Arithmetic, Circular Array & Drift Invariants
    // =========================================================================

    @Test
    fun testRingBuffer_CircularArrayWrappingUnderPrimeAndOddChunks() {
        // Use a ring buffer of capacity 100 ms = 19,200 bytes
        val ring = JitterRingBuffer(capacityMs = 100)
        val capacity = ring.capacity
        assertEquals(19200, capacity)

        // Write and read 1,337 bytes (odd prime-like size) over 200 cycles, wrapping many times
        val chunkSize = 1337
        val testPattern = ByteArray(chunkSize) { (it % 251).toByte() }
        val readBack = ByteArray(chunkSize)

        for (cycle in 0 until 200) {
            val written = ring.writeDirect(testPattern, 0, testPattern.size)
            assertEquals(chunkSize, written)
            assertEquals(chunkSize, ring.availableBytes)

            val read = ring.readPcm(readBack, 0, chunkSize)
            assertEquals(chunkSize, read)
            assertEquals(0, ring.availableBytes)

            assertArrayEquals("Cycle $cycle bit-for-bit mismatch after wrapping", testPattern, readBack)
        }
    }

    @Test
    fun testRingBuffer_OverrunArithmeticBugDemonstration() {
        val ring = JitterRingBuffer(capacityMs = 50) // 50 ms = 9600 bytes
        val capacity = ring.capacity
        assertEquals(9600, capacity)

        // When writeLen > 2 * capacityBytes (e.g. 25,000 bytes into 9,600 capacity):
        // writeDirect val secondChunk = writeLen - firstChunk > capacityBytes
        // System.arraycopy tries to copy > capacityBytes into buffer -> ArrayIndexOutOfBoundsException!
        val overflowSize = capacity * 3 // 28,800 bytes
        val overflowData = ByteArray(overflowSize) { 0x2A.toByte() }

        try {
            ring.writeDirect(overflowData, 0, overflowData.size)
            // If it survived, availableBytes must not exceed capacity
            assertTrue("availableBytes must not exceed capacity", ring.availableBytes <= capacity)
        } catch (e: ArrayIndexOutOfBoundsException) {
            fail("EMPIRICAL BUG CONFIRMED: JitterRingBuffer.writeDirect crashed with ArrayIndexOutOfBoundsException on large overrun: ${e.message}")
        }
    }

    @Test
    fun testRingBuffer_LowWaterZeroCrossingExpansionArithmetic() {
        val ring = JitterRingBuffer(
            capacityMs = 500,
            lowWaterMs = 20.0,
            highWaterMs = 70.0,
            compensationSamples = 96 // 96 stereo samples = 384 bytes = 2 ms
        )

        // Synthesize 40 ms PCM with a 440 Hz stereo sine wave (960 samples/20ms * 2 = 1920 samples = 7680 bytes)
        val pcm = createSineWavePcm(numStereoSamples = 1920)

        // Low water (<20ms): buffer is empty (0 ms)
        val expanded = ring.duplicateZeroCrossing(pcm, 0, pcm.size)
        assertNotNull("duplicateZeroCrossing must not return null", expanded)
        assertEquals(
            "Expansion must add exactly COMPENSATION_BYTES (384 bytes = 96 stereo samples)",
            pcm.size + JitterRingBuffer.COMPENSATION_BYTES,
            expanded!!.size
        )

        // Verify that the expansion preserved stereo channel interleaving (4 bytes per frame)
        assertEquals(0, expanded.size % JitterRingBuffer.BYTES_PER_FRAME)
    }

    @Test
    fun testRingBuffer_HighWaterZeroCrossingContractionArithmetic() {
        val ring = JitterRingBuffer(
            capacityMs = 500,
            lowWaterMs = 20.0,
            highWaterMs = 70.0,
            compensationSamples = 96 // 96 stereo samples = 384 bytes = 2 ms
        )

        val pcm = createSineWavePcm(numStereoSamples = 1920) // 7680 bytes

        val contracted = ring.dropZeroCrossing(pcm, 0, pcm.size)
        assertNotNull("dropZeroCrossing must not return null", contracted)
        assertEquals(
            "Contraction must remove exactly COMPENSATION_BYTES (384 bytes = 96 stereo samples)",
            pcm.size - JitterRingBuffer.COMPENSATION_BYTES,
            contracted!!.size
        )
        assertEquals(0, contracted.size % JitterRingBuffer.BYTES_PER_FRAME)
    }

    @Test
    fun testRingBuffer_DecayingPlcExponentialDecayAndClamping() {
        val ring = JitterRingBuffer(capacityMs = 500)

        // 1. Calling generatePlcFrame before ANY frame has been written must return a zeroed frame of 3840 bytes
        val initialPlc = ring.generatePlcFrame()
        assertEquals(JitterRingBuffer.PLC_FRAME_BYTES, initialPlc.size)
        for (b in initialPlc) {
            assertEquals(0.toByte(), b)
        }

        // 2. Feed a high-amplitude sine wave (amplitude 30,000, close to 16-bit max 32,767)
        val original = createSineWavePcm(numStereoSamples = 960, amplitude = 30000)
        ring.writePcm(original, 0, original.size)

        // 3. Test consecutive PLC frames with decay factor 0.7
        val decay = 0.7f
        var prevAmp = 30000.0

        for (consecutive in 1..5) {
            val plcFrame = ring.generatePlcFrame(decayFactor = decay)
            assertEquals(original.size, plcFrame.size)

            val expectedDecay = decay.toDouble().pow(consecutive.toDouble())

            // Measure first non-zero sample
            val sampleOrig = readShortLe(original, 100).toInt()
            val samplePlc = readShortLe(plcFrame, 100).toInt()

            val expectedSample = (sampleOrig * expectedDecay).roundToInt().coerceIn(-32768, 32767)
            assertEquals(
                "PLC consecutive $consecutive amplitude must follow 0.7^$consecutive",
                expectedSample.toDouble(),
                samplePlc.toDouble(),
                3.0
            )
        }

        // 4. Writing a new valid PCM frame must reset consecutivePlcCount to 0
        ring.writePcm(original, 0, original.size)
        val plcAfterReset = ring.generatePlcFrame(decayFactor = decay)
        val sampleAfterReset = readShortLe(plcAfterReset, 100).toInt()
        val expectedAfterReset = (readShortLe(original, 100).toInt() * 0.7).roundToInt()
        assertEquals(expectedAfterReset.toDouble(), sampleAfterReset.toDouble(), 3.0)
    }

    @Test
    fun testRingBuffer_EwmaJitterMath_Rfc3550Conformance() {
        val ring = JitterRingBuffer()

        // RFC 3550 transit delay jitter EWMA: J_k = 0.875 * J_{k-1} + 0.125 * |transit_k - transit_{k-1}|
        val now = 10000L
        // Packet 1: transit = 10000 - 9980 = 20 ms
        ring.updateTransitTime(ptsMs = 9980L, nowMs = now)
        assertEquals(0.0, ring.getTelemetry(now).jitterMs, 0.0001)

        // Packet 2: transit = 10020 - 9995 = 25 ms. Delta = |25 - 20| = 5 ms
        // J = 0.875 * 0.0 + 0.125 * 5.0 = 0.625 ms
        ring.updateTransitTime(ptsMs = 9995L, nowMs = now + 20)
        assertEquals(0.625, ring.getTelemetry(now + 20).jitterMs, 0.0001)

        // Packet 3: transit = 10040 - 10020 = 20 ms. Delta = |20 - 25| = 5 ms
        // J = 0.875 * 0.625 + 0.125 * 5.0 = 0.546875 + 0.625 = 1.171875 ms
        ring.updateTransitTime(ptsMs = 10020L, nowMs = now + 40)
        assertEquals(1.171875, ring.getTelemetry(now + 40).jitterMs, 0.0001)
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    private fun createSineWavePcm(numStereoSamples: Int, amplitude: Int = 16000): ByteArray {
        val bytes = ByteArray(numStereoSamples * 4)
        for (i in 0 until numStereoSamples) {
            val angle = 2.0 * Math.PI * 440.0 * (i.toDouble() / 48000.0)
            val sampleVal = (Math.sin(angle) * amplitude).toInt().coerceIn(-32768, 32767).toShort()
            val offset = i * 4
            // Left channel
            bytes[offset] = (sampleVal.toInt() and 0xFF).toByte()
            bytes[offset + 1] = ((sampleVal.toInt() shr 8) and 0xFF).toByte()
            // Right channel
            bytes[offset + 2] = (sampleVal.toInt() and 0xFF).toByte()
            bytes[offset + 3] = ((sampleVal.toInt() shr 8) and 0xFF).toByte()
        }
        return bytes
    }

    private fun readShortLe(arr: ByteArray, offset: Int): Short {
        val b0 = arr[offset].toInt() and 0xFF
        val b1 = arr[offset + 1].toInt() and 0xFF
        return ((b1 shl 8) or b0).toShort()
    }
}
