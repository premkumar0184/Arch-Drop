package org.archdrop.audio

import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.Build
import android.util.Log

/**
 * Common pluggable audio sink interface for rendering decoded PCM audio.
 */
interface AudioSink {
    /**
     * Initializes the audio sink with given parameters.
     */
    fun init(sampleRate: Int = DEFAULT_SAMPLE_RATE, channels: Int = DEFAULT_CHANNELS, bufferSizeInBytes: Int = 0)

    /**
     * Begins or resumes playback.
     */
    fun play()

    /**
     * Pauses playback.
     */
    fun pause()

    /**
     * Stops playback and flushes hardware buffers.
     */
    fun stop()

    /**
     * Writes 16-bit PCM bytes to the output sink.
     * Returns the number of bytes written.
     */
    fun write(data: ByteArray, offset: Int = 0, size: Int = data.size): Int

    /**
     * Writes 16-bit PCM shorts to the output sink.
     * Returns the number of shorts written.
     */
    fun write(data: ShortArray, offset: Int = 0, size: Int = data.size): Int

    /**
     * Releases hardware audio sink resources.
     */
    fun release()

    /**
     * Dynamically routes audio to the specified preferred output device (e.g. Wired Headphones, Bluetooth).
     * If null, reverts to system default routing.
     */
    fun setPreferredOutputDevice(deviceInfo: AudioDeviceInfo?): Boolean

    /**
     * Returns the current playback head position in frames.
     */
    fun getPlaybackHeadPosition(): Long

    /**
     * Sets the hardware volume gain (0.0f to 1.0f).
     */
    fun setVolume(gain: Float)

    /**
     * Returns true if audio is actively playing.
     */
    fun isPlaying(): Boolean
}

/**
 * Low-latency Android AudioTrack sink.
 *
 * Configured with:
 * - AudioAttributes: USAGE_MEDIA, FLAG_LOW_LATENCY, CONTENT_TYPE_MUSIC
 * - AudioFormat: ENCODING_PCM_16BIT, CHANNEL_OUT_STEREO, 48000 Hz
 * - Performance Mode: PERFORMANCE_MODE_LOW_LATENCY
 */
class AudioTrackSink : AudioSink {

    companion object {
        private const val TAG = "ArchDrop:AudioTrackSink"
    }

    private var audioTrack: AudioTrack? = null
    var isInitialized: Boolean = false
        private set
    private var sampleRate: Int = DEFAULT_SAMPLE_RATE
    private var channels: Int = DEFAULT_CHANNELS

    override fun init(sampleRate: Int, channels: Int, bufferSizeInBytes: Int) {
        if (isInitialized) {
            release()
        }
        this.sampleRate = sampleRate
        this.channels = channels

        val channelMask = if (channels == 1) {
            AudioFormat.CHANNEL_OUT_MONO
        } else {
            AudioFormat.CHANNEL_OUT_STEREO
        }

        val minBufferSize = AudioTrack.getMinBufferSize(
            sampleRate,
            channelMask,
            AudioFormat.ENCODING_PCM_16BIT
        )

        val targetBufferSize = if (bufferSizeInBytes > 0) {
            maxOf(bufferSizeInBytes, minBufferSize)
        } else {
            // Double the minimum buffer size to provide ~40-60 ms buffer headroom
            maxOf(minBufferSize * 2, PCM_BYTES_PER_MS * 60)
        }

        try {
            val attributes = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .setFlags(AudioAttributes.FLAG_LOW_LATENCY)
                .build()

            val format = AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(sampleRate)
                .setChannelMask(channelMask)
                .build()

            val builder = AudioTrack.Builder()
                .setAudioAttributes(attributes)
                .setAudioFormat(format)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .setBufferSizeInBytes(targetBufferSize)

            audioTrack = builder.build()
            isInitialized = true
            Log.i(TAG, "AudioTrack initialized: $sampleRate Hz, channels=$channels, bufferSize=$targetBufferSize bytes")
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to initialize AudioTrack: ${e.message}", e)
            audioTrack = null
            isInitialized = false
        }
    }

