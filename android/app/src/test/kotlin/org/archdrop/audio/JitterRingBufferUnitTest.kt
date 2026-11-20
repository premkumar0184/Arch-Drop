package org.archdrop.audio

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs

class JitterRingBufferUnitTest {

    @Test
    fun testBufferCapacityAndBasicArithmetic() {
        val ring = JitterRingBuffer(capacityMs = 500)

        assertEquals(96000, ring.capacity)
        assertEquals(0, ring.availableBytes)
        assertEquals(0.0, ring.depthMs, 0.001)

        val sampleRate = 48000
        val bytesPerMs = 192 // 48000 * 4 / 1000
        assertEquals(bytesPerMs, JitterRingBuffer.BYTES_PER_MS)

        // Write 20 ms of PCM (3840 bytes)
        val pcm = ByteArray(3840) { (it and 0xFF).toByte() }
        val written = ring.writeDirect(pcm, 0, pcm.size)
        assertEquals(3840, written)
        assertEquals(3840, ring.availableBytes)
        assertEquals(20.0, ring.depthMs, 0.001)

        // Read 10 ms (1920 bytes)
        val readBuf = ByteArray(1920)
        val read = ring.readPcm(readBuf, 0, readBuf.size)
        assertEquals(1920, read)
        assertEquals(1920, ring.availableBytes)
        assertEquals(10.0, ring.depthMs, 0.001)

        // Verify read content matches first 1920 bytes
        for (i in 0 until 1920) {
            assertEquals(pcm[i], readBuf[i])
        }

        ring.clear()
        assertEquals(0, ring.availableBytes)
        assertEquals(0.0, ring.depthMs, 0.001)
    }

    @Test
    fun testCircularWrappingReadAndWrite() {
        val ring = JitterRingBuffer(capacityMs = 100) // 100 ms = 19200 bytes capacity
        val capacity = ring.capacity

        // Fill up to capacity - 1000
        val fillSize = capacity - 1000
        val initialData = ByteArray(fillSize) { 1 }
        ring.writeDirect(initialData, 0, fillSize)

        // Drain fillSize - 500 (leaves 500 in buffer)
        val drainBuf = ByteArray(fillSize - 500)
        ring.readPcm(drainBuf, 0, drainBuf.size)
        assertEquals(500, ring.availableBytes)

        // Now writing 2000 bytes will wrap around the circular buffer boundary
        val wrapData = ByteArray(2000) { (it % 100).toByte() }
        val written = ring.writeDirect(wrapData, 0, 2000)
        assertEquals(2000, written)
        assertEquals(2500, ring.availableBytes)

        // Drain the remaining initial 500
        val drain500 = ByteArray(500)
        ring.readPcm(drain500, 0, 500)

        // Now read the 2000 wrapped bytes
        val wrappedRead = ByteArray(2000)
        val read = ring.readPcm(wrappedRead, 0, 2000)
        assertEquals(2000, read)
        assertArrayEquals(wrapData, wrappedRead)
        assertEquals(0, ring.availableBytes)
    }

    @Test
    fun testBufferOverrunAndUnderrunHandling() {
        val ring = JitterRingBuffer(capacityMs = 10) // 10 ms = 1920 bytes
        val capacity = ring.capacity

        // Test underrun: reading empty buffer
        val drain = ByteArray(100)
        val readWhenEmpty = ring.readPcm(drain, 0, 100)
        assertEquals(0, readWhenEmpty)

        val telemetry1 = ring.getTelemetry()
        assertEquals(1L, telemetry1.underruns)

        // Test overrun: write 2500 bytes into 1920 byte capacity
        val bigData = ByteArray(2500) { 0x55.toByte() }
        ring.writeDirect(bigData, 0, bigData.size)

        assertEquals(capacity, ring.availableBytes)
        val telemetry2 = ring.getTelemetry()
        assertEquals(1L, telemetry2.overruns)
    }

    @Test
    fun testZeroCrossingExpansionOnLowWater() {
        // Low-water threshold is 20 ms.
        val ring = JitterRingBuffer(
            capacityMs = 500,
            lowWaterMs = 20.0,
            highWaterMs = 70.0
        )

        // Buffer is empty (0 ms depth < 20 ms low water)
        // Synthesize 20 ms frame (3840 bytes) with a clear zero crossing near sample 200
        val pcm = createSineWavePcm(numStereoSamples = 960)

        val initialDepth = ring.depthMs
        assertEquals(0.0, initialDepth, 0.001)

        val now = 1000L
        ring.writePcm(pcm, 0, pcm.size, ptsMs = now, nowMs = now)

        val telemetry = ring.getTelemetry(now)
        assertEquals(1L, telemetry.expansions)
        assertEquals(0L, telemetry.contractions)

        // Expansion adds 96 stereo samples (384 bytes = 2 ms)
        val expectedBytes = pcm.size + JitterRingBuffer.COMPENSATION_BYTES
        assertEquals(expectedBytes, ring.availableBytes)
        assertEquals(expectedBytes / 192.0, ring.depthMs, 0.001)
    }

