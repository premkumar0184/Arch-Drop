package org.archdrop.audio

import okio.BufferedSource
import java.io.EOFException
import java.io.InputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Constants for canonical 32-byte binary wire framing ("ADAU").
 */
val ADAU_MAGIC = byteArrayOf(0x41, 0x44, 0x41, 0x55) // "ADAU"
const val HEADER_SIZE = 32
const val CODEC_OPUS: Byte = 0x01
const val CODEC_PCM: Byte = 0x02
const val CHANNELS_STEREO: Byte = 0x02
const val FLAG_NORMAL: Int = 0x0000
const val FLAG_DISCONTINUITY: Int = 0x0001
const val DEFAULT_SAMPLE_RATE: Int = 48000
const val DEFAULT_CHANNELS: Int = 2
const val DEFAULT_FRAME_DURATION_MS: Int = 20
const val PCM_BYTES_PER_MS: Int = 192 // 48000 * 2 channels * 2 bytes/sample / 1000
const val PCM_FRAME_SIZE_BYTES: Int = 3840 // 20 ms * 192 bytes/ms

/**
 * Supported wire audio codecs.
 */
enum class AudioCodec(val id: Byte) {
    OPUS(CODEC_OPUS),
    PCM(CODEC_PCM);

    companion object {
        fun fromId(id: Byte): AudioCodec = when (id) {
            CODEC_OPUS -> OPUS
            CODEC_PCM -> PCM
            else -> throw AudioFramingException("Unknown audio codec ID: 0x%02X (expected 0x01=Opus, 0x02=PCM)".format(id))
        }

        fun fromString(str: String): AudioCodec = when (str.lowercase()) {
            "opus" -> OPUS
            "pcm" -> PCM
            else -> throw IllegalArgumentException("Unknown codec string: $str")
        }
    }

    fun asParam(): String = when (this) {
        OPUS -> "opus"
        PCM -> "pcm"
    }
}

/**
 * Framing exception thrown when header magic or wire framing invariants are violated.
 */
class AudioFramingException(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * Canonical 32-byte binary wire header ("ADAU").
 *
 * Offset  Bytes  Field           Type      Description
 * 0       4      Magic           [u8; 4]   0x41, 0x44, 0x41, 0x55 ("ADAU")
 * 4       1      Codec           u8        0x01: Opus (48kHz stereo), 0x02: PCM (s16le stereo)
 * 5       1      Channels        u8        0x02: Stereo
 * 6       2      Flags           u16 BE    0x0001: Discontinuity, 0x0000: Normal
 * 8       4      SampleRate      u32 BE    48000 Hz
 * 12      8      Sequence        u64 BE    Monotonic packet counter (0, 1, 2...)
 * 20      8      TimestampMs     u64 BE    Unix epoch milliseconds
 * 28      4      PayloadLen      u32 BE    Payload byte length N following header
 */
data class AudioHeader(
    val magic: ByteArray = ADAU_MAGIC.clone(),
    val codec: AudioCodec,
    val channels: Int = DEFAULT_CHANNELS,
    val flags: Int = FLAG_NORMAL,
    val sampleRate: Int = DEFAULT_SAMPLE_RATE,
    val sequence: Long,
    val timestampMs: Long,
    val payloadLen: Int
) {
    init {
        require(channels in 1..8) { "Invalid channels: $channels" }
        require(sampleRate > 0) { "Invalid sample rate: $sampleRate" }
        require(payloadLen >= 0) { "Payload length cannot be negative: $payloadLen" }
    }

    val isDiscontinuity: Boolean
        get() = (flags and FLAG_DISCONTINUITY) != 0

    fun toByteArray(): ByteArray {
        val buf = ByteBuffer.allocate(HEADER_SIZE).order(ByteOrder.BIG_ENDIAN)
        buf.put(magic)
        buf.put(codec.id)
        buf.put(channels.toByte())
        buf.putShort(flags.toShort())
        buf.putInt(sampleRate)
        buf.putLong(sequence)
        buf.putLong(timestampMs)
        buf.putInt(payloadLen)
        return buf.array()
    }

    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (javaClass != other?.javaClass) return false
        other as AudioHeader
        if (!magic.contentEquals(other.magic)) return false
        if (codec != other.codec) return false
        if (channels != other.channels) return false
        if (flags != other.flags) return false
        if (sampleRate != other.sampleRate) return false
        if (sequence != other.sequence) return false
        if (timestampMs != other.timestampMs) return false
        if (payloadLen != other.payloadLen) return false
        return true
    }

    override fun hashCode(): Int {
        var result = magic.contentHashCode()
        result = 31 * result + codec.hashCode()
        result = 31 * result + channels
        result = 31 * result + flags
        result = 31 * result + sampleRate
        result = 31 * result + sequence.hashCode()
        result = 31 * result + timestampMs.hashCode()
        result = 31 * result + payloadLen
        return result
    }

    companion object {
        fun fromByteArray(bytes: ByteArray, offset: Int = 0): AudioHeader {
            if (bytes.size - offset < HEADER_SIZE) {
                throw AudioFramingException("Buffer too short for audio header: need at least $HEADER_SIZE bytes, got ${bytes.size - offset}")
            }
            val buf = ByteBuffer.wrap(bytes, offset, HEADER_SIZE).order(ByteOrder.BIG_ENDIAN)
            val magic = ByteArray(4)
            buf.get(magic)
            if (!magic.contentEquals(ADAU_MAGIC)) {
                throw AudioFramingException(
                    "Invalid magic bytes: expected ADAU (%02X %02X %02X %02X), got (%02X %02X %02X %02X)".format(
                        ADAU_MAGIC[0], ADAU_MAGIC[1], ADAU_MAGIC[2], ADAU_MAGIC[3],
                        magic[0], magic[1], magic[2], magic[3]
                    )
                )
            }
            val codecId = buf.get()
            val codec = AudioCodec.fromId(codecId)
            val channels = buf.get().toInt() and 0xFF
            val flags = buf.getShort().toInt() and 0xFFFF
            val sampleRate = buf.getInt()
            val sequence = buf.getLong()
            val timestampMs = buf.getLong()
            val payloadLen = buf.getInt()

            return AudioHeader(
                magic = magic,
                codec = codec,
                channels = channels,
                flags = flags,
                sampleRate = sampleRate,
                sequence = sequence,
                timestampMs = timestampMs,
                payloadLen = payloadLen
            )
        }
    }
}

