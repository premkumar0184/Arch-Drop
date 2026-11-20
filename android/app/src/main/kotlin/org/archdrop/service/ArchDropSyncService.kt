package org.archdrop.service

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Environment
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import androidx.room.withTransaction
import org.archdrop.ArchDropApp
import org.archdrop.companion.R
import org.archdrop.clipboard.ClipboardSyncManager
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.archdrop.db.OutboxEntity
import org.archdrop.mesh.DerpMeshClient
import org.archdrop.mesh.LoopbackTunnelBridge
import org.archdrop.mesh.MeshIdentity
import org.archdrop.transfer.HttpReceiver
import java.io.File
import java.security.MessageDigest
import java.util.UUID

/**
 * Core Android background foreground service for Arch Drop.
 *
 * Coordinates:
 * 1. LocalSend v2 HTTP streaming receiver (0.0.0.0:53317).
 * 2. Pure-Kotlin userspace DERP client (wss://:443) and LoopbackTunnelBridge (127.0.0.1:53318).
 * 3. Android Room database (ArchDropDatabase) event ledger and durable outbox reconciliation.
 * 4. Wayland-compatible clipboard synchronization with deterministic LWW arbitration.
 * 5. Android Local-Only Hotspot (LOHS) activation and BLE GATT peripheral advertisement.
 *
 * Declares FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE and FOREGROUND_SERVICE_TYPE_DATA_SYNC
 * to guarantee uninhibited operation without claiming Android's VpnService.
 */
class ArchDropSyncService : Service() {

    companion object {
        private const val TAG = "ArchDrop:SyncService"
        const val NOTIFICATION_ID = 53317
        const val ACTION_START = "org.archdrop.action.START"
        const val ACTION_STOP = "org.archdrop.action.STOP"
        const val ACTION_START_HOTSPOT = "org.archdrop.action.START_HOTSPOT"
        const val ACTION_STOP_HOTSPOT = "org.archdrop.action.STOP_HOTSPOT"
        const val ACTION_CONNECT_RELAY = "org.archdrop.action.CONNECT_RELAY"
        const val ACTION_BROADCAST_CLIPBOARD = "org.archdrop.action.BROADCAST_CLIPBOARD"
        const val ACTION_START_AUDIO_STREAM = "org.archdrop.action.START_AUDIO_STREAM"
        const val ACTION_STOP_AUDIO_STREAM = "org.archdrop.action.STOP_AUDIO_STREAM"
        const val EXTRA_RELAY_URL = "org.archdrop.extra.RELAY_URL"
        const val EXTRA_CLIPBOARD_TEXT = "org.archdrop.extra.CLIPBOARD_TEXT"
        const val EXTRA_PEER_HOST = "org.archdrop.extra.PEER_HOST"
        const val EXTRA_PEER_PORT = "org.archdrop.extra.PEER_PORT"
        const val EXTRA_AUDIO_HOST = "org.archdrop.extra.AUDIO_HOST"
        const val EXTRA_AUDIO_PORT = "org.archdrop.extra.AUDIO_PORT"
        const val EXTRA_AUDIO_CODEC = "org.archdrop.extra.AUDIO_CODEC"
        const val EXTRA_AUDIO_BITRATE = "org.archdrop.extra.AUDIO_BITRATE"
    }

    private val serviceScope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    private var database: ArchDropDatabase? = null
    private var identity: MeshIdentity? = null
    private var httpReceiver: HttpReceiver? = null
    private var derpMeshClient: DerpMeshClient? = null
    private var loopbackBridge: LoopbackTunnelBridge? = null
    private var clipboardManager: ClipboardSyncManager? = null
    private var lohsManager: LOHSManager? = null
    private var blePeripheralManager: BlePeripheralManager? = null
    var transferRepository: org.archdrop.transfer.TransferRepository? = null
    var audioReceiver: org.archdrop.audio.AudioStreamReceiver? = null
        private set

    fun getOrCreateAudioReceiver(): org.archdrop.audio.AudioStreamReceiver {
        if (audioReceiver == null) {
            audioReceiver = org.archdrop.audio.AudioStreamReceiver(this)
        }
        return audioReceiver!!
    }

