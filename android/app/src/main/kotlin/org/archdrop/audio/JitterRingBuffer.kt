package org.archdrop.audio

import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * Telemetry snapshot for audio receiver performance and buffer health.
 */
data class JitterTelemetry(
    val bufferDepthMs: Double,
    val targetDepthMs: Double,
    val jitterMs: Double,
    val bitrateKbps: Double,
    val packetsReceived: Long,
    val packetsLost: Long,
    val expansions: Long,
    val contractions: Long,
    val underruns: Long,
    val overruns: Long
)

/**
 * Dynamic Jitter Ring Buffer for low-latency wireless desktop audio streaming.
 *
 * Maintains a 500 ms preallocated circular buffer (96,000 bytes for 48 kHz 16-bit stereo PCM)
 * with a 30–60 ms sweet spot (40 ms nominal target).
 *
 * Implements gentle zero-crossing drift compensation:
 * - Low-water (<20 ms): duplicate 96 stereo samples (2 ms) across zero crossing.
 * - High-water (>70 ms): drop 96 stereo samples (2 ms) across zero crossing.
 *
 * Includes Packet Loss Concealment (PLC) with gentle exponential decay and RFC 3550 EWMA jitter tracking.
 */
class JitterRingBuffer(
    val capacityMs: Int = CAPACITY_MS_DEFAULT,
    val targetDepthMs: Double = TARGET_DEPTH_MS_DEFAULT,
    val lowWaterMs: Double = LOW_WATER_MS_DEFAULT,
    val highWaterMs: Double = HIGH_WATER_MS_DEFAULT,
    val compensationSamples: Int = COMPENSATION_SAMPLES_DEFAULT
) {

    companion object {
        const val CAPACITY_MS_DEFAULT = 500
        const val TARGET_DEPTH_MS_DEFAULT = 60.0   // 60 ms nominal — stays well clear of both water marks on Wi-Fi
        const val MIN_TARGET_DEPTH_MS = 40.0
        const val MAX_TARGET_DEPTH_MS = 80.0
        const val LOW_WATER_MS_DEFAULT = 20.0
        const val HIGH_WATER_MS_DEFAULT = 120.0    // Raised from 70 ms: Wi-Fi jitter typically lands 60–100 ms, avoid constant drops
        const val COMPENSATION_SAMPLES_DEFAULT = 96 // 2 ms at 48 kHz (96 stereo samples = 384 bytes)

        const val BYTES_PER_SAMPLE = 2 // 16-bit signed PCM
        const val CHANNELS = 2 // Stereo
        const val BYTES_PER_FRAME = BYTES_PER_SAMPLE * CHANNELS // 4 bytes
        const val BYTES_PER_MS = 192 // 48000 * 4 / 1000

        const val CAPACITY_BYTES = CAPACITY_MS_DEFAULT * BYTES_PER_MS // 96,000 bytes
        const val COMPENSATION_BYTES = COMPENSATION_SAMPLES_DEFAULT * BYTES_PER_FRAME // 384 bytes
        const val PLC_FRAME_BYTES = DEFAULT_FRAME_DURATION_MS * BYTES_PER_MS // 3840 bytes (20 ms)
    }

    private val lock = ReentrantLock()
    private val buffer = ByteArray(capacityMs * BYTES_PER_MS)
    private val capacityBytes = buffer.size

    private var head = 0 // Write index
    private var tail = 0 // Read index
    private var available = 0 // Available bytes in ring buffer

    // Telemetry counters
    private var packetsReceivedCount = 0L
    private var packetsLostCount = 0L
    private var expansionCount = 0L
    private var contractionCount = 0L
    private var underrunCount = 0L
    private var overrunCount = 0L

    // Jitter tracking (RFC 3550 EWMA)
    private var prevTransitMs: Long? = null
    private var ewmaJitterMs: Double = 0.0

    // Bitrate calculation (rolling 1-second sliding accumulator)
    private val bitrateLock = Any()
    private val byteTimestamps = LongArray(128)
    private val byteCounts = IntArray(128)
    private var byteWindowHead = 0
    private var byteWindowCount = 0

    // PLC State
    private var lastValidFrame: ByteArray? = null
    private var consecutivePlcCount = 0

    // Minimum time between drift compensations — raised to 250 ms to prevent overcorrection
    // on bursty Wi-Fi arrivals that cause the buffer to transiently spike then drain.
    private var lastCompensationTimeMs: Long = 0L
    private val minCompensationIntervalMs: Long = 250L

    /**
     * Current buffer depth in milliseconds.
     */
    val depthMs: Double
        get() = lock.withLock { available.toDouble() / BYTES_PER_MS }

    /**
     * Current available bytes in buffer.
     */
    val availableBytes: Int
        get() = lock.withLock { available }

    /**
     * Capacity in bytes.
     */
    val capacity: Int
        get() = capacityBytes

    /**
     * Writes decoded 16-bit PCM bytes into the ring buffer, applying zero-crossing drift compensation
     * if the buffer is below low-water (<20 ms) or above high-water (>70 ms).
     */
    fun writePcm(
        pcm: ByteArray,
        offset: Int = 0,
        length: Int = pcm.size,
        ptsMs: Long? = null,
        nowMs: Long = System.currentTimeMillis()
    ): Int {
        if (length <= 0) return 0

        // Track jitter transit time if timestamp provided
        if (ptsMs != null) {
            updateTransitTime(ptsMs, nowMs)
        }
        recordBytes(length, nowMs)

        lock.withLock {
            packetsReceivedCount++
            consecutivePlcCount = 0

            // Cache last full frame for PLC
            if (length >= PLC_FRAME_BYTES) {
                if (lastValidFrame == null || lastValidFrame!!.size != PLC_FRAME_BYTES) {
                    lastValidFrame = ByteArray(PLC_FRAME_BYTES)
                }
                System.arraycopy(pcm, offset + length - PLC_FRAME_BYTES, lastValidFrame!!, 0, PLC_FRAME_BYTES)
            } else if (lastValidFrame == null) {
                lastValidFrame = ByteArray(length)
                System.arraycopy(pcm, offset, lastValidFrame!!, 0, length)
            }

            // Check drift compensation conditions
            val currentDepth = available.toDouble() / BYTES_PER_MS
            val timeSinceLastComp = nowMs - lastCompensationTimeMs
            var compensatedBytes: ByteArray? = null

            if (timeSinceLastComp >= minCompensationIntervalMs && length >= COMPENSATION_BYTES * 2) {
                if (currentDepth < lowWaterMs) {
                    // Low-water: duplicate 96 stereo samples (2 ms) across zero crossing
                    compensatedBytes = duplicateZeroCrossing(pcm, offset, length)
                    if (compensatedBytes != null) {
                        expansionCount++
                        lastCompensationTimeMs = nowMs
                    }
                } else if (currentDepth > highWaterMs) {
                    // High-water: drop 96 stereo samples (2 ms) across zero crossing
                    compensatedBytes = dropZeroCrossing(pcm, offset, length)
                    if (compensatedBytes != null) {
                        contractionCount++
                        lastCompensationTimeMs = nowMs
                    }
                }
            }

            val dataToWrite = compensatedBytes ?: pcm
            val dataOffset = if (compensatedBytes != null) 0 else offset
            val dataLen = if (compensatedBytes != null) compensatedBytes.size else length

            // Write into circular buffer
            return writeDirect(dataToWrite, dataOffset, dataLen)
        }
    }

    /**
     * Directly writes bytes into the ring buffer without drift compensation.
     * Drops oldest bytes if buffer overflows.
     */
    fun writeDirect(data: ByteArray, offset: Int = 0, length: Int = data.size): Int {
        lock.withLock {
            if (length <= 0) return 0

            val effectiveOffset = if (length > capacityBytes) offset + length - capacityBytes else offset
            val writeLen = minOf(length, capacityBytes)

            if (available + writeLen > capacityBytes || length > capacityBytes) {
                val overflow = (available + writeLen) - capacityBytes
                overrunCount++
                if (overflow > 0) {
                    tail = (tail + overflow) % capacityBytes
                    available -= overflow
                }
            }

            val firstChunk = minOf(writeLen, capacityBytes - head)
            System.arraycopy(data, effectiveOffset, buffer, head, firstChunk)

            val secondChunk = writeLen - firstChunk
            if (secondChunk > 0) {
                System.arraycopy(data, effectiveOffset + firstChunk, buffer, 0, secondChunk)
            }

            head = (head + writeLen) % capacityBytes
            available += writeLen
            return length
        }
    }

    /**
     * Reads up to [length] PCM bytes from the ring buffer into [dest].
     * Returns the actual number of bytes read. If empty, increments underruns.
     */
    fun readPcm(dest: ByteArray, offset: Int = 0, length: Int = dest.size): Int {
        lock.withLock {
            if (available == 0) {
                underrunCount++
                return 0
            }

            val toRead = minOf(length, available)
            val firstChunk = minOf(toRead, capacityBytes - tail)
            System.arraycopy(buffer, tail, dest, offset, firstChunk)

            val secondChunk = toRead - firstChunk
            if (secondChunk > 0) {
                System.arraycopy(buffer, 0, dest, offset + firstChunk, secondChunk)
            }

            tail = (tail + toRead) % capacityBytes
            available -= toRead
            return toRead
        }
    }

    /**
     * Finds the nearest zero crossing in 16-bit stereo PCM and duplicates 96 stereo samples (2 ms).
     */
    internal fun duplicateZeroCrossing(pcm: ByteArray, offset: Int, length: Int): ByteArray? {
        val totalSamples = length / BYTES_PER_FRAME
        if (totalSamples < compensationSamples * 2) return null

        val bestIndex = findZeroCrossingSampleIndex(pcm, offset, length, compensationSamples)
        val bestByteOffset = bestIndex * BYTES_PER_FRAME

        // New size is length + 384 bytes (COMPENSATION_BYTES)
        val out = ByteArray(length + COMPENSATION_BYTES)

        // Copy up to zero crossing
        System.arraycopy(pcm, offset, out, 0, bestByteOffset)
        // Insert duplicated 96 samples
        System.arraycopy(pcm, offset + bestByteOffset, out, bestByteOffset, COMPENSATION_BYTES)
        // Copy rest
        System.arraycopy(
            pcm,
            offset + bestByteOffset,
            out,
            bestByteOffset + COMPENSATION_BYTES,
            length - bestByteOffset
        )

        return out
    }

    /**
     * Finds the nearest zero crossing in 16-bit stereo PCM and drops 96 stereo samples (2 ms).
     */
    internal fun dropZeroCrossing(pcm: ByteArray, offset: Int, length: Int): ByteArray? {
        val totalSamples = length / BYTES_PER_FRAME
        if (totalSamples <= compensationSamples * 2) return null

        val bestIndex = findZeroCrossingSampleIndex(pcm, offset, length, compensationSamples)
        val bestByteOffset = bestIndex * BYTES_PER_FRAME

        // New size is length - 384 bytes (COMPENSATION_BYTES)
        val out = ByteArray(length - COMPENSATION_BYTES)

        // Copy up to zero crossing
        System.arraycopy(pcm, offset, out, 0, bestByteOffset)
        // Skip 96 samples, copy remaining
        val remaining = length - (bestByteOffset + COMPENSATION_BYTES)
        if (remaining > 0) {
            System.arraycopy(
                pcm,
                offset + bestByteOffset + COMPENSATION_BYTES,
                out,
                bestByteOffset,
                remaining
            )
        }

        return out
    }

    /**
     * Scans for the optimal zero crossing sample index within [0, totalSamples - searchWindow].
     * Minimizes |sampleLeft| + |sampleRight| or detects sign alternation.
     */
    private fun findZeroCrossingSampleIndex(
        pcm: ByteArray,
        offset: Int,
        length: Int,
        searchWindow: Int
    ): Int {
        val totalSamples = length / BYTES_PER_FRAME
        val maxCandidate = totalSamples - searchWindow
        if (maxCandidate <= 0) return 0

        var minAmp = Int.MAX_VALUE
        var bestIdx = 0

        var prevLeft = 0
        var prevRight = 0

        for (i in 0 until maxCandidate) {
            val bytePos = offset + (i * BYTES_PER_FRAME)
            val left = readShortLe(pcm, bytePos).toInt()
            val right = readShortLe(pcm, bytePos + 2).toInt()

            val amp = abs(left) + abs(right)

            // Exact zero or sign change across adjacent samples is an ideal zero crossing
            val signChange = (i > 0) && ((prevLeft <= 0 && left >= 0) || (prevLeft >= 0 && left <= 0))
            if (signChange && amp < 500) {
                return i
            }

            if (amp < minAmp) {
                minAmp = amp
                bestIdx = i
                if (amp == 0) break
            }

            prevLeft = left
            prevRight = right
        }

        return bestIdx
    }

    /**
     * Generates a Packet Loss Concealment (PLC) frame by repeating the last valid frame
     * with an exponential amplitude decay to smoothly prevent harsh silence clicks.
     */
    fun generatePlcFrame(decayFactor: Float = 0.7f): ByteArray {
        lock.withLock {
            consecutivePlcCount++
            val template = lastValidFrame ?: return ByteArray(PLC_FRAME_BYTES)
            val out = ByteArray(template.size)

            val currentDecay = Math.pow(decayFactor.toDouble(), consecutivePlcCount.toDouble()).toFloat()

            for (i in 0 until template.size step 2) {
                val sample = readShortLe(template, i).toInt()
                val decayed = (sample * currentDecay).roundToInt().coerceIn(-32768, 32767)
                writeShortLe(out, i, decayed.toShort())
            }

            return out
        }
    }

    /**
     * Records a sequence gap indicating lost packets.
     */
    fun recordPacketLoss(lostCount: Long) {
        if (lostCount > 0) {
            lock.withLock {
                packetsLostCount += lostCount
            }
        }
    }

    /**
     * Updates RFC 3550 transit delay EWMA jitter calculation.
     */
    fun updateTransitTime(ptsMs: Long, nowMs: Long = System.currentTimeMillis()) {
        val transit = nowMs - ptsMs
        val prev = prevTransitMs
        if (prev != null) {
            val delta = abs(transit - prev).toDouble()
            // EWMA filter: J_k = 0.875 * J_{k-1} + 0.125 * delta
            ewmaJitterMs = (0.875 * ewmaJitterMs) + (0.125 * delta)
        }
        prevTransitMs = transit
    }

    /**
     * Records bytes received for live bitrate calculation.
     */
    fun recordBytes(bytes: Int, nowMs: Long = System.currentTimeMillis()) {
        synchronized(bitrateLock) {
            byteTimestamps[byteWindowHead] = nowMs
            byteCounts[byteWindowHead] = bytes
            byteWindowHead = (byteWindowHead + 1) % byteTimestamps.size
            if (byteWindowCount < byteTimestamps.size) {
                byteWindowCount++
            }
        }
    }

    /**
     * Calculates live bitrate in kbps over a 1-second rolling window.
     */
    fun getLiveBitrateKbps(nowMs: Long = System.currentTimeMillis()): Double {
        synchronized(bitrateLock) {
            if (byteWindowCount == 0) return 0.0

            var totalBytes = 0L
            val windowCutoff = nowMs - 1000L

            for (i in 0 until byteWindowCount) {
                val idx = (byteWindowHead - 1 - i + byteTimestamps.size) % byteTimestamps.size
                if (byteTimestamps[idx] >= windowCutoff) {
                    totalBytes += byteCounts[idx]
                }
            }

            // (bytes * 8 bits/byte) / 1000 = kbps
            return (totalBytes * 8.0) / 1000.0
        }
    }

    /**
     * Returns a point-in-time telemetry snapshot.
     */
    fun getTelemetry(nowMs: Long = System.currentTimeMillis()): JitterTelemetry {
        lock.withLock {
            return JitterTelemetry(
                bufferDepthMs = available.toDouble() / BYTES_PER_MS,
                targetDepthMs = targetDepthMs,
                jitterMs = ewmaJitterMs,
                bitrateKbps = getLiveBitrateKbps(nowMs),
                packetsReceived = packetsReceivedCount,
                packetsLost = packetsLostCount,
                expansions = expansionCount,
                contractions = contractionCount,
                underruns = underrunCount,
                overruns = overrunCount
            )
        }
    }

    /**
     * Flushes the ring buffer and resets indexes.
     */
    fun clear() {
        lock.withLock {
            head = 0
            tail = 0
            available = 0
            consecutivePlcCount = 0
        }
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
