package org.archdrop.webcam

import java.io.ByteArrayOutputStream

/**
 * High-performance, zero-GC reusable [ByteArrayOutputStream] for low-latency streaming.
 *
 * Avoids repeated heap allocations by:
 * 1. Preallocating a large initial buffer (default: 256 KB).
 * 2. Providing [reset] to reset byte count to 0 without reallocating memory.
 * 3. Exposing the underlying byte array buffer directly via [toByteArrayBuffer]
 *    without defensive array cloning.
 */
class ReusableByteArrayOutputStream(initialCapacity: Int = 256 * 1024) :
    ByteArrayOutputStream(initialCapacity) {

    /**
     * Direct reference to the internal buffer [buf] without copying or allocation.
     * The valid data range is [0 until size()].
     */
    fun toByteArrayBuffer(): ByteArray = buf

    /**
     * Direct alias to [size()] returning current written byte count.
     */
    fun dataLength(): Int = count

    /**
     * Resets the buffer byte count to zero without freeing or reallocating the underlying array.
     */
    @Synchronized
    override fun reset() {
        count = 0
    }
}