/**
 * Complete framed audio packet containing 32-byte ADAU header and payload bytes.
 */
data class AudioFrame(
    val header: AudioHeader,
    val payload: ByteArray
) {
    fun toByteArray(): ByteArray {
        val headerBytes = header.toByteArray()
        val out = ByteArray(HEADER_SIZE + payload.size)
        System.arraycopy(headerBytes, 0, out, 0, HEADER_SIZE)
        System.arraycopy(payload, 0, out, HEADER_SIZE, payload.size)
        return out
    }

    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (javaClass != other?.javaClass) return false
        other as AudioFrame
        if (header != other.header) return false
        if (!payload.contentEquals(other.payload)) return false
        return true
    }

    override fun hashCode(): Int {
        var result = header.hashCode()
        result = 31 * result + payload.contentHashCode()
        return result
    }

    companion object {
        fun parse(bytes: ByteArray, offset: Int = 0): AudioFrame {
            val header = AudioHeader.fromByteArray(bytes, offset)
            val totalNeeded = HEADER_SIZE + header.payloadLen
            if (bytes.size - offset < totalNeeded) {
                throw AudioFramingException("Buffer too short for payload: need $totalNeeded bytes, got ${bytes.size - offset}")
            }
            val payload = ByteArray(header.payloadLen)
            System.arraycopy(bytes, offset + HEADER_SIZE, payload, 0, header.payloadLen)
            return AudioFrame(header, payload)
        }
    }
}

/**
 * Stream frame parser supporting byte-by-byte resynchronization.
 *
 * Scans byte-by-byte for ADAU magic ('A', 'D', 'A', 'U') if corrupted or misaligned
 * bytes are received on the wire, drops malformed data, and tracks resync statistics.
 */