    @Test
    fun testZeroCrossingContractionOnHighWater() {
        // High-water threshold is 70 ms.
        val ring = JitterRingBuffer(
            capacityMs = 500,
            lowWaterMs = 20.0,
            highWaterMs = 70.0
        )

        // Pre-fill buffer to 80 ms (80 * 192 = 15,360 bytes) > 70 ms
        val prefill = ByteArray(80 * 192) { 0 }
        ring.writeDirect(prefill, 0, prefill.size)
        assertEquals(80.0, ring.depthMs, 0.001)

        // Now write a 20 ms frame (3840 bytes) with a zero crossing
        val pcm = createSineWavePcm(numStereoSamples = 960)

        val now = 2000L
        ring.writePcm(pcm, 0, pcm.size, ptsMs = now, nowMs = now)

        val telemetry = ring.getTelemetry(now)
        assertEquals(0L, telemetry.expansions)
        assertEquals(1L, telemetry.contractions)

        // Contraction drops 96 stereo samples (384 bytes = 2 ms)
        val expectedAdded = pcm.size - JitterRingBuffer.COMPENSATION_BYTES
        val expectedTotal = prefill.size + expectedAdded
        assertEquals(expectedTotal, ring.availableBytes)
        assertEquals(expectedTotal / 192.0, ring.depthMs, 0.001)
    }

    @Test
    fun testNormalRangeDoesNotTriggerCompensation() {
        val ring = JitterRingBuffer(
            capacityMs = 500,
            lowWaterMs = 20.0,
            highWaterMs = 70.0
        )

        // Pre-fill buffer to 40 ms (sweet spot)
        val prefill = ByteArray(40 * 192)
        ring.writeDirect(prefill, 0, prefill.size)
        assertEquals(40.0, ring.depthMs, 0.001)

        val pcm = createSineWavePcm(numStereoSamples = 960)
        ring.writePcm(pcm, 0, pcm.size, ptsMs = 3000L, nowMs = 3000L)

        val telemetry = ring.getTelemetry(3000L)
        assertEquals(0L, telemetry.expansions)
        assertEquals(0L, telemetry.contractions)
        assertEquals(prefill.size + pcm.size, ring.availableBytes)
    }

    @Test
    fun testPacketLossConcealment_DecayingRepeat() {
        val ring = JitterRingBuffer(capacityMs = 500)

        // Feed an initial frame with strong 16-bit sine amplitude
        val pcm = createSineWavePcm(numStereoSamples = 960, amplitude = 20000)
        ring.writePcm(pcm, 0, pcm.size)

        // First PLC frame
        val plc1 = ring.generatePlcFrame(decayFactor = 0.7f)
        assertEquals(pcm.size, plc1.size)

        // Verify amplitude decayed by approximately 0.7
        val origSample = readShortLe(pcm, 100)
        val plc1Sample = readShortLe(plc1, 100)
        val expectedPlc1 = (origSample * 0.7f).toInt()
        assertEquals(expectedPlc1.toDouble(), plc1Sample.toDouble(), 2.0)

        // Second consecutive PLC frame should decay by 0.7^2 = 0.49
        val plc2 = ring.generatePlcFrame(decayFactor = 0.7f)
        val plc2Sample = readShortLe(plc2, 100)
        val expectedPlc2 = (origSample * 0.49f).toInt()
        assertEquals(expectedPlc2.toDouble(), plc2Sample.toDouble(), 2.0)
    }

    @Test
    fun testEWMAJitterCalculation() {
        val ring = JitterRingBuffer()

        // Packet 1: sent at 100, received at 120 -> transit = 20ms
        ring.updateTransitTime(ptsMs = 100, nowMs = 120)
        var telemetry = ring.getTelemetry(120)
        assertEquals(0.0, telemetry.jitterMs, 0.001) // First packet sets baseline

        // Packet 2: sent at 120, received at 145 -> transit = 25ms, delta = 5ms
        // J_1 = 0.875 * 0 + 0.125 * 5 = 0.625 ms
        ring.updateTransitTime(ptsMs = 120, nowMs = 145)
        telemetry = ring.getTelemetry(145)
        assertEquals(0.625, telemetry.jitterMs, 0.001)

        // Packet 3: sent at 140, received at 180 -> transit = 40ms, delta = 15ms
        // J_2 = 0.875 * 0.625 + 0.125 * 15 = 0.546875 + 1.875 = 2.421875 ms
        ring.updateTransitTime(ptsMs = 140, nowMs = 180)
        telemetry = ring.getTelemetry(180)
        assertEquals(2.421875, telemetry.jitterMs, 0.001)
    }

    @Test
    fun testRollingBitrateCalculation() {
        val ring = JitterRingBuffer()
        val now = 10000L

        // Record 12,000 bytes at t=now (12000 * 8 / 1000 = 96 kbps)
        ring.recordBytes(12000, now)

        val bitrate = ring.getLiveBitrateKbps(now)
        assertEquals(96.0, bitrate, 0.01)

        // After 1.5 seconds, the 12,000 bytes slide outside the 1-second window
        val bitrateLater = ring.getLiveBitrateKbps(now + 1500L)
        assertEquals(0.0, bitrateLater, 0.01)
    }

    // Helper to generate synthetic 440 Hz stereo PCM waveform
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
