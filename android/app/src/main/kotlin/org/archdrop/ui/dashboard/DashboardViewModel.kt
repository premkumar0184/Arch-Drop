package org.archdrop.ui.dashboard

import android.app.Application
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import org.archdrop.clipboard.ClipContent
import org.archdrop.clipboard.ClipboardSyncManager
import org.archdrop.clipboard.models.ClipboardDescriptorPacket
import org.archdrop.clipboard.models.ClipboardInlinePacket
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.archdrop.mesh.MeshIdentity
import org.archdrop.service.ArchDropSyncService
import org.archdrop.transfer.DiscoveredPeer
import org.archdrop.transfer.RouteState
import org.archdrop.transfer.TransferClient
import org.archdrop.transfer.TransferRepository
import org.archdrop.transfer.TransportRoute
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID

sealed class SendClipboardResult {
    data object Empty : SendClipboardResult()
    data class Success(val summary: String) : SendClipboardResult()
    data class Error(val message: String) : SendClipboardResult()
}

data class TransferUiModel(
    val eventId: String,
    val fileName: String,
    val fileSizeFormatted: String,
    val sha256: String,
    val timestampFormatted: String,
    val isIncoming: Boolean,
    val isSuccess: Boolean
)

data class DashboardUiState(
    val deviceAlias: String = "",
    val meshKeyHex: String = "",
    val fingerprint: String = "",
    val activeRoute: TransportRoute? = null,
    val routeState: RouteState = RouteState.IDLE,
    val lohsActive: Boolean = false,
    val lohsSsid: String? = null,
    val bleAdvertising: Boolean = true,
    val derpConnected: Boolean = true,
    val derpLatencyMs: Long? = 28L,
    val transfers: List<TransferUiModel> = emptyList(),
    val isServiceRunning: Boolean = true,
    val isSendingClipboard: Boolean = false
)

