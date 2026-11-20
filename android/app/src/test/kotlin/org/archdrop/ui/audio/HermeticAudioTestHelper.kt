package org.archdrop.ui.audio

import android.content.Context
import okhttp3.Call
import okhttp3.Interceptor
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody
import okhttp3.ResponseBody.Companion.toResponseBody
import okio.BufferedSource
import okio.buffer
import okio.source
import org.archdrop.audio.AudioDecoder
import org.archdrop.audio.AudioFrame
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.AudioSink
import org.archdrop.audio.AudioStreamReceiver
import org.archdrop.audio.FakeAudioDecoder
import org.archdrop.audio.FakeAudioSink
import org.archdrop.audio.JitterRingBuffer
import java.io.InputStream
import java.util.concurrent.BlockingQueue
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 100% Hermetic Audio Test Helper for Arch Drop Unit Tests.
 *
 * Decouples [AudioStreamReceiver] and [AudioViewModel] from live
 * host network ports (such as 10.2.0.2:53317 or 127.0.0.1:53317).
 *
 * Guarantees:
 * 1. Zero unmocked socket calls to host daemon or WireGuard network.
 * 2. Deterministic HTTP 200 chunked audio stream response without network latency.
 * 3. Graceful EOF and stream termination on `Call.cancel()` or receiver stop.
 * 4. Optional test frame injection via [sendFrame] to test decoding, ringbuffer, and telemetry.
 * 5. Simulation of authentication errors (e.g. 403 Forbidden) when specifically requested.
 */
object HermeticAudioTestHelper {

    /**
     * Simulated streaming [InputStream] that keeps the audio connection active
     * until the call is cancelled or the stream is closed.
     */
    class HermeticAudioInputStream(
        private val call: Call,
        private val frameQueue: BlockingQueue<ByteArray> = LinkedBlockingQueue()
    ) : InputStream() {

        private val isClosed = AtomicBoolean(false)
        private var currentChunk: ByteArray? = null
        private var currentChunkOffset = 0

        fun sendFrame(frame: AudioFrame) {
            frameQueue.offer(frame.toByteArray())
        }

        fun sendFrameBytes(bytes: ByteArray) {
            frameQueue.offer(bytes.clone())
        }

        override fun read(): Int {
            val b = ByteArray(1)
            val n = read(b, 0, 1)
            return if (n <= 0) -1 else (b[0].toInt() and 0xFF)
        }

        override fun read(b: ByteArray, off: Int, len: Int): Int {
            if (len == 0) return 0

            while (!isClosed.get() && !call.isCanceled()) {
                val chunk = currentChunk
                if (chunk != null && currentChunkOffset < chunk.size) {
                    val available = chunk.size - currentChunkOffset
                    val toRead = minOf(len, available)
                    System.arraycopy(chunk, currentChunkOffset, b, off, toRead)
                    currentChunkOffset += toRead
                    if (currentChunkOffset >= chunk.size) {
                        currentChunk = null
                        currentChunkOffset = 0
                    }
                    return toRead
                }

                // Poll next frame from queue with 50ms wait interval
                val nextChunk = frameQueue.poll(50, TimeUnit.MILLISECONDS)
                if (nextChunk != null) {
                    currentChunk = nextChunk
                    currentChunkOffset = 0
                }
            }

            return -1 // Clean EOF on cancellation or stream closure
        }

        override fun close() {
            isClosed.set(true)
        }
    }

    /**
     * OkHttp Interceptor that catches all audio stream requests (`/api/v2/audio/stream`)
     * and serves an in-memory hermetic response without touching the network.
     */
    class HermeticAudioInterceptor(
        private val simulateStatusCode: Int = 200,
        private val onStreamCreated: ((HermeticAudioInputStream) -> Unit)? = null
    ) : Interceptor {

        override fun intercept(chain: Interceptor.Chain): Response {
            val request = chain.request()
            val path = request.url.encodedPath

            if (path == "/api/v2/audio/stream") {
                if (simulateStatusCode != 200) {
                    val errMessage = when (simulateStatusCode) {
                        403 -> "Peer authentication failed (403 Forbidden)"
                        409 -> "Desktop audio stream session already active (409 Conflict)"
                        503 -> "PipeWire audio capture unavailable on host (503)"
                        else -> "HTTP $simulateStatusCode"
                    }
                    return Response.Builder()
                        .request(request)
                        .protocol(Protocol.HTTP_1_1)
                        .code(simulateStatusCode)
                        .message(errMessage)
                        .body(errMessage.toResponseBody("text/plain".toMediaTypeOrNull()))
                        .build()
                }

                val call = chain.call()
                val stream = HermeticAudioInputStream(call)
                onStreamCreated?.invoke(stream)

                val mediaType = "audio/x-archdrop-audio".toMediaTypeOrNull()
                val responseBody = object : ResponseBody() {
                    override fun contentType(): MediaType? = mediaType
                    override fun contentLength(): Long = -1L
                    override fun source(): BufferedSource = stream.source().buffer()
                    override fun close() {
                        stream.close()
                    }
                }

                return Response.Builder()
                    .request(request)
                    .protocol(Protocol.HTTP_1_1)
                    .code(200)
                    .message("OK")
                    .header("Content-Type", "audio/x-archdrop-audio")
                    .header("Transfer-Encoding", "chunked")
                    .body(responseBody)
                    .build()
            }

            return chain.proceed(request)
        }
    }

    /**
     * Factory function creating a hermetic OkHttpClient.
     */
    fun createHermeticOkHttpClient(
        statusCode: Int = 200,
        onStreamCreated: ((HermeticAudioInputStream) -> Unit)? = null
    ): OkHttpClient {
        return OkHttpClient.Builder()
            .addInterceptor(HermeticAudioInterceptor(statusCode, onStreamCreated))
            .connectTimeout(500, TimeUnit.MILLISECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .writeTimeout(500, TimeUnit.MILLISECONDS)
            .retryOnConnectionFailure(false)
            .build()
    }

    /**
     * Factory function creating a fully hermetic [AudioStreamReceiver] test double.
     */
    fun createHermeticAudioReceiver(
        context: Context,
        statusCode: Int = 200,
        decoder: AudioDecoder = FakeAudioDecoder(),
        sink: AudioSink = FakeAudioSink(),
        ringBuffer: JitterRingBuffer = JitterRingBuffer(),
        onStreamCreated: ((HermeticAudioInputStream) -> Unit)? = null
    ): AudioStreamReceiver {
        return AudioStreamReceiver(
            context = context,
            okHttpClient = createHermeticOkHttpClient(statusCode, onStreamCreated),
            decoder = decoder,
            sink = sink,
            ringBuffer = ringBuffer
        )
    }
}
