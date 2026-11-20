package org.archdrop.ui.webcam

import android.app.Application
import android.content.Context
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.archdrop.mesh.MeshIdentity
import org.archdrop.webcam.WebcamConnectionState
import org.archdrop.webcam.WebcamLens
import org.archdrop.webcam.WebcamResolution
import org.archdrop.webcam.WebcamStreamManager
import java.io.File
import java.security.MessageDigest

/**
 * ViewModel managing the state and controls for the Arch Drop Virtual Webcam streaming pipeline.
 *
 * Coordinates CameraX capture, MJPEG chunked HTTP streaming egress to the Linux desktop host,
 * resolution and lens selection, low-power AMOLED blackout mode, and camera runtime permissions.
 */
class WebcamViewModel @JvmOverloads constructor(
    application: Application,
    val streamManager: WebcamStreamManager = WebcamStreamManager(application)
) : AndroidViewModel(application) {

    val targetPort: Int = WebcamStreamManager.DEFAULT_PORT // 53317

    private val prefs = application.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)

    private val _isStreaming = MutableStateFlow(false)
    val isStreaming: StateFlow<Boolean> = _isStreaming.asStateFlow()

    val fps: StateFlow<Double> = streamManager.fps
    val connectionState: StateFlow<WebcamConnectionState> = streamManager.connectionState

    private val _selectedResolution = MutableStateFlow(WebcamResolution.RES_720P)
    val selectedResolution: StateFlow<WebcamResolution> = _selectedResolution.asStateFlow()

    private val _selectedLens = MutableStateFlow(WebcamLens.BACK)
    val selectedLens: StateFlow<WebcamLens> = _selectedLens.asStateFlow()

    private val _isLowPowerMode = MutableStateFlow(false)
    val isLowPowerMode: StateFlow<Boolean> = _isLowPowerMode.asStateFlow()

    private val initialHost = prefs.getString("last_peer_host", null)?.trim()?.takeIf { it.isNotEmpty() } ?: "10.2.0.2"
    private val _targetHost = MutableStateFlow(initialHost)
    val targetHost: StateFlow<String> = _targetHost.asStateFlow()

    private val _hasCameraPermission = MutableStateFlow(false)
    val hasCameraPermission: StateFlow<Boolean> = _hasCameraPermission.asStateFlow()

    private var localFingerprint: String = "ANDROID_FP"

    init {
        loadMeshIdentityFingerprint()
        viewModelScope.launch {
            streamManager.connectionState.collect { state ->
                when (state) {
                    is WebcamConnectionState.Streaming -> {
                        _isStreaming.value = true
                    }
                    is WebcamConnectionState.Connecting -> {
                        _isStreaming.value = true
                    }
                    is WebcamConnectionState.Idle,
                    is WebcamConnectionState.Disconnected,
                    is WebcamConnectionState.Error -> {
                        _isStreaming.value = false
                    }
                }
            }
        }
    }

    private fun loadMeshIdentityFingerprint() {
        try {
            val keyFile = File(getApplication<Application>().filesDir, "mesh_identity.key")
            val identity = MeshIdentity.loadOrGenerate(keyFile)
            val md = MessageDigest.getInstance("SHA-256")
            localFingerprint = md.digest(identity.publicKeyBytes).joinToString("") { "%02X".format(it) }
        } catch (_: Exception) {
            localFingerprint = "ANDROID_FP"
        }
    }

    fun setTargetHost(host: String) {
        val trimmed = host.trim()
        if (trimmed.isNotEmpty()) {
            _targetHost.value = trimmed
            prefs.edit().putString("last_peer_host", trimmed).apply()
        }
    }

    fun setResolution(resolution: WebcamResolution, lifecycleOwner: LifecycleOwner? = null) {
        if (_selectedResolution.value == resolution) return
        _selectedResolution.value = resolution

        if (_isStreaming.value && lifecycleOwner != null) {
            streamManager.stopStreaming()
            streamManager.startStreaming(
                lifecycleOwner = lifecycleOwner,
                targetHost = _targetHost.value,
                targetPort = targetPort,
                resolution = resolution,
                lens = _selectedLens.value,
                localFingerprint = localFingerprint
            )
        }
    }

    fun setLens(lens: WebcamLens, lifecycleOwner: LifecycleOwner? = null) {
        if (_selectedLens.value == lens) return
        _selectedLens.value = lens

        if (_isStreaming.value && lifecycleOwner != null) {
            streamManager.stopStreaming()
            streamManager.startStreaming(
                lifecycleOwner = lifecycleOwner,
                targetHost = _targetHost.value,
                targetPort = targetPort,
                resolution = _selectedResolution.value,
                lens = lens,
                localFingerprint = localFingerprint
            )
        }
    }

    fun toggleLowPowerMode() {
        _isLowPowerMode.value = !_isLowPowerMode.value
    }

    fun setLowPowerMode(enabled: Boolean) {
        _isLowPowerMode.value = enabled
    }

    fun updateCameraPermission(granted: Boolean) {
        _hasCameraPermission.value = granted
    }

    fun startStreaming(lifecycleOwner: LifecycleOwner) {
        if (streamManager.isStreamingActive()) return
        _isStreaming.value = true
        streamManager.startStreaming(
            lifecycleOwner = lifecycleOwner,
            targetHost = _targetHost.value,
            targetPort = targetPort,
            resolution = _selectedResolution.value,
            lens = _selectedLens.value,
            localFingerprint = localFingerprint
        )
    }

    fun stopStreaming() {
        _isStreaming.value = false
        streamManager.stopStreaming()
    }

    override fun onCleared() {
        super.onCleared()
        stopStreaming()
    }
}
