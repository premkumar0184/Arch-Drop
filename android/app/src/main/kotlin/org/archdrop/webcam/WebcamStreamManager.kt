package org.archdrop.webcam

import android.content.Context
import android.graphics.ImageFormat
import android.graphics.Rect
import android.graphics.YuvImage
import android.util.Log
import android.util.Size
import androidx.camera.core.AspectRatio
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.resolutionselector.AspectRatioStrategy
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleOwner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.Call
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.Response
import okio.BufferedSink
import java.io.IOException
import java.io.OutputStream
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

/**
 * Target camera capture resolutions supported by Arch Drop.
 */
enum class WebcamResolution(val width: Int, val height: Int, val label: String) {
    RES_480P(640, 480, "480p (SD)"),
    RES_720P(1280, 720, "720p (HD)"),
    RES_1080P(1920, 1080, "1080p (FHD)")
}

/**
 * Camera lens choices.
 */
enum class WebcamLens(val cameraSelector: CameraSelector, val label: String) {
    BACK(CameraSelector.DEFAULT_BACK_CAMERA, "Back Camera"),
    FRONT(CameraSelector.DEFAULT_FRONT_CAMERA, "Front Camera")
}

/**
 * Stream connection states.
 */
sealed class WebcamConnectionState {
    object Idle : WebcamConnectionState()
    object Connecting : WebcamConnectionState()
    object Streaming : WebcamConnectionState()
    object Disconnected : WebcamConnectionState()
    data class Error(val message: String) : WebcamConnectionState()
}

/**
 * Zero-boxing, zero-GC rolling FPS calculator over a sliding time window.
 */
class RollingFpsCalculator(
    private val capacity: Int = 120,
    private val windowMs: Long = 1000L
) {
    private val timestamps = LongArray(capacity)
    private var head = 0
    private var count = 0

    @Synchronized
    fun onFrame(): Double {
        val now = System.currentTimeMillis()
        timestamps[head] = now
        head = (head + 1) % capacity
        if (count < capacity) {
            count++
        }
        return computeFps(now)
    }

    @Synchronized
    fun getFps(): Double {
        return computeFps(System.currentTimeMillis())
    }

    private fun computeFps(now: Long): Double {
        val cutoff = now - windowMs
        var validFrames = 0
        var oldestTime = now

        for (i in 0 until count) {
            val idx = (head - 1 - i + capacity) % capacity
            val t = timestamps[idx]
            if (t >= cutoff) {
                validFrames++
                oldestTime = t
            } else {
                break
            }
        }

        if (validFrames <= 1) return 0.0
        val duration = (now - oldestTime).coerceAtLeast(1L)
        return (validFrames - 1) * 1000.0 / duration
    }

    @Synchronized
    fun reset() {
        head = 0
        count = 0
    }
}

/**
 * Pluggable frame compressor interface enabling native YuvImage compression in production
 * and mock/synthetic compression in JVM unit tests.
 */
interface FrameCompressor {
    fun compress(
        nv21: ByteArray,
        width: Int,
        height: Int,
        quality: Int,
        outputStream: OutputStream
    ): Boolean {
        return compress(nv21, width, height, quality, outputStream, Rect(0, 0, width, height))
    }

    fun compress(
        nv21: ByteArray,
        width: Int,
        height: Int,
        quality: Int,
        outputStream: OutputStream,
        cropRect: Rect?
    ): Boolean {
        return compress(nv21, width, height, quality, outputStream)
    }
}

/**
 * Standard Android [YuvImage] compressor.
 */
object YuvImageCompressor : FrameCompressor {
    override fun compress(
        nv21: ByteArray,
        width: Int,
        height: Int,
        quality: Int,
        outputStream: OutputStream,
        cropRect: Rect?
    ): Boolean {
        val yuvImage = YuvImage(nv21, ImageFormat.NV21, width, height, null)
        val rect = cropRect ?: Rect(0, 0, width, height)
        return yuvImage.compressToJpeg(rect, quality, outputStream)
    }

    override fun compress(
        nv21: ByteArray,
        width: Int,
        height: Int,
        quality: Int,
        outputStream: OutputStream
    ): Boolean {
        return compress(nv21, width, height, quality, outputStream, null)
    }
}


/**
 * Reusable frame buffer container holding a preallocated [ReusableByteArrayOutputStream].
 */
