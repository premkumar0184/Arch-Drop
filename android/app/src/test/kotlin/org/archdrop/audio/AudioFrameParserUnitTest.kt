package org.archdrop.audio

import okio.Buffer
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.EOFException

class AudioFrameParserUnitTest {

    @Test
    fun testHeaderSerializationRoundTrip_Opus() {
        val original = AudioHeader(
            codec = AudioCodec.OPUS,
            channels = 2,
            flags = FLAG_NORMAL,
            sampleRate = 48000,
            sequence = 123456789L,
            timestampMs = 1700000000000L,
            payloadLen = 240
        )

        val bytes = original.toByteArray()
        assertEquals(HEADER_SIZE, bytes.size)
        assertEquals(32, bytes.size)

        // Verify exact byte positions
        assertEquals(0x41.toByte(), bytes[0]) // 'A'
        assertEquals(0x44.toByte(), bytes[1]) // 'D'
        assertEquals(0x41.toByte(), bytes[2]) // 'A'
        assertEquals(0x55.toByte(), bytes[3]) // 'U'
        assertEquals(0x01.toByte(), bytes[4]) // Opus codec
        assertEquals(0x02.toByte(), bytes[5]) // Stereo channels
        assertEquals(0x00.toByte(), bytes[6]) // Flags u16 BE
        assertEquals(0x00.toByte(), bytes[7])

        // Parse back
        val parsed = AudioHeader.fromByteArray(bytes)
        assertEquals(original, parsed)
        assertFalse(parsed.isDiscontinuity)
    }

    @Test
    fun testHeaderSerializationRoundTrip_PCM_Discontinuity() {
        val original = AudioHeader(
            codec = AudioCodec.PCM,
            channels = 2,
            flags = FLAG_DISCONTINUITY,
            sampleRate = 48000,
            sequence = 9999L,
            timestampMs = 1700000005000L,
            payloadLen = 3840
        )

        val bytes = original.toByteArray()
        val parsed = AudioHeader.fromByteArray(bytes)

        assertEquals(original, parsed)
        assertTrue(parsed.isDiscontinuity)
        assertEquals(AudioCodec.PCM, parsed.codec)
        assertEquals(3840, parsed.payloadLen)
    }

    @Test
    fun testInvalidMagicThrowsFramingException() {
        val header = AudioHeader(
            codec = AudioCodec.OPUS,
            channels = 2,
            flags = FLAG_NORMAL,
            sampleRate = 48000,
            sequence = 1L,
            timestampMs = 1000L,
            payloadLen = 100
        )
        val bytes = header.toByteArray()
        bytes[0] = 0x58 // Corrupt 'A' to 'X'

        try {
            AudioHeader.fromByteArray(bytes)
            fail("Expected AudioFramingException on invalid magic")
        } catch (e: AudioFramingException) {
            assertTrue(e.message?.contains("Invalid magic bytes") == true)
        }
    }

    @Test
    fun testInvalidCodecIdThrowsFramingException() {
        val header = AudioHeader(
            codec = AudioCodec.OPUS,
            channels = 2,
            flags = FLAG_NORMAL,
            sampleRate = 48000,
            sequence = 1L,
            timestampMs = 1000L,
            payloadLen = 100
        )
        val bytes = header.toByteArray()
        bytes[4] = 0x7F // Invalid codec ID

        try {
            AudioHeader.fromByteArray(bytes)
            fail("Expected AudioFramingException on unknown codec ID")
        } catch (e: AudioFramingException) {
            assertTrue(e.message?.contains("Unknown audio codec ID") == true)
        }
    }

    @Test
    fun testBufferTooShortThrowsFramingException() {
        val shortBytes = ByteArray(31) // Need at least 32 bytes
        try {
            AudioHeader.fromByteArray(shortBytes)
            fail("Expected AudioFramingException on short buffer")
        } catch (e: AudioFramingException) {
            assertTrue(e.message?.contains("Buffer too short") == true)
        }
    }

