package org.archdrop.audio

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.nio.ByteBuffer
import java.nio.ByteOrder

@RunWith(RobolectricTestRunner::class)
class AudioDecoderUnitTest {

    @Test
    fun testRawPcmPassthroughDecoder_PassesBytesUnmodified() {
        val decoder = RawPcmPassthroughDecoder()
        decoder.init(48000, 2)
        assertTrue(decoder.isInitialized)

        val inputBytes = ByteArray(1920) { (it and 0xFF).toByte() }
        var capturedOutput: ByteArray? = null
        var capturedLen: Int = 0

        decoder.decode(inputBytes, offset = 0, length = inputBytes.size, ptsMs = 12345L) { pcm, len ->
            capturedOutput = pcm
            capturedLen = len
        }

        assertNotNull(capturedOutput)
        assertEquals(inputBytes.size, capturedLen)
        assertArrayEquals(inputBytes, capturedOutput)

        // Test offset slice
        var sliceOutput: ByteArray? = null
        decoder.decode(inputBytes, offset = 100, length = 500, ptsMs = 0L) { pcm, len ->
            sliceOutput = pcm
            assertEquals(500, len)
        }
        val expectedSlice = ByteArray(500)
        System.arraycopy(inputBytes, 100, expectedSlice, 0, 500)
        assertArrayEquals(expectedSlice, sliceOutput)

        decoder.flush()
        decoder.release()
        assertFalse(decoder.isInitialized)
    }

    @Test
    fun testFakeAudioDecoder_TracksLifecycleAndPayloads() {
        val simulated = byteArrayOf(0x10, 0x20, 0x30)
        val fake = FakeAudioDecoder(simulatedOutputPcm = simulated)

        assertEquals(0, fake.initCount)
        fake.init(sampleRate = 48000, channels = 2)
        assertEquals(1, fake.initCount)
        assertEquals(48000, fake.lastSampleRate)
        assertEquals(2, fake.lastChannels)

        val testPayload = byteArrayOf(0x01, 0x02)
        var captured: ByteArray? = null
        fake.decode(testPayload, 0, testPayload.size, ptsMs = 999L) { pcm, len ->
            captured = pcm
        }

        assertEquals(1, fake.decodeCount)
        assertEquals(999L, fake.lastPtsMs)
        assertEquals(1, fake.receivedPayloads.size)
        assertArrayEquals(testPayload, fake.receivedPayloads[0])
        assertArrayEquals(simulated, captured)

        fake.flush()
        assertEquals(1, fake.flushCount)

        fake.release()
        assertEquals(1, fake.releaseCount)
    }

    @Test
    fun testMediaCodecAudioDecoder_OpusHeadCsd0Structure() {
        val csd0 = MediaCodecAudioDecoder.buildOpusHeadCsd0(sampleRate = 48000, channels = 2)

        // Exact 19 bytes per RFC 7845
        assertEquals(19, csd0.size)

        // 0..7: Magic "OpusHead"
        val magicString = String(csd0, 0, 8, Charsets.US_ASCII)
        assertEquals("OpusHead", magicString)

        // 8: Version 1
        assertEquals(1.toByte(), csd0[8])

        // 9: Channels = 2
        assertEquals(2.toByte(), csd0[9])

        // 10..11: Pre-skip = 0 (little endian)
        assertEquals(0.toByte(), csd0[10])
        assertEquals(0.toByte(), csd0[11])

        // 12..15: Sample rate = 48000 little endian (48000 = 0x0000BB80 -> 80 BB 00 00)
        val sampleRateBuf = ByteBuffer.wrap(csd0, 12, 4).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(48000, sampleRateBuf.getInt())

        // 16..17: Output gain = 0
        assertEquals(0.toByte(), csd0[16])
        assertEquals(0.toByte(), csd0[17])

        // 18: Mapping family = 0
        assertEquals(0.toByte(), csd0[18])
    }

    @Test
    fun testMediaCodecAudioDecoder_Csd1AndCsd2Structure() {
        val csd1 = MediaCodecAudioDecoder.buildPreSkipCsd1(0L)
        assertEquals(8, csd1.size)
        val csd1Val = ByteBuffer.wrap(csd1).order(ByteOrder.LITTLE_ENDIAN).getLong()
        assertEquals(0L, csd1Val)

        val csd2 = MediaCodecAudioDecoder.buildSeekPreRollCsd2(80_000_000L)
        assertEquals(8, csd2.size)
        val csd2Val = ByteBuffer.wrap(csd2).order(ByteOrder.LITTLE_ENDIAN).getLong()
        assertEquals(80_000_000L, csd2Val)
    }

    @Test
    fun testMediaCodecAudioDecoder_Lifecycle() {
        val decoder = MediaCodecAudioDecoder()
        decoder.init(48000, 2)
        assertTrue(decoder.isInitialized)

        decoder.flush()
        decoder.release()
        assertFalse(decoder.isInitialized)
    }

    @Test
    fun testMediaCodecAudioDecoder_UninitializedDecodeSafeNoOp() {
        val decoder = MediaCodecAudioDecoder()
        assertFalse(decoder.isInitialized)
        var called = false
        decoder.decode(byteArrayOf(1, 2, 3)) { _, _ -> called = true }
        assertFalse(called)
        decoder.flush()
        decoder.release()
    }
}