class DashboardViewModel(
    application: Application,
    private val database: ArchDropDatabase
) : AndroidViewModel(application) {

    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    private val _deviceAlias = MutableStateFlow(
        application.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .getString("device_alias", Build.MODEL ?: "Android Device") ?: "Android Device"
    )
    private val _meshKey = MutableStateFlow("")
    private val _fingerprint = MutableStateFlow("")
    private val _lohsActive = MutableStateFlow(false)
    private val _lohsSsid = MutableStateFlow<String?>(null)
    private val _isServiceRunning = MutableStateFlow(true)
    private val _isSendingClipboard = MutableStateFlow(false)

    init {
        loadMeshIdentity()
    }

    private fun loadMeshIdentity() {
        viewModelScope.launch(Dispatchers.IO) {
            val context = getApplication<Application>()
            val keyFile = File(context.filesDir, "mesh_identity.key")
            val identity = MeshIdentity.loadOrGenerate(keyFile)
            val keyHex = identity.toHex()
            val md = MessageDigest.getInstance("SHA-256")
            val fp = md.digest(identity.publicKeyBytes).joinToString("") { "%02X".format(it) }

            _meshKey.value = keyHex
            _fingerprint.value = fp
        }
    }

    val transferRepository: TransferRepository = TransferRepository(context = application, database = database)

    val uiState: StateFlow<DashboardUiState> = combine(
        combine(_deviceAlias, _meshKey, _fingerprint, _lohsActive, _isServiceRunning) { alias, meshKey, fp, lohsActive, isRunning ->
            DashboardUiState(
                deviceAlias = alias,
                meshKeyHex = meshKey,
                fingerprint = fp,
                lohsActive = lohsActive,
                lohsSsid = if (lohsActive) "DIRECT-ArchDrop-${meshKey.take(4)}" else null,
                bleAdvertising = isRunning,
                derpConnected = isRunning,
                derpLatencyMs = if (isRunning) 28L else null,
                isServiceRunning = isRunning
            )
        },
        transferRepository.activeRoute,
        transferRepository.routeState,
        _isSendingClipboard,
        database.eventDao().observeTransferEvents(15)
    ) { baseState, route, rState, isSending, events ->
        baseState.copy(
            activeRoute = route,
            routeState = rState,
            transfers = events.map { eventToTransferUiModel(it) },
            isSendingClipboard = isSending
        )
    }.stateIn(
        scope = viewModelScope,
        started = SharingStarted.WhileSubscribed(5_000L),
        initialValue = DashboardUiState()
    )

    fun updateAlias(newAlias: String) {
        val trimmed = newAlias.trim()
        if (trimmed.isEmpty()) return
        _deviceAlias.value = trimmed
        getApplication<Application>()
            .getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .edit()
            .putString("device_alias", trimmed)
            .apply()
    }

    fun toggleHotspot() {
        val next = !_lohsActive.value
        _lohsActive.value = next
        _lohsSsid.value = if (next) "DIRECT-ArchDrop-${_meshKey.value.take(4)}" else null

        val context = getApplication<Application>()
        val intent = Intent(context, ArchDropSyncService::class.java).apply {
            action = if (next) ArchDropSyncService.ACTION_START_HOTSPOT else ArchDropSyncService.ACTION_STOP_HOTSPOT
        }
        try {
            context.startService(intent)
        } catch (_: Exception) {}
    }

    fun toggleService() {
        val next = !_isServiceRunning.value
        _isServiceRunning.value = next
        val context = getApplication<Application>()
        val intent = Intent(context, ArchDropSyncService::class.java).apply {
            action = if (next) ArchDropSyncService.ACTION_START else ArchDropSyncService.ACTION_STOP
        }
        try {
            if (next) {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    context.startForegroundService(intent)
                } else {
                    context.startService(intent)
                }
            } else {
                context.startService(intent)
            }
        } catch (_: Exception) {}
    }

    suspend fun sendClipboard(
        context: Context,
        targetHost: String? = null,
        targetPort: Int = 53317
    ): SendClipboardResult {
        _isSendingClipboard.value = true
        return try {
            withContext(Dispatchers.IO) {
                val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                val clip = cm?.primaryClip
                if (clip == null || clip.itemCount == 0) {
                    return@withContext SendClipboardResult.Empty
                }

                val syncManager = ClipboardSyncManager(context, _meshKey.value.ifEmpty { "ANDROID_DEVICE" })
                val resolved = syncManager.resolveClip(clip)

                if (targetHost != null) {
                    // Direct destination explicitly provided (e.g. testing or explicit host override)
                    val route = TransportRoute.DirectLan(host = targetHost, port = targetPort)
                    transferRepository.setActiveRoute(route)

                    when (resolved) {
                        is ClipContent.Empty -> SendClipboardResult.Empty
                        is ClipContent.Error -> SendClipboardResult.Error(resolved.message)
                        is ClipContent.Text -> {
                            val text = resolved.text
                            val hash = ClipboardSyncManager.sha256(text.toByteArray(Charsets.UTF_8))
                            syncManager.setLastPastedHash(hash)

                            val packet = syncManager.packText(text, downloadUrlBase = "http://127.0.0.1:53317")
                            val sendOk = sendClipboardPacket(packet, listOf(targetHost), targetPort)
                            if (sendOk) {
                                val snippet = if (text.length > 25) text.take(25) + "..." else text
                                SendClipboardResult.Success("Text \"$snippet\"")
                            } else {
                                SendClipboardResult.Error("Could not reach desktop at $targetHost:$targetPort")
                            }
                        }
                        is ClipContent.Image -> {
                            val hash = resolved.sha256
                            syncManager.setLastPastedHash(hash)

                            val packet = syncManager.packImage(resolved.bytes, resolved.mimeType, downloadUrlBase = "http://127.0.0.1:53317")
                            val sendOk = sendClipboardPacket(packet, listOf(targetHost), targetPort)
                            if (sendOk) {
                                SendClipboardResult.Success("Image (${formatBytes(resolved.bytes.size.toLong())})")
                            } else {
                                SendClipboardResult.Error("Could not reach desktop at $targetHost:$targetPort")
                            }
                        }
                        is ClipContent.FilePayload -> {
                            val client = TransferClient(context)
                            val result = client.sendStagedFile(
                                targetHost = targetHost,
                                targetPort = targetPort,
                                uri = resolved.uri,
                                fileName = resolved.fileName,
                                fileSize = resolved.fileSize,
                                mimeType = resolved.mimeType,
                                senderAlias = _deviceAlias.value,
                                senderFingerprint = _fingerprint.value.ifEmpty { "ANDROID_FP" }
                            )

                            if (result.success) {
                                recordTransferEvent(resolved.fileName, resolved.fileSize, resolved.sha256 ?: "")
                                SendClipboardResult.Success("File ${resolved.fileName} (${formatBytes(resolved.fileSize)})")
                            } else {
                                SendClipboardResult.Error(result.errorMessage ?: "Failed to upload file")
                            }
                        }
                    }
                } else {
                    // Dynamic 3-tier routing via TransferRepository (replaces hardcoded lists)
                    val trustedPeer = database.peerDao().getAllPeers().firstOrNull { it.isTrusted }
                        ?.let { DiscoveredPeer.fromEntity(it) }
                        ?: DiscoveredPeer(
                            peerId = _meshKey.value.ifEmpty { "DESKTOP_DEFAULT" },
                            alias = "Arch Linux Desktop",
                            fingerprint = _fingerprint.value.ifEmpty { "DESKTOP_FP" },
                            host = null
                        )

                    when (resolved) {
                        is ClipContent.Empty -> SendClipboardResult.Empty
                        is ClipContent.Error -> SendClipboardResult.Error(resolved.message)
                        is ClipContent.Text -> {
                            val text = resolved.text
                            val hash = ClipboardSyncManager.sha256(text.toByteArray(Charsets.UTF_8))
                            syncManager.setLastPastedHash(hash)

                            val packet = syncManager.packText(text, downloadUrlBase = "http://127.0.0.1:53317")
                            val sendOk = transferRepository.sendClipboard(trustedPeer, packet, _fingerprint.value.ifEmpty { "ANDROID_FP" })
                            if (sendOk) {
                                val snippet = if (text.length > 25) text.take(25) + "..." else text
                                SendClipboardResult.Success("Text \"$snippet\"")
                            } else {
                                SendClipboardResult.Error("Could not reach desktop via dynamic transport route")
                            }
                        }
                        is ClipContent.Image -> {
                            val hash = resolved.sha256
                            syncManager.setLastPastedHash(hash)

                            val packet = syncManager.packImage(resolved.bytes, resolved.mimeType, downloadUrlBase = "http://127.0.0.1:53317")
                            val sendOk = transferRepository.sendClipboard(trustedPeer, packet, _fingerprint.value.ifEmpty { "ANDROID_FP" })
                            if (sendOk) {
                                SendClipboardResult.Success("Image (${formatBytes(resolved.bytes.size.toLong())})")
                            } else {
                                SendClipboardResult.Error("Could not reach desktop via dynamic transport route")
                            }
                        }
                        is ClipContent.FilePayload -> {
                            val result = transferRepository.sendStagedFileWithFailover(
                                peer = trustedPeer,
                                uri = resolved.uri,
                                fileName = resolved.fileName,
                                fileSize = resolved.fileSize,
                                mimeType = resolved.mimeType,
                                sha256 = resolved.sha256,
                                senderAlias = _deviceAlias.value,
                                senderFingerprint = _fingerprint.value.ifEmpty { "ANDROID_FP" }
                            )

                            if (result.success) {
                                SendClipboardResult.Success("File ${resolved.fileName} (${formatBytes(resolved.fileSize)})")
                            } else {
                                SendClipboardResult.Error(result.errorMessage ?: "Failed to upload file")
                            }
                        }
                    }
                }
            }
        } catch (e: SecurityException) {
            SendClipboardResult.Error("Permission denied: ${e.message}")
        } catch (e: Exception) {
            SendClipboardResult.Error(e.message ?: "Unknown error")
        } finally {
            _isSendingClipboard.value = false
        }
    }

    private fun resolveTargets(targetHost: String?): List<String> {
        if (targetHost != null) return listOf(targetHost)
        val targets = mutableListOf<String>()
        val prefs = getApplication<Application>().getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
        val lastHost = prefs.getString("last_peer_host", null)
        if (!lastHost.isNullOrEmpty()) {
            targets.add(lastHost)
        }
        targets.add("10.2.0.2")
        return targets.distinct()
    }

    private fun sendClipboardPacket(packet: Any, targets: List<String>, targetPort: Int): Boolean {
        val jsonStr = when (packet) {
            is ClipboardInlinePacket -> json.encodeToString(packet)
            is ClipboardDescriptorPacket -> json.encodeToString(packet)
            else -> return false
        }

        val mediaType = "application/json; charset=utf-8".toMediaType()
        val requestBody = jsonStr.toRequestBody(mediaType)
        val client = okhttp3.OkHttpClient.Builder()
            .connectTimeout(3, java.util.concurrent.TimeUnit.SECONDS)
            .readTimeout(5, java.util.concurrent.TimeUnit.SECONDS)
            .build()

        var success = false
        for (host in targets) {
            val url = "http://$host:$targetPort/api/v2/clipboard"
            val request = okhttp3.Request.Builder()
                .url(url)
                .post(requestBody)
                .header("Content-Type", "application/json")
                .header("X-Fingerprint", _fingerprint.value.ifEmpty { "ANDROID_FP" })
                .build()

            try {
                client.newCall(request).execute().use { resp ->
                    if (resp.isSuccessful) {
                        success = true
                    }
                }
            } catch (_: Exception) {}
            if (success) break
        }
        return success
    }

    private suspend fun recordTransferEvent(fileName: String, fileSize: Long, sha256: String) {
        try {
            val origin = _meshKey.value.ifEmpty { "ANDROID_LOCAL" }
            val nextSeq = database.eventDao().getNextSequenceNumber(origin)
            val now = System.currentTimeMillis()
            val payload = "{\"file\":\"$fileName\",\"size\":$fileSize,\"sha256\":\"$sha256\"}"
            val hash = ClipboardSyncManager.sha256(payload.toByteArray(Charsets.UTF_8))
            val event = EventEntity(
                originDeviceId = origin,
                sequenceNumber = nextSeq,
                eventId = UUID.randomUUID().toString(),
                timestampMs = now,
                eventType = "transfer.completed",
                payload = payload,
                payloadHash = hash,
                ackStatus = 0
            )
            database.eventDao().insertEvent(event)
        } catch (_: Exception) {}
    }

    companion object {
        fun eventToTransferUiModel(event: EventEntity): TransferUiModel {
            var fileName = "File"
            var fileSize = 0L
            var sha256 = event.payloadHash

            try {
                if (event.payload.startsWith("{")) {
                    val json = JSONObject(event.payload)
                    fileName = json.optString("file", json.optString("fileName", "File"))
                    fileSize = json.optLong("size", json.optLong("fileSize", 0L))
                    sha256 = json.optString("sha256", sha256)
                }
            } catch (_: Exception) {}

            val isIncoming = event.eventType.contains("incoming") || event.eventType.contains("completed")
            val isSuccess = !event.eventType.contains("failed")

            return TransferUiModel(
                eventId = event.eventId,
                fileName = fileName,
                fileSizeFormatted = formatBytes(fileSize),
                sha256 = sha256,
                timestampFormatted = formatTimestamp(event.timestampMs),
                isIncoming = isIncoming,
                isSuccess = isSuccess
            )
        }

        fun formatBytes(bytes: Long): String {
            if (bytes <= 0) return "0 B"
            val units = arrayOf("B", "KiB", "MiB", "GiB", "TiB")
            val digitGroups = (Math.log10(bytes.toDouble()) / Math.log10(1024.0)).toInt()
            val value = bytes / Math.pow(1024.0, digitGroups.toDouble())
            return String.format(Locale.US, "%.1f %s", value, units[digitGroups])
        }

        fun formatTimestamp(timestampMs: Long): String {
            val now = System.currentTimeMillis()
            val diffSecs = (now - timestampMs) / 1000L
            return when {
                diffSecs < 60 -> "Just now"
                diffSecs < 3600 -> "${diffSecs / 60}m ago"
                diffSecs < 86400 -> "${diffSecs / 3600}h ago"
                else -> SimpleDateFormat("MMM d, HH:mm", Locale.getDefault()).format(Date(timestampMs))
            }
        }
    }
}