    @Test
    fun testAudioFrameParseRoundtrip() {
        val payload = ByteArray(256) { (it and 0xFF).toByte() }
        val header = AudioHeader(
            codec = AudioCodec.OPUS,
            channels = 2,
            flags = FLAG_NORMAL,
            sampleRate = 48000,
            sequence = 42L,
            timestampMs = 1700000001000L,
            payloadLen = payload.size
        )
        val frame = AudioFrame(header, payload)
        val wireBytes = frame.toByteArray()

        assertEquals(HEADER_SIZE + payload.size, wireBytes.size)

        val parsed = AudioFrame.parse(wireBytes)
        assertEquals(frame, parsed)
        assertArrayEquals(payload, parsed.payload)
    }

    @Test
    fun testStreamParser_CleanSequentialFrames() {
        val buffer = Buffer()
        val parser = AudioFrameParser()

        // Create 3 sequential frames
        val payload1 = ByteArray(100) { 1 }
        val payload2 = ByteArray(200) { 2 }
        val payload3 = ByteArray(300) { 3 }

        val frame1 = AudioFrame(AudioHeader(codec = AudioCodec.OPUS, sequence = 0, timestampMs = 100, payloadLen = 100), payload1)
        val frame2 = AudioFrame(AudioHeader(codec = AudioCodec.OPUS, sequence = 1, timestampMs = 120, payloadLen = 200), payload2)
        val frame3 = AudioFrame(AudioHeader(codec = AudioCodec.PCM, sequence = 2, timestampMs = 140, payloadLen = 300), payload3)

        buffer.write(frame1.toByteArray())
        buffer.write(frame2.toByteArray())
        buffer.write(frame3.toByteArray())

        val read1 = parser.readNextFrame(buffer)
        assertEquals(frame1, read1)

        val read2 = parser.readNextFrame(buffer)
        assertEquals(frame2, read2)

        val read3 = parser.readNextFrame(buffer)
        assertEquals(frame3, read3)

        assertEquals(0L, parser.corruptedBytesSkipped)

        try {
            parser.readNextFrame(buffer)
            fail("Expected EOFException after all frames read")
        } catch (e: EOFException) {
            // Expected
        }
    }

    @Test
    fun testStreamParser_ByteByByteResyncAfterCorruption() {
        val buffer = Buffer()
        val parser = AudioFrameParser()

        val validPayload = ByteArray(120) { 0x42.toByte() }
        val validFrame = AudioFrame(
            AudioHeader(codec = AudioCodec.OPUS, sequence = 10, timestampMs = 1000, payloadLen = 120),
            validPayload
        )

        // Prepend 15 random noise bytes, including partial magic "ADA"
        val noise = byteArrayOf(
            0x00, 0xFF.toByte(), 0x12, 0x34,
            0x41, // 'A'
            0x44, // 'D'
            0x41, // 'A'
            0x58, // 'X' (not 'U', aborts candidate)
            0x77, 0x88.toByte(), 0x99.toByte(), 0x01, 0x02, 0x03, 0x04
        )
        buffer.write(noise)
        buffer.write(validFrame.toByteArray())

        val parsed = parser.readNextFrame(buffer)
        assertEquals(validFrame, parsed)
        assertArrayEquals(validPayload, parsed.payload)
        assertTrue("Corrupted bytes skipped must be >= noise length", parser.corruptedBytesSkipped >= noise.size)
    }

    @Test
    fun testStreamParser_InputStreamResync() {
        val validPayload = ByteArray(64) { 0x7F.toByte() }
        val validFrame = AudioFrame(
            AudioHeader(codec = AudioCodec.PCM, sequence = 1, timestampMs = 500, payloadLen = 64),
            validPayload
        )

        val noise = byteArrayOf(0x01, 0x02, 0x03, 0x04, 0x05)
        val combined = noise + validFrame.toByteArray()

        val stream = ByteArrayInputStream(combined)
        val parser = AudioFrameParser()

        val parsed = parser.readNextFrame(stream)
        assertEquals(validFrame, parsed)
        assertArrayEquals(validPayload, parsed.payload)
        assertEquals(noise.size.toLong(), parser.corruptedBytesSkipped)
    }
}
