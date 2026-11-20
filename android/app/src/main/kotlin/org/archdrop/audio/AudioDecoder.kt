package org.archdrop.audio

import android.media.AudioFormat
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Common pluggable audio decoder interface.
 */
interface AudioDecoder {
    /**
     * Initializes the decoder with the specified sample rate and channel count.
     */
    fun init(sampleRate: Int = DEFAULT_SAMPLE_RATE, channels: Int = DEFAULT_CHANNELS)

    /**
     * Decodes an incoming audio payload (e.g. 20 ms Opus frame or PCM buffer).
     * Decoded raw 16-bit signed stereo PCM bytes are emitted via [onPcmOutput].
     */
    fun decode(
        payload: ByteArray,
        offset: Int = 0,
        length: Int = payload.size,
        ptsMs: Long = 0,
        onPcmOutput: (ByteArray, Int) -> Unit
    )

    /**
     * Flushes internal decoder queues (e.g. on stream discontinuity or seeking).
     */
    fun flush()

    /**
     * Releases decoder resources.
     */
    fun release()
}

/**
 * Direct passthrough decoder for raw 16-bit stereo PCM streams (s16le).
 * Completely bypasses hardware decoding for zero-CPU local streaming.
 */
class RawPcmPassthroughDecoder : AudioDecoder {
    var isInitialized: Boolean = false
        private set

    override fun init(sampleRate: Int, channels: Int) {
        isInitialized = true
    }

    override fun decode(
        payload: ByteArray,
        offset: Int,
        length: Int,
        ptsMs: Long,
        onPcmOutput: (ByteArray, Int) -> Unit
    ) {
        val out = ByteArray(length)
        System.arraycopy(payload, offset, out, 0, length)
        onPcmOutput(out, length)
    }

    override fun flush() {
        // No-op for unbuffered passthrough
    }

    override fun release() {
        isInitialized = false
    }
}

/**
 * Opus audio decoder utilizing Android's native MediaCodec framework.
 *
 * Configures the decoder using synthetic Codec-Specific Data (csd-0 OpusHead, csd-1, csd-2)
 * according to RFC 7845 and AOSP MediaCodec specifications.
 */