class FrameBuffer(initialCapacity: Int = 256 * 1024) {
    val stream = ReusableByteArrayOutputStream(initialCapacity)
    var timestampMs: Long = 0L
    var width: Int = 0
    var height: Int = 0
}

/**
 * Production-hardened, zero-GC low-latency CameraX MJPEG streaming engine for Arch Drop.
 *
 * Captures frames via CameraX [ImageAnalysis], converts YUV_420_888 to NV21 with stride padding handling,
 * compresses to JPEG using reusable buffers, and streams chunked multipart JPEG frames over HTTP POST
 * to the desktop daemon (`/api/v2/webcam/stream`) with immediate socket flushing and <80ms latency.
 */
open class WebcamStreamManager(
    private val context: Context,
    private val okHttpClient: OkHttpClient = defaultHttpClient()
) {
    companion object {
        private const val TAG = "ArchDrop:WebcamStream"
        const val DEFAULT_PORT = 53317
        const val BOUNDARY = "frame"

        fun defaultHttpClient(): OkHttpClient {
            return OkHttpClient.Builder()
                .connectTimeout(5, TimeUnit.SECONDS)
                .readTimeout(0, TimeUnit.MILLISECONDS) // Infinite read timeout for long-lived streams
                .writeTimeout(10, TimeUnit.SECONDS)
                .retryOnConnectionFailure(false)
                .build()
        }

        /**
         * Calculates centered crop rectangle aligned to even coordinates to strictly match
         * the target resolution's aspect ratio (e.g., 16:9 for 720p/1080p).
         */
        fun calculateCropRect(width: Int, height: Int, target: WebcamResolution): Rect {
            val targetWidth = target.width
            val targetHeight = target.height

            val currentRatio = width.toLong() * targetHeight
            val targetRatio = height.toLong() * targetWidth

            return when {
                currentRatio > targetRatio -> {
                    // Frame is wider than target aspect ratio: crop horizontally
                    val cropWidth = ((height.toLong() * targetWidth / targetHeight).toInt() / 2) * 2
                    val left = (((width - cropWidth) / 4) * 2).coerceAtLeast(0)
                    Rect(left, 0, left + cropWidth, height)
                }
                currentRatio < targetRatio -> {
                    // Frame is taller than target aspect ratio: crop vertically (e.g. 1280x960 -> 1280x720)
                    val cropHeight = ((width.toLong() * targetHeight / targetWidth).toInt() / 2) * 2
                    val top = (((height - cropHeight) / 4) * 2).coerceAtLeast(0)
                    Rect(0, top, width, top + cropHeight)
                }
                else -> {
                    Rect(0, 0, width, height)
                }
            }
        }

        /**
         * Stride-aware converter from [ImageFormat.YUV_420_888] [ImageProxy] to NV21 byte array.
         *
         * NV21 requires:
         * - Y plane: width * height bytes
         * - UV plane: interleaved V then U bytes (V0, U0, V1, U1...)
         *
         * Handles arbitrary [rowStride] padding and both planar ([pixelStride] = 1)
         * and interleaved ([pixelStride] = 2) UV planes with ZERO memory allocations in the loop.
         */
        fun yuv420ToNv21(image: ImageProxy, outputNv21: ByteArray) {
            val width = image.width
            val height = image.height
            val planes = image.planes
            require(planes.size >= 3) { "ImageProxy must have at least 3 planes for YUV_420_888" }
            val requiredSize = width * height * 3 / 2
            require(outputNv21.size >= requiredSize) {
                "Output buffer size (${outputNv21.size}) is smaller than required ($requiredSize)"
            }

            val yPlane = planes[0]
            val uPlane = planes[1]
            val vPlane = planes[2]

            val yBuffer = yPlane.buffer
            val uBuffer = uPlane.buffer
            val vBuffer = vPlane.buffer

            val yRowStride = yPlane.rowStride
            val yPixelStride = yPlane.pixelStride

            val uRowStride = uPlane.rowStride
            val uPixelStride = uPlane.pixelStride

            val vRowStride = vPlane.rowStride
            val vPixelStride = vPlane.pixelStride

            // 1. Copy Y plane
            var yOutPos = 0
            if (yPixelStride == 1 && yRowStride == width) {
                yBuffer.position(0)
                yBuffer.get(outputNv21, 0, width * height)
            } else {
                for (row in 0 until height) {
                    if (yPixelStride == 1) {
                        yBuffer.position(row * yRowStride)
                        yBuffer.get(outputNv21, yOutPos, width)
                        yOutPos += width
                    } else {
                        val rowOffset = row * yRowStride
                        for (col in 0 until width) {
                            outputNv21[yOutPos++] = yBuffer.get(rowOffset + col * yPixelStride)
                        }
                    }
                }
            }

            // 2. Interleave and copy UV planes (NV21: V followed by U)
            var uvOutPos = width * height
            val chromaHeight = height / 2
            val chromaWidth = width / 2

            for (row in 0 until chromaHeight) {
                val vRowOffset = row * vRowStride
                val uRowOffset = row * uRowStride
                for (col in 0 until chromaWidth) {
                    outputNv21[uvOutPos++] = vBuffer.get(vRowOffset + col * vPixelStride)
                    outputNv21[uvOutPos++] = uBuffer.get(uRowOffset + col * uPixelStride)
                }
            }
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var streamingJob: Job? = null
    private var terminatingJob: Job? = null
    private var cameraExecutor: ExecutorService? = null
    private var cameraProvider: ProcessCameraProvider? = null
    private var activeCamera: Camera? = null
    private var activeCall: Call? = null

    protected val _connectionState = MutableStateFlow<WebcamConnectionState>(WebcamConnectionState.Idle)
    open val connectionState: StateFlow<WebcamConnectionState> = _connectionState.asStateFlow()

    protected val _fps = MutableStateFlow(0.0)
    open val fps: StateFlow<Double> = _fps.asStateFlow()

    val fpsCalculator = RollingFpsCalculator(capacity = 120, windowMs = 1000L)

    private val isStreaming = AtomicBoolean(false)
    private val currentSessionId = AtomicInteger(0)
    private val frameSignal = Semaphore(0)
    private val latestReadyFrame = AtomicReference<FrameBuffer?>(null)

    // Preallocated buffer pool (triple-buffering: 1 in network write, 1 in ready slot, 1 in analyzer)
    private val bufferPool = ConcurrentLinkedQueue<FrameBuffer>().apply {
        repeat(3) { offer(FrameBuffer()) }
    }

    // Reusable preallocated NV21 buffer resized only upon resolution changes
    private var nv21Buffer = ByteArray(0)

    var compressor: FrameCompressor = YuvImageCompressor
    var jpegQuality: Int = 80
    var activeResolution: WebcamResolution = WebcamResolution.RES_720P

    @Synchronized
    fun ensureNv21Buffer(width: Int, height: Int): ByteArray {
        val needed = width * height * 3 / 2
        if (nv21Buffer.size != needed) {
            nv21Buffer = ByteArray(needed)
        }
        return nv21Buffer
    }

    private fun acquireFrameBuffer(): FrameBuffer {
        val buf = bufferPool.poll()
        if (buf != null) return buf
        // Reclaim any unconsumed frame waiting in the latest slot
        val dropped = latestReadyFrame.getAndSet(null)
        if (dropped != null) return dropped
        // Fallback allocation if all 3 are simultaneously in-flight
        return FrameBuffer()
    }

    private fun releaseFrameBuffer(buffer: FrameBuffer) {
        bufferPool.offer(buffer)
    }

    open fun isStreamingActive(): Boolean = isStreaming.get()

    /**
     * Starts camera capture and streaming over HTTP chunked POST.
     */
    open fun startStreaming(
        lifecycleOwner: LifecycleOwner,
        targetHost: String,
        targetPort: Int = DEFAULT_PORT,
        resolution: WebcamResolution = WebcamResolution.RES_720P,
        lens: WebcamLens = WebcamLens.BACK,
        localFingerprint: String = "ANDROID_FP",
        quality: Int = 80
    ) {
        if (isStreaming.getAndSet(true)) {
            Log.w(TAG, "Streaming session is already active; ignoring duplicate start request.")
            return
        }

        val sessionId = currentSessionId.incrementAndGet()
        this.activeResolution = resolution
        this.jpegQuality = quality.coerceIn(10, 100)
        _connectionState.value = WebcamConnectionState.Connecting
        fpsCalculator.reset()
        _fps.value = 0.0

        val cameraExec = Executors.newSingleThreadExecutor()
        this.cameraExecutor = cameraExec

        val previousJob = streamingJob ?: terminatingJob
        val previousCall = activeCall
        streamingJob = null
        activeCall = null
        terminatingJob = null

        // 1. Launch network egress streaming task
        streamingJob = scope.launch {
            if (previousJob != null && previousJob.isActive) {
                previousCall?.cancel()
                withTimeoutOrNull(300) {
                    previousJob.join()
                }
            }
            if (!isStreaming.get() || currentSessionId.get() != sessionId) return@launch
            runNetworkStreamingLoop(targetHost, targetPort, localFingerprint, sessionId)
        }

        // 2. Initialize CameraX on Main/UI thread
        try {
            val cameraProviderFuture = ProcessCameraProvider.getInstance(context)
            cameraProviderFuture.addListener({
                try {
                    if (!isStreaming.get()) return@addListener

                    val provider = cameraProviderFuture.get()
                    cameraProvider = provider

                    val targetAspectRatio = when (resolution) {
                        WebcamResolution.RES_480P -> AspectRatio.RATIO_4_3
                        else -> AspectRatio.RATIO_16_9
                    }

                    val resolutionSelector = ResolutionSelector.Builder()
                        .setAspectRatioStrategy(
                            AspectRatioStrategy(targetAspectRatio, AspectRatioStrategy.FALLBACK_RULE_AUTO)
                        )
                        .setResolutionStrategy(
                            ResolutionStrategy(
                                Size(resolution.width, resolution.height),
                                ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER
                            )
                        )
                        .build()

                    val imageAnalysis = ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .setResolutionSelector(resolutionSelector)
                        .setOutputImageFormat(ImageAnalysis.OUTPUT_IMAGE_FORMAT_YUV_420_888)
                        .setImageQueueDepth(1)
                        .build()

                    imageAnalysis.setAnalyzer(cameraExec) { image ->
                        processCameraFrame(image)
                    }

                    provider.unbindAll()
                    activeCamera = provider.bindToLifecycle(
                        lifecycleOwner,
                        lens.cameraSelector,
                        imageAnalysis
                    )
                    Log.i(TAG, "CameraX bound to lifecycle with resolution ${resolution.width}x${resolution.height}")
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to bind CameraX capture engine: ${e.message}", e)
                    _connectionState.value = WebcamConnectionState.Error("Camera init error: ${e.message}")
                    stopStreaming()
                }
            }, ContextCompat.getMainExecutor(context))
        } catch (e: Exception) {
            Log.e(TAG, "Failed to initialize CameraX provider: ${e.message}", e)
            _connectionState.value = WebcamConnectionState.Error("Camera init error: ${e.message}")
            stopStreaming()
        }
    }

    /**
     * Direct frame submission for synthetic testing or external camera sources.
     */
    fun sendDirectFrame(
        jpegBytes: ByteArray,
        length: Int,
        timestampMs: Long = System.currentTimeMillis()
    ) {
        val frameBuffer = acquireFrameBuffer()
        frameBuffer.stream.reset()
        frameBuffer.stream.write(jpegBytes, 0, length)
        frameBuffer.timestampMs = timestampMs
        enqueueReadyFrame(frameBuffer)
    }

    /**
     * Processes an incoming CameraX frame, converts YUV to NV21, compresses to JPEG,
     * and atomically updates the conflated frame slot.
     */
    fun processCameraFrame(image: ImageProxy) {
        try {
            if (!isStreaming.get()) return
            if (image.format != ImageFormat.YUV_420_888) {
                Log.w(TAG, "Unexpected image format: ${image.format}; expected YUV_420_888")
                return
            }

            val width = image.width
            val height = image.height
            val nv21 = ensureNv21Buffer(width, height)

            yuv420ToNv21(image, nv21)

            val cropRect = calculateCropRect(width, height, activeResolution)
            val outWidth = cropRect.width()
            val outHeight = cropRect.height()

            val frameBuffer = acquireFrameBuffer()
            frameBuffer.stream.reset()

            val success = compressor.compress(
                nv21,
                width,
                height,
                jpegQuality,
                frameBuffer.stream,
                cropRect
            )

            if (success && frameBuffer.stream.size() > 0) {
                frameBuffer.timestampMs = System.currentTimeMillis()
                frameBuffer.width = outWidth
                frameBuffer.height = outHeight
                enqueueReadyFrame(frameBuffer)
            } else {
                releaseFrameBuffer(frameBuffer)
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error analyzing camera frame: ${e.message}", e)
        } finally {
            try {
                image.close()
            } catch (e: Exception) {
                Log.w(TAG, "Error closing ImageProxy: ${e.message}")
            }
        }
    }

    private fun enqueueReadyFrame(frameBuffer: FrameBuffer) {
        val dropped = latestReadyFrame.getAndSet(frameBuffer)
        if (dropped != null) {
            releaseFrameBuffer(dropped)
        }
        // Drain excess permits to prevent unbounded queue accumulation
        frameSignal.drainPermits()
        frameSignal.release()
    }

    /**
     * OkHttp network egress loop streaming multipart frames chunked to the desktop endpoint.
     */
    private fun runNetworkStreamingLoop(
        targetHost: String,
        targetPort: Int,
        localFingerprint: String
    ) {
        runNetworkStreamingLoop(targetHost, targetPort, localFingerprint, currentSessionId.get())
    }

    private fun runNetworkStreamingLoop(
        targetHost: String,
        targetPort: Int,
        localFingerprint: String,
        sessionId: Int
    ) {
        val formattedHost = if (targetHost.contains(":") && !targetHost.startsWith("[")) {
            "[$targetHost]"
        } else {
            targetHost
        }
        val url = "http://$formattedHost:$targetPort/api/v2/webcam/stream"

        val requestBody = object : RequestBody() {
            override fun contentType(): MediaType =
                "multipart/x-mixed-replace; boundary=$BOUNDARY".toMediaType()

            override fun contentLength(): Long = -1L // Transfer-Encoding: chunked

            override fun writeTo(sink: BufferedSink) {
                _connectionState.value = WebcamConnectionState.Streaming
                Log.i(TAG, "Connected to desktop webcam stream endpoint; streaming active.")

                var idleCount = 0
                try {
                    while (isStreaming.get() && currentSessionId.get() == sessionId) {
                        val acquired = frameSignal.tryAcquire(100, TimeUnit.MILLISECONDS)
                        if (!isStreaming.get() || currentSessionId.get() != sessionId) break
                        if (!acquired) {
                            idleCount++
                            if (idleCount >= 25) { // 2.5 seconds with zero camera frames delivered
                                Log.w(TAG, "No camera frames received for 2.5s; terminating stream session cleanly")
                                break
                            }
                            try {
                                sink.writeUtf8("\r\n")
                                sink.flush()
                            } catch (e: Exception) {
                                if (isStreaming.get()) {
                                    Log.d(TAG, "Socket write check failed during idle: ${e.message}")
                                    throw e
                                }
                            }
                            continue
                        }
                        idleCount = 0

                        val frame = latestReadyFrame.getAndSet(null) ?: continue
                        try {
                            val count = frame.stream.size()
                            val buf = frame.stream.toByteArrayBuffer()
                            val ts = frame.timestampMs

                            sink.writeUtf8("--$BOUNDARY\r\n")
                            sink.writeUtf8("Content-Type: image/jpeg\r\n")
                            sink.writeUtf8("Content-Length: $count\r\n")
                            sink.writeUtf8("X-Timestamp-Ms: $ts\r\n\r\n")
                            sink.write(buf, 0, count)
                            sink.writeUtf8("\r\n")
                            sink.flush()

                            val currentFps = fpsCalculator.onFrame()
                            _fps.value = currentFps
                        } finally {
                            releaseFrameBuffer(frame)
                        }
                    }

                    // Send clean multipart stream termination boundary
                    try {
                        sink.writeUtf8("--$BOUNDARY--\r\n")
                        sink.flush()
                    } catch (_: Exception) {}
                } catch (e: Exception) {
                    if (isStreaming.get()) {
                        Log.w(TAG, "Socket write error during streaming: ${e.message}")
                        throw e
                    }
                }
            }
        }

        val request = Request.Builder()
            .url(url)
            .post(requestBody)
            .header("Content-Type", "multipart/x-mixed-replace; boundary=$BOUNDARY")
            .header("X-Fingerprint", localFingerprint)
            .header("Connection", "close")
            .build()

        var currentCall = okHttpClient.newCall(request)
        activeCall = currentCall

        try {
            var response: Response? = null
            var lastErr: String? = null
            for (attempt in 1..2) {
                val resp = try {
                    currentCall.execute()
                } catch (e: Exception) {
                    if (lastErr != null) {
                        break
                    }
                    if (isStreaming.get() && currentSessionId.get() == sessionId) {
                        throw e
                    } else {
                        return
                    }
                }
                if (resp.code == 409) {
                    lastErr = "Server rejected stream: Another stream session is active (HTTP 409)"
                    if (attempt < 2 && isStreaming.get() && currentSessionId.get() == sessionId) {
                        resp.close()
                        Log.w(TAG, "Server returned 409 Conflict (previous session draining); retrying in 200ms...")
                        try {
                            Thread.sleep(200)
                        } catch (_: InterruptedException) {
                            break
                        }
                        if (!isStreaming.get() || currentSessionId.get() != sessionId) break
                        currentCall = okHttpClient.newCall(request)
                        activeCall = currentCall
                        continue
                    }
                }
                response = resp
                break
            }
            val finalResp = response ?: run {
                if (lastErr != null) {
                    Log.e(TAG, lastErr)
                    if (currentSessionId.get() == sessionId) {
                        _connectionState.value = WebcamConnectionState.Error(lastErr)
                    }
                }
                return
            }
            if (!finalResp.isSuccessful) {
                val err = when (finalResp.code) {
                    403 -> "Server rejected stream: Device not paired (HTTP 403)"
                    409 -> "Server rejected stream: Another stream session is active (HTTP 409)"
                    else -> "Server rejected stream: HTTP ${finalResp.code}"
                }
                Log.e(TAG, err)
                if (currentSessionId.get() == sessionId) {
                    _connectionState.value = WebcamConnectionState.Error(err)
                }
            } else {
                if (currentSessionId.get() == sessionId) {
                    _connectionState.value = WebcamConnectionState.Disconnected
                }
            }
        } catch (e: Exception) {
            if (isStreaming.get() && currentSessionId.get() == sessionId) {
                val err = "Streaming connection error: ${e.message ?: "Unknown error"}"
                Log.e(TAG, err, e)
                _connectionState.value = WebcamConnectionState.Error(err)
            } else if (currentSessionId.get() == sessionId) {
                _connectionState.value = WebcamConnectionState.Disconnected
            }
        } finally {
            if (currentSessionId.get() == sessionId) {
                stopStreaming()
            }
        }
    }

    /**
     * Cleanly terminates the stream, unbinds CameraX, closes the HTTP call, and resets state.
     */
    open fun stopStreaming() {
        currentSessionId.incrementAndGet()
        if (!isStreaming.getAndSet(false)) return

        Log.i(TAG, "Stopping webcam streaming engine...")
        frameSignal.release()

        try {
            ContextCompat.getMainExecutor(context).execute {
                try {
                    cameraProvider?.unbindAll()
                } catch (e: Exception) {
                    Log.w(TAG, "Error unbinding CameraX: ${e.message}")
                }
            }
        } catch (_: Exception) {}
        activeCamera = null

        cameraExecutor?.shutdown()
        try {
            cameraExecutor?.awaitTermination(500, TimeUnit.MILLISECONDS)
        } catch (_: Exception) {}
        cameraExecutor = null

        val jobToCancel = streamingJob
        val callToCancel = activeCall
        streamingJob = null
        activeCall = null

        terminatingJob = scope.launch {
            try {
                callToCancel?.cancel()
                withTimeoutOrNull(150) {
                    jobToCancel?.join()
                }
                jobToCancel?.cancel()
            } catch (_: Exception) {}
        }

        // Recycle any dangling frame in the ready slot
        val remaining = latestReadyFrame.getAndSet(null)
        if (remaining != null) {
            releaseFrameBuffer(remaining)
        }

        fpsCalculator.reset()
        _fps.value = 0.0

        if (_connectionState.value is WebcamConnectionState.Streaming ||
            _connectionState.value is WebcamConnectionState.Connecting) {
            _connectionState.value = WebcamConnectionState.Disconnected
        }
        Log.i(TAG, "Webcam streaming engine stopped successfully.")
    }
}
