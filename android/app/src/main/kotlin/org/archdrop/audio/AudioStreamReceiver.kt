package org.archdrop.audio

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.PowerManager
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import okhttp3.Call
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.EOFException
import java.io.IOException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.roundToInt

/**
 * Connection and lifecycle states of the AudioStreamReceiver.
 */
sealed class AudioReceiverState {
    object Idle : AudioReceiverState()
    object Connecting : AudioReceiverState()
    object Streaming : AudioReceiverState()
    object Paused : AudioReceiverState()
    object Disconnected : AudioReceiverState()
    data class Error(val message: String, val statusCode: Int? = null) : AudioReceiverState()
}

/**
 * Production low-latency wireless desktop audio receiver engine for Arch Drop.
 *
 * Responsibilities:
 * - Streams chunked ADAU binary frames over HTTP `GET /api/v2/audio/stream` using OkHttp with readTimeout = 0.
 * - Authenticates peer with `X-Fingerprint` header.
 * - Manages `PowerManager.PARTIAL_WAKE_LOCK` and `WifiManager.WIFI_MODE_FULL_LOW_LATENCY`.
 * - Manages `AudioManager` audio focus (handling ducking and pause on transient loss).
 * - Decodes Opus or raw PCM payloads into `JitterRingBuffer`.
 * - Applies software volume boost (100%–200%) with saturation clamping and mute control.
 * - Outputs low-latency PCM to `AudioSink` with dynamic device routing.
 */
