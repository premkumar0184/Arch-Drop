package org.archdrop.ui.audio

import android.app.Application
import android.content.Context
import android.content.Intent
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build
import android.util.Log
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.archdrop.audio.AudioCodec
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.AudioStreamReceiver
import org.archdrop.audio.JitterTelemetry
import org.archdrop.mesh.MeshIdentity
import org.archdrop.service.ArchDropSyncService
import java.io.File
import kotlin.math.roundToInt

/**
 * Categorization of physical audio output hardware sinks.
 */
enum class AudioDeviceType {
    DEFAULT,
    SPEAKER,
    WIRED,
    BLUETOOTH,
    USB
}

/**
 * UI representation of an audio output device.
 */
data class AudioDeviceUiModel(
    val id: Int,
    val name: String,
    val type: AudioDeviceType,
    val isDefault: Boolean = false,
    val rawDeviceInfo: AudioDeviceInfo? = null
) {
    val isBluetooth: Boolean
        get() = (type == AudioDeviceType.BLUETOOTH)

    companion object {
        val SYSTEM_DEFAULT = AudioDeviceUiModel(
            id = -1,
            name = "System Default",
            type = AudioDeviceType.DEFAULT,
            isDefault = true,
            rawDeviceInfo = null
        )
    }
}

/**
 * Typealias for UI components referencing AudioDeviceOption.
 */
typealias AudioDeviceOption = AudioDeviceUiModel

/**
 * ViewModel managing low-latency wireless desktop audio streaming controls,
 * state flows, output device routing, software volume boost, and telemetry for Arch Drop.
 */