    private var outboxSyncJob: Job? = null
    private var clipboardListener: ClipboardManager.OnPrimaryClipChangedListener? = null
    var lastPeerHost: String? = null
    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "Creating ArchDropSyncService...")

        // 1. Initialize Android Room Database
        val db = ArchDropDatabase.getInstance(this)
        database = db

        // 2. Load or generate local Mesh Ed25519 Identity
        val keyFile = File(filesDir, "mesh_identity.key")
        val meshId = MeshIdentity.loadOrGenerate(keyFile)
        identity = meshId
        Log.i(TAG, "Initialized Mesh Identity public key: ${meshId.toHex()}")

        val downloadDir = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
            ?: File(filesDir, "downloads")

        val deviceName = Build.MODEL ?: "Android Device"

        // 3. Initialize LocalSend HTTP Receiver with Room DB ledger ingestion
        httpReceiver = HttpReceiver(
            port = 53317,
            downloadDir = downloadDir,
            deviceAlias = deviceName,
            fingerprint = meshId.toHex(),
            onFileReceived = { file, sha256 ->
                serviceScope.launch {
                    val payload = "{\"file\":\"${file.name}\",\"size\":${file.length()},\"sha256\":\"$sha256\"}"
                    recordLocalEvent(
                        eventType = "transfer.completed",
                        payload = payload
                    )
                }
            }
        )

        // 4. Initialize Userspace DERP Mesh Client & Loopback Bridge
        val client = DerpMeshClient(identity = meshId)
        derpMeshClient = client

        val bridge = LoopbackTunnelBridge(
            derpClient = client,
            httpReceiverPort = 53317,
            bridgeListenPort = 53318
        )
        loopbackBridge = bridge

        // 5. Initialize Clipboard, LOHS, and BLE Peripheral
        val cm = ClipboardSyncManager(this, meshId.toHex())
        clipboardManager = cm
        httpReceiver?.clipboardManager = cm
        httpReceiver?.onClipboardReceived = { text, remoteHost ->
            lastPeerHost = remoteHost
            getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
                .edit()
                .putString("last_peer_host", remoteHost)
                .apply()
            serviceScope.launch {
                val payloadJson = json.encodeToString(mapOf("text" to text, "origin" to remoteHost))
                recordLocalEvent(
                    eventType = "clipboard.synced",
                    payload = payloadJson
                )
            }
        }
        setupClipboardMonitoring()

        val lohs = LOHSManager(this)
        lohsManager = lohs
        val ble = BlePeripheralManager(this, lohs, db.peerDao())
        ble.clipboardSyncManager = cm
        ble.onClipboardReceived = { text, remoteHost ->
            serviceScope.launch {
                val payloadJson = json.encodeToString(mapOf("text" to text, "origin" to "BLE:$remoteHost"))
                recordLocalEvent(
                    eventType = "clipboard.synced",
                    payload = payloadJson
                )
            }
        }
        blePeripheralManager = ble

        // 6. Initialize 3-Tier TransferRepository
        transferRepository = org.archdrop.transfer.TransferRepository(
            context = this,
            database = db,
            lohsManager = lohs,
            derpMeshClient = client,
            loopbackBridge = bridge,
            transferClient = org.archdrop.transfer.TransferClient(this)
        )

        // 7. Start Outbox background monitor
        startOutboxSyncMonitor()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Log.i(TAG, "onStartCommand received action: ${intent?.action}")

        when (intent?.action) {
            ACTION_STOP -> {
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_START_HOTSPOT -> {
                lohsManager?.startHotspot()
                return START_STICKY
            }
            ACTION_STOP_HOTSPOT -> {
                lohsManager?.stopHotspot()
                return START_STICKY
            }
            ACTION_CONNECT_RELAY -> {
                val relayUrl = intent.getStringExtra(EXTRA_RELAY_URL)
                if (!relayUrl.isNullOrEmpty()) {
                    serviceScope.launch {
                        try {
                            derpMeshClient?.connect(relayUrl)
                        } catch (e: Exception) {
                            Log.w(TAG, "Failed to connect to relay $relayUrl: ${e.message}")
                        }
                    }
                }
                return START_STICKY
            }
            ACTION_BROADCAST_CLIPBOARD -> {
                val text = intent.getStringExtra(EXTRA_CLIPBOARD_TEXT)
                val host = intent.getStringExtra(EXTRA_PEER_HOST)
                val port = intent.getIntExtra(EXTRA_PEER_PORT, 53317)
                if (!text.isNullOrEmpty()) {
                    broadcastClipboard(text, host, port)
                }
            }
            ACTION_START_AUDIO_STREAM -> {
                val host = intent.getStringExtra(EXTRA_AUDIO_HOST) ?: lastPeerHost ?: "10.2.0.2"
                val port = intent.getIntExtra(EXTRA_AUDIO_PORT, 53317)
                val codecStr = intent.getStringExtra(EXTRA_AUDIO_CODEC) ?: "opus"
                val bitrate = intent.getIntExtra(EXTRA_AUDIO_BITRATE, 96000)
                val codec = org.archdrop.audio.AudioCodec.fromString(codecStr)
                val receiver = getOrCreateAudioReceiver()
                receiver.start(
                    host = host,
                    port = port,
                    fingerprint = identity?.toHex(),
                    codec = codec,
                    targetBitrate = bitrate
                )
            }
            ACTION_STOP_AUDIO_STREAM -> {
                audioReceiver?.stop()
            }
        }

        startInForeground()
        httpReceiver?.start()
        loopbackBridge?.start()
        blePeripheralManager?.start()

        return START_STICKY
    }

    /**
     * Records a local event into Room DB and optionally enqueues it in the durable outbox queue.
     */
    suspend fun recordLocalEvent(
        eventType: String,
        payload: String,
        targetPeerId: String? = null
    ): Long {
        val db = database ?: return -1L
        val origin = identity?.toHex() ?: "ANDROID_LOCAL"
        val nextSeq = db.eventDao().getNextSequenceNumber(origin)
        val now = System.currentTimeMillis()
        val hash = sha256(payload)

        val event = EventEntity(
            originDeviceId = origin,
            sequenceNumber = nextSeq,
            eventId = UUID.randomUUID().toString(),
            timestampMs = now,
            eventType = eventType,
            payload = payload,
            payloadHash = hash,
            ackStatus = 0
        )

        db.eventDao().insertEvent(event)
        db.vectorClockDao().advanceHighWaterMark(origin, nextSeq, now, "COMMITTED")

        if (targetPeerId != null) {
            val outbox = OutboxEntity(
                originDeviceId = origin,
                sequenceNumber = nextSeq,
                targetPeerId = targetPeerId,
                status = "PENDING"
            )
            db.outboxDao().enqueueOne(outbox)
        }

        Log.d(TAG, "Recorded local event seq=$nextSeq type=$eventType origin=$origin")
        return nextSeq
    }

    /**
     * Ingests remote events delivered via mesh or local receiver inside a transaction,
     * advancing vector clock high-water marks and arbitrating clipboard updates via LWW.
     */
    suspend fun ingestRemoteEvents(events: List<EventEntity>) {
        if (events.isEmpty()) return
        val db = database ?: return

        db.withTransaction {
            db.eventDao().insertEvents(events)
            val grouped = events.groupBy { it.originDeviceId }
            val now = System.currentTimeMillis()
            for ((origin, originEvents) in grouped) {
                val maxSeq = originEvents.maxOf { it.sequenceNumber }
                db.vectorClockDao().advanceHighWaterMark(origin, maxSeq, now, "SYNCED")
            }
        }

        // Deterministic Last-Write-Wins clipboard arbitration
        val clipboardEvents = events.filter { it.eventType.startsWith("clipboard.") }
        val winningClipboard = clipboardEvents.maxWithOrNull { a, b ->
            val timeComp = a.timestampMs.compareTo(b.timestampMs)
            if (timeComp != 0) timeComp else a.originDeviceId.compareTo(b.originDeviceId)
        }

        if (winningClipboard != null) {
            val raw = winningClipboard.payload
            val textToPaste = try {
                val elem = json.parseToJsonElement(raw)
                elem.jsonObject["text"]?.jsonPrimitive?.content ?: raw
            } catch (_: Exception) {
                raw
            }
            clipboardManager?.pasteToSystem(textToPaste)
            Log.i(TAG, "Arbitrated winning clipboard event from ${winningClipboard.originDeviceId}: ${winningClipboard.payloadHash}")
        }
    }

    private fun startOutboxSyncMonitor() {
        outboxSyncJob?.cancel()
        outboxSyncJob = serviceScope.launch {
            while (isActive) {
                delay(5_000L)
                val db = database ?: continue
                try {
                    val count = db.outboxDao().countOutbox()
                    if (count > 0) {
                        Log.d(TAG, "Pending outbox queue items: $count")
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Error checking outbox queue", e)
                }
            }
        }
    }

    private fun startInForeground() {
        val notification = createNotification()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            var serviceType = ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE or
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                serviceType = serviceType or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            }
            startForeground(NOTIFICATION_ID, notification, serviceType)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun createNotification(): Notification {
        val stopIntent = Intent(this, ArchDropSyncService::class.java).apply {
            action = ACTION_STOP
        }
        val stopPendingIntent = PendingIntent.getService(
            this,
            0,
            stopIntent,
            PendingIntent.FLAG_IMMUTABLE
        )

        return NotificationCompat.Builder(this, ArchDropApp.CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(getString(R.string.sync_service_running))
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Stop", stopPendingIntent)
            .build()
    }

    override fun onDestroy() {
        super.onDestroy()
        Log.i(TAG, "Destroying ArchDropSyncService...")

        audioReceiver?.stop()
        audioReceiver = null

        outboxSyncJob?.cancel()
        outboxSyncJob = null

        loopbackBridge?.close()
        loopbackBridge = null

        derpMeshClient?.close()
        derpMeshClient = null

        blePeripheralManager?.stop()
        blePeripheralManager = null

        lohsManager?.stopHotspot()
        lohsManager = null

        clipboardListener?.let {
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            cm?.removePrimaryClipChangedListener(it)
        }
        clipboardListener = null

        httpReceiver?.stop()
        httpReceiver = null
    }

    private fun setupClipboardMonitoring() {
        val cm = getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
        val listener = ClipboardManager.OnPrimaryClipChangedListener {
            try {
                val clip = cm.primaryClip
                if (clip != null && clip.itemCount > 0) {
                    broadcastClip(clip)
                }
            } catch (e: Exception) {
                Log.d(TAG, "Could not access primary clip: ${e.message}")
            }
        }
        clipboardListener = listener
        cm.addPrimaryClipChangedListener(listener)
    }

    fun broadcastClip(clip: ClipData, targetHost: String? = null, targetPort: Int = 53317) {
        val cm = clipboardManager ?: return
        val resolved = cm.resolveClip(clip)
        when (resolved) {
            is org.archdrop.clipboard.ClipContent.Empty -> return
            is org.archdrop.clipboard.ClipContent.Error -> {
                Log.w(TAG, "Clip resolution error: ${resolved.message}")
                return
            }
            is org.archdrop.clipboard.ClipContent.Text -> {
                broadcastClipboard(resolved.text, targetHost, targetPort)
            }
            is org.archdrop.clipboard.ClipContent.Image -> {
                if (cm.isRecentHash(resolved.sha256)) {
                    Log.d(TAG, "Echo suppressed: image hash matches last remote paste")
                    return
                }
                cm.setLastPastedHash(resolved.sha256)
                serviceScope.launch {
                    try {
                        val trustedPeer = database?.peerDao()?.getAllPeers()?.firstOrNull { it.isTrusted }
                            ?.let { org.archdrop.transfer.DiscoveredPeer.fromEntity(it) }
                            ?: org.archdrop.transfer.DiscoveredPeer(
                                peerId = identity?.toHex() ?: "DESKTOP_DEFAULT",
                                alias = "Arch Linux Desktop",
                                fingerprint = identity?.toHex() ?: "DESKTOP_FP",
                                host = targetHost ?: lastPeerHost
                            )
                        val packet = cm.packImage(resolved.bytes, resolved.mimeType, downloadUrlBase = "http://127.0.0.1:53317")
                        val repo = transferRepository ?: org.archdrop.transfer.TransferRepository(context = this@ArchDropSyncService, database = database)
                        val ok = repo.sendClipboard(trustedPeer, packet, identity?.toHex() ?: "ANDROID_FP")
                        if (ok) {
                            Log.i(TAG, "Successfully broadcasted image clipboard via dynamic transport route")
                        } else {
                            Log.w(TAG, "Failed to broadcast image clipboard via dynamic transport route")
                        }
                    } catch (e: Exception) {
                        Log.w(TAG, "Exception broadcasting image clipboard", e)
                    }
                }
            }
            is org.archdrop.clipboard.ClipContent.FilePayload -> {
                serviceScope.launch {
                    try {
                        val trustedPeer = database?.peerDao()?.getAllPeers()?.firstOrNull { it.isTrusted }
                            ?.let { org.archdrop.transfer.DiscoveredPeer.fromEntity(it) }
                            ?: org.archdrop.transfer.DiscoveredPeer(
                                peerId = identity?.toHex() ?: "DESKTOP_DEFAULT",
                                alias = "Arch Linux Desktop",
                                fingerprint = identity?.toHex() ?: "DESKTOP_FP",
                                host = targetHost ?: lastPeerHost
                            )
                        val repo = transferRepository ?: org.archdrop.transfer.TransferRepository(context = this@ArchDropSyncService, database = database)
                        val res = repo.sendStagedFileWithFailover(
                            peer = trustedPeer,
                            uri = resolved.uri,
                            fileName = resolved.fileName,
                            fileSize = resolved.fileSize,
                            mimeType = resolved.mimeType,
                            sha256 = resolved.sha256,
                            senderAlias = Build.MODEL ?: "Android Device",
                            senderFingerprint = identity?.toHex() ?: "ANDROID_FP"
                        )
                        if (res.success) {
                            Log.i(TAG, "Staged file ${resolved.fileName} sent successfully via dynamic transport route")
                        } else {
                            Log.w(TAG, "Failed to send staged file via dynamic transport route: ${res.errorMessage}")
                        }
                    } catch (e: Exception) {
                        Log.w(TAG, "Exception sending staged file transfer", e)
                    }
                }
            }
        }
    }

    fun broadcastClipboard(text: String, targetHost: String? = null, targetPort: Int = 53317) {
        val cm = clipboardManager ?: return
        if (cm.isRecentPaste(text)) {
            Log.d(TAG, "Echo suppressed: copied text matches last remote paste")
            return
        }
        val textHash = sha256(text)
        cm.setLastPastedHash(textHash)

        serviceScope.launch {
            val isWifi = isWifiConnected()
            var lanSuccess = false

            if (isWifi) {
                try {
                    val trustedPeer = database?.peerDao()?.getAllPeers()?.firstOrNull { it.isTrusted }
                        ?.let { org.archdrop.transfer.DiscoveredPeer.fromEntity(it) }
                        ?: org.archdrop.transfer.DiscoveredPeer(
                            peerId = identity?.toHex() ?: "DESKTOP_DEFAULT",
                            alias = "Arch Linux Desktop",
                            fingerprint = identity?.toHex() ?: "DESKTOP_FP",
                            host = targetHost ?: lastPeerHost
                        )

                    val packet = cm.packText(text, downloadUrlBase = "http://127.0.0.1:53317")
                    val repo = transferRepository ?: org.archdrop.transfer.TransferRepository(context = this@ArchDropSyncService, database = database)
                    lanSuccess = repo.sendClipboard(trustedPeer, packet, identity?.toHex() ?: "ANDROID_FP")
                    if (lanSuccess) {
                        Log.i(TAG, "Successfully broadcasted clipboard via dynamic transport route")
                    } else {
                        Log.w(TAG, "Failed to broadcast clipboard via dynamic transport route; falling back to BLE GATT")
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Exception broadcasting clipboard via LAN: ${e.message}; falling back to BLE GATT")
                }
            } else {
                Log.i(TAG, "Wi-Fi disconnected; initiating Zero-Wi-Fi BLE GATT clipboard synchronization")
            }

            if (!lanSuccess) {
                val mimeType = if (text.startsWith("file://") || text.startsWith("http://") || text.startsWith("https://")) {
                    BlePeripheralManager.MIME_TYPE_URI_LIST
                } else {
                    BlePeripheralManager.MIME_TYPE_TEXT_PLAIN
                }
                val bleSynced = blePeripheralManager?.writeOrNotifyClipboard(text, mimeType) ?: false
                if (bleSynced) {
                    Log.i(TAG, "Synced clipboard payload directly over Zero-Wi-Fi BLE GATT characteristic")
                }
            }
        }
    }

    private fun isWifiConnected(): Boolean {
        return try {
            val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as? android.net.ConnectivityManager ?: return false
            val activeNet = cm.activeNetwork ?: return false
            val caps = cm.getNetworkCapabilities(activeNet) ?: return false
            caps.hasTransport(android.net.NetworkCapabilities.TRANSPORT_WIFI)
        } catch (_: Exception) {
            false
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun sha256(content: String): String {
        val md = MessageDigest.getInstance("SHA-256")
        return md.digest(content.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
    }
}