open class AudioStreamReceiver(
    private val context: Context,
    private val okHttpClient: OkHttpClient = createDefaultHttpClient(),
    val decoder: AudioDecoder = MediaCodecAudioDecoder(),
    val sink: AudioSink = AudioTrackSink(),
    val ringBuffer: JitterRingBuffer = JitterRingBuffer()
) {

    companion object {
        private const val TAG = "ArchDrop:AudioReceiver"
        const val DEFAULT_PORT = 53317
        const val PLAYBACK_CHUNK_BYTES = 1920 // 10 ms at 48kHz stereo 16-bit (480 stereo samples)
        const val DUCK_ATTENUATION_FACTOR = 0.2f // Duck to 20% on transient duckable audio focus loss

        fun createDefaultHttpClient(): OkHttpClient {
            return OkHttpClient.Builder()
                .connectTimeout(5, TimeUnit.SECONDS)
                .readTimeout(0, TimeUnit.MILLISECONDS) // Infinite read timeout for continuous streams
                .writeTimeout(5, TimeUnit.SECONDS)
                .retryOnConnectionFailure(false)
                .build()
        }
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    private val _state = MutableStateFlow<AudioReceiverState>(AudioReceiverState.Idle)
    val state: StateFlow<AudioReceiverState> = _state.asStateFlow()

    private val _telemetry = MutableStateFlow(ringBuffer.getTelemetry())
    val telemetry: StateFlow<JitterTelemetry> = _telemetry.asStateFlow()

    // Control parameters
    private val isRunning = AtomicBoolean(false)
    private val isStreamPaused = AtomicBoolean(false)

    @Volatile
    var volumeBoost: Float = 1.0f // 1.0 (100%) to 2.0 (200%)
        private set

    @Volatile
    var isMuted: Boolean = false
        private set

    @Volatile
    private var duckFactor: Float = 1.0f

    private var activeCall: Call? = null
    private var networkJob: Job? = null
    private var playbackJob: Job? = null
    private var telemetryJob: Job? = null

    // System resource handles
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var audioManager: AudioManager? = null
    private var audioFocusRequest: AudioFocusRequest? = null
    private var audioFocusChangeListener: AudioManager.OnAudioFocusChangeListener? = null

    private var selectedDevice: AudioDeviceInfo? = null

    init {
        initializeLocks()
    }

    private fun initializeLocks() {
        try {
            val pm = context.getSystemService(Context.POWER_SERVICE) as? PowerManager
            wakeLock = pm?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "ArchDrop:AudioPlaybackWakeLock")?.apply {
                setReferenceCounted(false)
            }

            val wm = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            wifiLock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                wm?.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "ArchDrop:AudioWifiLock")
            } else {
                wm?.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "ArchDrop:AudioWifiLock")
            }?.apply {
                setReferenceCounted(false)
            }

            audioManager = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
        } catch (e: Exception) {
            Log.w(TAG, "Error initializing system locks: ${e.message}", e)
        }
    }

    /**
     * Starts audio streaming session.
     */
    fun start(
        host: String,
        port: Int = DEFAULT_PORT,
        fingerprint: String? = null,
        codec: AudioCodec = AudioCodec.OPUS,
        targetBitrate: Int = 96000,
        sinkMode: String = "monitor"
    ) {
        if (!isRunning.compareAndSet(false, true)) {
            Log.w(TAG, "AudioStreamReceiver is already running")
            return
        }

        _state.value = AudioReceiverState.Connecting

        // 1. Acquire power and low-latency Wi-Fi locks
        acquireLocks()

        // 2. Request Android Audio Focus
        requestAudioFocus()

        // 3. Initialize sink and decoder
        decoder.init(DEFAULT_SAMPLE_RATE, DEFAULT_CHANNELS)
        sink.init(DEFAULT_SAMPLE_RATE, DEFAULT_CHANNELS)
        if (selectedDevice != null) {
            sink.setPreferredOutputDevice(selectedDevice)
        }
        sink.play()
        ringBuffer.clear()

        // 4. Start playback loop
        startPlaybackLoop()

        // 5. Start telemetry monitor
        startTelemetryMonitor()

        // 6. Connect HTTP streaming ingestion loop
        val url = "http://$host:$port/api/v2/audio/stream?codec=${codec.asParam()}&bitrate=$targetBitrate&sink=$sinkMode"
        val requestBuilder = Request.Builder()
            .url(url)
            .header("Accept", "audio/x-archdrop-audio")

        if (!fingerprint.isNullOrEmpty()) {
            requestBuilder.header("X-Fingerprint", fingerprint)
        }

        networkJob = scope.launch {
            runIngestionLoop(requestBuilder.build())
        }
    }

    /**
     * Ingestion loop: consumes chunked ADAU binary frames from OkHttp response.
     */
    private suspend fun runIngestionLoop(request: Request) {
        var response: Response? = null
        try {
            val call = okHttpClient.newCall(request)
            activeCall = call
            response = call.execute()

            if (!response.isSuccessful) {
                val code = response.code
                val err = when (code) {
                    403 -> "Peer authentication failed (403 Forbidden)"
                    409 -> "Desktop audio stream session already active (409 Conflict)"
                    503 -> "PipeWire audio capture unavailable on host (503)"
                    else -> "Audio stream request failed: HTTP $code"
                }
                Log.e(TAG, err)
                _state.value = AudioReceiverState.Error(err, code)
                stopInternal(revertToIdle = false)
                return
            }

            val body = response.body
            if (body == null) {
                _state.value = AudioReceiverState.Error("Empty audio response body")
                stopInternal(revertToIdle = false)
                return
            }

            _state.value = AudioReceiverState.Streaming
            Log.i(TAG, "Connected to desktop audio stream: ${request.url}")

            val source = body.source()
            val parser = AudioFrameParser()
            var lastSeq: Long? = null

            while (isRunning.get() && scope.isActive) {
                val frame = try {
                    parser.readNextFrame(source)
                } catch (e: EOFException) {
                    Log.i(TAG, "Audio stream ended cleanly by remote server")
                    break
                }

                // Detect packet sequence gaps
                val seq = frame.header.sequence
                val prev = lastSeq
                if (prev != null && seq > prev + 1) {
                    val lostCount = (seq - prev - 1)
                    ringBuffer.recordPacketLoss(lostCount)
                }
                lastSeq = seq

                // Decode payload and feed ring buffer
                decoder.decode(
                    payload = frame.payload,
                    offset = 0,
                    length = frame.payload.size,
                    ptsMs = frame.header.timestampMs
                ) { pcmBytes, pcmLen ->
                    ringBuffer.writePcm(
                        pcm = pcmBytes,
                        offset = 0,
                        length = pcmLen,
                        ptsMs = frame.header.timestampMs
                    )
                }
            }

            if (_state.value is AudioReceiverState.Streaming) {
                _state.value = AudioReceiverState.Disconnected
            }
        } catch (e: Exception) {
            if (isRunning.get()) {
                Log.w(TAG, "Audio streaming network error: ${e.message}", e)
                _state.value = AudioReceiverState.Error("Stream disconnected: ${e.message}")
            }
        } finally {
            try {
                response?.close()
            } catch (_: Exception) {}
            stopInternal(revertToIdle = false)
        }
    }

    /**
     * Playback loop: drains decoded PCM from ring buffer, applies software volume boost, and writes to sink.
     */
    private fun startPlaybackLoop() {
        playbackJob = scope.launch(Dispatchers.Default) {
            val chunk = ByteArray(PLAYBACK_CHUNK_BYTES)
            var isPreRolling = true
            // Pre-roll until ring buffer has at least 20 ms (3,840 bytes) to cushion against initial network arrival jitter
            val preRollThresholdBytes = PLAYBACK_CHUNK_BYTES * 2

            while (isRunning.get() && isActive) {
                if (isStreamPaused.get()) {
                    delay(20L)
                    continue
                }

                if (isPreRolling) {
                    if (ringBuffer.availableBytes < preRollThresholdBytes) {
                        delay(5L)
                        continue
                    }
                    isPreRolling = false
                }

                val read = ringBuffer.readPcm(chunk, 0, chunk.size)
                if (read > 0) {
                    // Apply software volume boost, mute, and ducking
                    applyVolumeAndMute(chunk, read)
                    val written = sink.write(chunk, 0, read)
                    if (written <= 0) {
                        delay(2L)
                    }
                } else {
                    // Buffer starving: wait 10 ms (duration of 1 playback chunk) for the next network frame
                    // to arrive instead of spamming synthesized PLC frames at 5x real-time rate.
                    delay(10L)
                    if (ringBuffer.availableBytes == 0) {
                        isPreRolling = true
                    }
                }
            }
        }
    }

    /**
     * In-place software volume amplification (100%–200%) with saturation clamping and mute zeroing.
     */
    internal fun applyVolumeAndMute(pcm: ByteArray, length: Int) {
        val muted = isMuted
        val gain = if (muted) 0f else (volumeBoost * duckFactor)

        if (muted) {
            pcm.fill(0, 0, length)
            return
        }

        if (gain == 1.0f) {
            return
        }

        val numSamples = length / 2
        for (i in 0 until numSamples) {
            val offset = i * 2
            val b0 = pcm[offset].toInt() and 0xFF
            val b1 = pcm[offset + 1].toInt() and 0xFF
            val sample = ((b1 shl 8) or b0).toShort().toInt()

            val boosted = (sample * gain).roundToInt().coerceIn(-32768, 32767)

            pcm[offset] = (boosted and 0xFF).toByte()
            pcm[offset + 1] = ((boosted shr 8) and 0xFF).toByte()
        }
    }

    /**
     * Telemetry emitter running at 100 ms intervals.
     */
    private fun startTelemetryMonitor() {
        telemetryJob = scope.launch {
            while (isRunning.get() && isActive) {
                _telemetry.value = ringBuffer.getTelemetry()
                delay(100L)
            }
        }
    }

    /**
     * Sets software volume boost factor (clamped between 1.0f and 2.0f).
     */
    fun setVolumeBoost(boost: Float) {
        this.volumeBoost = if (boost.isNaN()) 1.0f else boost.coerceIn(1.0f, 2.0f)
    }

    /**
     * Toggles audio mute state.
     */
    fun setMute(muted: Boolean) {
        this.isMuted = muted
    }

    /**
     * Sets preferred audio output device (e.g. Wired Headphones, Bluetooth Headset, Phone Speaker).
     */
    fun setPreferredOutputDevice(deviceInfo: AudioDeviceInfo?): Boolean {
        selectedDevice = deviceInfo
        return sink.setPreferredOutputDevice(deviceInfo)
    }

    /**
     * Stops the audio streaming session and releases all resources.
     */
    fun stop() {
        stopInternal(revertToIdle = true)
    }

    private fun stopInternal(revertToIdle: Boolean) {
        if (!isRunning.compareAndSet(true, false)) {
            if (revertToIdle && _state.value !is AudioReceiverState.Idle) {
                _state.value = AudioReceiverState.Idle
            }
            return
        }

        Log.i(TAG, "Stopping AudioStreamReceiver...")

        try {
            activeCall?.cancel()
        } catch (_: Exception) {}
        activeCall = null

        networkJob?.cancel()
        networkJob = null

        playbackJob?.cancel()
        playbackJob = null

        telemetryJob?.cancel()
        telemetryJob = null

        try {
            sink.stop()
            sink.release()
        } catch (e: Exception) {
            Log.w(TAG, "Error stopping sink: ${e.message}", e)
        }

        try {
            decoder.flush()
            decoder.release()
        } catch (e: Exception) {
            Log.w(TAG, "Error releasing decoder: ${e.message}", e)
        }

        ringBuffer.clear()
        abandonAudioFocus()
        releaseLocks()

        if (revertToIdle) {
            _state.value = AudioReceiverState.Idle
        }
    }

    // =========================================================================
    // Audio Focus Handling
    // =========================================================================

    private fun requestAudioFocus() {
        val am = audioManager ?: return

        audioFocusChangeListener = AudioManager.OnAudioFocusChangeListener { focusChange ->
            when (focusChange) {
                AudioManager.AUDIOFOCUS_GAIN -> {
                    Log.i(TAG, "Audio focus GAIN: restoring normal playback")
                    duckFactor = 1.0f
                    if (isStreamPaused.get()) {
                        isStreamPaused.set(false)
                        sink.play()
                        if (isRunning.get()) {
                            _state.value = AudioReceiverState.Streaming
                        }
                    }
                }
                AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
                    Log.i(TAG, "Audio focus LOSS_TRANSIENT_CAN_DUCK: ducking to 20%")
                    duckFactor = DUCK_ATTENUATION_FACTOR
                }
                AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                    Log.i(TAG, "Audio focus LOSS_TRANSIENT (e.g. phone call): pausing playback")
                    isStreamPaused.set(true)
                    sink.pause()
                    if (isRunning.get()) {
                        _state.value = AudioReceiverState.Paused
                    }
                }
                AudioManager.AUDIOFOCUS_LOSS -> {
                    Log.i(TAG, "Audio focus LOSS: stopping audio stream")
                    stop()
                }
            }
        }

        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val attrs = AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build()

                val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                    .setAudioAttributes(attrs)
                    .setAcceptsDelayedFocusGain(true)
                    .setOnAudioFocusChangeListener(audioFocusChangeListener!!)
                    .build()

                audioFocusRequest = request
                am.requestAudioFocus(request)
            } else {
                @Suppress("DEPRECATION")
                am.requestAudioFocus(
                    audioFocusChangeListener,
                    AudioManager.STREAM_MUSIC,
                    AudioManager.AUDIOFOCUS_GAIN
                )
            }
        } catch (e: Exception) {
            Log.w(TAG, "Failed to request audio focus: ${e.message}", e)
        }
    }

    private fun abandonAudioFocus() {
        val am = audioManager ?: return
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                audioFocusRequest?.let { am.abandonAudioFocusRequest(it) }
                audioFocusRequest = null
            } else {
                audioFocusChangeListener?.let {
                    @Suppress("DEPRECATION")
                    am.abandonAudioFocus(it)
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error abandoning audio focus: ${e.message}", e)
        }
    }

    // =========================================================================
    // Power & Wi-Fi Lock Management
    // =========================================================================

    private fun acquireLocks() {
        try {
            wakeLock?.let {
                if (!it.isHeld) {
                    it.acquire(6 * 60 * 60 * 1000L) // 6 hours safety timeout
                    Log.d(TAG, "Acquired PARTIAL_WAKE_LOCK for audio streaming")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error acquiring WakeLock: ${e.message}", e)
        }

        try {
            wifiLock?.let {
                if (!it.isHeld) {
                    it.acquire()
                    Log.d(TAG, "Acquired low-latency WifiLock for audio streaming")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error acquiring WifiLock: ${e.message}", e)
        }
    }

    private fun releaseLocks() {
        try {
            wakeLock?.let {
                if (it.isHeld) {
                    it.release()
                    Log.d(TAG, "Released PARTIAL_WAKE_LOCK")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error releasing WakeLock: ${e.message}", e)
        }

        try {
            wifiLock?.let {
                if (it.isHeld) {
                    it.release()
                    Log.d(TAG, "Released WifiLock")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error releasing WifiLock: ${e.message}", e)
        }
    }
}