class AudioViewModel @JvmOverloads constructor(
    application: Application,
    val audioReceiver: AudioStreamReceiver = AudioStreamReceiver(application),
    val audioManager: AudioManager? = application.getSystemService(Context.AUDIO_SERVICE) as? AudioManager,
    private val serviceIntentSender: ((Intent) -> Unit)? = null
) : AndroidViewModel(application) {

    companion object {
        private const val TAG = "ArchDrop:AudioViewModel"
        const val PREFS_NAME = "archdrop_prefs"
        const val KEY_LAST_PEER_HOST = "last_peer_host"
        const val DEFAULT_FALLBACK_HOST = "10.2.0.2"
        const val DEFAULT_BITRATE = 96000
    }

    private val prefs = application.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    // 1. Streaming & Connection State
    private val _isStreaming = MutableStateFlow(false)
    val isStreaming: StateFlow<Boolean> = _isStreaming.asStateFlow()

    val receiverState: StateFlow<AudioReceiverState> = audioReceiver.state
    val telemetry: StateFlow<JitterTelemetry> = audioReceiver.telemetry

    // 2. Target Host & Port
    private val initialHost = prefs.getString(KEY_LAST_PEER_HOST, null)
        ?.trim()
        ?.takeIf { it.isNotEmpty() }
        ?: DEFAULT_FALLBACK_HOST

    private val _targetHost = MutableStateFlow(initialHost)
    val targetHost: StateFlow<String> = _targetHost.asStateFlow()

    private val _targetPort = MutableStateFlow(AudioStreamReceiver.DEFAULT_PORT)
    val targetPort: StateFlow<Int> = _targetPort.asStateFlow()

    // 3. Audio Codec & Stream Parameters
    private val _selectedCodec = MutableStateFlow(AudioCodec.OPUS)
    val selectedCodec: StateFlow<AudioCodec> = _selectedCodec.asStateFlow()

    private val _selectedBitrate = MutableStateFlow(DEFAULT_BITRATE)
    val selectedBitrate: StateFlow<Int> = _selectedBitrate.asStateFlow()

    private val _sinkMode = MutableStateFlow("monitor")
    val sinkMode: StateFlow<String> = _sinkMode.asStateFlow()

    // 4. Volume Boost & Mute
    private val _volumeBoost = MutableStateFlow(1.0f)
    val volumeBoost: StateFlow<Float> = _volumeBoost.asStateFlow()

    val volumeBoostPercent: Int
        get() = (_volumeBoost.value * 100).roundToInt()

    private val _isMuted = MutableStateFlow(false)
    val isMuted: StateFlow<Boolean> = _isMuted.asStateFlow()

    // 5. Audio Output Devices
    private val _availableOutputDevices = MutableStateFlow<List<AudioDeviceUiModel>>(
        listOf(AudioDeviceUiModel.SYSTEM_DEFAULT)
    )
    val availableOutputDevices: StateFlow<List<AudioDeviceUiModel>> = _availableOutputDevices.asStateFlow()
    val availableDevices: StateFlow<List<AudioDeviceUiModel>> = availableOutputDevices

    private val _selectedOutputDevice = MutableStateFlow(AudioDeviceUiModel.SYSTEM_DEFAULT)
    val selectedOutputDevice: StateFlow<AudioDeviceUiModel> = _selectedOutputDevice.asStateFlow()
    val selectedDevice: StateFlow<AudioDeviceUiModel> = selectedOutputDevice

    // 6. Mesh Identity Fingerprint
    var peerFingerprint: String = "ANDROID_FP"
        private set

    private val audioDeviceCallback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>?) {
            refreshAudioDevices()
        }

        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>?) {
            refreshAudioDevices()
        }
    }

    init {
        loadMeshIdentityFingerprint()
        observeReceiverState()
        registerAudioDeviceCallbackSafely()
        refreshAudioDevices()
    }

    private fun loadMeshIdentityFingerprint() {
        try {
            val keyFile = File(getApplication<Application>().filesDir, "mesh_identity.key")
            val identity = MeshIdentity.loadOrGenerate(keyFile)
            peerFingerprint = identity.toHex()
        } catch (e: Exception) {
            Log.w(TAG, "Error loading mesh identity fingerprint: ${e.message}")
            peerFingerprint = "ANDROID_FP"
        }
    }

    private fun observeReceiverState() {
        viewModelScope.launch {
            audioReceiver.state.collect { state ->
                when (state) {
                    is AudioReceiverState.Streaming,
                    is AudioReceiverState.Connecting,
                    is AudioReceiverState.Paused -> {
                        _isStreaming.value = true
                    }
                    is AudioReceiverState.Idle,
                    is AudioReceiverState.Disconnected,
                    is AudioReceiverState.Error -> {
                        _isStreaming.value = false
                    }
                }
            }
        }
    }

    private fun registerAudioDeviceCallbackSafely() {
        try {
            audioManager?.registerAudioDeviceCallback(audioDeviceCallback, null)
        } catch (e: Throwable) {
            Log.w(TAG, "Error registering AudioDeviceCallback: ${e.message}")
        }
    }

    /**
     * Refreshes the list of available audio output devices.
     */
    fun refreshAudioDevices() {
        try {
            val am = audioManager ?: return
            val rawDevices = am.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
            val uiDevices = rawDevices.map { mapDeviceInfoToUiModel(it) }
            val fullList = listOf(AudioDeviceUiModel.SYSTEM_DEFAULT) + uiDevices
            _availableOutputDevices.value = fullList

            // If selected device was disconnected, fallback to System Default
            val current = _selectedOutputDevice.value
            if (!current.isDefault && fullList.none { it.id == current.id }) {
                selectOutputDevice(AudioDeviceUiModel.SYSTEM_DEFAULT)
            }
        } catch (e: Throwable) {
            Log.w(TAG, "Error enumerating audio output devices: ${e.message}")
        }
    }

    /**
     * Maps an Android [AudioDeviceInfo] to an [AudioDeviceUiModel].
     */
    fun mapDeviceInfoToUiModel(device: AudioDeviceInfo): AudioDeviceUiModel {
        val type = when (device.type) {
            AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
            AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> AudioDeviceType.SPEAKER

            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_LINE_ANALOG,
            AudioDeviceInfo.TYPE_LINE_DIGITAL -> AudioDeviceType.WIRED

            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
            AudioDeviceInfo.TYPE_BLE_HEADSET,
            AudioDeviceInfo.TYPE_BLE_SPEAKER,
            AudioDeviceInfo.TYPE_HEARING_AID -> AudioDeviceType.BLUETOOTH

            AudioDeviceInfo.TYPE_USB_DEVICE,
            AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_USB_ACCESSORY -> AudioDeviceType.USB

            else -> AudioDeviceType.SPEAKER
        }

        val rawName = device.productName?.toString()?.trim()
        val name = if (!rawName.isNullOrEmpty()) {
            rawName
        } else {
            when (type) {
                AudioDeviceType.SPEAKER -> if (device.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE) "Earpiece" else "Phone Speaker"
                AudioDeviceType.WIRED -> "Wired Headphones"
                AudioDeviceType.BLUETOOTH -> "Bluetooth Audio"
                AudioDeviceType.USB -> "USB Audio"
                AudioDeviceType.DEFAULT -> "System Default"
            }
        }

        return AudioDeviceUiModel(
            id = device.id,
            name = name,
            type = type,
            isDefault = false,
            rawDeviceInfo = device
        )
    }

    /**
     * Sets and persists the target host IP address.
     */
    fun setTargetHost(host: String) {
        val trimmed = host.trim()
        if (trimmed.isNotEmpty()) {
            _targetHost.value = trimmed
            prefs.edit().putString(KEY_LAST_PEER_HOST, trimmed).apply()
        }
    }

    /**
     * Sets target port (1..65535).
     */
    fun setTargetPort(port: Int) {
        if (port in 1..65535) {
            _targetPort.value = port
        }
    }

    /**
     * Sets audio codec (Opus or raw PCM).
     */
    fun setCodec(codec: AudioCodec) {
        _selectedCodec.value = codec
    }

    fun setSelectedCodec(codec: AudioCodec) {
        setCodec(codec)
    }

    /**
     * Sets target bitrate (e.g. 64000, 96000, 128000 bps).
     */
    fun setBitrate(bitrate: Int) {
        _selectedBitrate.value = bitrate.coerceIn(64000, 128000)
    }

    /**
     * Sets desktop sink mode ("monitor" or "virtual").
     */
    fun setSinkMode(mode: String) {
        _sinkMode.value = mode
    }

    /**
     * Sets software volume boost factor, clamped to [1.0f, 2.0f] (100%–200%).
     */
    fun setVolumeBoost(boost: Float) {
        val clamped = if (boost.isNaN()) 1.0f else boost.coerceIn(1.0f, 2.0f)
        _volumeBoost.value = clamped
        audioReceiver.setVolumeBoost(clamped)
    }

    /**
     * Sets mute state.
     */
    fun setMute(muted: Boolean) {
        _isMuted.value = muted
        audioReceiver.setMute(muted)
    }

    /**
     * Toggles mute state.
     */
    fun toggleMute() {
        setMute(!_isMuted.value)
    }

    /**
     * Selects preferred audio output device.
     */
    fun selectOutputDevice(device: AudioDeviceUiModel) {
        _selectedOutputDevice.value = device
        audioReceiver.setPreferredOutputDevice(device.rawDeviceInfo)
    }

    fun selectDevice(device: AudioDeviceUiModel?) {
        selectOutputDevice(device ?: AudioDeviceUiModel.SYSTEM_DEFAULT)
    }

    /**
     * Starts audio streaming session.
     */
    fun startStreaming(
        useServiceIntent: Boolean = false,
        host: String = _targetHost.value,
        port: Int = _targetPort.value,
        codec: AudioCodec = _selectedCodec.value,
        bitrate: Int = _selectedBitrate.value,
        sinkMode: String = _sinkMode.value
    ) {
        val trimmedHost = host.trim().ifEmpty { DEFAULT_FALLBACK_HOST }

        if (useServiceIntent) {
            val intent = Intent(getApplication(), ArchDropSyncService::class.java).apply {
                action = ArchDropSyncService.ACTION_START_AUDIO_STREAM
                putExtra(ArchDropSyncService.EXTRA_AUDIO_HOST, trimmedHost)
                putExtra(ArchDropSyncService.EXTRA_AUDIO_PORT, port)
                putExtra(ArchDropSyncService.EXTRA_AUDIO_CODEC, codec.asParam())
                putExtra(ArchDropSyncService.EXTRA_AUDIO_BITRATE, bitrate)
            }
            dispatchServiceIntent(intent)
        } else {
            _isStreaming.value = true
            audioReceiver.start(
                host = trimmedHost,
                port = port,
                fingerprint = peerFingerprint,
                codec = codec,
                targetBitrate = bitrate,
                sinkMode = sinkMode
            )
            // Ensure background service foreground notification is running
            dispatchForegroundNotificationIntent()
        }
    }

    /**
     * Stops audio streaming session.
     */
    fun stopStreaming(useServiceIntent: Boolean = false) {
        if (useServiceIntent) {
            val intent = Intent(getApplication(), ArchDropSyncService::class.java).apply {
                action = ArchDropSyncService.ACTION_STOP_AUDIO_STREAM
            }
            dispatchServiceIntent(intent)
        } else {
            audioReceiver.stop()
        }
        _isStreaming.value = false
    }

    /**
     * Toggles streaming session on or off.
     */
    fun toggleStreaming() {
        if (_isStreaming.value) {
            stopStreaming()
        } else {
            startStreaming()
        }
    }

    private fun dispatchServiceIntent(intent: Intent) {
        try {
            if (serviceIntentSender != null) {
                serviceIntentSender.invoke(intent)
            } else {
                val app = getApplication<Application>()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    app.startForegroundService(intent)
                } else {
                    app.startService(intent)
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Error dispatching service intent: ${e.message}")
        }
    }

    private fun dispatchForegroundNotificationIntent() {
        try {
            val intent = Intent(getApplication(), ArchDropSyncService::class.java).apply {
                action = ArchDropSyncService.ACTION_START
            }
            dispatchServiceIntent(intent)
        } catch (_: Exception) {}
    }

    override fun onCleared() {
        super.onCleared()
        try {
            audioManager?.unregisterAudioDeviceCallback(audioDeviceCallback)
        } catch (_: Throwable) {}
        stopStreaming()
    }
}