class AudioFrameParser(
    private val maxPayloadLen: Int = 128 * 1024 // 128 KB safety ceiling
) {
    var corruptedBytesSkipped: Long = 0
        private set

    /**
     * Reads the next valid AudioFrame from an Okio BufferedSource.
     * Throws EOFException if end of stream is reached cleanly before reading any bytes of a new frame.
     */
    fun readNextFrame(source: BufferedSource): AudioFrame {
        var state = 0
        while (true) {
            if (source.exhausted()) {
                throw EOFException("End of stream reached")
            }

            val b = source.readByte()
            when (state) {
                0 -> {
                    if (b == 0x41.toByte()) { // 'A'
                        state = 1
                    } else {
                        corruptedBytesSkipped++
                    }
                }
                1 -> {
                    if (b == 0x44.toByte()) { // 'D'
                        state = 2
                    } else if (b == 0x41.toByte()) { // Another 'A'
                        state = 1
                        corruptedBytesSkipped++
                    } else {
                        state = 0
                        corruptedBytesSkipped += 2
                    }
                }
                2 -> {
                    if (b == 0x41.toByte()) { // 'A'
                        state = 3
                    } else {
                        state = 0
                        corruptedBytesSkipped += 3
                    }
                }
                3 -> {
                    if (b == 0x55.toByte()) { // 'U' - full magic matched!
                        // Now read the remaining 28 bytes of the 32-byte header
                        val restOfHeader = ByteArray(28)
                        try {
                            source.readFully(restOfHeader)
                        } catch (e: EOFException) {
                            corruptedBytesSkipped += 4 + restOfHeader.size
                            throw e
                        }

                        val fullHeaderBytes = ByteArray(HEADER_SIZE)
                        System.arraycopy(ADAU_MAGIC, 0, fullHeaderBytes, 0, 4)
                        System.arraycopy(restOfHeader, 0, fullHeaderBytes, 4, 28)

                        val header = try {
                            val candidate = AudioHeader.fromByteArray(fullHeaderBytes)
                            if (candidate.payloadLen > maxPayloadLen || candidate.sampleRate <= 0) {
                                null
                            } else {
                                candidate
                            }
                        } catch (e: Exception) {
                            null
                        }

                        if (header != null) {
                            val payload = ByteArray(header.payloadLen)
                            source.readFully(payload)
                            return AudioFrame(header, payload)
                        } else {
                            // False positive magic inside random noise: resume scanning
                            corruptedBytesSkipped += HEADER_SIZE
                            state = 0
                        }
                    } else if (b == 0x44.toByte()) { // 'D'
                        state = 2
                        corruptedBytesSkipped += 2
                    } else if (b == 0x41.toByte()) { // 'A'
                        state = 1
                        corruptedBytesSkipped += 3
                    } else {
                        state = 0
                        corruptedBytesSkipped += 4
                    }
                }
            }
        }
    }

    /**
     * Reads the next valid AudioFrame from a standard java.io.InputStream.
     */
    fun readNextFrame(stream: InputStream): AudioFrame {
        var state = 0
        while (true) {
            val byteInt = stream.read()
            if (byteInt == -1) {
                throw EOFException("End of stream reached")
            }
            val b = byteInt.toByte()

            when (state) {
                0 -> {
                    if (b == 0x41.toByte()) {
                        state = 1
                    } else {
                        corruptedBytesSkipped++
                    }
                }
                1 -> {
                    if (b == 0x44.toByte()) {
                        state = 2
                    } else if (b == 0x41.toByte()) {
                        state = 1
                        corruptedBytesSkipped++
                    } else {
                        state = 0
                        corruptedBytesSkipped += 2
                    }
                }
                2 -> {
                    if (b == 0x41.toByte()) {
                        state = 3
                    } else {
                        state = 0
                        corruptedBytesSkipped += 3
                    }
                }
                3 -> {
                    if (b == 0x55.toByte()) {
                        val restOfHeader = ByteArray(28)
                        readFully(stream, restOfHeader)

                        val fullHeaderBytes = ByteArray(HEADER_SIZE)
                        System.arraycopy(ADAU_MAGIC, 0, fullHeaderBytes, 0, 4)
                        System.arraycopy(restOfHeader, 0, fullHeaderBytes, 4, 28)

                        val header = try {
                            val candidate = AudioHeader.fromByteArray(fullHeaderBytes)
                            if (candidate.payloadLen > maxPayloadLen || candidate.sampleRate <= 0) {
                                null
                            } else {
                                candidate
                            }
                        } catch (e: Exception) {
                            null
                        }

                        if (header != null) {
                            val payload = ByteArray(header.payloadLen)
                            readFully(stream, payload)
                            return AudioFrame(header, payload)
                        } else {
                            corruptedBytesSkipped += HEADER_SIZE
                            state = 0
                        }
                    } else if (b == 0x44.toByte()) { // 'D'
                        state = 2
                        corruptedBytesSkipped += 2
                    } else if (b == 0x41.toByte()) { // 'A'
                        state = 1
                        corruptedBytesSkipped += 3
                    } else {
                        state = 0
                        corruptedBytesSkipped += 4
                    }
                }
            }
        }
    }

    private fun readFully(stream: InputStream, buffer: ByteArray) {
        var totalRead = 0
        while (totalRead < buffer.size) {
            val read = stream.read(buffer, totalRead, buffer.size - totalRead)
            if (read == -1) {
                throw EOFException("Premature EOF while reading ${buffer.size} bytes (read $totalRead bytes)")
            }
            totalRead += read
        }
    }
}