class MediaCodecAudioDecoder(
    private val customCodecName: String? = null
) : AudioDecoder {

    companion object {
        private const val TAG = "ArchDrop:MediaCodecDecoder"
        const val MIMETYPE_OPUS = "audio/opus"
        const val DEQUEUE_TIMEOUT_US = 5000L // 5ms timeout

        /**
         * Synthesizes 19-byte OpusHead structure (csd-0) per RFC 7845.
         *
         * Offset  Length  Field
         * 0       8       "OpusHead" magic
         * 8       1       Version (1)
         * 9       1       Channel count
         * 10      2       Pre-skip (0) (little-endian)
         * 12      4       Input sample rate (48000) (little-endian)
         * 16      2       Output gain (0) (little-endian)
         * 18      1       Channel mapping family (0)
         */
        fun buildOpusHeadCsd0(sampleRate: Int = DEFAULT_SAMPLE_RATE, channels: Int = DEFAULT_CHANNELS): ByteArray {
            val buf = ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN)
            buf.put("OpusHead".toByteArray(Charsets.US_ASCII)) // 8 bytes
            buf.put(1.toByte())                                // Version 1
            buf.put(channels.toByte())                         // Channels
            buf.putShort(0.toShort())                          // Pre-skip = 0
            buf.putInt(sampleRate)                             // Sample rate = 48000
            buf.putShort(0.toShort())                          // Output gain = 0 dB
            buf.put(0.toByte())                                // Mapping family 0 (mono/stereo)
            return buf.array()
        }

        /**
         * Synthesizes csd-1 (pre-skip in nanoseconds) as 8-byte little-endian Long.
         */
        fun buildPreSkipCsd1(preSkipNs: Long = 0L): ByteArray {
            return ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(preSkipNs).array()
        }

        /**
         * Synthesizes csd-2 (seek pre-roll in nanoseconds, typically 80ms) as 8-byte little-endian Long.
         */
        fun buildSeekPreRollCsd2(seekPreRollNs: Long = 80_000_000L): ByteArray {
            return ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(seekPreRollNs).array()
        }
    }

    private var codec: MediaCodec? = null
    var isInitialized: Boolean = false
        private set
    private val bufferInfo = MediaCodec.BufferInfo()

    override fun init(sampleRate: Int, channels: Int) {
        if (isInitialized) {
            release()
        }

        try {
            val format = MediaFormat.createAudioFormat(MIMETYPE_OPUS, sampleRate, channels)
            format.setInteger(MediaFormat.KEY_SAMPLE_RATE, sampleRate)
            format.setInteger(MediaFormat.KEY_CHANNEL_COUNT, channels)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                format.setInteger(MediaFormat.KEY_PCM_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
            }

            val csd0 = buildOpusHeadCsd0(sampleRate, channels)
            val csd1 = buildPreSkipCsd1(0L)
            val csd2 = buildSeekPreRollCsd2(80_000_000L) // 80 ms seek pre-roll

            format.setByteBuffer("csd-0", ByteBuffer.wrap(csd0))
            format.setByteBuffer("csd-1", ByteBuffer.wrap(csd1))
            format.setByteBuffer("csd-2", ByteBuffer.wrap(csd2))

            val instance = if (customCodecName != null) {
                MediaCodec.createByCodecName(customCodecName)
            } else {
                MediaCodec.createDecoderByType(MIMETYPE_OPUS)
            }

            instance.configure(format, null, null, 0)
            instance.start()
            codec = instance
            isInitialized = true
            Log.i(TAG, "MediaCodec Opus decoder initialized successfully ($sampleRate Hz, $channels ch)")
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to initialize native MediaCodec Opus decoder: ${e.message}", e)
            codec = null
            isInitialized = false
        }
    }

    override fun decode(
        payload: ByteArray,
        offset: Int,
        length: Int,
        ptsMs: Long,
        onPcmOutput: (ByteArray, Int) -> Unit
    ) {
        val activeCodec = codec ?: return

        try {
            // 1. Enqueue input buffer
            val inputIndex = activeCodec.dequeueInputBuffer(DEQUEUE_TIMEOUT_US)
            if (inputIndex >= 0) {
                val inputBuffer = activeCodec.getInputBuffer(inputIndex)
                if (inputBuffer != null) {
                    inputBuffer.clear()
                    inputBuffer.put(payload, offset, length)
                    val ptsUs = ptsMs * 1000L
                    activeCodec.queueInputBuffer(inputIndex, 0, length, ptsUs, 0)
                }
            }

            // 2. Dequeue output buffers
            drainOutput(activeCodec, onPcmOutput)
        } catch (e: Exception) {
            Log.w(TAG, "MediaCodec decode error: ${e.message}", e)
        }
    }

    private fun drainOutput(activeCodec: MediaCodec, onPcmOutput: (ByteArray, Int) -> Unit) {
        var outputIndex = activeCodec.dequeueOutputBuffer(bufferInfo, DEQUEUE_TIMEOUT_US)
        while (outputIndex >= 0) {
            val outputBuffer = activeCodec.getOutputBuffer(outputIndex)
            if (outputBuffer != null && bufferInfo.size > 0) {
                outputBuffer.position(bufferInfo.offset)
                outputBuffer.limit(bufferInfo.offset + bufferInfo.size)

                val pcmOut = ByteArray(bufferInfo.size)
                outputBuffer.get(pcmOut)
                onPcmOutput(pcmOut, bufferInfo.size)
            }
            activeCodec.releaseOutputBuffer(outputIndex, false)
            outputIndex = activeCodec.dequeueOutputBuffer(bufferInfo, 0)
        }
    }

    override fun flush() {
        try {
            codec?.flush()
        } catch (e: Exception) {
            Log.w(TAG, "Error flushing MediaCodec: ${e.message}", e)
        }
    }

    override fun release() {
        try {
            codec?.stop()
        } catch (_: Exception) {}
        try {
            codec?.release()
        } catch (_: Exception) {}
        codec = null
        isInitialized = false
    }
}

/**
 * Fake audio decoder for unit testing on standard JVM / Robolectric without native blobs.
 */
class FakeAudioDecoder(
    var simulatedOutputPcm: ByteArray? = null
) : AudioDecoder {
    var initCount: Int = 0
    var decodeCount: Int = 0
    var flushCount: Int = 0
    var releaseCount: Int = 0
    var lastSampleRate: Int = 0
    var lastChannels: Int = 0
    var lastPtsMs: Long = 0
    val receivedPayloads = mutableListOf<ByteArray>()

    override fun init(sampleRate: Int, channels: Int) {
        initCount++
        lastSampleRate = sampleRate
        lastChannels = channels
    }

    override fun decode(
        payload: ByteArray,
        offset: Int,
        length: Int,
        ptsMs: Long,
        onPcmOutput: (ByteArray, Int) -> Unit
    ) {
        decodeCount++
        lastPtsMs = ptsMs
        val copy = ByteArray(length)
        System.arraycopy(payload, offset, copy, 0, length)
        receivedPayloads.add(copy)

        val out = simulatedOutputPcm ?: copy
        onPcmOutput(out, out.size)
    }

    override fun flush() {
        flushCount++
    }

    override fun release() {
        releaseCount++
    }
}
