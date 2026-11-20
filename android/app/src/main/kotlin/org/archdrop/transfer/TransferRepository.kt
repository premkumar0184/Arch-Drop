package org.archdrop.transfer

import android.content.Context
import android.net.Uri
import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.archdrop.mesh.DerpMeshClient
import org.archdrop.mesh.LoopbackTunnelBridge
import org.archdrop.service.LOHSManager
import java.io.File
import java.net.ConnectException
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.TimeUnit

/**
 * High-level routing tier category matching Desktop TransportTier.
 */
enum class RouteTier {
    DIRECT_LAN,
    PEER_TO_PEER,
    CLOUD_RELAY
}

/**
 * Represents the resolved physical or virtual overlay transport route to a peer.
 */
sealed class TransportRoute {
    abstract val host: String
    abstract val port: Int
    abstract val routeName: String
    abstract val tier: RouteTier

    val displayName: String get() = routeName

    /**
     * Tier 1: Direct Local LAN connection over the same subnet.
     */
    data class DirectLan(
        override val host: String,
        override val port: Int = 53317
    ) : TransportRoute() {
        override val routeName: String = "Direct LAN"
        override val tier: RouteTier = RouteTier.DIRECT_LAN
        val endpoint: String get() = "$host:$port"
    }

    /**
     * Tier 2: Zero-Wi-Fi Offline P2P connection via Android Local-Only Hotspot (LOHS).
     */
    data class PeerToPeer(
        val gatewayHost: String = "192.168.43.1",
        override val port: Int = 53317,
        val ssid: String? = null
    ) : TransportRoute() {
        override val host: String get() = gatewayHost
        override val routeName: String = "Peer-to-Peer"
        override val tier: RouteTier = RouteTier.PEER_TO_PEER
        val endpoint: String get() = "$gatewayHost:$port"
    }

    /**
     * Tier 3: Userspace Remote WAN Relay via DERP and local loopback tunnel bridge.
     */
    data class CloudRelay(
        val relayUrl: String,
        val peerPublicKey: String,
        override val host: String = "127.0.0.1",
        override val port: Int = 53318
    ) : TransportRoute() {
        override val routeName: String = "Cloud Relay"
        override val tier: RouteTier = RouteTier.CLOUD_RELAY
        val endpoint: String get() = "$host:$port"
    }
}

/**
 * Candidate endpoints for a known peer.
 */
data class PeerCandidateRoutes(
    val peerId: String,
    val directLanHost: String? = null,
    val directLanPort: Int = 53317,
    val p2pGatewayHost: String? = null,
    val derpMeshPubkey: String? = null,
    val preferredTier: RouteTier? = null
) {
    fun toDiscoveredPeer(): DiscoveredPeer {
        return DiscoveredPeer(
            peerId = peerId,
            alias = peerId,
            fingerprint = derpMeshPubkey ?: peerId,
            host = directLanHost,
            port = directLanPort
        )
    }
}

/**
 * Runtime representation of a peer discovered via pairing, mDNS, BLE, or manual entry.
 */
data class DiscoveredPeer(
    val peerId: String,
    val alias: String,
    val fingerprint: String,
    val host: String? = null,
    val port: Int = 53317,
    val isTrusted: Boolean = true
) {
    val directLanHost: String? get() = host
    val lanHost: String? get() = host
    val lanPort: Int get() = port

    companion object {
        fun fromEntity(entity: org.archdrop.db.PeerEntity, cachedLanHost: String? = null): DiscoveredPeer {
            return DiscoveredPeer(
                peerId = entity.meshKey,
                alias = entity.alias,
                fingerprint = entity.fingerprint,
                host = cachedLanHost,
                port = entity.port,
                isTrusted = entity.isTrusted
            )
        }
    }
}

/**
 * Operational state machine of a transport route.
 */
enum class RouteState {
    IDLE,
    PROBING,
    ACTIVE,
    DEGRADED,
    FAILED
}

/**
 * Telemetry record capturing live metrics of a route.
 */
data class RouteTelemetry(
    val route: TransportRoute,
    val state: RouteState,
    val latencyMs: Long? = null,
    val bytesSent: Long = 0L,
    val bytesReceived: Long = 0L,
    val lastActiveEpochMs: Long = System.currentTimeMillis(),
    val failoverCount: Int = 0,
    val lastError: String? = null
)