    override fun play() {
        try {
            val track = audioTrack ?: return
            if (track.playState != AudioTrack.PLAYSTATE_PLAYING) {
                track.play()
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error starting AudioTrack playback: ${e.message}", e)
        }
    }

    override fun pause() {
        try {
            audioTrack?.pause()
        } catch (e: Exception) {
            Log.w(TAG, "Error pausing AudioTrack: ${e.message}", e)
        }
    }

    override fun stop() {
        try {
            audioTrack?.stop()
        } catch (e: Exception) {
            Log.w(TAG, "Error stopping AudioTrack: ${e.message}", e)
        }
    }

    override fun write(data: ByteArray, offset: Int, size: Int): Int {
        val track = audioTrack ?: return 0
        return try {
            track.write(data, offset, size, AudioTrack.WRITE_BLOCKING)
        } catch (e: Exception) {
            Log.w(TAG, "AudioTrack write error: ${e.message}", e)
            0
        }
    }

    override fun write(data: ShortArray, offset: Int, size: Int): Int {
        val track = audioTrack ?: return 0
        return try {
            track.write(data, offset, size, AudioTrack.WRITE_BLOCKING)
        } catch (e: Exception) {
            Log.w(TAG, "AudioTrack write short error: ${e.message}", e)
            0
        }
    }

    override fun release() {
        try {
            audioTrack?.stop()
        } catch (_: Exception) {}
        try {
            audioTrack?.release()
        } catch (_: Exception) {}
        audioTrack = null
        isInitialized = false
    }

    override fun setPreferredOutputDevice(deviceInfo: AudioDeviceInfo?): Boolean {
        val track = audioTrack ?: return false
        return try {
            track.setPreferredDevice(deviceInfo)
        } catch (e: Exception) {
            Log.w(TAG, "Error setting preferred device: ${e.message}", e)
            false
        }
    }

    override fun getPlaybackHeadPosition(): Long {
        return audioTrack?.playbackHeadPosition?.toLong() ?: 0L
    }

    override fun setVolume(gain: Float) {
        val clamped = gain.coerceIn(0.0f, 1.0f)
        try {
            audioTrack?.setVolume(clamped)
        } catch (e: Exception) {
            Log.w(TAG, "Error setting volume on AudioTrack: ${e.message}", e)
        }
    }

    override fun isPlaying(): Boolean {
        return audioTrack?.playState == AudioTrack.PLAYSTATE_PLAYING
    }
}

/**
 * Fake audio sink for unit testing on standard JVM / Robolectric without audio drivers.
 */
class FakeAudioSink : AudioSink {
    var isInitialized: Boolean = false
        private set
    var isPlayingState: Boolean = false
        private set

    var playCount: Int = 0
    var pauseCount: Int = 0
    var stopCount: Int = 0
    var releaseCount: Int = 0
    var totalBytesWritten: Long = 0
    var totalShortsWritten: Long = 0
    var preferredDevice: AudioDeviceInfo? = null
    var currentVolume: Float = 1.0f

    val writtenByteBuffers = mutableListOf<ByteArray>()

    override fun init(sampleRate: Int, channels: Int, bufferSizeInBytes: Int) {
        isInitialized = true
    }

    override fun play() {
        playCount++
        isPlayingState = true
    }

    override fun pause() {
        pauseCount++
        isPlayingState = false
    }

    override fun stop() {
        stopCount++
        isPlayingState = false
    }

    override fun write(data: ByteArray, offset: Int, size: Int): Int {
        val chunk = ByteArray(size)
        System.arraycopy(data, offset, chunk, 0, size)
        writtenByteBuffers.add(chunk)
        totalBytesWritten += size
        return size
    }

    override fun write(data: ShortArray, offset: Int, size: Int): Int {
        totalShortsWritten += size
        totalBytesWritten += (size * 2)
        return size
    }

    override fun release() {
        releaseCount++
        isPlayingState = false
        isInitialized = false
    }

    override fun setPreferredOutputDevice(deviceInfo: AudioDeviceInfo?): Boolean {
        preferredDevice = deviceInfo
        return true
    }

    override fun getPlaybackHeadPosition(): Long {
        // Return frame count assuming 16-bit stereo (4 bytes per frame)
        return totalBytesWritten / 4
    }

    override fun setVolume(gain: Float) {
        currentVolume = gain.coerceIn(0.0f, 1.0f)
    }

    override fun isPlaying(): Boolean = isPlayingState

    fun clearBuffers() {
        writtenByteBuffers.clear()
        totalBytesWritten = 0
        totalShortsWritten = 0
    }
}