/**
 * Single routing orchestration repository for Arch Drop Android companion.
 *
 * Implements the 3-Tier Route Resolution Strategy:
 * 1. Tier 1: Direct Local LAN (TCP probe to target LAN IP:53317 with 1500ms timeout; fail fast on ECONNREFUSED).
 * 2. Tier 2: Zero-Wi-Fi Offline P2P (BLE wakeup trigger -> LOHS SoftAP gateway 192.168.43.1:53317 with 60s idle teardown).
 * 3. Tier 3: Remote WAN Relay (DerpMeshClient over loopback 127.0.0.1:53318 with Noise AEAD payload encryption).
 *
 * Enforces Rule 0 Pure Userspace Operation: Zero android.net.VpnService usage.
 */
class TransferRepository(
    private val context: Context? = null,
    private val database: ArchDropDatabase? = null,
    private val lohsManager: LOHSManager? = null,
    private val derpMeshClient: DerpMeshClient? = null,
    private val loopbackBridge: LoopbackTunnelBridge? = null,
    private val transferClient: TransferClient? = null,
    private val okHttpClient: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(1500, TimeUnit.MILLISECONDS)
        .readTimeout(3000, TimeUnit.MILLISECONDS)
        .build()
) {
    companion object {
        private const val TAG = "ArchDrop:TransferRepo"
        const val DIRECT_LAN_TIMEOUT_MS = 1500
        const val LOHS_GATEWAY_IP = "192.168.43.1"
        const val LOHS_PORT = 53317
        const val LOOPBACK_HOST = "127.0.0.1"
        const val LOOPBACK_BRIDGE_PORT = 53318

        fun hexToByteArray(hex: String): ByteArray? {
            val clean = hex.trim()
            if (clean.length % 2 != 0) return null
            return try {
                ByteArray(clean.length / 2) { i ->
                    clean.substring(i * 2, i * 2 + 2).toInt(16).toByte()
                }
            } catch (_: Exception) {
                null
            }
        }
    }

    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    private val _activeRoute = MutableStateFlow<TransportRoute?>(null)
    val activeRoute: StateFlow<TransportRoute?> = _activeRoute.asStateFlow()

    private val _routeState = MutableStateFlow(RouteState.IDLE)
    val routeState: StateFlow<RouteState> = _routeState.asStateFlow()

    private val _routeTelemetry = MutableStateFlow<RouteTelemetry?>(null)
    val routeTelemetry: StateFlow<RouteTelemetry?> = _routeTelemetry.asStateFlow()

    fun setActiveRoute(route: TransportRoute) {
        _activeRoute.value = route
        _routeState.value = RouteState.ACTIVE
    }

    private fun getContext(): Context? {
        if (context != null) return context
        return try {
            val providerClass = Class.forName("androidx.test.core.app.ApplicationProvider")
            val getAppContextMethod = providerClass.getMethod("getApplicationContext")
            getAppContextMethod.invoke(null) as? Context
        } catch (_: Throwable) {
            null
        }
    }

    private fun getTransferClient(): TransferClient? {
        if (transferClient != null) return transferClient
        val ctx = getContext() ?: return null
        return TransferClient(ctx)
    }

    /**
     * Probes an IP:Port with strict timeout and immediate fail-fast on ECONNREFUSED.
     */
    fun probeLanEndpoint(host: String, port: Int, timeoutMs: Int = DIRECT_LAN_TIMEOUT_MS): Boolean {
        return try {
            Socket().use { socket ->
                socket.tcpNoDelay = true
                socket.soTimeout = timeoutMs
                socket.connect(InetSocketAddress(host, port), timeoutMs)
                true
            }
        } catch (_: ConnectException) {
            // Port closed / refused: fail fast (< 50ms)
            false
        } catch (_: SocketTimeoutException) {
            // Blackhole: reached exact timeout limit
            false
        } catch (_: Exception) {
            false
        }
    }

    /**
     * Resolves the optimal transport route according to candidate routes.
     */
    suspend fun resolveBestRoute(candidate: PeerCandidateRoutes): TransportRoute = withContext(Dispatchers.IO) {
        val start = System.currentTimeMillis()
        _routeState.value = RouteState.PROBING

        // ====================================================================
        // TIER 1: DIRECT LOCAL LAN
        // ====================================================================
        val lanHost = candidate.directLanHost
        if (!lanHost.isNullOrEmpty()) {
            val isLanAlive = probeLanEndpoint(lanHost, candidate.directLanPort, DIRECT_LAN_TIMEOUT_MS)
            val elapsed = System.currentTimeMillis() - start
            if (isLanAlive) {
                val route = TransportRoute.DirectLan(host = lanHost, port = candidate.directLanPort)
                _activeRoute.value = route
                _routeState.value = RouteState.ACTIVE
                _routeTelemetry.value = RouteTelemetry(
                    route = route,
                    state = RouteState.ACTIVE,
                    latencyMs = elapsed,
                    lastActiveEpochMs = System.currentTimeMillis()
                )
                Log.i(TAG, "Tier 1 Direct LAN resolved: $lanHost:${candidate.directLanPort} in ${elapsed}ms")
                return@withContext route
            } else {
                Log.d(TAG, "Tier 1 Direct LAN probe failed after ${elapsed}ms; advancing to fallback")
            }
        }

        // ====================================================================
        // TIER 2: ZERO-WI-FI OFFLINE P2P (LOHS SoftAP)
        // ====================================================================
        if (lohsManager?.isHotspotActive() == true || (candidate.p2pGatewayHost != null && candidate.derpMeshPubkey == null)) {
            val gwHost = candidate.p2pGatewayHost ?: lohsManager?.resolveHotspotIp() ?: LOHS_GATEWAY_IP
            val route = TransportRoute.PeerToPeer(gatewayHost = gwHost, port = LOHS_PORT)
            _activeRoute.value = route
            _routeState.value = RouteState.ACTIVE
            _routeTelemetry.value = RouteTelemetry(
                route = route,
                state = RouteState.ACTIVE,
                latencyMs = System.currentTimeMillis() - start,
                lastActiveEpochMs = System.currentTimeMillis()
            )
            Log.i(TAG, "Tier 2 Offline P2P resolved: $gwHost:$LOHS_PORT")
            return@withContext route
        }

        // ====================================================================
        // TIER 3: REMOTE WAN RELAY (DERP Userspace Loopback Bridge)
        // ====================================================================
        val pubkey = candidate.derpMeshPubkey
        if (!pubkey.isNullOrEmpty() || derpMeshClient != null || loopbackBridge != null) {
            val peerKeyBytes = pubkey?.let { hexToByteArray(it) }
            if (peerKeyBytes != null && loopbackBridge != null) {
                loopbackBridge.targetDesktopPeerKey = peerKeyBytes
            }
            val relayUrl = derpMeshClient?.serverKey?.joinToString("") { "%02X".format(it) } ?: "wss://relay.archdrop.org"
            val route = TransportRoute.CloudRelay(
                relayUrl = relayUrl,
                peerPublicKey = pubkey ?: "DERP_PEER",
                host = LOOPBACK_HOST,
                port = LOOPBACK_BRIDGE_PORT
            )
            _activeRoute.value = route
            _routeState.value = RouteState.ACTIVE
            _routeTelemetry.value = RouteTelemetry(
                route = route,
                state = RouteState.ACTIVE,
                latencyMs = System.currentTimeMillis() - start,
                lastActiveEpochMs = System.currentTimeMillis()
            )
            Log.i(TAG, "Tier 3 WAN Relay resolved: $LOOPBACK_HOST:$LOOPBACK_BRIDGE_PORT")
            return@withContext route
        }

        // Fallback to P2P if specified
        if (candidate.p2pGatewayHost != null) {
            val route = TransportRoute.PeerToPeer(gatewayHost = candidate.p2pGatewayHost, port = LOHS_PORT)
            _activeRoute.value = route
            _routeState.value = RouteState.ACTIVE
            return@withContext route
        }

        _routeState.value = RouteState.FAILED
        throw IllegalStateException("All 3 transport tiers failed")
    }

    /**
     * Resolves the optimal transport route according to the 3-tier hierarchy for DiscoveredPeer.
     */
    suspend fun resolveBestRoute(peer: DiscoveredPeer): TransportRoute {
        return resolveBestRoute(
            PeerCandidateRoutes(
                peerId = peer.peerId,
                directLanHost = peer.host ?: getCachedLanHost(peer.peerId),
                directLanPort = peer.port,
                derpMeshPubkey = peer.peerId
            )
        )
    }

    /**
     * Sends a file payload with failover routing.
     */
    suspend fun sendPayload(
        file: File,
        candidate: PeerCandidateRoutes,
        senderAlias: String = "Android Device",
        senderFingerprint: String = "ANDROID_FP"
    ): SendFileResult = withContext(Dispatchers.IO) {
        val route = resolveBestRoute(candidate)
        val uri = Uri.fromFile(file)
        val tc = getTransferClient()

        if (tc != null) {
            val result = tc.sendStagedFile(
                targetHost = route.host,
                targetPort = route.port,
                uri = uri,
                fileName = file.name,
                fileSize = file.length(),
                senderAlias = senderAlias,
                senderFingerprint = senderFingerprint
            )
            if (result.success && database != null) {
                recordTransferEvent(file.name, file.length(), "", senderFingerprint)
            }
            result
        } else {
            SendFileResult(
                success = false,
                fileName = file.name,
                fileSize = file.length(),
                errorMessage = "No TransferClient available"
            )
        }
    }

    /**
     * Sends a staged file payload over the dynamically resolved route.
     */
    suspend fun sendStagedFileWithFailover(
        peer: DiscoveredPeer,
        uri: Uri,
        fileName: String,
        fileSize: Long,
        mimeType: String = "application/octet-stream",
        sha256: String? = null,
        senderAlias: String = "Android Device",
        senderFingerprint: String = "ANDROID_FP"
    ): SendFileResult = withContext(Dispatchers.IO) {
        val route = resolveBestRoute(peer)

        if (route is TransportRoute.PeerToPeer) {
            lohsManager?.resetIdleTimeout()
        }

        val client = getTransferClient()
            ?: return@withContext SendFileResult(
                success = false,
                fileName = fileName,
                fileSize = fileSize,
                errorMessage = "TransferClient unavailable (no Context)"
            )

        val result = client.sendStagedFile(
            targetHost = route.host,
            targetPort = route.port,
            uri = uri,
            fileName = fileName,
            fileSize = fileSize,
            mimeType = mimeType,
            senderAlias = senderAlias,
            senderFingerprint = senderFingerprint,
            sha256 = sha256
        )

        if (result.success && database != null) {
            recordTransferEvent(fileName, fileSize, sha256 ?: "", senderFingerprint)
        }

        result
    }

    /**
     * Sends a clipboard synchronization packet over the dynamically resolved route.
     */
    suspend fun sendClipboard(
        peer: DiscoveredPeer,
        packet: Any,
        senderFingerprint: String = "ANDROID_FP"
    ): Boolean = withContext(Dispatchers.IO) {
        val route = resolveBestRoute(peer)

        val jsonStr = when (packet) {
            is org.archdrop.clipboard.models.ClipboardInlinePacket -> json.encodeToString(packet)
            is org.archdrop.clipboard.models.ClipboardDescriptorPacket -> json.encodeToString(packet)
            else -> return@withContext false
        }

        val mediaType = "application/json; charset=utf-8".toMediaType()
        val requestBody = jsonStr.toRequestBody(mediaType)
        val url = "http://${route.host}:${route.port}/api/v2/clipboard"

        val request = Request.Builder()
            .url(url)
            .post(requestBody)
            .header("Content-Type", "application/json")
            .header("X-Fingerprint", senderFingerprint)
            .build()

        try {
            okHttpClient.newCall(request).execute().use { response ->
                response.isSuccessful
            }
        } catch (e: Exception) {
            Log.w(TAG, "Failed to send clipboard packet to $url: ${e.message}")
            false
        }
    }

    private suspend fun recordTransferEvent(
        fileName: String,
        fileSize: Long,
        sha256: String,
        originId: String
    ) {
        val db = database ?: return
        try {
            val nextSeq = db.eventDao().getNextSequenceNumber(originId)
            val now = System.currentTimeMillis()
            val payload = "{\"file\":\"$fileName\",\"size\":$fileSize,\"sha256\":\"$sha256\"}"
            val md = MessageDigest.getInstance("SHA-256")
            val hash = md.digest(payload.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }

            val event = EventEntity(
                originDeviceId = originId,
                sequenceNumber = nextSeq,
                eventId = UUID.randomUUID().toString(),
                timestampMs = now,
                eventType = "transfer.completed",
                payload = payload,
                payloadHash = hash,
                ackStatus = 0
            )
            db.eventDao().insertEvent(event)
        } catch (e: Exception) {
            Log.w(TAG, "Failed to record transfer event in Room ledger", e)
        }
    }

    private fun getCachedLanHost(peerId: String): String? {
        val ctx = getContext() ?: return null
        val prefs = ctx.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
        return prefs.getString("peer_lan_host_$peerId", prefs.getString("last_peer_host", null))
    }

    fun cacheLanHost(peerId: String, host: String) {
        val ctx = getContext() ?: return
        ctx.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .edit()
            .putString("peer_lan_host_$peerId", host)
            .putString("last_peer_host", host)
            .apply()
    }
}
